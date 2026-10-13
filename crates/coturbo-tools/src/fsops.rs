//! File reads, writes, edits and search.

use crate::{Mode, Res, SKELETON_MIN_LINES, ToolOutput, Tools, arg};
use serde_json::Value;
use std::{fs, path::Path, process::Stdio};

const MAX_READ: usize = 100_000;
const MAX_GREP: usize = 100;
const MAX_LIST: usize = 500;

fn read_text(p: &Path) -> Result<String, String> {
    fs::read_to_string(p).map_err(|e| format!("cannot read {}: {e}", p.display()))
}

fn truncate(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut cut = max;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("\n[truncated]");
    }
    s
}

/// Formats Rust source with rustfmt; returns the input unchanged if rustfmt
/// is unavailable or fails, so a formatter problem never blocks an edit.
fn rustfmt(src: &str) -> String {
    use std::io::Write;
    let child = std::process::Command::new("rustfmt")
        .args(["--edition", "2024", "--emit", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else { return src.to_string() };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(src.as_bytes());
    }
    match child.wait_with_output() {
        Ok(out) if out.status.success() && !out.stdout.is_empty() => {
            String::from_utf8(out.stdout).unwrap_or_else(|_| src.to_string())
        }
        _ => src.to_string(),
    }
}

impl Tools {
    fn walker(&self, input: &Value) -> Result<ignore::Walk, String> {
        let base = self.resolve(input["path"].as_str().unwrap_or("."))?;
        Ok(ignore::WalkBuilder::new(base).require_git(false).build())
    }

    pub(crate) fn read_file(&self, input: &Value) -> Res {
        let path = self.resolve(arg(input, "path")?)?;
        let src = read_text(&path)?;
        let full = input["full"].as_bool().unwrap_or(false);
        let is_rs = path.extension().is_some_and(|e| e == "rs");
        let lines = src.lines().count();
        if self.mode == Mode::Ast && is_rs && !full && lines > SKELETON_MIN_LINES {
            let header = format!(
                "// skeleton of {} ({lines} lines); bodies elided — read_symbol for one, full=true for all\n",
                self.display(&path)
            );
            return Ok(ToolOutput::ok(header + &coturbo_ast::skeleton(&src)));
        }
        Ok(ToolOutput::ok(truncate(src, MAX_READ)))
    }

    pub(crate) fn write_file(&self, input: &Value) -> Res {
        let path = self.resolve(arg(input, "path")?)?;
        let content = arg(input, "content")?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        fs::write(&path, content).map_err(|e| e.to_string())?;
        Ok(ToolOutput::ok(format!("wrote {} ({} bytes)", self.display(&path), content.len())))
    }

    pub(crate) fn edit_file(&self, input: &Value) -> Res {
        let path = self.resolve(arg(input, "path")?)?;
        let (old, new) = (arg(input, "old")?, arg(input, "new")?);
        let src = read_text(&path)?;
        match src.matches(old).count() {
            1 => {
                fs::write(&path, src.replacen(old, new, 1)).map_err(|e| e.to_string())?;
                Ok(ToolOutput::ok(format!("edited {}", self.display(&path))))
            }
            0 => Err(format!("`old` not found in {}", self.display(&path))),
            n => Err(format!("`old` matches {n} times in {}; add context to make it unique", self.display(&path))),
        }
    }

    pub(crate) fn list_files(&self, input: &Value) -> Res {
        let mut files: Vec<String> = self
            .walker(input)?
            .flatten()
            .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
            .map(|e| self.display(e.path()))
            .collect();
        files.sort();
        let total = files.len();
        files.truncate(MAX_LIST);
        let mut out = files.join("\n");
        if total > MAX_LIST {
            out.push_str(&format!("\n[{} more]", total - MAX_LIST));
        }
        Ok(ToolOutput::ok(out))
    }

    pub(crate) fn grep(&self, input: &Value) -> Res {
        let re = regex::Regex::new(arg(input, "pattern")?).map_err(|e| e.to_string())?;
        let mut hits = Vec::new();
        let mut entries: Vec<_> = self.walker(input)?.flatten().filter(|e| e.file_type().is_some_and(|t| t.is_file())).collect();
        entries.sort_by(|a, b| a.path().cmp(b.path()));
        'files: for e in entries {
            let Ok(text) = fs::read_to_string(e.path()) else { continue };
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    hits.push(format!("{}:{}: {}", self.display(e.path()), i + 1, line.trim()));
                    if hits.len() >= MAX_GREP {
                        hits.push("[match limit reached]".into());
                        break 'files;
                    }
                }
            }
        }
        Ok(ToolOutput::ok(if hits.is_empty() { "no matches".into() } else { hits.join("\n") }))
    }

    pub(crate) fn read_symbol(&self, input: &Value) -> Res {
        let path = self.resolve(arg(input, "path")?)?;
        let src = read_text(&path)?;
        coturbo_ast::read_symbol(&src, arg(input, "symbol")?).map(ToolOutput::ok).map_err(|e| e.to_string())
    }

    fn apply_ast_edit(
        &self,
        input: &Value,
        edit: impl FnOnce(&str) -> Result<String, coturbo_ast::AstError>,
        verb: &str,
    ) -> Res {
        let path = self.resolve(arg(input, "path")?)?;
        let src = read_text(&path)?;
        let edited = edit(&src).map_err(|e| e.to_string())?;
        fs::write(&path, rustfmt(&edited)).map_err(|e| e.to_string())?;
        Ok(ToolOutput::ok(format!("{verb} in {}", self.display(&path))))
    }

    pub(crate) fn replace_symbol(&self, input: &Value) -> Res {
        let (sym, code) = (arg(input, "symbol")?, arg(input, "code")?);
        self.apply_ast_edit(input, |src| coturbo_ast::replace_symbol(src, sym, code), &format!("replaced `{sym}`"))
    }

    pub(crate) fn insert_symbol(&self, input: &Value) -> Res {
        let (after, code) = (arg(input, "after")?, arg(input, "code")?);
        self.apply_ast_edit(input, |src| coturbo_ast::insert_symbol(src, after, code), &format!("inserted after `{after}`"))
    }
}

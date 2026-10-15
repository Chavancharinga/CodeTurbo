//! File reads, writes, edits and search.

use crate::{Mode, Res, SKELETON_MIN_LINES, ToolOutput, Tools, arg};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};

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
        self.record_read(&self.display(&path), &src);
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
        let rel = self.display(&path);
        if self.mode == Mode::Ast
            && !full
            && lines > SKELETON_MIN_LINES
            && coturbo_verify::skeleton::is_js(&rel)
            && let Some(sk) = coturbo_verify::skeleton::js_skeleton(&rel, &src)
        {
            let header = format!(
                "// skeleton of {rel} ({lines} lines); function bodies elided — grep for a function or full=true to see bodies before editing\n"
            );
            return Ok(ToolOutput::ok(header + &sk));
        }
        Ok(ToolOutput::ok(truncate(src, MAX_READ)))
    }

    /// Path, current content and the content an edit tool would write, without
    /// writing anything. Shared by execution and by the approval preview.
    pub(crate) fn plan_edit(&self, name: &str, input: &Value) -> Result<(PathBuf, String, String), String> {
        let path = self.resolve(arg(input, "path")?)?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        // Formatter for this kind of file, if any: rustfmt for Rust, the
        // built-in HTML formatter for pages.
        let format = |src: &str| -> Option<String> {
            match ext.as_str() {
                "rs" => Some(rustfmt(src)),
                "html" | "htm" => coturbo_verify::html::format(src),
                _ => None,
            }
        };
        // Keep formatted files formatted and format new ones; never reformat
        // a file that was not formatted before.
        let keep_formatted = |old: &str, new: String| {
            if !self.format_on_write() || !(old.is_empty() || format(old).as_deref() == Some(old)) {
                return new;
            }
            format(&new).unwrap_or(new)
        };
        if name == "write_file" {
            let content = arg(input, "content")?;
            let old = if path.exists() { read_text(&path)? } else { String::new() };
            let new = keep_formatted(&old, content.to_string());
            return Ok((path, old, new));
        }
        let src = read_text(&path)?;
        let new = match name {
            "edit_file" => {
                let (old, new) = (arg(input, "old")?, arg(input, "new")?);
                match src.matches(old).count() {
                    1 => keep_formatted(&src, src.replacen(old, new, 1)),
                    0 => return Err(format!("`old` not found in {}", self.display(&path))),
                    n => return Err(format!("`old` matches {n} times in {}; add context to make it unique", self.display(&path))),
                }
            }
            "replace_symbol" => {
                let (sym, code) = (arg(input, "symbol")?, arg(input, "code")?);
                rustfmt(&coturbo_ast::replace_symbol(&src, sym, code).map_err(|e| e.to_string())?)
            }
            "insert_symbol" => {
                let (after, code) = (arg(input, "after")?, arg(input, "code")?);
                rustfmt(&coturbo_ast::insert_symbol(&src, after, code).map_err(|e| e.to_string())?)
            }
            _ => return Err(format!("{name} is not an edit tool")),
        };
        Ok((path, src, new))
    }

    fn apply_edit(&self, name: &str, input: &Value, verb: String) -> Res {
        let (path, _, new) = self.plan_edit(name, input)?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        fs::write(&path, &new).map_err(|e| e.to_string())?;
        Ok(ToolOutput::ok(format!("{verb} {}", self.display(&path))))
    }

    pub(crate) fn write_file(&self, input: &Value) -> Res {
        let len = arg(input, "content")?.len();
        // Warn (do not block) when overwriting a file that changed since the
        // model read it, or one it never read: it may be clobbering work.
        let conflict = self
            .resolve(arg(input, "path")?)
            .ok()
            .filter(|p| p.is_file())
            .and_then(|p| Some((self.display(&p), std::fs::read_to_string(&p).ok()?)))
            .filter(|(rel, cur)| self.write_conflict(rel, cur))
            .map(|(rel, _)| rel);
        let out = self.apply_edit("write_file", input, "wrote".into())?;
        let mut content = format!("{} ({len} bytes)", out.content);
        if let Some(rel) = conflict {
            content.push_str(&format!(
                "\n! conflito: {rel} mudou no disco desde a última leitura (ou não foi lido antes);                  se não era a intenção, releia o arquivo e refaça a alteração"
            ));
        }
        if let Ok(abs) = self.resolve(arg(input, "path")?) {
            self.record_read(&self.display(&abs), arg(input, "content")?);
        }
        Ok(ToolOutput::ok(content))
    }

    pub(crate) fn edit_file(&self, input: &Value) -> Res {
        self.apply_edit("edit_file", input, "edited".into())
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
        if let Ok(p) = self.resolve(arg(input, "path").unwrap_or(""))
            && let Ok(src) = read_text(&p)
        {
            self.record_read(&self.display(&p), &src);
        }
        let path = self.resolve(arg(input, "path")?)?;
        let src = read_text(&path)?;
        coturbo_ast::read_symbol(&src, arg(input, "symbol")?).map(ToolOutput::ok).map_err(|e| e.to_string())
    }

    pub(crate) fn replace_symbol(&self, input: &Value) -> Res {
        let sym = arg(input, "symbol")?;
        self.apply_edit("replace_symbol", input, format!("replaced `{sym}` in"))
    }

    pub(crate) fn insert_symbol(&self, input: &Value) -> Res {
        let after = arg(input, "after")?;
        self.apply_edit("insert_symbol", input, format!("inserted after `{after}` in"))
    }
}

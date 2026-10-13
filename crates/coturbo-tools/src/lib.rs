//! Tools the agent can call, in two sets: `Naive` mirrors what today's coding
//! agents offer (whole-file reads, search/replace edits, raw build output);
//! `Ast` adds symbol-level reads/edits and compact compiler diagnostics.

mod cargo;
mod fsops;
mod shell;

use coturbo_providers::ToolSpec;
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Naive,
    Ast,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: false }
    }
    pub fn err(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: true }
    }
}

type Res = Result<ToolOutput, String>;

pub struct Tools {
    root: PathBuf,
    mode: Mode,
}

/// Rust files longer than this are returned as a skeleton by `read_file` in Ast mode.
const SKELETON_MIN_LINES: usize = 60;

fn obj(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required})
}

fn spec(name: &str, description: &str, schema: Value) -> ToolSpec {
    ToolSpec { name: name.into(), description: description.into(), input_schema: schema }
}

fn s(desc: &str) -> Value {
    json!({"type": "string", "description": desc})
}

fn arg<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input[key].as_str().ok_or_else(|| format!("missing string argument `{key}`"))
}

impl Tools {
    pub fn new(root: &Path, mode: Mode) -> Self {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        Self { root, mode }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Tool definitions, sorted by name so the request prefix is byte-stable.
    pub fn specs(&self) -> Vec<ToolSpec> {
        let read_desc = match self.mode {
            Mode::Naive => "Read a file.",
            Mode::Ast => "Read a file. Large .rs files return a skeleton (bodies elided as `{ … }`); use read_symbol for a body or full=true for everything.",
        };
        let mut v = vec![
            spec("read_file", read_desc, obj(json!({"path": s("relative path"), "full": {"type": "boolean"}}), &["path"])),
            spec("write_file", "Create or overwrite a file.", obj(json!({"path": s("relative path"), "content": s("full file content")}), &["path", "content"])),
            spec("edit_file", "Replace the unique occurrence of `old` with `new` in a file.", obj(json!({"path": s("relative path"), "old": s("exact text, must match once"), "new": s("replacement")}), &["path", "old", "new"])),
            spec("list_files", "List files (respects .gitignore).", obj(json!({"path": s("relative dir, default .")}), &[])),
            spec("grep", "Regex search; returns path:line: text.", obj(json!({"pattern": s("regex"), "path": s("relative dir or file, default .")}), &["pattern"])),
            spec("bash", "Run a shell command in the project root.", obj(json!({"command": s("command")}), &["command"])),
        ];
        if self.mode == Mode::Ast {
            let sym = s("symbol path, e.g. `Point::new` or `utils::parse`");
            v.extend([
                spec("read_symbol", "Read one item (fn, struct, impl method, ...) of a .rs file.", obj(json!({"path": s("relative .rs path"), "symbol": sym.clone()}), &["path", "symbol"])),
                spec("replace_symbol", "Replace one item of a .rs file with new code (doc comments included); file is rustfmt'd.", obj(json!({"path": s("relative .rs path"), "symbol": sym.clone(), "code": s("complete new item")}), &["path", "symbol", "code"])),
                spec("insert_symbol", "Insert a new item right after an existing one in a .rs file; file is rustfmt'd.", obj(json!({"path": s("relative .rs path"), "after": sym, "code": s("complete new item")}), &["path", "after", "code"])),
                spec("cargo_check", "Type-check the crate; returns only errors as path:line:col with the source line.", obj(json!({}), &[])),
            ]);
        }
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// Resolves a model-supplied path, refusing anything outside the root.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf, String> {
        let p = Path::new(rel);
        let rel = if p.is_absolute() {
            p.strip_prefix(&self.root).map_err(|_| format!("path outside project: {rel}"))?.to_path_buf()
        } else {
            p.to_path_buf()
        };
        let mut out = PathBuf::new();
        for c in rel.components() {
            match c {
                Component::Normal(x) => out.push(x),
                Component::CurDir => {}
                Component::ParentDir => {
                    if !out.pop() {
                        return Err(format!("path outside project: {}", rel.display()));
                    }
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(format!("path outside project: {}", rel.display()));
                }
            }
        }
        Ok(self.root.join(out))
    }

    pub(crate) fn display(&self, p: &Path) -> String {
        p.strip_prefix(&self.root).unwrap_or(p).to_string_lossy().replace('\\', "/")
    }

    /// Runs a tool. Failures come back as `is_error` outputs, never panics.
    pub async fn call(&self, name: &str, input: &Value) -> ToolOutput {
        let ast_only = matches!(name, "read_symbol" | "replace_symbol" | "insert_symbol" | "cargo_check");
        if ast_only && self.mode != Mode::Ast {
            return ToolOutput::err(format!("unknown tool: {name}"));
        }
        let r: Res = match name {
            "read_file" => self.read_file(input),
            "write_file" => self.write_file(input),
            "edit_file" => self.edit_file(input),
            "list_files" => self.list_files(input),
            "grep" => self.grep(input),
            "bash" => self.bash(input).await,
            "read_symbol" => self.read_symbol(input),
            "replace_symbol" => self.replace_symbol(input),
            "insert_symbol" => self.insert_symbol(input),
            "cargo_check" => cargo::check(&self.root).await,
            _ => Err(format!("unknown tool: {name}")),
        };
        r.unwrap_or_else(ToolOutput::err)
    }
}

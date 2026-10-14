//! Tools the agent can call, in two sets: `Naive` mirrors what today's coding
//! agents offer (whole-file reads, search/replace edits, raw build output);
//! `Ast` adds symbol-level reads/edits and compact compiler diagnostics.

mod fsops;
mod shell;

use coturbo_providers::ToolSpec;
use serde_json::{Value, json};
use coturbo_verify::lsp::LspManager;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Naive,
    Ast,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
    /// Images for the model, `(media type, base64 data)`.
    pub images: Vec<(String, String)>,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: false, images: Vec::new() }
    }
    pub fn err(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: true, images: Vec::new() }
    }
}

/// Capability tier of a tool call, least to most privileged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    Read,
    Write,
    Exec,
}

/// Tools that change files; `preview` can show their diff before they run.
pub const EDIT_TOOLS: &[&str] = &["write_file", "edit_file", "replace_symbol", "insert_symbol"];

/// Fixed tier per tool. Unknown tools (MCP, third-party) are `Exec`.
pub fn tier(name: &str) -> Tier {
    match name {
        "read_file" | "read_symbol" | "list_files" | "grep" | "workspace_diagnostics" | "propose_plan" => Tier::Read,
        n if EDIT_TOOLS.contains(&n) || n == "design_brief" => Tier::Write,
        "design_review" => Tier::Read,
        _ => Tier::Exec,
    }
}

const PREVIEW_MAX_LINES: usize = 200;

fn native_note(rel: &str, diags: &[coturbo_verify::Diag]) -> String {
    use coturbo_verify::diag::{Level, count, render_list};
    if diags.is_empty() {
        return format!("check {rel}: clean");
    }
    let (e, w) = (count(diags, Level::Error), count(diags, Level::Warning));
    format!("check {rel}: {e} error(s), {w} warning(s)\n{}", render_list(diags, 20))
}

type Res = Result<ToolOutput, String>;

pub struct Tools {
    root: PathBuf,
    mode: Mode,
    /// Language-server diagnostics (and formatting) on write; optional.
    lsp: Option<Arc<LspManager>>,
    /// Built-in Rust/JS/TS/CSS/JSON/TOML checks after every edit.
    native_checks: bool,
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
        Self { root, mode, lsp: None, native_checks: false }
    }

    /// Enables the built-in checks after every edit (no external tools).
    pub fn with_native_checks(mut self, on: bool) -> Self {
        self.native_checks = on;
        self
    }

    /// Enables diagnostics and formatting after every edit.
    pub fn with_lsp(mut self, lsp: Arc<LspManager>) -> Self {
        self.lsp = Some(lsp);
        self
    }

    /// Whether files written by `write_file`/`edit_file` are formatted
    /// (rustfmt for Rust, the built-in formatter for HTML).
    pub(crate) fn format_on_write(&self) -> bool {
        self.lsp.as_ref().is_some_and(|l| l.format_on_write())
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
            spec(
                "design_brief",
                "Call before writing any user interface (HTML, CSS, JSX, Vue, Svelte). Describe the design in a few words; returns the locked design tokens (written to DESIGN.md) that every style must use.",
                obj(
                    json!({
                        "subject": s("what the product or page is about"),
                        "audience": s("who it is for"),
                        "tone": s("tone in a few words, e.g. editorial and calm, playful, technical"),
                        "signature": s("the one distinctive visual element of this interface"),
                        "brand_colors": {"type": "array", "items": {"type": "string"}, "description": "the brand's existing colors as #rrggbb, most important first; omit to detect them from the project's files"},
                        "new_palette": {"type": "boolean", "description": "true only if the user asked to replace the brand colors"}
                    }),
                    &["subject", "audience", "tone", "signature"],
                ),
            ),
            spec(
                "design_review",
                "Render an HTML page in the browser and get its screenshot plus measurements, to critique the design as a user would see it. Use after building a page.",
                obj(json!({"path": s("relative .html path")}), &["path"]),
            ),
            spec("propose_plan", "Plan mode only: submit .codeturbo/plans/<slug>-plan.md for the user's approval.", obj(json!({"slug": s("plan slug")}), &["slug"])),
        ];
        if self.mode == Mode::Ast {
            let sym = s("symbol path, e.g. `Point::new` or `utils::parse`");
            v.extend([
                spec("read_symbol", "Read one item (fn, struct, impl method, ...) of a .rs file.", obj(json!({"path": s("relative .rs path"), "symbol": sym.clone()}), &["path", "symbol"])),
                spec("replace_symbol", "Replace one item of a .rs file with new code (doc comments included); file is rustfmt'd.", obj(json!({"path": s("relative .rs path"), "symbol": sym.clone(), "code": s("complete new item")}), &["path", "symbol", "code"])),
                spec("insert_symbol", "Insert a new item right after an existing one in a .rs file; file is rustfmt'd.", obj(json!({"path": s("relative .rs path"), "after": sym, "code": s("complete new item")}), &["path", "after", "code"])),
                spec("workspace_diagnostics", "Type-check the whole project (every language found: cargo check, tsc, go build, pyright); returns errors and warnings as path:line:col with the source line.", obj(json!({}), &[])),
            ]);
        }
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// Compiles the brief into tokens and locks them in `DESIGN.md`.
    fn design_brief(&self, input: &Value) -> Res {
        use coturbo_design::{compiler, contract};
        let given: Vec<String> =
            input["brand_colors"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
        let brand = if input["new_palette"].as_bool() == Some(true) {
            Vec::new()
        } else if !given.is_empty() {
            given
        } else {
            // A redesign keeps the colors the project already uses.
            let files: Vec<(String, String)> = ignore::WalkBuilder::new(&self.root)
                .build()
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
                .map(|e| self.display(e.path()))
                .filter(|rel| coturbo_design::fiscal::is_ui_file(rel))
                .take(40)
                .filter_map(|rel| Some((rel.clone(), std::fs::read_to_string(self.root.join(&rel)).ok()?)))
                .collect();
            coturbo_design::fiscal::brand_colors(&files)
        };
        let brief = compiler::Brief {
            subject: arg(input, "subject")?.to_string(),
            audience: arg(input, "audience")?.to_string(),
            tone: arg(input, "tone")?.to_string(),
            signature: arg(input, "signature")?.to_string(),
            brand,
        };
        let tokens = compiler::compile(&brief);
        let path = self.root.join(contract::FILE);
        std::fs::write(&path, contract::markdown(&tokens)).map_err(|e| format!("{}: {e}", path.display()))?;
        let kept = if brief.brand.is_empty() {
            String::new()
        } else {
            format!(
                " Brand colors kept and organized: {} (primary = main brand color for fills, primary-ink = the same hue safe for text, \
                 gray-* = neutral scale for hierarchy). Do not drift from the brand.",
                brief.brand.join(", ")
            )
        };
        Ok(ToolOutput::ok(format!(
            "wrote {} (tone: {}).{kept} Use only these tokens: every color, font, size, space, radius and shadow through var(--…); \
             the design check rejects anything else and generic patterns (default fonts/palettes, emoji icons, identical card grids, \
             template section order, one radius everywhere, centered body text).\n\
             Layout: {}.\nSignature element to build around: {}.\nFonts:\n{}\n{}",
            contract::FILE,
            tokens.tone.name(),
            tokens.layout,
            brief.signature,
            contract::font_link(&tokens),
            contract::css(&tokens)
        )))
    }

    /// Renders a page: what the rules and measurements say, and the screenshot.
    async fn design_review(&self, input: &Value) -> Res {
        use coturbo_design::{contract::Contract, fiscal, render};
        let rel = arg(input, "path")?;
        let abs = self.resolve(rel)?;
        let text = std::fs::read_to_string(&abs).map_err(|e| format!("{rel}: {e}"))?;
        let rel = self.display(&abs);
        let mut findings = fiscal::check(&rel, &text, Contract::load(&self.root).as_ref());
        let browser = render::find_browser().ok_or("nenhum navegador encontrado (Edge ou Chrome); defina CODETURBO_BROWSER")?;
        let stem = abs.file_stem().map_or("page".into(), |s| s.to_string_lossy().into_owned());
        let shot = self.root.join(".codeturbo/design").join(format!("{stem}.png"));
        let page = render::measure(&browser, &abs, &shot).await?;
        findings.extend(render::judge(&page));
        let phone = render::mobile_shot(&shot);
        let mut out = ToolOutput::ok(format!(
            "{}\nScreenshots attached: desktop 1440px ({}) and phone 390px ({}). Look at them as a demanding designer: \
             anything overlapping, cramped, cut off, misaligned or scrolling sideways? Does it look generic? \
             Is the signature element the first thing you see? Fix every problem, keeping DESIGN.md, then call design_review again.",
            fiscal::render(&rel, &findings),
            self.display(&shot),
            self.display(&phone)
        ));
        for p in [&shot, &phone] {
            if let Ok(png) = std::fs::read(p) {
                use base64::Engine;
                out.images.push(("image/png".into(), base64::engine::general_purpose::STANDARD.encode(png)));
            }
        }
        Ok(out)
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

    /// Unified diff an edit tool would apply, without writing. `None` for
    /// non-edit tools; an error message if the edit itself would fail.
    pub fn preview(&self, name: &str, input: &Value) -> Option<Result<String, String>> {
        if !EDIT_TOOLS.contains(&name) {
            return None;
        }
        Some(self.plan_edit(name, input).map(|(path, old, new)| {
            let rel = self.display(&path);
            let diff = similar::TextDiff::from_lines(&old, &new)
                .unified_diff()
                .context_radius(3)
                .header(&format!("a/{rel}"), &format!("b/{rel}"))
                .to_string();
            let total = diff.lines().count();
            if total <= PREVIEW_MAX_LINES {
                return diff;
            }
            let mut out: String = diff.lines().take(PREVIEW_MAX_LINES).map(|l| format!("{l}
")).collect();
            out.push_str(&format!("[{} more diff lines]
", total - PREVIEW_MAX_LINES));
            out
        }))
    }

    /// Runs a tool. Failures come back as `is_error` outputs, never panics.
    pub async fn call(&self, name: &str, input: &Value) -> ToolOutput {
        let ast_only = matches!(name, "read_symbol" | "replace_symbol" | "insert_symbol" | "workspace_diagnostics");
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
            "workspace_diagnostics" => {
                let (text, clean) = coturbo_verify::workspace::render(&coturbo_verify::workspace::check(&self.root).await);
                Ok(if clean { ToolOutput::ok(text) } else { ToolOutput::err(text) })
            }
            "propose_plan" => Err("plan mode is not active".into()),
            "design_brief" => self.design_brief(input),
            "design_review" => self.design_review(input).await,
            _ => Err(format!("unknown tool: {name}")),
        };
        let mut out = r.unwrap_or_else(ToolOutput::err);
        if self.native_checks
            && !out.is_error
            && EDIT_TOOLS.contains(&name)
            && let Some(p) = input["path"].as_str()
            && let Ok(abs) = self.resolve(p)
            && let Ok(text) = std::fs::read_to_string(&abs)
        {
            let rel = self.display(&abs);
            if let Some(diags) = coturbo_verify::native::check(&rel, &text) {
                out.content.push('\n');
                out.content.push_str(&native_note(&rel, &diags));
            }
            if coturbo_design::fiscal::is_ui_file(&rel) {
                let contract = coturbo_design::contract::Contract::load(&self.root);
                let findings = coturbo_design::fiscal::check(&rel, &text, contract.as_ref());
                out.content.push('\n');
                out.content.push_str(&coturbo_design::fiscal::render(&rel, &findings));
            }
        }
        if let Some(lsp) = &self.lsp {
            if !out.is_error
                && EDIT_TOOLS.contains(&name)
                && let Some(p) = input["path"].as_str()
                && let Ok(abs) = self.resolve(p)
            {
                let notes = lsp.after_write(&self.display(&abs)).await;
                if notes.formatted {
                    out.content.push_str(" (formatted)");
                }
                if !notes.text.is_empty() {
                    out.content.push('\n');
                    out.content.push_str(&notes.text);
                }
            }
            for late in lsp.drain_late() {
                out.content.push('\n');
                out.content.push_str(&late);
            }
        }
        out
    }
}

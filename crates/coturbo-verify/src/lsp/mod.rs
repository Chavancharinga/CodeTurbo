//! Diagnostics on write, modelled on oh-my-pi's `lsp.diagnosticsOnWrite` /
//! `lsp.formatOnWrite`: after every edit the file is formatted and its errors
//! and warnings ride along in the edit's own result. A slow server never
//! blocks the edit: diagnostics that miss the inline window are delivered
//! with the next tool result instead.

pub mod client;
pub mod rpc;

use crate::diag::{self, Diag};
use client::Client;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const MAX_DIAGS: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub struct ServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    /// The server is used only if one of these exists at the project root.
    pub root_markers: Vec<String>,
    /// File extensions it handles, without the dot.
    pub extensions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LspConfig {
    pub servers: Vec<ServerConfig>,
    pub format_on_write: bool,
    /// How long an edit waits for fresh diagnostics.
    pub inline_wait: Duration,
    /// How long a late fetch keeps waiting after the edit returned.
    pub late_wait: Duration,
    pub request_timeout: Duration,
}

fn server(
    name: &str,
    command: &str,
    args: &[&str],
    markers: &[&str],
    exts: &[&str],
) -> ServerConfig {
    let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect();
    ServerConfig {
        name: name.into(),
        command: command.into(),
        args: v(args),
        root_markers: v(markers),
        extensions: v(exts),
    }
}

impl Default for LspConfig {
    fn default() -> Self {
        Self {
            servers: vec![
                server(
                    "rust-analyzer",
                    "rust-analyzer",
                    &[],
                    &["Cargo.toml", "rust-analyzer.toml"],
                    &["rs"],
                ),
                server(
                    "typescript-language-server",
                    "typescript-language-server",
                    &["--stdio"],
                    &["tsconfig.json", "jsconfig.json", "package.json"],
                    &["ts", "tsx", "js", "jsx", "mts", "cts", "mjs", "cjs"],
                ),
            ],
            format_on_write: true,
            inline_wait: Duration::from_millis(1500),
            late_wait: Duration::from_secs(10),
            request_timeout: Duration::from_secs(10),
        }
    }
}

pub fn language_id(ext: &str) -> &'static str {
    match ext {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "py" => "python",
        "go" => "go",
        _ => "plaintext",
    }
}

/// Byte offset of an LSP position (UTF-16 columns) in `text`.
fn offset(text: &str, line: usize, character: usize) -> usize {
    let mut start = 0;
    for _ in 0..line {
        match text[start..].find('\n') {
            Some(i) => start += i + 1,
            None => return text.len(),
        }
    }
    let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    let mut units = 0;
    for (i, c) in text[start..end].char_indices() {
        if units >= character {
            return start + i;
        }
        units += c.len_utf16();
    }
    end
}

/// Applies LSP `TextEdit`s (non-overlapping, any order).
pub fn apply_edits(text: &str, edits: &[Value]) -> String {
    let mut spans: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|e| {
            let r = &e["range"];
            let pos = |p: &Value| {
                offset(
                    text,
                    p["line"].as_u64().unwrap_or(0) as usize,
                    p["character"].as_u64().unwrap_or(0) as usize,
                )
            };
            (
                pos(&r["start"]),
                pos(&r["end"]),
                e["newText"].as_str().unwrap_or(""),
            )
        })
        .collect();
    spans.sort_by_key(|a| std::cmp::Reverse(a.0));
    let mut out = text.to_string();
    for (s, e, new) in spans {
        if s <= e && e <= out.len() {
            out.replace_range(s..e, new);
        }
    }
    out
}

/// What an edit tool appends to its result.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct WriteNotes {
    /// The file was reformatted on disk.
    pub formatted: bool,
    /// Diagnostics or status lines for the model.
    pub text: String,
}

pub struct LspManager {
    root: PathBuf,
    cfg: LspConfig,
    clients: tokio::sync::Mutex<HashMap<String, Arc<Client>>>,
    /// Servers that failed to start: reported once, then left alone.
    broken: Mutex<HashSet<String>>,
    late: Arc<Mutex<Vec<String>>>,
}

fn render(rel: &str, diags: &[Diag]) -> String {
    if diags.is_empty() {
        return format!("lsp {rel}: clean");
    }
    let (e, w) = (
        diag::count(diags, diag::Level::Error),
        diag::count(diags, diag::Level::Warning),
    );
    format!(
        "lsp {rel}: {e} error(s), {w} warning(s)\n{}",
        diag::render_list(diags, MAX_DIAGS)
    )
}

impl LspManager {
    pub fn new(root: &Path, cfg: LspConfig) -> Self {
        Self {
            root: root.to_path_buf(),
            cfg,
            clients: tokio::sync::Mutex::new(HashMap::new()),
            broken: Mutex::new(HashSet::new()),
            late: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn format_on_write(&self) -> bool {
        self.cfg.format_on_write
    }

    fn server_for(&self, ext: &str) -> Option<&ServerConfig> {
        self.cfg.servers.iter().find(|s| {
            s.extensions.iter().any(|e| e == ext)
                && s.root_markers.iter().any(|m| self.root.join(m).exists())
        })
    }

    /// Running client for `server`, restarting a dead one. `Err` once per
    /// server when it cannot be started.
    async fn client(&self, server: &ServerConfig) -> Result<Option<Arc<Client>>, String> {
        if self.broken.lock().unwrap().contains(&server.name) {
            return Ok(None);
        }
        let mut clients = self.clients.lock().await;
        if let Some(c) = clients.get(&server.name)
            && c.alive()
        {
            return Ok(Some(c.clone()));
        }
        match Client::start(
            &server.command,
            &server.args,
            &self.root,
            self.cfg.request_timeout,
        )
        .await
        {
            Ok(c) => {
                clients.insert(server.name.clone(), c.clone());
                Ok(Some(c))
            }
            Err(e) => {
                self.broken.lock().unwrap().insert(server.name.clone());
                Err(format!(
                    "[lsp] {} not available ({e}); built-in checks only — install it for type-level diagnostics",
                    server.name
                ))
            }
        }
    }

    /// Formats `rel` (non-Rust files, when the server can) and returns its
    /// diagnostics, or arranges for them to arrive late.
    pub async fn after_write(&self, rel: &str) -> WriteNotes {
        let mut notes = WriteNotes::default();
        let ext = rel.rsplit('.').next().unwrap_or("");
        let Some(server) = self.server_for(ext) else {
            return notes;
        };
        let client = match self.client(server).await {
            Ok(Some(c)) => c,
            Ok(None) => return notes,
            Err(e) => {
                notes.text = e;
                return notes;
            }
        };
        let path = self.root.join(rel);
        let Ok(mut text) = std::fs::read_to_string(&path) else {
            return notes;
        };
        let lang = language_id(ext);
        let Ok((mut version, mut seq)) = client.sync(&path, lang, &text).await else {
            return notes;
        };

        let can_format = !client.capabilities()["documentFormattingProvider"].is_null()
            && client.capabilities()["documentFormattingProvider"] != false;
        if self.cfg.format_on_write && ext != "rs" && can_format {
            let params = json!({
                "textDocument": {"uri": rpc::path_to_uri(&path)},
                "options": {"tabSize": 4, "insertSpaces": true}
            });
            if let Ok(Value::Array(edits)) = client
                .request("textDocument/formatting", params, self.cfg.request_timeout)
                .await
            {
                let formatted = apply_edits(&text, &edits);
                if formatted != text && std::fs::write(&path, &formatted).is_ok() {
                    text = formatted;
                    notes.formatted = true;
                    if let Ok(vs) = client.sync(&path, lang, &text).await {
                        (version, seq) = vs;
                    }
                }
            }
        }

        if let Some(d) = client
            .diagnostics(&path, version, seq, self.cfg.inline_wait)
            .await
        {
            notes.text = render(rel, &d);
            return notes;
        }
        // Too slow for the edit: deliver with a later tool result.
        let (late, owned, wait) = (self.late.clone(), rel.to_string(), self.cfg.late_wait);
        tokio::spawn(async move {
            if let Some(d) = client.diagnostics(&path, version, seq, wait).await
                && !d.is_empty()
            {
                late.lock()
                    .unwrap()
                    .push(format!("[late] {}", render(&owned, &d)));
            }
        });
        notes.text = format!("lsp {rel}: pending (will follow with a later result)");
        notes
    }

    /// Code navigation through the language server: `definition`,
    /// `references` or `hover` for `symbol` on line `line` (1-based) of `rel`.
    pub async fn navigate(
        &self,
        action: &str,
        rel: &str,
        line: u32,
        symbol: &str,
    ) -> Result<String, String> {
        let ext = rel.rsplit('.').next().unwrap_or("");
        let server = self.server_for(ext).ok_or_else(|| {
            format!("no language server is set up for .{ext} files in this project")
        })?;
        let client = self
            .client(server)
            .await?
            .ok_or("the language server is not available")?;
        let path = self.root.join(rel);
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{rel}: {e}"))?;
        let row = text
            .lines()
            .nth(line.saturating_sub(1) as usize)
            .ok_or_else(|| format!("{rel} has no line {line}"))?;
        let byte = row
            .find(symbol)
            .ok_or_else(|| format!("`{symbol}` is not on line {line} of {rel}"))?;
        // LSP columns count UTF-16 units.
        let character = row[..byte].encode_utf16().count();
        client.sync(&path, language_id(ext), &text).await?;
        let position = json!({"textDocument": {"uri": rpc::path_to_uri(&path)}, "position": {"line": line.saturating_sub(1), "character": character}});
        let (method, params) = match action {
            "definition" => ("textDocument/definition", position),
            "references" => {
                let mut p = position;
                p["context"] = json!({"includeDeclaration": true});
                ("textDocument/references", p)
            }
            "hover" => ("textDocument/hover", position),
            other => {
                return Err(format!(
                    "unknown action `{other}`: definition, references or hover"
                ));
            }
        };
        let result = client
            .request(method, params, self.cfg.request_timeout)
            .await?;
        if action == "hover" {
            let c = &result["contents"];
            let text = c["value"]
                .as_str()
                .map(str::to_string)
                .or_else(|| c.as_str().map(str::to_string))
                .unwrap_or_else(|| {
                    c.as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|p| p["value"].as_str().or(p.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n")
                });
            return Ok(if text.trim().is_empty() {
                format!("no information for `{symbol}`")
            } else {
                text
            });
        }
        let locations: Vec<Value> = match result {
            Value::Array(a) => a,
            Value::Null => Vec::new(),
            one => vec![one],
        };
        let lines: Vec<String> = locations
            .iter()
            .take(60)
            .filter_map(|l| {
                let uri = l["uri"].as_str().or(l["targetUri"].as_str())?;
                let range = if l["range"].is_object() {
                    &l["range"]
                } else {
                    &l["targetSelectionRange"]
                };
                let file = rpc::uri_key(uri);
                // Keys are normalised (lower case on Windows): compare the same way.
                let root_key = rpc::uri_key(&self.root.to_string_lossy());
                let shown = file
                    .strip_prefix(&root_key)
                    .map(|r| r.trim_start_matches('/').to_string())
                    .unwrap_or_else(|| file.clone());
                let n = range["start"]["line"].as_u64()? + 1;
                let src = std::fs::read_to_string(&file)
                    .ok()
                    .and_then(|t| t.lines().nth(n as usize - 1).map(|s| s.trim().to_string()))
                    .unwrap_or_default();
                Some(format!("{shown}:{n}: {src}"))
            })
            .collect();
        if lines.is_empty() {
            return Ok(format!("no {action} found for `{symbol}`"));
        }
        Ok(format!(
            "{} {action} of `{symbol}`:\n{}",
            lines.len(),
            lines.join("\n")
        ))
    }

    /// Late diagnostics that arrived since the last call.
    pub fn drain_late(&self) -> Vec<String> {
        std::mem::take(&mut *self.late.lock().unwrap())
    }
}

//! Subscription plans through the official tools: the `claude` (Claude
//! Code) and `codex` (Codex CLI) programs the user already installed and
//! signed in to. CodeTurbo runs them in print mode with their own tools
//! off and its own instructions; the model asks for CodeTurbo's tools in
//! a small text protocol, and CodeTurbo runs them. The plan is used by the
//! official tool, as its terms allow.

use crate::{BoxFuture, ContentBlock, Delta, Provider, ProviderError, Request, Response, Role, TextSink, Usage};
use serde_json::{Value, json};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    ClaudeCode,
    Codex,
}

impl Tool {
    pub fn program(self) -> &'static str {
        match self {
            Tool::ClaudeCode => "claude",
            Tool::Codex => "codex",
        }
    }
}

pub struct CliProvider {
    tool: Tool,
}

impl CliProvider {
    pub fn new(tool: Tool) -> Self {
        Self { tool }
    }
}

/// Whether the official program is installed (on the PATH).
pub fn installed(tool: Tool) -> bool {
    let exts: &[&str] = if cfg!(windows) { &["exe", "cmd", "bat"] } else { &[""] };
    std::env::var_os("PATH").is_some_and(|p| {
        std::env::split_paths(&p).any(|d| {
            exts.iter().any(|e| {
                let f = if e.is_empty() { d.join(tool.program()) } else { d.join(format!("{}.{e}", tool.program())) };
                f.is_file()
            })
        })
    })
}

const OPEN: &str = "<tool_call>";
const CLOSE: &str = "</tool_call>";

/// The system prompt plus how to call tools in plain text.
pub fn protocol_system(req: &Request) -> String {
    let tools: Vec<Value> = req
        .tools
        .iter()
        .map(|t| json!({"name": t.name, "description": t.description, "input": t.input_schema["properties"]}))
        .collect();
    format!(
        "{}\n\nTools (you cannot run anything yourself; ask and the harness runs it):\n{}\n\n\
         To use tools, write one block per call, exactly:\n{OPEN}{{\"name\": \"read_file\", \"input\": {{\"path\": \"src/main.rs\"}}}}{CLOSE}\n\
         You may write several blocks; their results come back in the next message. \
         When the task is done, answer in plain text with no {OPEN} block.",
        req.system,
        serde_json::to_string(&tools).unwrap_or_default()
    )
}

/// The conversation as one text prompt.
pub fn protocol_prompt(req: &Request) -> String {
    let mut out = String::new();
    let total = image_count(req);
    let mut seen = 0usize;
    for m in &req.messages {
        for b in &m.content {
            match (m.role, b) {
                (_, ContentBlock::Image { .. }) => {
                    seen += 1;
                    // Only the latest images are kept: each one costs a read.
                    if total - seen < MAX_IMAGES {
                        let name = image_name(seen - 1);
                        out.push_str(&format!("\n[image attached: ./{name} - look at it with your own built-in Read tool (not a <tool_call>) before answering]\n"));
                    } else {
                        out.push_str("\n[an earlier image was omitted]\n");
                    }
                }
                (Role::User, ContentBlock::Text { text }) => out.push_str(&format!("\n[user]\n{text}\n")),
                (Role::Assistant, ContentBlock::Text { text }) => out.push_str(&format!("\n[assistant]\n{text}\n")),
                (_, ContentBlock::ToolUse { name, input, .. }) => {
                    out.push_str(&format!("{OPEN}{}{CLOSE}\n", json!({"name": name, "input": input})));
                }
                (_, ContentBlock::ToolResult { content, is_error, .. }) => {
                    let tag = if *is_error { "tool error" } else { "tool result" };
                    out.push_str(&format!("\n[{tag}]\n{content}\n"));
                }
                _ => {}
            }
        }
    }
    out.push_str("\n[assistant]\n");
    out
}

/// How many of the latest images stay visible to the model.
const MAX_IMAGES: usize = 4;

fn image_count(req: &Request) -> usize {
    req.messages.iter().flat_map(|m| &m.content).filter(|b| matches!(b, ContentBlock::Image { .. })).count()
}

fn image_name(i: usize) -> String {
    format!("vision-{}-{i}.png", std::process::id())
}

/// The images of the conversation (the latest ones), decoded, with the file
/// name each is referenced by in the prompt.
pub fn prompt_images(req: &Request) -> Vec<(String, Vec<u8>)> {
    use base64::Engine;
    let all: Vec<&str> = req
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            ContentBlock::Image { data, .. } => Some(data.as_str()),
            _ => None,
        })
        .collect();
    let first_kept = all.len().saturating_sub(MAX_IMAGES);
    all.iter()
        .enumerate()
        .skip(first_kept)
        .filter_map(|(i, d)| base64::engine::general_purpose::STANDARD.decode(d).ok().map(|b| (image_name(i), b)))
        .collect()
}

/// Tool calls and plain text of a reply in the text protocol.
pub fn parse_reply(text: &str, turn: usize) -> Vec<ContentBlock> {
    let mut blocks = Vec::new();
    let mut prose = String::new();
    let mut rest = text;
    let mut n = 0;
    loop {
        // A tool call may come as our <tool_call>{json} or as Claude's
        // native <invoke name="..."><parameter name="...">...</invoke>.
        let json_at = rest.find(OPEN);
        let xml_at = rest.find("<invoke ").filter(|&i| json_at.is_none_or(|j| i < j));
        if let Some(start) = xml_at {
            let after = &rest[start..];
            let Some(end) = after.find("</invoke>") else {
                prose.push_str(&rest[..start]);
                rest = "";
                break;
            };
            prose.push_str(&rest[..start]);
            let block = &after[..end];
            if let Some((name, input)) = parse_invoke(block) {
                n += 1;
                blocks.push(ContentBlock::ToolUse { id: format!("cli-{turn}-{n}"), name, input });
            }
            rest = &after[end + "</invoke>".len()..];
            continue;
        }
        let Some(start) = json_at else { break };
        prose.push_str(&rest[..start]);
        let after = &rest[start + OPEN.len()..];
        let Some(end) = after.find(CLOSE) else {
            prose.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let raw = after[..end].trim();
        // Claude sometimes puts <invoke> inside <tool_call>.
        if raw.contains("<invoke ") {
            if let Some((name, input)) = parse_invoke(raw) {
                n += 1;
                blocks.push(ContentBlock::ToolUse { id: format!("cli-{turn}-{n}"), name, input });
            }
        } else {
            let v: Value = serde_json::from_str(raw).unwrap_or_else(|_| json!({"name": "", "input": {crate::INVALID_JSON_KEY: raw}}));
            let name = v["name"].as_str().unwrap_or("").to_string();
            let input = if v["input"].is_object() { v["input"].clone() } else { json!({}) };
            n += 1;
            blocks.push(ContentBlock::ToolUse { id: format!("cli-{turn}-{n}"), name, input });
        }
        rest = &after[end + CLOSE.len()..];
    }
    prose.push_str(rest);
    // Strip stray protocol tags the model left in prose.
    let prose = prose.replace(OPEN, "").replace(CLOSE, "").trim().to_string();
    if !prose.is_empty() {
        blocks.insert(0, ContentBlock::Text { text: prose });
    }
    blocks
}

/// Parses Claude's native `<invoke name="tool">` ... `<parameter name="k">v</parameter>` form.
fn parse_invoke(block: &str) -> Option<(String, Value)> {
    let name_start = block.find("name=\"")? + 6;
    let name = block[name_start..].split('"').next()?.to_string();
    let mut input = serde_json::Map::new();
    let mut rest = block;
    while let Some(ps) = rest.find("<parameter name=\"") {
        let after = &rest[ps + "<parameter name=\"".len()..];
        let Some(key_end) = after.find('"') else { break };
        let key = after[..key_end].to_string();
        let after_key = &after[key_end + 1..];
        let Some(gt) = after_key.find('>') else { break };
        let value_part = &after_key[gt + 1..];
        let Some(close) = value_part.find("</parameter>") else { break };
        let raw = &value_part[..close];
        // A value may be JSON (object/array/number/bool) or plain text.
        let value = serde_json::from_str::<Value>(raw.trim()).unwrap_or_else(|_| Value::String(raw.to_string()));
        input.insert(key, value);
        rest = &value_part[close + "</parameter>".len()..];
    }
    Some((name, Value::Object(input)))
}

/// Usage reported by `claude -p --output-format json`.
fn claude_usage(v: &Value) -> Usage {
    let u = &v["usage"];
    let n = |k: &str| u[k].as_u64().unwrap_or(0);
    Usage {
        input: n("input_tokens"),
        cache_write: n("cache_creation_input_tokens"),
        cache_read: n("cache_read_input_tokens"),
        output: n("output_tokens"),
    }
}

impl Provider for CliProvider {
    fn complete<'a>(&'a self, req: &'a Request, on_text: &'a mut TextSink<'_>) -> BoxFuture<'a, Result<Response, ProviderError>> {
        Box::pin(async move {
            let system = protocol_system(req);
            let prompt = protocol_prompt(req);
            // Outside the project: the official tool must not load its own
            // project instructions or touch files.
            let work = std::env::temp_dir().join("codeturbo-cli");
            let _ = std::fs::create_dir_all(&work);
            let last = work.join(format!("ultima-{}.txt", std::process::id()));
            // Claude Code can look at images through its Read tool, limited
            // to this work folder; it stays off when there is nothing to see.
            let images = if self.tool == Tool::ClaudeCode { prompt_images(req) } else { Vec::new() };
            for (name, bytes) in &images {
                let _ = std::fs::write(work.join(name), bytes);
            }
            let mut cmd = tokio::process::Command::new(self.tool.program());
            match self.tool {
                Tool::ClaudeCode => {
                    let tools = if images.is_empty() { "" } else { "Read" };
                    cmd.args(["-p", "--output-format", "json", "--tools", tools, "--no-session-persistence", "--strict-mcp-config"]);
                    if !images.is_empty() {
                        cmd.args(["--allowedTools", "Read(./**)"]);
                    }
                    cmd.arg("--system-prompt").arg(&system);
                    if !req.model.is_empty() && req.model != "padrao" {
                        cmd.arg("--model").arg(&req.model);
                    }
                }
                Tool::Codex => {
                    cmd.args(["exec", "--skip-git-repo-check", "--sandbox", "read-only", "--output-last-message"]).arg(&last);
                    if !req.model.is_empty() && req.model != "padrao" {
                        cmd.arg("--model").arg(&req.model);
                    }
                    cmd.arg("-");
                }
            }
            cmd.current_dir(&work).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
            let mut child = cmd.spawn().map_err(|e| {
                ProviderError::Other(format!("não foi possível iniciar `{}` ({e}); instale e faça login na ferramenta oficial", self.tool.program()))
            })?;
            let input = match self.tool {
                Tool::ClaudeCode => prompt,
                Tool::Codex => format!("{system}\n\n{prompt}"),
            };
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(input.as_bytes()).await.map_err(|e| ProviderError::Other(e.to_string()))?;
            }
            let out = tokio::time::timeout(Duration::from_secs(600), child.wait_with_output())
                .await
                .map_err(|_| ProviderError::Other("a ferramenta oficial não respondeu em 10 minutos".into()))?
                .map_err(|e| ProviderError::Other(e.to_string()))?;
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let (text, usage) = match self.tool {
                Tool::ClaudeCode => {
                    let v: Value = serde_json::from_str(stdout.trim()).map_err(|_| {
                        ProviderError::Other(format!("resposta inesperada do claude: {}", String::from_utf8_lossy(&out.stderr).trim()))
                    })?;
                    if v["is_error"].as_bool() == Some(true) {
                        return Err(ProviderError::Other(format!("claude: {}", v["result"].as_str().unwrap_or("erro"))));
                    }
                    (v["result"].as_str().unwrap_or("").to_string(), claude_usage(&v))
                }
                Tool::Codex => {
                    let text = std::fs::read_to_string(&last).unwrap_or(stdout);
                    let _ = std::fs::remove_file(&last);
                    if !out.status.success() && text.trim().is_empty() {
                        return Err(ProviderError::Other(format!("codex: {}", String::from_utf8_lossy(&out.stderr).trim())));
                    }
                    // Codex does not report tokens here: an estimate.
                    let est = |s: &str| (s.len() / 4) as u64;
                    let usage = Usage { input: est(&input), cache_write: 0, cache_read: 0, output: est(&text) };
                    (text, usage)
                }
            };
            let content = parse_reply(&text, req.messages.len());
            for b in &content {
                if let ContentBlock::Text { text } = b {
                    on_text(Delta::Text(text.clone()));
                }
            }
            let calls = content.iter().any(|b| matches!(b, ContentBlock::ToolUse { .. }));
            Ok(Response { content, stop_reason: if calls { "tool_use".into() } else { "end_turn".into() }, usage })
        })
    }
}

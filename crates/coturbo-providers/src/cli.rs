//! Subscription plans through the official tools: the `claude` (Claude
//! Code) and `codex` (Codex CLI) programs the user already installed and
//! signed in to. CodeTurbo runs them in print mode with their own tools
//! off and its own instructions; the model asks for CodeTurbo's tools in
//! a small text protocol, and CodeTurbo runs them. The plan is used by the
//! official tool, as its terms allow.

use crate::reply_stream::ReplyStream;
use crate::{BoxFuture, ContentBlock, Delta, Provider, ProviderError, Request, Response, Role, TextSink, Usage};
use serde_json::{Value, json};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    ClaudeCode,
    Codex,
    /// Antigravity CLI (`agy`), signed in with the user's Google account.
    Antigravity,
}

impl Tool {
    pub fn program(self) -> &'static str {
        match self {
            Tool::ClaudeCode => "claude",
            Tool::Codex => "codex",
            Tool::Antigravity => "agy",
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
    on_path(tool.program()) || (tool == Tool::Antigravity && agy_path().is_some())
}

/// Where the Antigravity installer may have put `agy` when its folder is not on the PATH.
fn agy_homes() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        out.push(std::path::PathBuf::from(home).join(".local").join("bin"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        out.push(std::path::PathBuf::from(local).join("agy").join("bin"));
    }
    out
}

/// The `agy` program, wherever it is installed.
pub fn agy_path() -> Option<std::path::PathBuf> {
    agy_homes().into_iter().flat_map(|d| [d.join("agy"), d.join("agy.exe")]).find(|f| f.is_file())
}

/// The program to run: the one on the PATH, or where the Antigravity
/// installer puts `agy` when that folder is not on it.
pub fn resolve(tool: Tool) -> std::path::PathBuf {
    let name = tool.program();
    if tool == Tool::Antigravity
        && !on_path(name)
        && let Some(f) = agy_path()
    {
        return f;
    }
    name.into()
}

/// Whether `program` is on the PATH.
pub fn on_path(program: &str) -> bool {
    let exts: &[&str] = if cfg!(windows) { &["exe", "cmd", "bat"] } else { &[""] };
    std::env::var_os("PATH").is_some_and(|p| {
        std::env::split_paths(&p).any(|d| {
            exts.iter().any(|e| {
                let f = if e.is_empty() { d.join(program) } else { d.join(format!("{program}.{e}")) };
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

/// The conversation as one text prompt. Images are not text: the latest ones
/// travel inside the message itself (see [`prompt_images`]) and the prompt
/// says which is which.
pub fn protocol_prompt(req: &Request) -> String {
    let mut out = String::new();
    let total = image_count(req);
    let first_kept = total.saturating_sub(MAX_IMAGES);
    let mut seen = 0usize;
    for m in &req.messages {
        for b in &m.content {
            match (m.role, b) {
                (_, ContentBlock::Image { .. }) => {
                    seen += 1;
                    if seen > first_kept {
                        out.push_str(&format!(
                            "\n[image {} of {} is attached to this message, after the text]\n",
                            seen - first_kept,
                            total - first_kept
                        ));
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

/// The latest images of the conversation as `(media type, base64 data)`.
pub fn prompt_images(req: &Request) -> Vec<(String, String)> {
    let all: Vec<(String, String)> = req
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            ContentBlock::Image { media_type, data } => Some((media_type.clone(), data.clone())),
            _ => None,
        })
        .collect();
    let first_kept = all.len().saturating_sub(MAX_IMAGES);
    all.into_iter().skip(first_kept).collect()
}

/// The single line Claude Code reads in streaming-input mode: the prompt as
/// text, then the images inline. With no tools on, the model simply sees them.
pub fn stream_message(prompt: &str, images: &[(String, String)]) -> String {
    let mut content = vec![json!({"type": "text", "text": prompt})];
    content.extend(images.iter().map(|(media, data)| {
        json!({"type": "image", "source": {"type": "base64", "media_type": media, "data": data}})
    }));
    format!("{}\n", json!({"type": "user", "message": {"role": "user", "content": content}}))
}

/// The final `result` message of Claude Code's streaming output.
pub fn result_message(stdout: &str) -> Option<Value> {
    stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
        .rfind(|v| v["type"] == "result")
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

/// Reads Claude Code's streamed output while the reply is written: prose and
/// reasoning go to the screen as they arrive, tool calls as progress. Returns
/// the full reply text, its usage, and whether anything was already shown.
async fn read_claude(child: &mut tokio::process::Child, on_text: &mut TextSink<'_>) -> Result<(String, Usage, bool), ProviderError> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    let other = |e: String| ProviderError::Other(e);
    let stdout = child.stdout.take().ok_or_else(|| other("claude did not open its output".into()))?;
    let mut lines = BufReader::new(stdout).lines();
    let mut stream = ReplyStream::default();
    let mut shown = false;
    let mut result: Option<Value> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
    loop {
        let line = match tokio::time::timeout_at(deadline, lines.next_line()).await {
            Err(_) => return Err(other("the official tool did not answer in 10 minutes".into())),
            Ok(Err(e)) => return Err(other(e.to_string())),
            Ok(Ok(None)) => break,
            Ok(Ok(Some(l))) => l,
        };
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { continue };
        match v["type"].as_str() {
            Some("stream_event") if v["event"]["type"] == "content_block_delta" => {
                let d = &v["event"]["delta"];
                match d["type"].as_str() {
                    Some("text_delta") => {
                        for delta in stream.push(d["text"].as_str().unwrap_or("")) {
                            shown = true;
                            on_text(delta);
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(t) = d["thinking"].as_str().filter(|t| !t.is_empty()) {
                            on_text(Delta::Thinking(t.to_string()));
                        }
                    }
                    _ => {}
                }
            }
            Some("result") => {
                result = Some(v);
                break;
            }
            _ => {}
        }
    }
    for delta in stream.finish() {
        shown = true;
        on_text(delta);
    }
    let Some(v) = result else {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = tokio::time::timeout(Duration::from_millis(500), e.read_to_string(&mut err)).await;
        }
        return Err(other(format!("resposta inesperada do claude: {}", err.trim())));
    };
    if v["is_error"].as_bool() == Some(true) {
        return Err(other(format!("claude: {}", v["result"].as_str().unwrap_or("erro"))));
    }
    let text = v["result"].as_str().filter(|t| !t.is_empty()).map_or_else(|| stream.text().to_string(), str::to_string);
    Ok((text, claude_usage(&v), shown))
}

/// The reply and token counts from the JSON `agy -p --output-format json` prints.
pub fn parse_antigravity(body: &str) -> Result<(String, Usage), String> {
    let v: Value = serde_json::from_str(body.trim()).map_err(|_| "resposta inesperada".to_string())?;
    if v["status"].as_str().is_some_and(|s| s != "SUCCESS") || v["error"].is_string() {
        return Err(v["error"].as_str().unwrap_or("the run failed").to_string());
    }
    let n = |k: &str| v["usage"][k].as_u64().unwrap_or(0);
    let usage = Usage { input: n("input_tokens").saturating_sub(n("cache_read_tokens")), cache_write: 0, cache_read: n("cache_read_tokens"), output: n("output_tokens") };
    Ok((v["response"].as_str().unwrap_or_default().to_string(), usage))
}

/// What `agy -p --output-format stream-json` printed: the final result, and
/// whether the model reached for Antigravity's own tools instead of ours.
pub fn read_antigravity_stream(body: &str) -> Result<(String, Usage, bool), String> {
    let mut native = false;
    let mut result = None;
    for line in body.lines().filter(|l| l.trim_start().starts_with('{')) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v["event"].as_str() {
            Some("step_update") if v["step_update"]["step_type"] == "tool" => native = true,
            Some("result") => result = Some(v["result"].to_string()),
            _ => {}
        }
    }
    let (text, usage) = parse_antigravity(&result.ok_or("no result")?)?;
    Ok((text, usage, native))
}

/// One reply through Antigravity. Its own tools are not wanted here (the work
/// goes through CodeTurbo's); when the model uses them anyway, it is told so
/// and asked again, up to twice.
async fn agy_complete(req: &Request, system: &str, prompt: &str, work: &std::path::Path, last: &std::path::Path, on_text: &mut TextSink<'_>) -> Result<Response, ProviderError> {
    let mut nudge = String::new();
    let mut total = Usage::default();
    for attempt in 0..3 {
        let full = format!(
            "IMPORTANT: you are running inside a harness. Your built-in tools (file writing, shell, browsing) are DISABLED and must not be used;              anything you do with them is lost. To act, write <tool_call> blocks exactly as described below and nothing else.

{system}

{prompt}{nudge}"
        );
        // Print mode takes the prompt as an argument; a long one goes through a
        // file, since command lines have a size limit.
        let arg = if full.len() > 24_000 {
            std::fs::write(last, &full).map_err(|e| ProviderError::Other(e.to_string()))?;
            format!("Read the file {} (instructions and conversation) and answer it following exactly the format it asks for; do not use other tools.", last.display())
        } else {
            full
        };
        let mut cmd = tokio::process::Command::new(resolve(Tool::Antigravity));
        cmd.arg("-p").arg(arg).args(["--output-format", "stream-json"]);
        if !req.model.is_empty() && req.model != "padrao" {
            cmd.arg("--model").arg(&req.model);
        }
        cmd.current_dir(work).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let child = cmd.spawn().map_err(|e| ProviderError::Other(format!("could not start `agy` ({e}); install the Antigravity CLI and sign in")))?;
        let out = tokio::time::timeout(Duration::from_secs(600), child.wait_with_output())
            .await
            .map_err(|_| ProviderError::Other("Antigravity did not answer in 10 minutes".into()))?
            .map_err(|e| ProviderError::Other(e.to_string()))?;
        let _ = std::fs::remove_file(last);
        let body = String::from_utf8_lossy(&out.stdout).into_owned();
        let (text, usage, native) = read_antigravity_stream(&body)
            .map_err(|e| ProviderError::Other(format!("agy: {e} {}", String::from_utf8_lossy(&out.stderr).trim())))?;
        total.input += usage.input;
        total.cache_read += usage.cache_read;
        total.output += usage.output;
        let has_calls = text.contains(OPEN) || text.contains("<invoke ");
        if native && !has_calls && attempt < 2 {
            nudge = "

(Your previous answer used built-in tools, which do nothing here. Answer again using only <tool_call> blocks.)".into();
            continue;
        }
        // After tool results, Gemini often ends with no text at all: that is the
        // end of the task, not a reason to ask again for minutes.
        let text = if text.trim().is_empty() && req.messages.len() > 2 { "Done.".to_string() } else { text };
        let content = parse_reply(&text, req.messages.len());
        for b in &content {
            if let ContentBlock::Text { text } = b {
                on_text(Delta::Text(text.clone()));
            }
        }
        let calls = content.iter().any(|b| matches!(b, ContentBlock::ToolUse { .. }));
        return Ok(Response { content, stop_reason: if calls { "tool_use".into() } else { "end_turn".into() }, usage: total });
    }
    Err(ProviderError::Other("Antigravity kept using its own tools".into()))
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
            // Claude Code sees images when they ride inside the message
            // (streaming input); with none, the plain text prompt is used.
            let images = if self.tool == Tool::ClaudeCode { prompt_images(req) } else { Vec::new() };
            let streaming = !images.is_empty();
            if self.tool == Tool::Antigravity {
                return agy_complete(req, &system, &prompt, &work, &last, on_text).await;
            }
            let mut cmd = tokio::process::Command::new(resolve(self.tool));
            match self.tool {
                Tool::ClaudeCode => {
                    cmd.args(["-p", "--tools", "", "--no-session-persistence", "--strict-mcp-config"]);
                    // The reply is read as it is written; images need the
                    // streaming input form, plain text does not.
                    cmd.args(["--output-format", "stream-json", "--include-partial-messages", "--verbose"]);
                    if streaming {
                        cmd.args(["--input-format", "stream-json"]);
                    }
                    // Without these the official tool flushes its telemetry after
                    // every reply, which costs about two seconds each time. Only
                    // this child process is affected, not the user's own `claude`.
                    cmd.env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1").env("DISABLE_TELEMETRY", "1");
                    cmd.arg("--system-prompt").arg(&system);
                    if !req.model.is_empty() && req.model != "padrao" {
                        cmd.arg("--model").arg(&req.model);
                    }
                }
                Tool::Antigravity => unreachable!("handled by agy_complete"),
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
                ProviderError::Other(format!("could not start `{}` ({e}); install the official tool and sign in to it", self.tool.program()))
            })?;
            let input = match self.tool {
                Tool::ClaudeCode if streaming => stream_message(&prompt, &images),
                Tool::ClaudeCode => prompt,
                Tool::Codex => format!("{system}\n\n{prompt}"),
                Tool::Antigravity => String::new(),
            };
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(input.as_bytes()).await.map_err(|e| ProviderError::Other(e.to_string()))?;
            }
            let (text, usage, streamed) = match self.tool {
                Tool::ClaudeCode => read_claude(&mut child, on_text).await?,
                Tool::Antigravity => unreachable!("handled by agy_complete"),
                Tool::Codex => {
                    let out = tokio::time::timeout(Duration::from_secs(600), child.wait_with_output())
                        .await
                        .map_err(|_| ProviderError::Other("the official tool did not answer in 10 minutes".into()))?
                        .map_err(|e| ProviderError::Other(e.to_string()))?;
                    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
                    let text = std::fs::read_to_string(&last).unwrap_or(stdout);
                    let _ = std::fs::remove_file(&last);
                    if !out.status.success() && text.trim().is_empty() {
                        return Err(ProviderError::Other(format!("{}: {}", self.tool.program(), String::from_utf8_lossy(&out.stderr).trim())));
                    }
                    // Codex does not report tokens here: an estimate.
                    let est = |s: &str| (s.len() / 4) as u64;
                    let usage = Usage { input: est(&input), cache_write: 0, cache_read: 0, output: est(&text) };
                    (text, usage, false)
                }
            };
            let content = parse_reply(&text, req.messages.len());
            // What was not shown while it streamed (Codex, or a reply that
            // arrived all at once) is shown now.
            if !streamed {
                for b in &content {
                    if let ContentBlock::Text { text } = b {
                        on_text(Delta::Text(text.clone()));
                    }
                }
            }
            let calls = content.iter().any(|b| matches!(b, ContentBlock::ToolUse { .. }));
            Ok(Response { content, stop_reason: if calls { "tool_use".into() } else { "end_turn".into() }, usage })
        })
    }
}

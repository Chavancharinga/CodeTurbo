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
    /// The warm Antigravity process, between turns.
    agy: std::sync::Mutex<Option<AgySession>>,
}

impl CliProvider {
    pub fn new(tool: Tool) -> Self {
        Self { tool, agy: std::sync::Mutex::new(None) }
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
        "{}\n\nThe harness runs on the user's own computer, inside their local network: through the tools below you reach \
         their files, their programs and the devices on their network. Never say you cannot; call the tool.\n\n\
         Tools (you cannot run anything yourself; ask and the harness runs it):\n{}\n\n\
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
            if let ContentBlock::Image { .. } = b {
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
            } else {
                render_block(&mut out, m.role, b);
            }
        }
    }
    out.push_str("\n[assistant]\n");
    out
}

/// One text block of the conversation in the text protocol.
fn render_block(out: &mut String, role: Role, b: &ContentBlock) {
    match (role, b) {
        (_, ContentBlock::Image { .. }) => out.push_str("\n[an image was omitted]\n"),
        (Role::User, ContentBlock::Text { text }) => out.push_str(&format!("\n[user]\n{text}\n")),
        (Role::Assistant, ContentBlock::Text { text }) => out.push_str(&format!("\n[assistant]\n{text}\n")),
        (_, ContentBlock::ToolUse { name, input, .. }) => {
            out.push_str(&format!("{OPEN}{}{CLOSE}\n", json!({"name": name, "input": input})));
        }
        (_, ContentBlock::ToolResult { content, is_error, .. }) => {
            let tag = if *is_error { "tool error" } else { "tool result" };
            out.push_str(&format!("\n[{tag}]\n{content}\n"));
        }
        (_, ContentBlock::Opaque(_)) => {}
    }
}

/// Messages in the text protocol, images left out (Antigravity gets text only).
pub fn render_messages(messages: &[crate::Message]) -> String {
    let mut out = String::new();
    for m in messages {
        for b in &m.content {
            render_block(&mut out, m.role, b);
        }
    }
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
        return Err(other(format!("unexpected answer from claude: {}", err.trim())));
    };
    if v["is_error"].as_bool() == Some(true) {
        return Err(other(format!("claude: {}", v["result"].as_str().unwrap_or("error"))));
    }
    let text = v["result"].as_str().filter(|t| !t.is_empty()).map_or_else(|| stream.text().to_string(), str::to_string);
    Ok((text, claude_usage(&v), shown))
}

/// The reply and token counts from the JSON `agy -p --output-format json` prints.
pub fn parse_antigravity(body: &str) -> Result<(String, Usage), String> {
    let v: Value = serde_json::from_str(body.trim()).map_err(|_| "unexpected answer".to_string())?;
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

/// A warm `agy` process kept between turns. In streaming input it reads one
/// message per line and keeps the conversation, so a turn sends only what is
/// new and its prompt cache stays hot: seconds per turn instead of a cold
/// start that re-reads the whole history every time.
pub struct AgySession {
    _child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    lines: tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
    /// Model and effort it started with, and the system prompt it was given.
    key: String,
    system: String,
    /// The conversation it has seen, rendered, and how many messages that is.
    seen: String,
    count: usize,
    /// Its running token totals (agy reports them cumulatively).
    used: Usage,
}

const HARNESS_NOTE: &str = "IMPORTANT: you are running inside CodeTurbo, on the user's own computer and local network. Antigravity's built-in tools are switched off here; CodeTurbo's tools listed below replace them and DO work: files, shell commands, a real browser, and the devices on the user's network. Do everything by writing <tool_call> blocks exactly as described below; never answer that you cannot do something those tools can.";

/// No word from agy for this long ends the turn.
const AGY_IDLE: Duration = Duration::from_secs(300);

fn agy_key(model: &str, effort: Option<&str>) -> String {
    format!("{model}\u{1}{effort:?}")
}

/// The folder the official tools run in: outside the project, so they load
/// no project instructions of their own and touch no files.
fn cli_work() -> std::path::PathBuf {
    let work = std::env::temp_dir().join("codeturbo-cli");
    let _ = std::fs::create_dir_all(&work);
    work
}

fn start_agy(model: &str, effort: Option<&str>) -> Result<AgySession, ProviderError> {
    let mut cmd = tokio::process::Command::new(resolve(Tool::Antigravity));
    cmd.args(["--input-format", "stream-json", "--output-format", "stream-json"]);
    if let Some(effort) = effort {
        cmd.arg("--effort").arg(effort);
    }
    if !model.is_empty() && model != "padrao" {
        cmd.arg("--model").arg(model);
    }
    cmd.current_dir(cli_work()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| ProviderError::Other(format!("could not start `agy` ({e}); install the Antigravity CLI and sign in")))?;
    let stdin = child.stdin.take().ok_or_else(|| ProviderError::Other("agy did not open its input".into()))?;
    let stdout = child.stdout.take().ok_or_else(|| ProviderError::Other("agy did not open its output".into()))?;
    use tokio::io::AsyncBufReadExt;
    Ok(AgySession { _child: child, stdin, lines: tokio::io::BufReader::new(stdout).lines(),
        key: agy_key(model, effort),
        system: String::new(),
        seen: String::new(), count: 0, used: Usage::default() })
}

/// What one Antigravity turn gave back.
struct AgyTurn {
    text: String,
    usage: Usage,
    /// The model reached for Antigravity's own tools instead of ours.
    native: bool,
    shown: bool,
}

/// Sends `message` and reads the turn it starts, showing the reply as it is written.
async fn agy_turn(s: &mut AgySession, message: &str, on_text: &mut TextSink<'_>) -> Result<AgyTurn, String> {
    let line = format!("{}\n", json!({"event": "user", "message": {"content": message}}));
    s.stdin.write_all(line.as_bytes()).await.map_err(|e| format!("agy closed its input ({e})"))?;
    s.stdin.flush().await.map_err(|e| e.to_string())?;
    let mut stream = ReplyStream::default();
    let (mut native, mut shown) = (false, false);
    let mut last: Option<(u64, u64)> = None;
    loop {
        let line = match tokio::time::timeout(AGY_IDLE, s.lines.next_line()).await {
            Err(_) => return Err("Antigravity stopped answering".into()),
            Ok(Err(e)) => return Err(e.to_string()),
            Ok(Ok(None)) => return Err("agy exited".into()),
            Ok(Ok(Some(l))) => l,
        };
        if std::env::var_os("CODETURBO_DEBUG_AGY").is_some() {
            eprintln!("agy> {}", line.chars().take(300).collect::<String>());
        }
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { continue };
        match v["event"].as_str() {
            Some("step_update") => {
                let step = &v["step_update"];
                if step["step_type"] == "tool" {
                    native = true;
                } else if step["step_type"] == "agent_response" {
                    if let Some(t) = step["text_delta"].as_str() {
                        for delta in stream.push(t) {
                            shown = true;
                            on_text(delta);
                        }
                    }
                    // The prompt size of the latest model call: what the context holds.
                    let u = &step["usage"];
                    if let Some(sent) = u["input_tokens"].as_u64() {
                        let cached = u["cache_read_tokens"].as_u64().unwrap_or(0);
                        last = Some((sent.saturating_sub(cached), cached));
                    }
                }
            }
            Some("result") => {
                let (text, total) = parse_antigravity(&v["result"].to_string())?;
                for delta in stream.finish() {
                    shown = true;
                    on_text(delta);
                }
                let (input, cache_read) = last.unwrap_or((total.input.saturating_sub(s.used.input), 0));
                let usage = Usage { input, cache_write: 0, cache_read, output: total.output.saturating_sub(s.used.output) };
                s.used = total;
                let text = if text.is_empty() { stream.text().to_string() } else { text };
                return Ok(AgyTurn { text, usage, native, shown });
            }
            _ => {}
        }
    }
}

impl CliProvider {
    /// One reply through Antigravity, on the warm process when it already
    /// holds this conversation. Its own tools are not wanted (the work goes
    /// through CodeTurbo's): when the model uses them anyway it is told so and
    /// asked again. A broken stream starts a fresh process and tries again.
    async fn agy_complete(&self, req: &Request, system: &str, on_text: &mut TextSink<'_>) -> Result<Response, ProviderError> {
        let key = agy_key(&req.model, req.effort.as_deref());
        // Taken out while the turn runs: a turn that is cancelled drops (and
        // kills) the process instead of leaving it half way through a reply.
        let mut warm = self.agy.lock().ok().and_then(|mut g| g.take()).filter(|s| s.key == key);
        let mut total = Usage::default();
        let mut nudge: Option<&str> = None;
        let mut last_error = String::new();
        for _ in 0..4 {
            let (mut s, message) = match (warm.take(), nudge) {
                (Some(s), Some(n)) => (s, n.to_string()),
                (Some(s), None)
                    if s.count > 0 && s.system == system && req.messages.len() > s.count && render_messages(&req.messages[..s.count]) == s.seen =>
                {
                    let fresh = render_messages(&req.messages[s.count..]);
                    (s, format!("{fresh}\n[assistant]\n"))
                }
                (fresh, _) => (
                    // A process started ahead and not used yet takes the whole conversation too.
                    match fresh.filter(|s| s.count == 0) {
                        Some(s) => s,
                        None => start_agy(&req.model, req.effort.as_deref())?,
                    },
                    format!(
                        "{HARNESS_NOTE}\n\n{system}\n\n{}\n(Remember: built-in tools are off; act only through <tool_call> blocks.)\n[assistant]\n",
                        render_messages(&req.messages)
                    ),
                ),
            };
            let turn = match agy_turn(&mut s, &message, on_text).await {
                Ok(t) => t,
                Err(e) => {
                    // "The stream was interrupted" and the like: a new process.
                    last_error = e;
                    nudge = None;
                    continue;
                }
            };
            total.input += turn.usage.input;
            total.cache_read += turn.usage.cache_read;
            total.output += turn.usage.output;
            let has_calls = turn.text.contains(OPEN) || turn.text.contains("<invoke ");
            if turn.native && !has_calls && nudge.is_none() {
                nudge = Some("(Your previous answer used built-in tools, which do nothing here. Answer again using only <tool_call> blocks.)");
                warm = Some(s);
                continue;
            }
            // After tool results, Gemini often ends with no text at all: that is
            // the end of the task, not a reason to ask again.
            let text = if turn.text.trim().is_empty() && req.messages.len() > 2 { "Done.".to_string() } else { turn.text };
            let content = parse_reply(&text, req.messages.len());
            if !turn.shown {
                for b in &content {
                    if let ContentBlock::Text { text } = b {
                        on_text(Delta::Text(text.clone()));
                    }
                }
            }
            let calls = content.iter().any(|b| matches!(b, ContentBlock::ToolUse { .. }));
            let reply = crate::Message { role: Role::Assistant, content: content.clone() };
            s.seen = render_messages(&req.messages) + &render_messages(std::slice::from_ref(&reply));
            s.count = req.messages.len() + 1;
            s.system = system.to_string();
            if let Ok(mut g) = self.agy.lock() {
                *g = Some(s);
            }
            return Ok(Response { content, stop_reason: if calls { "tool_use".into() } else { "end_turn".into() }, usage: total });
        }
        Err(ProviderError::Other(format!("agy: {last_error}")))
    }
}

impl Provider for CliProvider {
    fn warm(&self, model: &str, effort: Option<&str>) {
        if self.tool != Tool::Antigravity {
            return;
        }
        let Ok(mut g) = self.agy.lock() else { return };
        if g.as_ref().is_some_and(|s| s.key == agy_key(model, effort)) {
            return;
        }
        // Started now, it has loaded by the time the first message is sent.
        *g = start_agy(model, effort).ok();
    }

    fn complete<'a>(&'a self, req: &'a Request, on_text: &'a mut TextSink<'_>) -> BoxFuture<'a, Result<Response, ProviderError>> {
        Box::pin(async move {
            let system = protocol_system(req);
            let prompt = protocol_prompt(req);
            // Outside the project: the official tool must not load its own
            // project instructions or touch files.
            let work = cli_work();
            let last = work.join(format!("ultima-{}.txt", std::process::id()));
            // Claude Code sees images when they ride inside the message
            // (streaming input); with none, the plain text prompt is used.
            let images = if self.tool == Tool::ClaudeCode { prompt_images(req) } else { Vec::new() };
            let streaming = !images.is_empty();
            if self.tool == Tool::Antigravity {
                return self.agy_complete(req, &system, on_text).await;
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
                    if let Some(effort) = &req.effort {
                        cmd.arg("--effort").arg(effort);
                    }
                }
                Tool::Antigravity => unreachable!("handled by agy_complete"),
                Tool::Codex => {
                    cmd.args(["exec", "--skip-git-repo-check", "--sandbox", "read-only", "--output-last-message"]).arg(&last);
                    if let Some(effort) = &req.effort {
                        // Codex takes low, medium and high.
                        let level = if matches!(effort.as_str(), "xhigh" | "max") { "high" } else { effort.as_str() };
                        cmd.arg("-c").arg(format!("model_reasoning_effort={level}"));
                    }
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

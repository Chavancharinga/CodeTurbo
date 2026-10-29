//! Provider-neutral chat types plus streaming clients for the Anthropic
//! Messages API and OpenAI-compatible endpoints (OpenAI, OpenRouter, Ollama).

pub mod anthropic;
pub mod cli;
pub mod dns;
mod http;
pub mod openai;
mod pricing;
pub mod reply_stream;
pub mod sse;

pub use pricing::{Price, PriceTable};

use serde_json::Value;
use std::{future::Future, ops::AddAssign, pin::Pin};

/// Key set on a tool input whose streamed JSON did not parse; the raw text is
/// kept under it so the agent can report the problem back to the model.
pub const INVALID_JSON_KEY: &str = "__coturbo_invalid_json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// An image the user side shows the model (e.g. a page screenshot),
    /// base64-encoded.
    Image {
        media_type: String,
        data: String,
    },
    /// Provider-specific block (thinking, redacted thinking, ...) that must be
    /// echoed back byte-for-byte to the provider that produced it.
    Opaque(Value),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub model: String,
    pub system: String,
    pub tools: Vec<ToolSpec>,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    /// Reasoning effort (`low`..`max`) where the provider supports it.
    pub effort: Option<String>,
}

/// Token usage split the way providers bill it. `input` is the uncached part.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub cache_write: u64,
    pub cache_read: u64,
    pub output: u64,
}

impl Usage {
    /// Every prompt token sent, cached or not.
    pub fn prompt_total(&self) -> u64 {
        self.input + self.cache_write + self.cache_read
    }
}

impl AddAssign for Usage {
    fn add_assign(&mut self, o: Self) {
        self.input += o.input;
        self.cache_write += o.cache_write;
        self.cache_read += o.cache_read;
        self.output += o.output;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub content: Vec<ContentBlock>,
    /// Normalized: `end_turn`, `tool_use`, `max_tokens`, `refusal`, or the raw value.
    pub stop_reason: String,
    pub usage: Usage,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("HTTP {status}: {body}")]
    Api { status: u16, body: String },
    #[error("stream error: {0}")]
    Stream(String),
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("missing credential: set {0}")]
    MissingKey(&'static str),
    #[error("{0}")]
    Other(String),
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What a provider streams to the UI while a response is generated.
#[derive(Debug, Clone, PartialEq)]
pub enum Delta {
    Text(String),
    /// Readable reasoning (a summary on current Claude models).
    Thinking(String),
    /// Rate-limit response headers (`*ratelimit*`), sent once per request.
    Limits(Vec<(String, String)>),
    /// A tool call being generated: its name, the `path` argument once it
    /// has streamed in, and the arguments' size so far.
    /// `code` is the end of what is being written (the last lines of the
    /// file's content), so the screen can show the code as it appears.
    Composing {
        tool: String,
        path: Option<String>,
        chars: usize,
        code: Option<String>,
    },
    /// Something the user should know (a retry after a rate limit).
    Notice(String),
}

/// The `path` argument of partially streamed tool JSON, once complete.
/// Only the start is searched: models put `path` before long contents.
pub fn partial_path(json: &str) -> Option<String> {
    let head = &json[..json.len().min(400)];
    let rest = &head[head.find("\"path\"")? + 6..];
    let rest = rest
        .trim_start()
        .strip_prefix(':')?
        .trim_start()
        .strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// How many lines of the code being written are shown live.
const LIVE_LINES: usize = 14;

/// The last lines of the content a tool call is writing (`content` or `new`),
/// from its arguments as they stream: JSON, or the `<invoke>` parameter form.
pub fn partial_code(call: &str) -> Option<String> {
    let raw = ["content", "new"].iter().find_map(|key| {
        let tag = format!("<parameter name=\"{key}\">");
        if let Some(i) = call.find(&tag) {
            let rest = &call[i + tag.len()..];
            return Some(rest[..rest.find("</parameter>").unwrap_or(rest.len())].to_string());
        }
        let k = format!("\"{key}\"");
        let rest = call[call.find(&k)? + k.len()..]
            .trim_start()
            .strip_prefix(':')?
            .trim_start()
            .strip_prefix('"')?;
        let mut out = String::new();
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => {}
                    Some('u') => {
                        let hex: String = chars.by_ref().take(4).collect();
                        out.push(
                            u32::from_str_radix(&hex, 16)
                                .ok()
                                .and_then(char::from_u32)
                                .unwrap_or('?'),
                        );
                    }
                    Some(other) => out.push(other),
                    None => break,
                },
                '"' => break,
                c => out.push(c),
            }
        }
        Some(out)
    })?;
    let lines: Vec<&str> = raw.lines().collect();
    if lines.is_empty() {
        return None;
    }
    Some(lines[lines.len().saturating_sub(LIVE_LINES)..].join("\n"))
}

pub type TextSink<'a> = dyn FnMut(Delta) + Send + 'a;

pub trait Provider: Send + Sync {
    /// Streams one completion; deltas go to `on_text` as they arrive.
    fn complete<'a>(
        &'a self,
        req: &'a Request,
        on_text: &'a mut TextSink<'_>,
    ) -> BoxFuture<'a, Result<Response, ProviderError>>;

    /// Gets ready for a first request with this model while the user is still
    /// typing (a program that is slow to start, started ahead). Optional.
    fn warm(&self, _model: &str, _effort: Option<&str>) {}
}

/// Parses accumulated tool-call JSON; invalid input is flagged, never dropped.
pub(crate) fn parse_tool_input(raw: &str) -> Value {
    if raw.trim().is_empty() {
        return Value::Object(Default::default());
    }
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::json!({ INVALID_JSON_KEY: raw }))
}

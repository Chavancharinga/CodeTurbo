//! Provider-neutral chat types plus streaming clients for the Anthropic
//! Messages API and OpenAI-compatible endpoints (OpenAI, OpenRouter, Ollama).

pub mod anthropic;
mod http;
pub mod openai;
mod pricing;
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
    Text { text: String },
    ToolUse { id: String, name: String, input: Value },
    ToolResult { tool_use_id: String, content: String, is_error: bool },
    /// An image the user side shows the model (e.g. a page screenshot),
    /// base64-encoded.
    Image { media_type: String, data: String },
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
}

pub type TextSink<'a> = dyn FnMut(Delta) + Send + 'a;

pub trait Provider: Send + Sync {
    /// Streams one completion; deltas go to `on_text` as they arrive.
    fn complete<'a>(
        &'a self,
        req: &'a Request,
        on_text: &'a mut TextSink<'_>,
    ) -> BoxFuture<'a, Result<Response, ProviderError>>;
}

/// Parses accumulated tool-call JSON; invalid input is flagged, never dropped.
pub(crate) fn parse_tool_input(raw: &str) -> Value {
    if raw.trim().is_empty() {
        return Value::Object(Default::default());
    }
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::json!({ INVALID_JSON_KEY: raw }))
}

//! Anthropic Messages API (`POST /v1/messages`, streaming).

use crate::{
    BoxFuture, ContentBlock, Message, Provider, ProviderError, Request, Response, Role, TextSink,
    Usage, http, parse_tool_input,
};
use serde_json::{Value, json};

const API_VERSION: &str = "2023-06-01";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// Models that accept `fallbacks: "default"` (route a refusal to another model).
const FALLBACK_MODELS: &[&str] =
    &["claude-fable-5-1", "claude-opus-5-5", "claude-opus-5", "claude-sonnet-5-5"];

pub struct Anthropic {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl Anthropic {
    pub fn new(api_key: String, base_url: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            base_url: base_url.unwrap_or_else(|| "https://api.anthropic.com".into()),
        }
    }

    pub fn from_env() -> Result<Self, ProviderError> {
        let key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| ProviderError::MissingKey("ANTHROPIC_API_KEY"))?;
        Ok(Self::new(key, std::env::var("ANTHROPIC_BASE_URL").ok()))
    }
}

fn uses_fallbacks(model: &str) -> bool {
    FALLBACK_MODELS.contains(&model)
}

fn block_json(b: &ContentBlock) -> Value {
    match b {
        ContentBlock::Text { text } => json!({"type": "text", "text": text}),
        ContentBlock::ToolUse { id, name, input } => {
            json!({"type": "tool_use", "id": id, "name": name, "input": input})
        }
        ContentBlock::ToolResult { tool_use_id, content, is_error } => json!({
            "type": "tool_result", "tool_use_id": tool_use_id, "content": content, "is_error": is_error
        }),
        ContentBlock::Opaque(v) => v.clone(),
    }
}

fn message_json(m: &Message) -> Value {
    let role = match m.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    json!({"role": role, "content": m.content.iter().map(block_json).collect::<Vec<_>>()})
}

/// Request body. The prefix (tools -> system) gets an explicit cache
/// breakpoint and the top-level `cache_control` caches the growing history.
pub fn build_body(req: &Request, fallbacks: bool) -> Value {
    let tools: Vec<Value> = req
        .tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.input_schema,
                "eager_input_streaming": true,
            })
        })
        .collect();
    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "stream": true,
        "cache_control": {"type": "ephemeral"},
        "system": [{"type": "text", "text": req.system, "cache_control": {"type": "ephemeral"}}],
        "messages": req.messages.iter().map(message_json).collect::<Vec<_>>(),
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    if let Some(effort) = &req.effort {
        body["output_config"] = json!({"effort": effort});
    }
    if fallbacks {
        body["fallbacks"] = json!("default");
    }
    body
}

enum Block {
    Text(String),
    Tool { id: String, name: String, json: String },
    /// Thinking/redacted/fallback blocks: kept as the provider sent them,
    /// with streamed `thinking`/`signature` deltas folded in.
    Opaque(Value),
}

/// Folds streamed events into a [`Response`].
#[derive(Default)]
pub struct StreamAcc {
    blocks: Vec<Option<Block>>,
    usage: Usage,
    stop_reason: String,
}

fn u(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

impl StreamAcc {
    /// Handles one SSE data payload. Returns a text delta to display, if any.
    pub fn on_data(&mut self, data: &str) -> Result<Option<String>, ProviderError> {
        let ev: Value = serde_json::from_str(data)
            .map_err(|e| ProviderError::Stream(format!("bad event JSON: {e}")))?;
        match ev["type"].as_str().unwrap_or_default() {
            "message_start" => {
                let us = &ev["message"]["usage"];
                self.usage.input = u(us, "input_tokens").unwrap_or(0);
                self.usage.cache_write = u(us, "cache_creation_input_tokens").unwrap_or(0);
                self.usage.cache_read = u(us, "cache_read_input_tokens").unwrap_or(0);
                self.usage.output = u(us, "output_tokens").unwrap_or(0);
            }
            "content_block_start" => {
                let i = ev["index"].as_u64().unwrap_or(0) as usize;
                let cb = &ev["content_block"];
                let block = match cb["type"].as_str() {
                    Some("text") => Block::Text(cb["text"].as_str().unwrap_or("").into()),
                    Some("tool_use") => Block::Tool {
                        id: cb["id"].as_str().unwrap_or("").into(),
                        name: cb["name"].as_str().unwrap_or("").into(),
                        json: String::new(),
                    },
                    _ => Block::Opaque(cb.clone()),
                };
                if self.blocks.len() <= i {
                    self.blocks.resize_with(i + 1, || None);
                }
                self.blocks[i] = Some(block);
            }
            "content_block_delta" => {
                let i = ev["index"].as_u64().unwrap_or(0) as usize;
                let d = &ev["delta"];
                match self.blocks.get_mut(i).and_then(Option::as_mut) {
                    Some(Block::Text(t)) => {
                        if let Some(s) = d["text"].as_str() {
                            t.push_str(s);
                            return Ok(Some(s.to_string()));
                        }
                    }
                    Some(Block::Tool { json, .. }) => {
                        json.push_str(d["partial_json"].as_str().unwrap_or(""));
                    }
                    Some(Block::Opaque(v)) => {
                        for key in ["thinking", "signature"] {
                            if let Some(s) = d[key].as_str() {
                                let cur = v[key].as_str().unwrap_or("").to_string();
                                v[key] = Value::String(cur + s);
                            }
                        }
                    }
                    None => {}
                }
            }
            "message_delta" => {
                if let Some(r) = ev["delta"]["stop_reason"].as_str() {
                    self.stop_reason = r.to_string();
                }
                let us = &ev["usage"];
                if let Some(o) = u(us, "output_tokens") {
                    self.usage.output = o;
                }
                if let Some(v) = u(us, "input_tokens") {
                    self.usage.input = v;
                }
                if let Some(v) = u(us, "cache_creation_input_tokens") {
                    self.usage.cache_write = v;
                }
                if let Some(v) = u(us, "cache_read_input_tokens") {
                    self.usage.cache_read = v;
                }
            }
            "error" => {
                return Err(ProviderError::Stream(ev["error"].to_string()));
            }
            _ => {}
        }
        Ok(None)
    }

    pub fn finish(self) -> Response {
        let content = self
            .blocks
            .into_iter()
            .flatten()
            .map(|b| match b {
                Block::Text(text) => ContentBlock::Text { text },
                Block::Tool { id, name, json } => {
                    ContentBlock::ToolUse { id, name, input: parse_tool_input(&json) }
                }
                Block::Opaque(v) => ContentBlock::Opaque(v),
            })
            .collect();
        Response { content, stop_reason: self.stop_reason, usage: self.usage }
    }
}

impl Provider for Anthropic {
    fn complete<'a>(
        &'a self,
        req: &'a Request,
        on_text: &'a mut TextSink<'_>,
    ) -> BoxFuture<'a, Result<Response, ProviderError>> {
        Box::pin(async move {
            let fallbacks = uses_fallbacks(&req.model);
            let body = build_body(req, fallbacks);
            let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
            let mut acc = StreamAcc::default();
            http::stream_sse(
                || {
                    let mut rb = self
                        .client
                        .post(&url)
                        .header("x-api-key", &self.api_key)
                        .header("anthropic-version", API_VERSION)
                        .json(&body);
                    if fallbacks {
                        rb = rb.header("anthropic-beta", FALLBACK_BETA);
                    }
                    rb
                },
                |data| {
                    if let Some(t) = acc.on_data(data)? {
                        on_text(&t);
                    }
                    Ok(true)
                },
            )
            .await?;
            Ok(acc.finish())
        })
    }
}

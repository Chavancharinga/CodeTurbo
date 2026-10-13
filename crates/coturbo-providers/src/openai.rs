//! OpenAI-compatible Chat Completions (OpenAI, OpenRouter, Ollama, ...).

use crate::{
    BoxFuture, ContentBlock, Provider, ProviderError, Request, Response, Role, TextSink, Usage,
    http, parse_tool_input,
};
use serde_json::{Value, json};

pub struct OpenAiCompat {
    client: reqwest::Client,
    api_key: Option<String>,
    base_url: String,
}

impl OpenAiCompat {
    /// `base_url` up to and including the version, e.g. `https://api.openai.com/v1`.
    pub fn new(base_url: impl Into<String>, api_key: Option<String>) -> Self {
        Self { client: reqwest::Client::new(), api_key, base_url: base_url.into() }
    }

    pub fn openai() -> Result<Self, ProviderError> {
        let key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| ProviderError::MissingKey("OPENAI_API_KEY"))?;
        Ok(Self::new("https://api.openai.com/v1", Some(key)))
    }

    pub fn openrouter() -> Result<Self, ProviderError> {
        let key = std::env::var("OPENROUTER_API_KEY")
            .map_err(|_| ProviderError::MissingKey("OPENROUTER_API_KEY"))?;
        Ok(Self::new("https://openrouter.ai/api/v1", Some(key)))
    }

    pub fn ollama() -> Self {
        let host = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".into());
        Self::new(format!("{}/v1", host.trim_end_matches('/')), None)
    }
}

/// Request body. Opaque (provider-specific) blocks are not sent: they only
/// make sense to the provider that produced them.
pub fn build_body(req: &Request) -> Value {
    let mut messages = vec![json!({"role": "system", "content": req.system})];
    for m in &req.messages {
        match m.role {
            Role::User => {
                let mut text = String::new();
                for b in &m.content {
                    match b {
                        ContentBlock::ToolResult { tool_use_id, content, .. } => messages.push(
                            json!({"role": "tool", "tool_call_id": tool_use_id, "content": content}),
                        ),
                        ContentBlock::Text { text: t } => text.push_str(t),
                        _ => {}
                    }
                }
                if !text.is_empty() {
                    messages.push(json!({"role": "user", "content": text}));
                }
            }
            Role::Assistant => {
                let mut text = String::new();
                let mut calls = Vec::new();
                for b in &m.content {
                    match b {
                        ContentBlock::Text { text: t } => text.push_str(t),
                        ContentBlock::ToolUse { id, name, input } => calls.push(json!({
                            "id": id, "type": "function",
                            "function": {"name": name, "arguments": input.to_string()},
                        })),
                        _ => {}
                    }
                }
                let mut msg = json!({"role": "assistant", "content": text});
                if !calls.is_empty() {
                    msg["tool_calls"] = Value::Array(calls);
                }
                messages.push(msg);
            }
        }
    }
    let mut body = json!({
        "model": req.model,
        "stream": true,
        "stream_options": {"include_usage": true},
        "messages": messages,
    });
    if !req.tools.is_empty() {
        body["tools"] = req
            .tools
            .iter()
            .map(|t| json!({"type": "function", "function": {
                "name": t.name, "description": t.description, "parameters": t.input_schema,
            }}))
            .collect();
    }
    body
}

#[derive(Default)]
struct Call {
    id: String,
    name: String,
    args: String,
}

/// Folds streamed chunks into a [`Response`].
#[derive(Default)]
pub struct StreamAcc {
    text: String,
    calls: Vec<Call>,
    finish: String,
    usage: Usage,
}

impl StreamAcc {
    /// Handles one SSE data payload. Returns a text delta to display, if any.
    pub fn on_data(&mut self, data: &str) -> Result<Option<String>, ProviderError> {
        if data.trim() == "[DONE]" {
            return Ok(None);
        }
        let ev: Value = serde_json::from_str(data)
            .map_err(|e| ProviderError::Stream(format!("bad chunk JSON: {e}")))?;
        if let Some(err) = ev.get("error") {
            return Err(ProviderError::Stream(err.to_string()));
        }
        if let Some(us) = ev.get("usage").filter(|u| u.is_object()) {
            let prompt = us["prompt_tokens"].as_u64().unwrap_or(0);
            let cached = us["prompt_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0);
            self.usage = Usage {
                input: prompt.saturating_sub(cached),
                cache_write: 0,
                cache_read: cached,
                output: us["completion_tokens"].as_u64().unwrap_or(0),
            };
        }
        let Some(choice) = ev["choices"].get(0) else { return Ok(None) };
        if let Some(r) = choice["finish_reason"].as_str() {
            self.finish = r.to_string();
        }
        let d = &choice["delta"];
        for tc in d["tool_calls"].as_array().into_iter().flatten() {
            let i = tc["index"].as_u64().unwrap_or(0) as usize;
            if self.calls.len() <= i {
                self.calls.resize_with(i + 1, Call::default);
            }
            let c = &mut self.calls[i];
            if let Some(id) = tc["id"].as_str() {
                c.id = id.to_string();
            }
            if let Some(n) = tc["function"]["name"].as_str() {
                c.name.push_str(n);
            }
            if let Some(a) = tc["function"]["arguments"].as_str() {
                c.args.push_str(a);
            }
        }
        match d["content"].as_str() {
            Some(s) if !s.is_empty() => {
                self.text.push_str(s);
                Ok(Some(s.to_string()))
            }
            _ => Ok(None),
        }
    }

    pub fn finish(self) -> Response {
        let mut content = Vec::new();
        if !self.text.is_empty() {
            content.push(ContentBlock::Text { text: self.text });
        }
        let has_calls = !self.calls.is_empty();
        for c in self.calls {
            content.push(ContentBlock::ToolUse { id: c.id, name: c.name, input: parse_tool_input(&c.args) });
        }
        let stop_reason = match self.finish.as_str() {
            "tool_calls" => "tool_use".to_string(),
            "length" => "max_tokens".to_string(),
            "stop" if has_calls => "tool_use".to_string(),
            "stop" | "" => "end_turn".to_string(),
            other => other.to_string(),
        };
        Response { content, stop_reason, usage: self.usage }
    }
}

impl Provider for OpenAiCompat {
    fn complete<'a>(
        &'a self,
        req: &'a Request,
        on_text: &'a mut TextSink<'_>,
    ) -> BoxFuture<'a, Result<Response, ProviderError>> {
        Box::pin(async move {
            let body = build_body(req);
            let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
            let mut acc = StreamAcc::default();
            http::stream_sse(
                || {
                    let rb = self.client.post(&url).json(&body);
                    match &self.api_key {
                        Some(k) => rb.bearer_auth(k),
                        None => rb,
                    }
                },
                |data| {
                    if data.trim() == "[DONE]" {
                        return Ok(false);
                    }
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

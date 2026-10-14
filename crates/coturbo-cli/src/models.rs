//! Models a provider offers, for the `/modelo` picker.

use serde_json::Value;
use std::time::Duration;

/// Claude models shown when the list can't be fetched (no key yet, offline).
pub const ANTHROPIC_FALLBACK: [&str; 4] = ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5", "claude-fable-5-1"];

/// OpenAI model ids that are not chat models.
const NOT_CHAT: [&str; 10] =
    ["embedding", "tts", "whisper", "dall-e", "audio", "realtime", "image", "moderation", "transcribe", "search"];

/// Model ids in a provider's list response: `data[].id` (Anthropic, OpenAI,
/// OpenRouter) or `models[].name` (Ollama).
pub fn parse_ids(provider: &str, body: &Value) -> Vec<String> {
    let (list, field) = if provider == "ollama" { ("models", "name") } else { ("data", "id") };
    let mut ids: Vec<String> = body[list]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m[field].as_str())
        .filter(|id| provider != "openai" || !NOT_CHAT.iter().any(|n| id.contains(n)))
        .map(str::to_string)
        .collect();
    // Anthropic lists newest first; the others are easier to scan sorted.
    if provider != "anthropic" {
        ids.sort();
    }
    ids.dedup();
    ids
}

/// Ollama's native API root from its OpenAI-compatible base URL.
fn ollama_root(base_url: Option<&str>) -> String {
    let host = match base_url {
        Some(url) => url.to_string(),
        None => std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".into()),
    };
    host.trim_end_matches('/').trim_end_matches("/v1").to_string()
}

/// Fetches the model list of `provider`.
pub async fn list(provider: &str, key: Option<&str>, base_url: Option<&str>) -> Result<Vec<String>, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(concat!("codeturbo/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let base = base_url.map(|u| u.trim_end_matches('/').to_string());
    let req = match provider {
        "anthropic" => {
            let key = key.ok_or("falta a chave de API da Anthropic")?;
            let root = base.unwrap_or_else(|| "https://api.anthropic.com".into());
            client
                .get(format!("{root}/v1/models?limit=100"))
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01")
        }
        "openai" => {
            let key = key.ok_or("falta a chave de API da OpenAI")?;
            let root = base.unwrap_or_else(|| "https://api.openai.com/v1".into());
            client.get(format!("{root}/models")).bearer_auth(key)
        }
        "openrouter" => client.get("https://openrouter.ai/api/v1/models"),
        "ollama" => client.get(format!("{}/api/tags", ollama_root(base_url))),
        // The official tools take their own model names.
        "claude-code" => return Ok(["padrao", "opus", "sonnet", "haiku"].map(String::from).to_vec()),
        "codex" => return Ok(["padrao", "gpt-5-codex", "gpt-5"].map(String::from).to_vec()),
        p => return Err(format!("provedor desconhecido: {p}")),
    };
    let resp = req.send().await.map_err(|e| {
        if provider == "ollama" {
            format!("o Ollama não respondeu em {} ({e})", ollama_root(base_url))
        } else {
            e.to_string()
        }
    })?;
    if !resp.status().is_success() {
        return Err(format!("o provedor respondeu {}", resp.status()));
    }
    let body: Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(parse_ids(provider, &body))
}

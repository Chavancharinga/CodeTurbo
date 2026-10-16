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
        .map(|id| id.strip_prefix("models/").unwrap_or(id).to_string())
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

/// `(id, label)` of each model: OpenRouter's name and context size help to
/// search hundreds of models; others show the id.
pub fn parse_named(provider: &str, body: &Value) -> Vec<(String, String)> {
    let ids = parse_ids(provider, body);
    if provider != "openrouter" {
        return ids.into_iter().map(|id| (id.clone(), id)).collect();
    }
    let by_id: std::collections::HashMap<&str, &Value> =
        body["data"].as_array().into_iter().flatten().filter_map(|m| Some((m["id"].as_str()?, m))).collect();
    ids.into_iter()
        .map(|id| {
            let m = by_id.get(id.as_str()).copied().unwrap_or(&Value::Null);
            let name = m["name"].as_str().unwrap_or("");
            let ctx = m["context_length"].as_u64().map(|c| format!(" · {}k context", c / 1000)).unwrap_or_default();
            let free = if id.ends_with(":free") { " · free" } else { "" };
            let label = if name.is_empty() { format!("{id}{ctx}{free}") } else { format!("{id} · {name}{ctx}{free}") };
            (id, label)
        })
        .collect()
}

/// Fetches the model list of `provider`: `(id, label)`.
pub async fn list(provider: &str, key: Option<&str>, base_url: Option<&str>) -> Result<Vec<(String, String)>, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(concat!("codeturbo/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let base = base_url.map(|u| u.trim_end_matches('/').to_string());
    let req = match provider {
        "anthropic" => {
            let key = key.ok_or("the Anthropic API key is missing")?;
            let root = base.unwrap_or_else(|| "https://api.anthropic.com".into());
            client
                .get(format!("{root}/v1/models?limit=100"))
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01")
        }
        "openai" => {
            let key = key.ok_or("the OpenAI API key is missing")?;
            let root = base.unwrap_or_else(|| "https://api.openai.com/v1".into());
            client.get(format!("{root}/models")).bearer_auth(key)
        }
        id if crate::settings::provider(id).is_some_and(|i| i.base_url.is_some()) => {
            let info = crate::settings::provider(id).expect("checked above");
            let root = base.unwrap_or_else(|| info.base_url.unwrap_or_default().to_string());
            let req = client.get(format!("{root}/models"));
            match key {
                Some(k) => req.bearer_auth(k),
                None => req,
            }
        }
        "ollama" => client.get(format!("{}/api/tags", ollama_root(base_url))),
        // The official tools take their own model names.
        "claude-code" => {
            return Ok([("padrao", "your plan's default"), ("opus", "opus"), ("sonnet", "sonnet"), ("haiku", "haiku")]
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .to_vec());
        }
        "antigravity" => return antigravity_models().await,
        "codex" => {
            return Ok([("padrao", "your plan's default"), ("gpt-5-codex", "gpt-5-codex"), ("gpt-5", "gpt-5")]
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .to_vec());
        }
        p => return Err(format!("unknown provider: {p}")),
    };
    let resp = req.send().await.map_err(|e| {
        if provider == "ollama" {
            format!("Ollama did not answer at {} ({e})", ollama_root(base_url))
        } else if provider == "lmstudio" {
            format!("LM Studio did not answer ({e}); open the app and start the local server")
        } else {
            e.to_string()
        }
    })?;
    if !resp.status().is_success() {
        return Err(format!("the provider answered {}", resp.status()));
    }
    let body: Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(parse_named(provider, &body))
}

/// The models `agy models` lists for the signed-in account: `id<TAB>name` lines.
async fn antigravity_models() -> Result<Vec<(String, String)>, String> {
    let program = coturbo_providers::cli::resolve(coturbo_providers::cli::Tool::Antigravity);
    let out = tokio::time::timeout(Duration::from_secs(40), tokio::process::Command::new(program).arg("models").output())
        .await
        .map_err(|_| "agy did not answer".to_string())?
        .map_err(|e| format!("could not run agy: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let list = parse_agy_models(&text);
    if list.is_empty() {
        return Err("agy listed no models: sign in to Antigravity first".into());
    }
    Ok(list)
}

/// `id<TAB>name` lines of `agy models`.
pub fn parse_agy_models(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let (id, name) = l.split_once(char::from(9))?;
            (!id.trim().is_empty()).then(|| (id.trim().to_string(), format!("{id} · {}", name.trim())))
        })
        .collect()
}

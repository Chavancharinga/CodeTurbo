//! User configuration (`config.toml` + optional `pricing.toml` in the config
//! dir) and the provider factory. API keys come only from the environment.

use coturbo_providers::{PriceTable, Provider, Usage, anthropic::Anthropic, openai::OpenAiCompat};
use coturbo_tools::Mode;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// `anthropic`, `openai`, `openrouter` or `ollama`.
    pub provider: String,
    pub model: String,
    /// `naive` or `ast`.
    pub tools: String,
    pub max_turns: u32,
    pub max_tokens: u32,
    pub effort: Option<String>,
    /// Overrides the provider's default endpoint.
    pub base_url: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: "anthropic".into(),
            model: "claude-sonnet-5-5".into(),
            tools: "ast".into(),
            max_turns: 40,
            max_tokens: 16_000,
            effort: None,
            base_url: None,
        }
    }
}

/// `%APPDATA%\coturbo` on Windows, `$XDG_CONFIG_HOME/coturbo` or
/// `~/.config/coturbo` elsewhere.
pub fn config_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    base.map(|b| b.join("coturbo"))
}

/// Reads `config.toml` and merges `pricing.toml` over the builtin prices.
/// Missing files are fine; malformed ones are errors.
pub fn load(dir: Option<&Path>) -> Result<(Config, PriceTable), String> {
    let mut prices = PriceTable::builtin();
    let Some(dir) = dir else { return Ok((Config::default(), prices)) };
    let read = |name: &str| -> Result<Option<String>, String> {
        let p = dir.join(name);
        match std::fs::read_to_string(&p) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", p.display())),
        }
    };
    let config = match read("config.toml")? {
        Some(s) => toml::from_str(&s).map_err(|e| format!("config.toml: {e}"))?,
        None => Config::default(),
    };
    if let Some(s) = read("pricing.toml")? {
        prices.merge(PriceTable::parse(&s).map_err(|e| format!("pricing.toml: {e}"))?);
    }
    Ok((config, prices))
}

pub fn parse_mode(s: &str) -> Result<Mode, String> {
    match s {
        "naive" => Ok(Mode::Naive),
        "ast" => Ok(Mode::Ast),
        _ => Err(format!("unknown tool set `{s}` (expected naive or ast)")),
    }
}

pub fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Naive => "naive",
        Mode::Ast => "ast",
    }
}

pub fn make_provider(cfg: &Config) -> Result<Box<dyn Provider>, String> {
    let env_key = |var: &'static str| std::env::var(var).map_err(|_| format!("missing credential: set {var}"));
    Ok(match cfg.provider.as_str() {
        "anthropic" => match &cfg.base_url {
            Some(url) => Box::new(Anthropic::new(env_key("ANTHROPIC_API_KEY")?, Some(url.clone()))),
            None => Box::new(Anthropic::from_env().map_err(|e| e.to_string())?),
        },
        "openai" => match &cfg.base_url {
            Some(url) => Box::new(OpenAiCompat::new(url.clone(), std::env::var("OPENAI_API_KEY").ok())),
            None => Box::new(OpenAiCompat::openai().map_err(|e| e.to_string())?),
        },
        "openrouter" => Box::new(OpenAiCompat::openrouter().map_err(|e| e.to_string())?),
        "ollama" => match &cfg.base_url {
            Some(url) => Box::new(OpenAiCompat::new(url.clone(), None)),
            None => Box::new(OpenAiCompat::ollama()),
        },
        p => return Err(format!("unknown provider `{p}` (anthropic, openai, openrouter, ollama)")),
    })
}

/// Prices a run: by model id, falling back to the `ollama` entry for local models.
pub struct Pricer {
    pub table: PriceTable,
    pub model: String,
    pub provider: String,
}

impl Pricer {
    pub fn cost(&self, u: &Usage) -> Option<f64> {
        self.table
            .cost(&self.model, u)
            .or_else(|| (self.provider == "ollama").then(|| self.table.cost("ollama", u)).flatten())
    }
}

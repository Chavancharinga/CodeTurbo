//! User configuration (`config.toml` + optional `pricing.toml` in the config
//! dir), the provider factory and the agent setup built from them. API keys
//! come from the environment or from keys saved in the interface.

use coturbo_agent::AgentConfig;
use coturbo_agent::approval::{ApprovalConfig, ApprovalMode};
use coturbo_providers::{PriceTable, Provider, Usage, anthropic::Anthropic, openai::OpenAiCompat};
use coturbo_tools::{Mode, Tools};
use coturbo_verify::GateConfig;
use coturbo_verify::lsp::{LspConfig, ServerConfig};
use serde::Deserialize;
use std::collections::BTreeMap;
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
    /// Model used while plan mode is on.
    pub plan_model: Option<String>,
    /// Context window of the model, in tokens.
    pub context_window: u32,
    /// `always-ask`, `write` or `yolo`.
    pub approval_mode: String,
    /// Tool name or `bash:<glob>` → `allow | deny | prompt`.
    pub approval: BTreeMap<String, String>,
    /// Build/lint/test the project before a task that edited code counts as done.
    pub final_gate: bool,
    /// Replaces detected gate steps with these shell commands.
    pub verify: Option<Vec<String>>,
    /// Smoke-run command added to the gate, e.g. `cargo run -- --help`.
    pub run: Option<String>,
    /// Gate failures sent back to the model before giving up.
    pub max_fix_rounds: u32,
    /// Whether the model reads images; unset guesses from the model name.
    pub vision: Option<bool>,
    /// Look for a newer CodeTurbo release (at most once a day).
    pub update_check: bool,
    /// Append language-server diagnostics to every edit result.
    pub diagnostics_on_write: bool,
    /// Format files after edits (rustfmt for Rust, the language server otherwise).
    pub format_on_write: bool,
    pub lsp: LspToml,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LspToml {
    /// Added to, or replacing by name, the built-in servers.
    pub servers: BTreeMap<String, ServerToml>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerToml {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub root_markers: Vec<String>,
    pub extensions: Vec<String>,
}

/// `.codeturbo.toml` at the project root. It can make the gate run arbitrary
/// commands, so it is only used after the user trusts it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub verify: Option<Vec<String>>,
    pub run: Option<String>,
}

impl ProjectConfig {
    pub fn commands(&self) -> Vec<String> {
        self.verify.iter().flatten().chain(self.run.iter()).cloned().collect()
    }
}

pub fn load_project(root: &Path) -> Result<Option<ProjectConfig>, String> {
    let p = root.join(".codeturbo.toml");
    match std::fs::read_to_string(&p) {
        Ok(s) => toml::from_str(&s).map(Some).map_err(|e| format!("{}: {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
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
            plan_model: None,
            context_window: 200_000,
            approval_mode: "write".into(),
            approval: BTreeMap::new(),
            final_gate: true,
            verify: None,
            run: None,
            max_fix_rounds: 3,
            vision: None,
            update_check: true,
            diagnostics_on_write: true,
            format_on_write: true,
            lsp: LspToml::default(),
        }
    }
}

/// `%APPDATA%\codeturbo` on Windows, `$XDG_CONFIG_HOME/codeturbo` or
/// `~/.config/codeturbo` elsewhere.
pub fn config_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    base.map(|b| b.join("codeturbo"))
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

impl Config {
    pub fn gate_config(&self) -> Option<GateConfig> {
        self.final_gate.then(|| GateConfig { verify: self.verify.clone(), run: self.run.clone(), ..GateConfig::default() })
    }

    /// Language servers: built-ins overridden or extended by `[lsp.servers.*]`.
    /// With `diagnostics_on_write` off, only Rust formatting remains.
    pub fn lsp_config(&self) -> Option<LspConfig> {
        if !self.diagnostics_on_write && !self.format_on_write {
            return None;
        }
        let mut cfg = LspConfig { format_on_write: self.format_on_write, ..LspConfig::default() };
        for (name, s) in &self.lsp.servers {
            let server = ServerConfig {
                name: name.clone(),
                command: s.command.clone(),
                args: s.args.clone(),
                root_markers: s.root_markers.clone(),
                extensions: s.extensions.clone(),
            };
            match cfg.servers.iter_mut().find(|x| &x.name == name) {
                Some(existing) => *existing = server,
                None => cfg.servers.push(server),
            }
        }
        if !self.diagnostics_on_write {
            cfg.servers.clear();
        }
        Some(cfg)
    }

    pub fn approval_config(&self) -> Result<ApprovalConfig, String> {
        let mode: ApprovalMode = self.approval_mode.parse()?;
        ApprovalConfig::new(mode, &self.approval)
    }
}

pub fn parse_mode(s: &str) -> Result<Mode, String> {
    match s {
        "naive" => Ok(Mode::Naive),
        "ast" => Ok(Mode::Ast),
        _ => Err(format!("conjunto de ferramentas desconhecido: `{s}` (use naive ou ast)")),
    }
}

pub fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Naive => "naive",
        Mode::Ast => "ast",
    }
}

/// The provider `cfg` names, with `key` as its API key (see
/// [`crate::settings::api_key`]).
pub fn make_provider(cfg: &Config, key: Option<String>) -> Result<Box<dyn Provider>, String> {
    let need = |var: &str| {
        key.clone().ok_or_else(|| format!("falta a chave de API: use /chave na interface ou defina {var}"))
    };
    Ok(match cfg.provider.as_str() {
        "anthropic" => {
            let base = cfg.base_url.clone().or_else(|| std::env::var("ANTHROPIC_BASE_URL").ok());
            Box::new(Anthropic::new(need("ANTHROPIC_API_KEY")?, base))
        }
        "openai" => match &cfg.base_url {
            // A compatible server may not need a key.
            Some(url) => Box::new(OpenAiCompat::new(url.clone(), key.clone())),
            None => Box::new(OpenAiCompat::new("https://api.openai.com/v1", Some(need("OPENAI_API_KEY")?))),
        },
        "openrouter" => Box::new(OpenAiCompat::new("https://openrouter.ai/api/v1", Some(need("OPENROUTER_API_KEY")?))),
        "ollama" => match &cfg.base_url {
            Some(url) => Box::new(OpenAiCompat::new(url.clone(), None)),
            None => Box::new(OpenAiCompat::ollama()),
        },
        p => return Err(format!("provedor desconhecido: `{p}` (anthropic, openai, openrouter, ollama)")),
    })
}

/// The tools for `root`, with the diagnostics and language servers `cfg` enables.
pub fn make_tools(root: &Path, mode: Mode, cfg: &Config) -> Tools {
    let tools = Tools::new(root, mode).with_native_checks(cfg.diagnostics_on_write);
    match cfg.lsp_config() {
        Some(lsp) => {
            let root = tools.root().to_path_buf();
            tools.with_lsp(std::sync::Arc::new(coturbo_verify::lsp::LspManager::new(&root, lsp)))
        }
        None => tools,
    }
}

pub fn agent_config(c: &Config) -> Result<AgentConfig, String> {
    Ok(AgentConfig {
        model: c.model.clone(),
        max_turns: c.max_turns,
        max_tokens: c.max_tokens,
        effort: c.effort.clone(),
        approval: c.approval_config()?,
        plan_model: c.plan_model.clone(),
        context_window: c.context_window,
        gate: c.gate_config(),
        max_fix_rounds: c.max_fix_rounds,
        vision: c.vision.unwrap_or_else(|| sees_images(&c.provider, &c.model)),
    })
}

/// Whether a model reads images, from its provider and name.
pub fn sees_images(provider: &str, model: &str) -> bool {
    let m = model.to_lowercase();
    let any = |keys: &[&str]| keys.iter().any(|k| m.contains(k));
    match provider {
        "anthropic" => true,
        "openai" => any(&["gpt-4o", "gpt-4.1", "gpt-5", "o3", "o4"]),
        _ => any(&[
            "claude", "gpt-4o", "gpt-4.1", "gpt-5", "gemini", "gemma3", "gemma4", "gemma-3", "llava", "vision", "-vl", "vl:",
            "minicpm-v", "moondream", "llama4", "llama-4", "pixtral", "mistral-small3",
        ]),
    }
}

/// Prices a run: by model id, falling back to the `ollama` entry for local models.
#[derive(Clone)]
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

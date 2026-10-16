//! Choices made in the interface that outlive the session: provider, model
//! and approval mode go to `config.toml` (edited in place, comments kept),
//! API keys to `credentials.toml` next to it.

use std::collections::BTreeMap;
use std::path::Path;

/// A provider the interface offers.
pub struct ProviderInfo {
    pub id: &'static str,
    pub label: &'static str,
    /// Environment variable holding its API key; `None` needs no key.
    pub key_var: Option<&'static str>,
    /// Default endpoint of a provider that speaks the OpenAI protocol.
    pub base_url: Option<&'static str>,
}

const fn p(id: &'static str, label: &'static str, key_var: Option<&'static str>, base_url: Option<&'static str>) -> ProviderInfo {
    ProviderInfo { id, label, key_var, base_url }
}

pub const PROVIDERS: &[ProviderInfo] = &[
    p("claude-code", "Claude Code (your Claude plan, through the official app)", None, None),
    p("codex", "Codex (your ChatGPT plan, through the official app)", None, None),
    p("antigravity", "Antigravity (your Google plan, through the agy CLI)", None, None),
    p("anthropic", "Anthropic (Claude)", Some("ANTHROPIC_API_KEY"), None),
    p("openai", "OpenAI (or any compatible server, with base_url)", Some("OPENAI_API_KEY"), None),
    p("gemini", "Google Gemini (AI Studio)", Some("GEMINI_API_KEY"), Some("https://generativelanguage.googleapis.com/v1beta/openai")),
    p("openrouter", "OpenRouter (hundreds of models, several free)", Some("OPENROUTER_API_KEY"), Some("https://openrouter.ai/api/v1")),
    p("groq", "Groq (very fast)", Some("GROQ_API_KEY"), Some("https://api.groq.com/openai/v1")),
    p("deepseek", "DeepSeek", Some("DEEPSEEK_API_KEY"), Some("https://api.deepseek.com/v1")),
    p("mistral", "Mistral", Some("MISTRAL_API_KEY"), Some("https://api.mistral.ai/v1")),
    p("xai", "xAI (Grok)", Some("XAI_API_KEY"), Some("https://api.x.ai/v1")),
    p("together", "Together AI", Some("TOGETHER_API_KEY"), Some("https://api.together.xyz/v1")),
    p("lmstudio", "LM Studio (local models, no key)", None, Some("http://localhost:1234/v1")),
    p("ollama", "Ollama (local models, no key)", None, None),
];

/// Whether something listens on a local port (a model server that is running).
fn listening(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(150)).is_ok()
}

/// Whether this computer has what `id` needs: the official program for the
/// plans, the program or a running server for the local models, a key for
/// the hosted ones. The picker lists only these.
pub fn detected(id: &str, creds: &Credentials) -> bool {
    use coturbo_providers::cli::on_path;
    let home = |sub: &str| std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(|h| std::path::PathBuf::from(h).join(sub));
    match id {
        "claude-code" => on_path("claude"),
        "codex" => on_path("codex"),
        "antigravity" => on_path("agy") || home(".local/bin/agy").is_some_and(|f| f.is_file() || f.with_extension("exe").is_file()),
        "ollama" => on_path("ollama") || listening(11434),
        "lmstudio" => on_path("lms") || home(".lmstudio").is_some_and(|d| d.is_dir()) || listening(1234),
        _ => provider(id).is_some_and(|p| p.key_var.is_some()) && api_key(id, creds).is_some(),
    }
}

/// How the provider stands on this computer, for the sign-in screen: whether
/// you are signed in (the official app's own login), have a key, or run it.
pub fn standing(id: &str, creds: &Credentials) -> String {
    let home = |sub: &str| std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(|h| std::path::PathBuf::from(h).join(sub));
    let has = |sub: &str| home(sub).is_some_and(|f| f.is_file());
    match id {
        "claude-code" => if has(".claude/.credentials.json") { " ● logged in (login)" } else { " · installed, sign in with `claude`" }.to_string(),
        "codex" => if has(".codex/auth.json") { " ● logged in (login)" } else { " · installed, sign in with `codex`" }.to_string(),
        // The official CLI shares the sign-in of the Antigravity IDE and app.
        "antigravity" => " ● installed (shares the IDE's login)".to_string(),
        "ollama" | "lmstudio" => " ● running (local)".to_string(),
        _ if provider(id).is_some_and(|p| p.key_var.is_some()) && api_key(id, creds).is_some() => " ● connected (api key)".to_string(),
        _ => String::new(),
    }
}

pub fn provider(id: &str) -> Option<&'static ProviderInfo> {
    PROVIDERS.iter().find(|p| p.id == id)
}

fn read_or_empty(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Sets (or, with `None`, removes) top-level keys of `config.toml`, leaving
/// everything else in the file as it was.
pub fn save_config(dir: &Path, values: &[(&str, Option<&str>)]) -> Result<(), String> {
    let path = dir.join("config.toml");
    let mut doc: toml_edit::DocumentMut =
        read_or_empty(&path)?.parse().map_err(|e| format!("{}: {e}", path.display()))?;
    for &(key, value) in values {
        match value {
            Some(v) => doc[key] = toml_edit::value(v),
            None => {
                doc.remove(key);
            }
        }
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::write(&path, doc.to_string()).map_err(|e| format!("{}: {e}", path.display()))
}

/// API keys typed into the interface, by provider id.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Credentials {
    keys: BTreeMap<String, String>,
}

const CREDENTIALS: &str = "credentials.toml";

impl Credentials {
    /// Stored keys; a missing or unreadable file means none.
    pub fn load(dir: Option<&Path>) -> Self {
        let Some(text) = dir.and_then(|d| std::fs::read_to_string(d.join(CREDENTIALS)).ok()) else {
            return Self::default();
        };
        let keys = text
            .parse::<toml_edit::DocumentMut>()
            .ok()
            .and_then(|doc| {
                let table = doc.get("keys")?.as_table()?;
                Some(table.iter().filter_map(|(k, v)| Some((k.to_string(), v.as_str()?.to_string()))).collect())
            })
            .unwrap_or_default();
        Self { keys }
    }

    pub fn get(&self, provider: &str) -> Option<&str> {
        self.keys.get(provider).map(String::as_str)
    }

    pub fn set(&mut self, provider: &str, key: &str) {
        self.keys.insert(provider.to_string(), key.trim().to_string());
    }

    /// Writes the keys to `credentials.toml`, readable only by the user
    /// where the platform allows it.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let mut doc = toml_edit::DocumentMut::new();
        let mut table = toml_edit::Table::new();
        for (p, k) in &self.keys {
            table[p.as_str()] = toml_edit::value(k.as_str());
        }
        doc["keys"] = toml_edit::Item::Table(table);
        let path = dir.join(CREDENTIALS);
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        std::fs::write(&path, doc.to_string()).map_err(|e| format!("{}: {e}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}

/// The API key for `provider`: its environment variable first, then the
/// key stored from the interface.
pub fn api_key(provider: &str, creds: &Credentials) -> Option<String> {
    let from_env = self::provider(provider)
        .and_then(|p| p.key_var)
        .and_then(|var| std::env::var(var).ok())
        .filter(|k| !k.trim().is_empty());
    from_env.or_else(|| creds.get(provider).map(str::to_string))
}

const TRUSTED: &str = "trusted.toml";

/// Folders the user allowed CodeTurbo to read, edit and run commands in.
/// A trusted folder covers everything inside it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Trust {
    folders: Vec<std::path::PathBuf>,
}

fn normalize(p: &Path) -> std::path::PathBuf {
    let p = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    // Windows paths compare without case.
    if cfg!(windows) { std::path::PathBuf::from(p.to_string_lossy().to_lowercase()) } else { p }
}

impl Trust {
    pub fn load(dir: Option<&Path>) -> Self {
        let folders = dir
            .and_then(|d| std::fs::read_to_string(d.join(TRUSTED)).ok())
            .and_then(|t| t.parse::<toml_edit::DocumentMut>().ok())
            .and_then(|doc| {
                Some(doc.get("folders")?.as_array()?.iter().filter_map(|v| v.as_str().map(std::path::PathBuf::from)).collect())
            })
            .unwrap_or_default();
        Self { folders }
    }

    /// Whether `folder` or a folder containing it was trusted.
    pub fn covers(&self, folder: &Path) -> bool {
        let f = normalize(folder);
        self.folders.iter().any(|t| f.starts_with(normalize(t)))
    }

    /// Trusts `folder` and saves the list.
    pub fn add(&mut self, dir: Option<&Path>, folder: &Path) -> Result<(), String> {
        let f = folder.canonicalize().unwrap_or_else(|_| folder.to_path_buf());
        if !self.covers(&f) {
            self.folders.push(f);
        }
        let Some(dir) = dir else { return Ok(()) };
        let mut arr = toml_edit::Array::new();
        for f in &self.folders {
            arr.push(f.to_string_lossy().trim_start_matches(r"\\?\").to_string());
        }
        let mut doc = toml_edit::DocumentMut::new();
        doc["folders"] = toml_edit::value(arr);
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        std::fs::write(dir.join(TRUSTED), doc.to_string()).map_err(|e| e.to_string())
    }
}

/// Whether CodeTurbo was already used in `folder` (its own files are there).
pub fn known_project(folder: &Path) -> bool {
    folder.join(".codeturbo").is_dir() || folder.join("CODETURBO.md").is_file() || folder.join(".codeturbo.toml").is_file()
}

/// A folder too broad to work in: the home folder or a drive root.
pub fn too_broad(folder: &Path) -> bool {
    let f = folder.canonicalize().unwrap_or_else(|_| folder.to_path_buf());
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(std::path::PathBuf::from);
    f.parent().is_none() || f.components().count() <= 2 || home.is_some_and(|h| h.canonicalize().unwrap_or(h) == f)
}

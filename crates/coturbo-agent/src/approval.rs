//! Tool approval, modelled on oh-my-pi: each tool has a fixed capability
//! tier, the active mode sets the highest tier approved without asking, and
//! user policies (`allow | deny | prompt`, per tool or per `bash:<glob>`)
//! override the mode. Approvals never change the tool list sent to the model,
//! so the cached prompt prefix stays intact.

use coturbo_tools::Tier;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Only reads run unasked.
    AlwaysAsk,
    /// Reads and file edits run unasked; commands ask.
    Write,
    /// Everything runs unasked (explicit `deny` policies still apply).
    Yolo,
}

impl ApprovalMode {
    fn max_tier(self) -> Tier {
        match self {
            Self::AlwaysAsk => Tier::Read,
            Self::Write => Tier::Write,
            Self::Yolo => Tier::Exec,
        }
    }
}

impl FromStr for ApprovalMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "always-ask" => Ok(Self::AlwaysAsk),
            "write" => Ok(Self::Write),
            "yolo" => Ok(Self::Yolo),
            _ => Err(format!("unknown approval mode `{s}` (always-ask, write, yolo)")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Policy {
    // Order matters: on equally specific bash patterns the greatest wins.
    Allow,
    Prompt,
    Deny,
}

impl FromStr for Policy {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "allow" => Ok(Self::Allow),
            "prompt" => Ok(Self::Prompt),
            "deny" => Ok(Self::Deny),
            _ => Err(format!("unknown approval policy `{s}` (allow, deny, prompt)")),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalConfig {
    pub mode: ApprovalMode,
    tools: BTreeMap<String, Policy>,
    /// `bash:<glob>` rules as (glob, policy).
    bash: Vec<(String, Policy)>,
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        Self { mode: ApprovalMode::Write, tools: BTreeMap::new(), bash: Vec::new() }
    }
}

impl ApprovalConfig {
    /// `rules` maps a tool name or `bash:<glob>` to `allow | deny | prompt`.
    pub fn new(mode: ApprovalMode, rules: &BTreeMap<String, String>) -> Result<Self, String> {
        let mut c = Self { mode, ..Self::default() };
        for (key, value) in rules {
            let policy: Policy = value.parse().map_err(|e| format!("approval.{key}: {e}"))?;
            match key.strip_prefix("bash:") {
                Some("") => return Err(format!("approval.\"{key}\": empty command pattern")),
                Some(glob) => c.bash.push((glob.to_string(), policy)),
                None => {
                    c.tools.insert(key.clone(), policy);
                }
            }
        }
        Ok(c)
    }

    pub fn yolo() -> Self {
        Self { mode: ApprovalMode::Yolo, ..Self::default() }
    }

    fn bash_rule(&self, command: &str) -> Option<Policy> {
        self.bash
            .iter()
            .filter(|(g, _)| glob_match(g, command))
            .max_by_key(|(g, p)| (g.len(), *p))
            .map(|(_, p)| *p)
    }
}

/// "Always for this session" grants: edit tools by name, `bash` by the
/// command's first word (`cargo test` grants every `cargo …`).
#[derive(Debug, Clone, Default)]
pub struct SessionGrants {
    tools: HashSet<String>,
    bash: HashSet<String>,
}

pub fn bash_base(command: &str) -> &str {
    command.split_whitespace().next().unwrap_or("")
}

impl SessionGrants {
    pub fn grant(&mut self, tool: &str, input: &Value) {
        if tool == "bash" {
            self.bash.insert(bash_base(input["command"].as_str().unwrap_or("")).to_string());
        } else {
            self.tools.insert(tool.to_string());
        }
    }

    fn covers(&self, tool: &str, input: &Value) -> bool {
        if tool == "bash" {
            let base = bash_base(input["command"].as_str().unwrap_or(""));
            !base.is_empty() && self.bash.contains(base)
        } else {
            self.tools.contains(tool)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny(String),
    Prompt,
}

fn norm_path(p: &str) -> String {
    let p = p.replace('\\', "/");
    p.split('/').filter(|s| !s.is_empty() && *s != ".").collect::<Vec<_>>().join("/")
}

/// Directory plan files live in, relative to the project root.
pub const PLAN_DIR: &str = ".coturbo/plans";

/// `Some(slug)` if `path` is `.coturbo/plans/<slug>-plan.md`.
pub fn plan_slug(path: &str) -> Option<String> {
    let p = norm_path(path);
    let name = p.strip_prefix(PLAN_DIR)?.strip_prefix('/')?;
    let slug = name.strip_suffix("-plan.md")?;
    let ok = !slug.is_empty() && slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    ok.then(|| slug.to_string())
}

/// Decides one tool call. In plan mode everything above `Read` is denied
/// except writing a plan file.
pub fn resolve(
    tool: &str,
    tier: Tier,
    input: &Value,
    cfg: &ApprovalConfig,
    grants: &SessionGrants,
    plan_mode: bool,
) -> Verdict {
    if plan_mode && tier > Tier::Read {
        let plan_file = tier == Tier::Write && input["path"].as_str().and_then(plan_slug).is_some();
        if !plan_file {
            return Verdict::Deny(format!(
                "plan mode is read-only: only {PLAN_DIR}/<slug>-plan.md may be written; call propose_plan when the plan is ready"
            ));
        }
        return Verdict::Allow;
    }
    let user = if tool == "bash" {
        cfg.bash_rule(input["command"].as_str().unwrap_or("")).or_else(|| cfg.tools.get(tool).copied())
    } else {
        cfg.tools.get(tool).copied()
    };
    match user {
        Some(Policy::Deny) => return Verdict::Deny(format!("{tool} is denied by the user's approval policy")),
        Some(Policy::Allow) => return Verdict::Allow,
        // A session grant answers the prompt the policy asked for.
        Some(Policy::Prompt) => {
            return if grants.covers(tool, input) { Verdict::Allow } else { Verdict::Prompt };
        }
        None => {}
    }
    if grants.covers(tool, input) || tier <= cfg.mode.max_tier() {
        Verdict::Allow
    } else {
        Verdict::Prompt
    }
}

/// Whole-string match where `*` matches any run of characters.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !text.starts_with(first) || text.len() < first.len() + last.len() || !text.ends_with(last) {
        return false;
    }
    let mut rest = &text[first.len()..text.len() - last.len()];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    true
}

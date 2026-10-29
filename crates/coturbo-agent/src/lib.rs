//! The agent loop: send → stream → run tools → repeat.
//!
//! History is append-only within a task: rewriting earlier messages would
//! invalidate the provider's prompt cache and, on current Claude models,
//! the signed thinking blocks echoed back. Context is reclaimed only between
//! tasks, by [`Agent::compact`], which swaps the whole history for a summary.

pub mod approval;
pub mod hooks;
pub mod plan;
pub mod sub;

use approval::{ApprovalConfig, SessionGrants, Verdict};
use coturbo_providers::{
    BoxFuture, ContentBlock, Delta, INVALID_JSON_KEY, Message, Provider, ProviderError, Request, Role, Usage,
};
use coturbo_tools::{EDIT_TOOLS, Mode, Tier, ToolOutput, Tools};
use coturbo_verify::{Finding, GateConfig, Snapshot, gate};
use plan::{PlanChoice, PlanReview};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: String,
    pub max_turns: u32,
    pub max_tokens: u32,
    pub effort: Option<String>,
    pub approval: ApprovalConfig,
    /// Model used while plan mode is on (defaults to `model`).
    pub plan_model: Option<String>,
    /// Context window in tokens, for the plan review's 95% rule.
    pub context_window: u32,
    /// Final build/lint/test gate; `None` turns it off.
    pub gate: Option<GateConfig>,
    /// Gate failures sent back to the model before giving up.
    pub max_fix_rounds: u32,
    /// The model reads images (screenshots from `design_review`).
    pub vision: bool,
    /// The user's commands run before and after tools and tasks.
    pub hooks: Vec<hooks::Hook>,
}

impl AgentConfig {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            max_turns: 40,
            max_tokens: 16_000,
            effort: None,
            approval: ApprovalConfig::default(),
            plan_model: None,
            context_window: 200_000,
            gate: Some(GateConfig::default()),
            max_fix_rounds: 3,
            vision: false,
            hooks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The model ended its turn without calling tools.
    Done,
    MaxTurns,
    Refused,
    /// The plan was saved without implementing it.
    PlanSaved,
}

#[derive(Debug, Clone)]
pub struct TaskReport {
    pub stop: Stop,
    pub turns: u32,
    pub usage: Usage,
    pub final_text: String,
    /// Relative paths changed by successful edit tools.
    pub edited: BTreeSet<String>,
    /// `Some(true)`: the final gate passed. `Some(false)`: code was edited
    /// but never passed it. `None`: no code edits, gate off, or nothing to run.
    pub verified: Option<bool>,
}

impl TaskReport {
    fn empty() -> Self {
        Self {
            stop: Stop::Done,
            turns: 0,
            usage: Usage::default(),
            final_text: String::new(),
            edited: BTreeSet::new(),
            verified: None,
        }
    }
}

/// Something the UI may want to show.
#[derive(Debug, Clone, Copy)]
pub enum Event<'a> {
    /// Work started that produces no output for a while: `thinking` (waiting
    /// for the model) or `verifying` (running the final gate).
    Waiting(&'a str),
    Text(&'a str),
    /// Readable reasoning streamed before the answer.
    Thinking(&'a str),
    /// Rate-limit headers of the last response.
    Limits(&'a [(String, String)]),
    ToolStart { name: &'a str, input: &'a Value },
    /// `diff` is the unified diff a successful edit applied.
    ToolEnd { name: &'a str, input: &'a Value, output: &'a ToolOutput, diff: Option<&'a str> },
    /// One model response: its usage, the task total so far, how long it
    /// took, and the model's context window.
    Turn { turn: &'a Usage, task: &'a Usage, secs: f64, window: u32 },
    /// The final gate ran; `summary` is exactly what the model is told.
    Gate { ok: bool, summary: &'a str },
    Notice(&'a str),
    /// A step of the final checks (build, lint, tests) starts or ends.
    /// `expected` is how long it took last time, when it has run before.
    Step { name: &'a str, phase: StepPhase, index: usize, total: usize, secs: f32, expected: Option<f32> },
    /// The checks failed and the model is asked to fix it: attempt `attempt` of `of`.
    Fixing { attempt: u32, of: u32 },
    /// The model is writing a tool call: which tool, the file once known,
    /// and how much it has written.
    Composing { tool: &'a str, path: Option<&'a str>, chars: usize, code: Option<&'a str> },
}

/// Where a step of the final checks is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepPhase {
    Start,
    Passed,
    Failed,
}

/// A tool call waiting for the user's decision.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub tool: String,
    pub tier: Tier,
    pub input: Value,
    /// Diff for edit tools.
    pub preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Once,
    /// Approve this tool (or, for `bash`, this command's first word) until exit.
    Session,
    /// Optional reason, passed back to the model.
    Deny(Option<String>),
}

/// The UI side of the agent: REPL, TUI, `run` or bench all implement this.
pub trait Host: Send {
    fn on_event(&mut self, _ev: Event<'_>) {}

    /// Asked only for calls the approval policy marks `Prompt`.
    fn approve<'a>(&'a mut self, _req: &'a ApprovalRequest) -> BoxFuture<'a, Decision> {
        Box::pin(async { Decision::Deny(Some("no interactive approver; use --auto-approve".into())) })
    }

    /// Asked when the model proposes a plan. Without a user, nothing is executed.
    fn review_plan<'a>(&'a mut self, _plan: &'a PlanReview) -> BoxFuture<'a, PlanChoice> {
        Box::pin(async { PlanChoice::SaveQuit })
    }

    /// Asked when an edit looks like gaming the checks (deleted test,
    /// `#[ignore]`, lint suppression, ...). `true` accepts it as legitimate.
    fn confirm_cheat<'a>(&'a mut self, _finding: &'a Finding) -> BoxFuture<'a, bool> {
        Box::pin(async { false })
    }

    /// A question the model asks the user, with options to pick (the user
    /// may also type their own answer). `None`: nobody to ask.
    fn ask_user<'a>(&'a mut self, _question: &'a str, _options: &'a [String]) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { None })
    }
}

/// Shows nothing, denies every prompt and saves every plan unexecuted.
pub struct NoopHost;
impl Host for NoopHost {}

const SUMMARY_TEXT_MAX: usize = 1500;
/// Empty replies retried before the task is taken as finished.
const EMPTY_RETRIES: u32 = 2;

/// Files with the project's standing instructions, read in this order.
pub const MEMORY_FILES: [&str; 3] = ["CODETURBO.md", "AGENTS.md", "CLAUDE.md"];
/// Longest memory file kept, in bytes.
const MEMORY_MAX: usize = 12_000;

/// The project's instruction files, appended to the system prompt so every
/// session follows its conventions. Part of the cached prefix: read once.
/// File holding standing behavioural rules the user set (like omp's TTSR):
/// short corrections injected every turn so a mistake is not repeated.
pub const RULES_FILE: &str = ".codeturbo/rules.md";

/// The project's standing rules, for the system prompt.
pub fn stream_rules(root: &std::path::Path) -> String {
    let Ok(text) = std::fs::read_to_string(root.join(RULES_FILE)) else { return String::new() };
    let rules: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    if rules.is_empty() {
        return String::new();
    }
    let list = rules.iter().map(|r| format!("- {}", r.trim_start_matches(['-', '*', ' ']))).collect::<Vec<_>>().join("
");
    format!("

Standing rules from the user — always follow, never repeat these mistakes:
{list}")
}

/// Appends a rule to [`RULES_FILE`].
pub fn add_rule(root: &std::path::Path, rule: &str) -> Result<(), String> {
    let rule = rule.trim();
    if rule.is_empty() {
        return Err("regra vazia".into());
    }
    let path = root.join(RULES_FILE);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!("- {}\n", rule.trim_start_matches(['-', '*', ' '])));
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn project_memory(root: &std::path::Path) -> String {
    let mut out = String::new();
    for name in MEMORY_FILES {
        let Ok(text) = std::fs::read_to_string(root.join(name)) else { continue };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let mut cut = text.len().min(MEMORY_MAX);
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        out.push_str(&format!("

Project instructions from {name} (follow them):
{}", &text[..cut]));
    }
    out
}

/// What was remembered in earlier sessions of this project (the index; the
/// full text through `recall`).
pub fn remembered(root: &std::path::Path) -> String {
    let index = coturbo_tools::memory::index(root);
    let rule = "\n\nMemory: you keep a memory across sessions with remember, recall and forget. Save what will matter later: \
                who the user is, how they want you to work (their corrections and what they approved), project goals and \
                decisions not visible in the code, and where things live outside it. Update a memory instead of duplicating it, \
                and forget one that turned out wrong.";
    if index.is_empty() {
        return rule.to_string();
    }
    format!("{rule}\n\nRemembered from earlier sessions (read one in full with recall; they reflect what was true when saved):\n{index}")
}

/// Images whose paths appear in `text` (absolute, or relative to `root`),
/// at most four, each under 5 MB.
pub fn images_in(text: &str, root: &std::path::Path) -> Vec<ContentBlock> {
    use base64::Engine;
    let mut out = Vec::new();
    for word in text.split(|c: char| c.is_whitespace() || matches!(c, '`' | '"' | '\'' | '(' | ')' | '<' | '>')) {
        let w = word.trim_end_matches([',', '.', ';', ':']);
        let lower = w.to_lowercase();
        let media = if lower.ends_with(".png") {
            "image/png"
        } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
            "image/jpeg"
        } else if lower.ends_with(".gif") {
            "image/gif"
        } else if lower.ends_with(".webp") {
            "image/webp"
        } else {
            continue;
        };
        let p = std::path::Path::new(w);
        let path = if p.is_absolute() { p.to_path_buf() } else { root.join(p) };
        if let Ok(bytes) = std::fs::read(&path)
            && bytes.len() < 5 * 1024 * 1024
            && out.len() < 4
        {
            out.push(ContentBlock::Image { media_type: media.into(), data: base64::engine::general_purpose::STANDARD.encode(bytes) });
        }
    }
    out
}

/// Where the agent runs, so it uses what this machine has.
pub fn environment() -> String {
    let termux = cfg!(target_os = "android") || std::env::var("PREFIX").is_ok_and(|p| p.contains("com.termux"));
    if termux {
        // Chromium from Termux's x11-repo runs without a window: the in-terminal browser works.
        let path = std::env::var_os("PATH").unwrap_or_default();
        let chromium = std::env::split_paths(&path).any(|d| d.join("chromium-browser").is_file() || d.join("chromium").is_file());
        let browser = if chromium {
            "Chromium is installed, so browse (the page in the terminal) and design_review work; observe_app does not (no windows on Android)."
        } else {
            "No browser is installed yet. When the task needs browse or design_review, install Chromium yourself with bash: \
             `pkg install -y x11-repo && pkg install -y chromium` (about 100 MB: say so; the user approves the command), then go on. \
             observe_app does not work on Android."
        };
        return format!(
            "Environment: Android, in Termux, on the user's phone. {browser} To show the user a page in the phone's own browser, \
             write plain HTML/CSS/JS that works opened as a file and run `termux-open <file.html>` (for a server, \
             `termux-open-url http://localhost:<port>`). Python, Node and the rest may not be installed: check with \
             `command -v` and install what the task needs with `pkg install -y <name>` (the user approves it); prefer files that need no server."
        );
    }
    match std::env::consts::OS {
        "windows" => "Environment: Windows. Commands run in sh (Git Bash) when it is installed, otherwise in cmd; \
                      write commands that work there. Open a page for the user with `start <file.html>`."
            .into(),
        "macos" => "Environment: macOS. Open a page for the user with `open <file.html>`.".into(),
        os => format!("Environment: {os}. Open a page for the user with `xdg-open <file.html>` when there is a desktop."),
    }
}

fn system_prompt(mode: Mode) -> String {
    let mut s = String::from(
        "You are CodeTurbo, a coding agent in the user's project; paths are relative to its root.          Make the smallest change that fully solves the task, and only what was asked: a fix or back-end task never changes design, text or features.          Be fast: every reply is a round trip, so send all independent tool calls together, and never read back a file you just wrote.          Work like a senior engineer: read the code before editing, fix root causes, run what you changed and iterate until it works;          decide instead of asking when the next step is clear.          For three or more steps, keep a checklist with tasks (short imperative English titles).          Tools for particular jobs (browser, network devices, reverse engineering, desktop apps, research, WiFi sensing, schedules, CI) are listed in tool_search: load the one you need.          Never say you cannot reach the network or the web; use them.          To recreate a program with a window, observe_app the original first, then compare yours with it until they are hard to tell apart.          Interfaces: call design_brief first and use its tokens, then layout_grid to place every element of the page on the grid; keep an existing brand's colours; draw icons as inline SVG (never emoji);          build every state (hover, loading, empty, error) and function, with prefers-reduced-motion respected;          then test it with browse at desktop and phone size, clicking everything, and call design_review.          Reply in the user's language with a short summary of what changed. Write text files as UTF-8.",
    );
    s.push(' ');
    s.push_str(&environment());
    if mode == Mode::Ast {
        s.push_str(
            " For Rust: start from read_file (large files come back as skeletons), read only the \
             symbols you need with read_symbol, edit with replace_symbol/insert_symbol, and check \
             with workspace_diagnostics instead of building through bash.",
        );
    }
    s
}

/// How a `propose_plan` call ends the current phase.
enum Handoff {
    /// Start a new phase from the plan, after compacting or clearing history.
    Run { compact: bool, prompt: String },
    Saved,
}

pub struct Agent<'a> {
    provider: &'a dyn Provider,
    tools: &'a Tools,
    config: AgentConfig,
    system: String,
    history: Vec<Message>,
    pending_summary: Option<String>,
    last: Option<(String, TaskReport)>,
    session: Usage,
    grants: SessionGrants,
    plan_mode: bool,
    /// Prompt tokens of the last request, for the context-window share.
    last_prompt: u64,
    /// Edited files as they were before the task, for the anti-cheat check.
    snapshot: Snapshot,
    /// Anti-cheat findings already decided in this task (true = accepted).
    decided: HashMap<(String, String), bool>,
    /// History length, pending summary and plan mode before the running
    /// task, to put back if it is interrupted.
    started: Option<(usize, Option<String>, bool)>,
    /// Per finished task, the files it changed and their content before it
    /// (`None`: the file did not exist), for `undo`.
    checkpoints: Vec<Vec<(std::path::PathBuf, String, Option<String>)>>,
    /// How long each check step took the last time it passed, to show how
    /// far a running one is.
    step_secs: HashMap<String, f32>,
    /// 0 for the main agent, 1 for a subagent (which starts none).
    depth: u8,
}

/// Project files (not CoTurbo's own plan files) changed in the task.
fn code_edits(edited: &BTreeSet<String>) -> bool {
    edited.iter().any(|p| !p.starts_with(".codeturbo/"))
}

impl<'a> Agent<'a> {
    pub fn new(provider: &'a dyn Provider, tools: &'a Tools, config: AgentConfig) -> Self {
        Self {
            provider,
            tools,
            config,
            system: system_prompt(tools.mode()) + &project_memory(tools.root()) + &stream_rules(tools.root()) + &remembered(tools.root()) + &coturbo_tools::skills::catalogue(tools.root()),
            history: Vec::new(),
            pending_summary: None,
            last: None,
            session: Usage::default(),
            grants: SessionGrants::default(),
            plan_mode: false,
            last_prompt: 0,
            snapshot: Snapshot::default(),
            decided: HashMap::new(),
            started: None,
            checkpoints: Vec::new(),
            step_secs: HashMap::new(),
            depth: 0,
        }
    }

    /// Puts back every file the last task changed, as it was before the
    /// task. Returns the files restored; the next task is told.
    pub fn undo(&mut self) -> Result<Vec<String>, String> {
        let files = self.checkpoints.pop().ok_or("no change to undo")?;
        let mut restored = Vec::new();
        for (path, rel, before) in files.into_iter().rev() {
            match before {
                Some(text) => std::fs::write(&path, text).map_err(|e| format!("{rel}: {e}"))?,
                None => {
                    let _ = std::fs::remove_file(&path);
                }
            }
            restored.push(rel);
        }
        restored.sort();
        restored.dedup();
        let note = format!("[The user undid the previous task's changes to: {}]\n\n", restored.join(", "));
        self.pending_summary = Some(self.pending_summary.take().unwrap_or_default() + &note);
        Ok(restored)
    }

    /// Shrinks old tool results to one line each, keeping the last
    /// `keep` messages whole. Returns how many results were shortened.
    pub fn shake(&mut self, keep: usize) -> usize {
        let cut = self.history.len().saturating_sub(keep);
        let mut n = 0;
        for m in &mut self.history[..cut] {
            for b in &mut m.content {
                if let ContentBlock::ToolResult { content, .. } = b
                    && content.len() > 400
                {
                    let first = content.lines().next().unwrap_or("").chars().take(160).collect::<String>();
                    let lines = content.lines().count();
                    *content = format!("{first}\n[output elided to save context: {lines} lines; read again if needed]");
                    n += 1;
                }
            }
        }
        n
    }

    /// Tasks that can still be undone.
    pub fn undo_depth(&self) -> usize {
        self.checkpoints.len()
    }

    /// Records a file's content before its first change in this task.
    fn checkpoint(&mut self, name: &str, input: &Value) {
        let rel = match name {
            "design_brief" => "DESIGN.md",
            n if EDIT_TOOLS.contains(&n) => match input["path"].as_str() {
                Some(p) => p,
                None => return,
            },
            _ => return,
        };
        let Ok(path) = self.tools.resolve(rel) else { return };
        let Some(current) = self.checkpoints.last_mut() else { return };
        if current.iter().any(|(p, _, _)| *p == path) {
            return;
        }
        let before = std::fs::read_to_string(&path).ok();
        current.push((path, rel.replace('\\', "/"), before));
    }

    /// Replaces the conversation of a long task with a summary of what was
    /// done, keeping the task, when the context is nearly full.
    fn compact_midway(&mut self, task: &str, report: &TaskReport) {
        let mut steps = Vec::new();
        let mut last_text = String::new();
        for m in &self.history {
            for b in &m.content {
                match b {
                    ContentBlock::ToolUse { name, input, .. } => {
                        let arg = ["path", "command", "pattern", "symbol"].iter().find_map(|k| input[*k].as_str()).unwrap_or("");
                        steps.push(format!("- {name} {arg}"));
                    }
                    ContentBlock::ToolResult { is_error: true, content, .. } => {
                        steps.push(format!("  failed: {}", content.lines().next().unwrap_or("")));
                    }
                    ContentBlock::Text { text } if m.role == Role::Assistant => last_text.clone_from(text),
                    _ => {}
                }
            }
        }
        let keep = steps.len().saturating_sub(60);
        let files = report.edited.iter().cloned().collect::<Vec<_>>().join(", ");
        let summary = format!(
            "[The conversation was compacted because the context was nearly full.]\nTask: {task}\n\
             Steps so far (latest last):\n{}\nFiles changed so far: {}\nLast note: {}\n\
             Continue the task from here; re-read any file you need.",
            steps[keep..].join("\n"),
            if files.is_empty() { "none".into() } else { files },
            last_text.chars().take(1500).collect::<String>()
        );
        self.history = vec![Message { role: Role::User, content: vec![ContentBlock::Text { text: summary }] }];
        self.last_prompt = 0;
    }

    /// Rebuilds the system prompt so a just-added rule takes effect.
    pub fn reload_rules(&mut self, root: &std::path::Path) {
        self.system = system_prompt(self.tools.mode()) + &project_memory(root) + &stream_rules(root);
    }

    pub fn session_usage(&self) -> Usage {
        self.session
    }

    pub fn plan_mode(&self) -> bool {
        self.plan_mode
    }

    /// Turns plan mode on or off for the next task.
    pub fn set_plan_mode(&mut self, on: bool) {
        self.plan_mode = on;
    }

    /// Replaces the whole history with a deterministic summary of the last
    /// task. Earlier thinking blocks are not replayed, so nothing is edited.
    pub fn compact(&mut self) {
        let Some((prompt, report)) = self.last.take() else { return };
        let mut text = report.final_text.clone();
        if text.len() > SUMMARY_TEXT_MAX {
            let mut cut = SUMMARY_TEXT_MAX;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
            text.push('…');
        }
        let files = if report.edited.is_empty() {
            "none".to_string()
        } else {
            report.edited.iter().cloned().collect::<Vec<_>>().join(", ")
        };
        let mut summary = self.pending_summary.take().unwrap_or_default();
        summary.push_str(&format!(
            "[Previous task]\nRequest: {prompt}\nFiles changed: {files}\nResult: {text}\n\n"
        ));
        self.pending_summary = Some(summary);
        self.history.clear();
    }

    /// Runs one task. On a provider error the history is restored to what it
    /// was before the task, so the next task starts from a valid state.
    pub async fn run_task(&mut self, prompt: &str, host: &mut dyn Host) -> Result<TaskReport, ProviderError> {
        self.started = Some((self.history.len(), self.pending_summary.clone(), self.plan_mode));
        self.checkpoints.push(Vec::new());
        let root = self.tools.root().to_path_buf();
        hooks::event(&self.config.hooks, &root, "task_start", &serde_json::json!({"prompt": prompt})).await;
        let r = self.run_inner(prompt, host).await;
        let detail = match &r {
            Ok(rep) => serde_json::json!({"ok": true, "answer": rep.final_text}),
            Err(e) => serde_json::json!({"ok": false, "error": e.to_string()}),
        };
        hooks::event(&self.config.hooks, &root, "task_end", &detail).await;
        if self.checkpoints.last().is_some_and(Vec::is_empty) {
            self.checkpoints.pop();
        }
        if r.is_err() {
            self.interrupted();
        }
        self.started = None;
        r
    }

    /// Puts the history back as it was before a task whose future was
    /// dropped midway (the user interrupted it), so the next task starts
    /// from a valid conversation. Does nothing between tasks.
    pub fn interrupted(&mut self) {
        if let Some((len, summary, plan_mode)) = self.started.take() {
            self.history.truncate(len);
            self.pending_summary = summary;
            self.plan_mode = plan_mode;
        }
    }

    async fn run_inner(&mut self, prompt: &str, host: &mut dyn Host) -> Result<TaskReport, ProviderError> {
        let mut total = TaskReport::empty();
        let mut next = prompt.to_string();
        self.snapshot.clear();
        self.decided.clear();
        loop {
            let (report, handoff) = self.run_phase(&next, host).await?;
            total.stop = report.stop;
            total.turns += report.turns;
            total.usage += report.usage;
            total.final_text.clone_from(&report.final_text);
            total.edited.extend(report.edited.iter().cloned());
            total.verified = report.verified;
            match handoff {
                Some(Handoff::Run { compact, prompt: plan_prompt }) => {
                    if compact {
                        self.last = Some((next, report));
                        self.compact();
                    } else {
                        self.history.clear();
                        self.pending_summary = None;
                    }
                    next = plan_prompt;
                }
                Some(Handoff::Saved) => {
                    total.stop = Stop::PlanSaved;
                    break;
                }
                None => break,
            }
        }
        if total.verified.is_none() && self.config.gate.is_some() && code_edits(&total.edited) && total.stop != Stop::Done {
            // Stopped (turn limit, refusal) before the gate could pass.
            total.verified = Some(false);
        }
        self.last = Some((prompt.to_string(), total.clone()));
        Ok(total)
    }

    /// One conversation run until the model stops, or until an approved plan
    /// hands off to a fresh phase.
    async fn run_phase(
        &mut self,
        prompt: &str,
        host: &mut dyn Host,
    ) -> Result<(TaskReport, Option<Handoff>), ProviderError> {
        let prompt = if self.plan_mode { format!("{}{prompt}", plan::PLAN_NOTICE) } else { prompt.to_string() };
        let user_text = match self.pending_summary.take() {
            Some(s) => format!("{s}{prompt}"),
            None => prompt,
        };
        let task = user_text.clone();
        // Pictures the user points to by path travel with the message.
        let pictures = if self.config.vision { images_in(&user_text, self.tools.root()) } else { Vec::new() };
        let mut content = vec![ContentBlock::Text { text: user_text }];
        content.extend(pictures);
        self.history.push(Message { role: Role::User, content });

        let mut report = TaskReport::empty();
        let mut handoff = None;
        let mut fix_rounds = 0;
        let mut empty_retries = 0;

        report.stop = loop {
            if report.turns >= self.config.max_turns {
                break Stop::MaxTurns;
            }
            report.turns += 1;
            let window = self.config.context_window as u64;
            if window > 0 && self.last_prompt * 10 >= window * 6 && self.last_prompt * 10 < window * 8 && self.shake(4) > 0 {
                host.on_event(Event::Notice("context above 60%: old tool results were summarized"));
            }
            if window > 0 && self.last_prompt * 10 >= window * 8 && self.history.len() > 2 {
                self.compact_midway(&task, &report);
                host.on_event(Event::Notice("context above 80%: the conversation was summarized to continue"));
            }
            let model = match (&self.config.plan_model, self.plan_mode) {
                (Some(m), true) => m.clone(),
                _ => self.config.model.clone(),
            };
            let req = Request {
                model,
                system: self.system.clone(),
                // Read each round: tools found with tool_search join the next one.
                tools: self.tools.specs(),
                messages: self.history.clone(),
                max_tokens: self.config.max_tokens,
                effort: self.config.effort.clone(),
            };
            host.on_event(Event::Waiting("thinking"));
            let started = std::time::Instant::now();
            let resp = self
                .provider
                .complete(&req, &mut |d: Delta| match &d {
                    Delta::Text(t) => host.on_event(Event::Text(t)),
                    Delta::Thinking(t) => host.on_event(Event::Thinking(t)),
                    Delta::Limits(l) => host.on_event(Event::Limits(l)),
                    Delta::Notice(msg) => host.on_event(Event::Notice(msg)),
                    Delta::Composing { tool, path, chars, code } => {
                        host.on_event(Event::Composing { tool, path: path.as_deref(), chars: *chars, code: code.as_deref() })
                    }
                })
                .await?;
            let secs = started.elapsed().as_secs_f64();
            report.usage += resp.usage;
            self.session += resp.usage;
            self.last_prompt = resp.usage.prompt_total();
            let window = self.config.context_window;
            host.on_event(Event::Turn { turn: &resp.usage, task: &report.usage, secs, window });

            report.final_text = resp
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            let calls: Vec<(String, String, Value)> = resp
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, name, input } => Some((id.clone(), name.clone(), input.clone())),
                    _ => None,
                })
                .collect();
            let truncated = resp.stop_reason == "max_tokens";
            // A reply with no text and no tool call (a local model whose
            // malformed tool call was dropped by the server) is asked again,
            // without touching the history.
            let empty = calls.is_empty() && report.final_text.trim().is_empty() && !truncated && resp.stop_reason != "refusal";
            if empty && empty_retries < EMPTY_RETRIES {
                empty_retries += 1;
                host.on_event(Event::Notice("the model answered with nothing; asking again"));
                continue;
            }
            self.history.push(Message { role: Role::Assistant, content: resp.content });

            if resp.stop_reason == "refusal" {
                break Stop::Refused;
            }
            if calls.is_empty() {
                if truncated {
                    // Cut off mid-answer: ask it to continue.
                    self.history.push(Message {
                        role: Role::User,
                        content: vec![ContentBlock::Text { text: "Continue.".into() }],
                    });
                    continue;
                }
                if self.plan_mode || self.config.gate.is_none() || !code_edits(&report.edited) {
                    break Stop::Done;
                }
                host.on_event(Event::Waiting("verifying"));
                let edited: Vec<String> = report.edited.iter().cloned().collect();
                let (ok, summary) = self.verify(host, &edited).await;
                host.on_event(Event::Gate { ok: ok.unwrap_or(true), summary: &summary });
                match ok {
                    None => break Stop::Done,
                    Some(true) => {
                        report.verified = Some(true);
                        break Stop::Done;
                    }
                    Some(false) if fix_rounds >= self.config.max_fix_rounds => {
                        report.verified = Some(false);
                        break Stop::Done;
                    }
                    Some(false) => {
                        fix_rounds += 1;
                        host.on_event(Event::Fixing { attempt: fix_rounds, of: self.config.max_fix_rounds });
                        report.verified = Some(false);
                        self.history.push(Message { role: Role::User, content: vec![ContentBlock::Text { text: summary }] });
                        continue;
                    }
                }
            }

            // Subagents of this round run first, all at the same time.
            let mut sub_results: HashMap<String, ToolOutput> = HashMap::new();
            let subs: Vec<sub::SubTask> = calls.iter().filter(|(_, n, _)| n == "agent").map(|(id, _, input)| sub::SubTask::from_input(id, input)).collect();
            if !subs.is_empty() && !truncated && handoff.is_none() {
                if self.depth > 0 {
                    for s in subs {
                        sub_results.insert(s.id, ToolOutput::err("a subagent cannot start other subagents; do the work yourself"));
                    }
                } else {
                    for (_, name, input) in calls.iter().filter(|(_, n, _)| n == "agent") {
                        host.on_event(Event::ToolStart { name, input });
                    }
                    host.on_event(Event::Waiting("agents"));
                    sub_results.extend(sub::run_all(self.provider, self.tools, &self.config, subs).await);
                }
            }
            // The reads of this round run together: they change nothing, so
            // their order does not matter, and waiting for each in turn only
            // costs time (several files, searches, pages at once).
            let mut read_results: HashMap<String, ToolOutput> = HashMap::new();
            if !truncated && handoff.is_none() {
                let reads: Vec<(String, String, Value)> = calls
                    .iter()
                    .filter(|(id, n, i)| {
                        !sub_results.contains_key(id)
                            && !matches!(n.as_str(), "agent" | "propose_plan" | "ask_user" | "tasks")
                            && i.get(INVALID_JSON_KEY).is_none()
                            && coturbo_tools::tier(n) == coturbo_tools::Tier::Read
                            && approval::resolve(n, coturbo_tools::Tier::Read, i, &self.config.approval, &self.grants, self.plan_mode) == Verdict::Allow
                    })
                    .cloned()
                    .collect();
                if reads.len() > 1 {
                    for (_, name, input) in &reads {
                        host.on_event(Event::ToolStart { name, input });
                    }
                    let (root, tools, hook_list) = (self.tools.root().to_path_buf(), self.tools, &self.config.hooks);
                    let done = futures_util::future::join_all(reads.iter().map(|(id, name, input)| {
                        let root = &root;
                        async move {
                            if let hooks::Verdict::Block(why) = hooks::before(hook_list, root, name, input).await {
                                return (id.clone(), ToolOutput::err(format!("blocked by a hook: {why}")));
                            }
                            let mut out = tools.call(name, input).await;
                            let notes = hooks::after(hook_list, root, name, input, &out.content, out.is_error).await;
                            if !notes.is_empty() {
                                out.content.push('\n');
                                out.content.push_str(&notes);
                            }
                            (id.clone(), out)
                        }
                    }))
                    .await;
                    read_results.extend(done);
                }
            }
            // Every tool_use gets exactly one result, in one user message.
            let mut results = Vec::with_capacity(calls.len());
            let mut images = Vec::new();
            for (id, name, input) in calls {
                let mut diff = None;
                let out = if truncated {
                    ToolOutput::err("tool call was cut off by the output limit; not executed — retry with a smaller input")
                } else if let Some(raw) = input.get(INVALID_JSON_KEY) {
                    ToolOutput::err(format!("invalid JSON input, not executed: {raw}"))
                } else if handoff.is_some() {
                    ToolOutput::err("not executed: the plan was already answered in this turn")
                } else if let Some(out) = sub_results.remove(&id) {
                    out
                } else if let Some(out) = read_results.remove(&id) {
                    out
                } else {
                    host.on_event(Event::ToolStart { name: &name, input: &input });
                    if name == "propose_plan" {
                        let (out, h) = self.propose_plan(&input, host).await;
                        handoff = h;
                        out
                    } else if name == "ask_user" {
                        let question = input["question"].as_str().unwrap_or("").to_string();
                        let options: Vec<String> = input["options"].as_array().into_iter().flatten().filter_map(|o| o.as_str().map(str::to_string)).collect();
                        match host.ask_user(&question, &options).await {
                            Some(answer) => ToolOutput::ok(format!("The user answered: {answer}")),
                            None => ToolOutput::err("there is no one to ask right now; decide with your best judgement and say what you chose"),
                        }
                    } else {
                        let (out, d) = self.gated_call(&name, &input, host).await;
                        diff = d;
                        out
                    }
                };
                if !out.is_error
                    && EDIT_TOOLS.contains(&name.as_str())
                    && let Some(p) = input["path"].as_str()
                {
                    report.edited.insert(p.replace('\\', "/"));
                }
                let diff = diff.filter(|_| !out.is_error);
                host.on_event(Event::ToolEnd { name: &name, input: &input, output: &out, diff: diff.as_deref() });
                let mut content = out.content;
                if self.config.vision {
                    images.extend(out.images.into_iter().map(|(media_type, data)| ContentBlock::Image { media_type, data }));
                } else if !out.images.is_empty() {
                    content.push_str("\n(this model cannot see images; the screenshot was saved for the user)");
                }
                results.push(ContentBlock::ToolResult { tool_use_id: id, content, is_error: out.is_error });
            }
            // Images after every result: tool results must come first.
            results.extend(images);
            self.history.push(Message { role: Role::User, content: results });
            if handoff.is_some() {
                break Stop::Done;
            }
        };

        Ok((report, handoff))
    }

    /// Shows the proposed plan to the user and applies their choice.
    async fn propose_plan(&mut self, input: &Value, host: &mut dyn Host) -> (ToolOutput, Option<Handoff>) {
        if !self.plan_mode {
            return (ToolOutput::err("plan mode is not active; carry on with the task"), None);
        }
        let slug = input["slug"].as_str().unwrap_or("");
        let Some(path) = plan::plan_path(slug) else {
            return (ToolOutput::err(format!("invalid slug `{slug}`: use letters, digits, `-` or `_`")), None);
        };
        let read = self.tools.resolve(&path).and_then(|p| std::fs::read_to_string(p).map_err(|e| e.to_string()));
        let content = match read {
            Ok(c) if !c.trim().is_empty() => c,
            Ok(_) => return (ToolOutput::err(format!("{path} is empty; write the plan first")), None),
            Err(e) => return (ToolOutput::err(format!("cannot read {path}: {e}; write the plan first")), None),
        };
        let window = u64::from(self.config.context_window.max(1));
        let context_pct = (self.last_prompt.saturating_mul(100) / window).min(100) as u32;
        let review = PlanReview { slug: slug.to_string(), path: path.clone(), content, context_pct };
        let mut choice = host.review_plan(&review).await;
        if choice == PlanChoice::KeepContext && !review.keep_context_allowed() {
            choice = PlanChoice::Compact;
            host.on_event(Event::Notice("context above 95%: compacting before implementing"));
        }
        match choice {
            PlanChoice::Refine(feedback) => (ToolOutput::ok(plan::refine_message(&feedback)), None),
            PlanChoice::SaveQuit => {
                self.plan_mode = false;
                (ToolOutput::ok(plan::saved_message(&path)), Some(Handoff::Saved))
            }
            PlanChoice::KeepContext => {
                self.plan_mode = false;
                (ToolOutput::ok(plan::APPROVED_KEEP), None)
            }
            PlanChoice::Execute | PlanChoice::Compact => {
                self.plan_mode = false;
                // The user may have edited the file during review: run what is on disk.
                let content = self
                    .tools
                    .resolve(&path)
                    .ok()
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .filter(|c| !c.trim().is_empty())
                    .unwrap_or(review.content);
                let prompt = plan::execute_prompt(&path, &content);
                let compact = choice == PlanChoice::Compact;
                (ToolOutput::ok(plan::APPROVED_HANDOFF), Some(Handoff::Run { compact, prompt }))
            }
        }
    }

    /// Anti-cheat review, then the build/lint/test gate. `None` when there
    /// was nothing to run.
    async fn verify(&mut self, host: &mut dyn Host, edited: &[String]) -> (Option<bool>, String) {
        let root = self.tools.root().to_path_buf();
        let yolo = self.config.approval.mode == approval::ApprovalMode::Yolo;
        let mut rejected = Vec::new();
        for f in self.snapshot.findings(&root) {
            let key = (f.path.clone(), f.detail.clone());
            let accepted = match self.decided.get(&key) {
                Some(&d) => d,
                // Unattended runs have nobody to vouch for the edit: refuse it.
                None => {
                    let d = !yolo && host.confirm_cheat(&f).await;
                    self.decided.insert(key, d);
                    d
                }
            };
            if !accepted {
                rejected.push(f.render());
            }
        }
        if !rejected.is_empty() {
            let list = rejected.iter().map(|r| format!("- {r}")).collect::<Vec<_>>().join("\n");
            return (
                Some(false),
                format!(
                    "✗ verification refused: these edits weaken the checks instead of fixing the code:\n{list}\n\
                     Restore the tests/lints and fix the underlying problem."
                ),
            );
        }
        let cfg = self.config.gate.clone().unwrap_or_default();
        let steps = gate::detect(&root, &cfg);
        let known = self.step_secs.clone();
        let mut took: Vec<(String, f32)> = Vec::new();
        let report = gate::run_gate_with(&root, &steps, &cfg, &mut |p| match p {
            gate::GateProgress::Started { name, index, total } => {
                host.on_event(Event::Step { name, phase: StepPhase::Start, index, total, secs: 0.0, expected: known.get(name).copied() });
            }
            gate::GateProgress::Finished { result, index, total, secs } => {
                let phase = if result.ok { StepPhase::Passed } else { StepPhase::Failed };
                if result.ok && !result.skipped {
                    took.push((result.name.clone(), secs));
                }
                host.on_event(Event::Step { name: &result.name, phase, index, total, secs, expected: known.get(&result.name).copied() });
            }
        })
        .await;
        self.step_secs.extend(took);
        let ok = if report.nothing_ran() { None } else { Some(report.ok()) };
        if edited.iter().any(|f| coturbo_design::fiscal::is_ui_file(f)) {
            host.on_event(Event::Waiting("design"));
        }
        match coturbo_design::review(&root, edited).await {
            // Interface files changed: the design review counts like a gate step.
            Some(design) => {
                let summary = if report.nothing_ran() { design.summary } else { format!("{}\n{}", report.summary(), design.summary) };
                (Some(ok.unwrap_or(true) && design.ok), summary)
            }
            None => (ok, report.summary()),
        }
    }

    /// Applies the approval policy, asking the host when needed. Returns the
    /// output and, for edit tools, the diff they apply.
    async fn gated_call(&mut self, name: &str, input: &Value, host: &mut dyn Host) -> (ToolOutput, Option<String>) {
        let tier = coturbo_tools::tier(name);
        let verdict = approval::resolve(name, tier, input, &self.config.approval, &self.grants, self.plan_mode);
        if let Verdict::Deny(why) = verdict {
            return (ToolOutput::err(format!("denied: {why}")), None);
        }
        let preview = match self.tools.preview(name, input) {
            // The edit would fail anyway: run it to report why, nothing is written.
            Some(Err(_)) => return (self.tools.call(name, input).await, None),
            Some(Ok(diff)) => Some(diff),
            None => None,
        };
        if verdict == Verdict::Prompt {
            let req = ApprovalRequest { tool: name.to_string(), tier, input: input.clone(), preview: preview.clone() };
            match host.approve(&req).await {
                Decision::Once => {}
                Decision::Session => self.grants.grant(name, input),
                Decision::Deny(reason) => {
                    let msg = match reason {
                        Some(r) if !r.trim().is_empty() => format!("denied by user: {}", r.trim()),
                        _ => "denied by user".to_string(),
                    };
                    return (ToolOutput::err(msg), None);
                }
            }
        }
        if EDIT_TOOLS.contains(&name)
            && let Some(p) = input["path"].as_str()
            && let Ok(abs) = self.tools.resolve(p)
            && let Ok(rel) = abs.strip_prefix(self.tools.root())
        {
            self.snapshot.record(self.tools.root(), &rel.to_string_lossy());
        }
        self.checkpoint(name, input);
        let root = self.tools.root().to_path_buf();
        if let hooks::Verdict::Block(why) = hooks::before(&self.config.hooks, &root, name, input).await {
            return (ToolOutput::err(format!("blocked by a hook: {why}")), None);
        }
        let mut out = self.tools.call(name, input).await;
        let notes = hooks::after(&self.config.hooks, &root, name, input, &out.content, out.is_error).await;
        if !notes.is_empty() {
            out.content.push('\n');
            out.content.push_str(&notes);
        }
        (out, preview)
    }
}

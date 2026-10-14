//! The agent loop: send → stream → run tools → repeat.
//!
//! History is append-only within a task: rewriting earlier messages would
//! invalidate the provider's prompt cache and, on current Claude models,
//! the signed thinking blocks echoed back. Context is reclaimed only between
//! tasks, by [`Agent::compact`], which swaps the whole history for a summary.

pub mod approval;
pub mod plan;

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
    /// The model is writing a tool call: which tool, the file once known,
    /// and how much it has written.
    Composing { tool: &'a str, path: Option<&'a str>, chars: usize },
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

fn system_prompt(mode: Mode) -> String {
    let mut s = String::from(
        "You are CodeTurbo, a coding agent working in the user's project. \
         Use the tools to inspect and change files; paths are relative to the project root. \
         Make the smallest change that fully solves the task, then verify it. \
         Work like a senior engineer: read the relevant code before editing, find the root cause of an error \
         instead of patching symptoms, run what you changed (build, tests, the program), read the output, \
         and keep iterating until it really works; decide and act instead of asking when the next step is clear. \
         To learn how an existing site, app or binary does something, use investigate before building. \
         When done, reply with a short summary of what changed. \
         Reply in the language the user writes in. Write text files as UTF-8. \
         Do exactly what was asked: a bug fix, optimization or back-end task never changes the design, \
         the text or the features, and never adds anything that was not requested. \
         When improving an existing interface, keep the brand: organize its colors, do not replace them. \
         For any user interface work, call design_brief first and build only with the tokens it returns; \
         the design check rejects generic patterns (default fonts and palettes, emoji icons, \
         identical card grids, the template section order). After building a page, call design_review          to see it rendered and fix whatever still looks generic.",
    );
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
            system: system_prompt(tools.mode()) + &project_memory(tools.root()),
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
        }
    }

    /// Puts back every file the last task changed, as it was before the
    /// task. Returns the files restored; the next task is told.
    pub fn undo(&mut self) -> Result<Vec<String>, String> {
        let files = self.checkpoints.pop().ok_or("nenhuma alteração para desfazer")?;
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
        let r = self.run_inner(prompt, host).await;
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
        self.history.push(Message { role: Role::User, content: vec![ContentBlock::Text { text: user_text }] });

        let specs = self.tools.specs();
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
                host.on_event(Event::Notice("contexto acima de 60%: resultados antigos de ferramentas foram resumidos"));
            }
            if window > 0 && self.last_prompt * 10 >= window * 8 && self.history.len() > 2 {
                self.compact_midway(&task, &report);
                host.on_event(Event::Notice("contexto acima de 80%: a conversa foi resumida para continuar"));
            }
            let model = match (&self.config.plan_model, self.plan_mode) {
                (Some(m), true) => m.clone(),
                _ => self.config.model.clone(),
            };
            let req = Request {
                model,
                system: self.system.clone(),
                tools: specs.clone(),
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
                    Delta::Composing { tool, path, chars } => {
                        host.on_event(Event::Composing { tool, path: path.as_deref(), chars: *chars })
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
                host.on_event(Event::Notice("o modelo respondeu vazio; pedindo de novo"));
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
                        report.verified = Some(false);
                        self.history.push(Message { role: Role::User, content: vec![ContentBlock::Text { text: summary }] });
                        continue;
                    }
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
                } else {
                    host.on_event(Event::ToolStart { name: &name, input: &input });
                    if name == "propose_plan" {
                        let (out, h) = self.propose_plan(&input, host).await;
                        handoff = h;
                        out
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
            host.on_event(Event::Notice("contexto acima de 95%: compactando antes de implementar"));
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
        let report = gate::run_gate(&root, &steps, &cfg).await;
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
        (self.tools.call(name, input).await, preview)
    }
}

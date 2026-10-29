//! The agent's [`Host`] for the interface: events become conversation
//! lines, questions go to the screen and wait for the answer.

use super::text::ansi_line;
use super::{Out, Question, StageState, StageUpdate, StreamKind, TaskItem, TaskState, debugger};
use crate::config::Pricer;
use crate::splash::Mood;
use crate::status::{status_line, tool_line};
use crate::view;
use coturbo_agent::plan::{PlanChoice, PlanReview};
use coturbo_agent::{ApprovalRequest, Decision, Event, Host, StepPhase};
use coturbo_providers::{BoxFuture, Usage};
use coturbo_verify::Finding;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use tokio::sync::oneshot;

/// Rows of code shown before the rest is folded away (Ctrl+B shows it).
pub const PREVIEW_ROWS: usize = 10;

/// A diff as conversation rows: all of it when short, otherwise the first
/// [`PREVIEW_ROWS`] rows plus a note, with the full diff kept for Ctrl+B.
pub fn diff_fold(diff: &str, width: usize) -> Out {
    let full: Vec<Line<'static>> = view::render_diff_upto(diff, width, true, usize::MAX)
        .iter()
        .map(|r| ansi_line(&format!("  {r}")))
        .collect();
    if full.len() <= PREVIEW_ROWS {
        return Out::Clipped(full);
    }
    let hint = Style::new().add_modifier(Modifier::DIM);
    let mut preview = full[..PREVIEW_ROWS].to_vec();
    preview.push(Line::styled(
        format!(
            "  … {} more lines · click or Ctrl+B to view",
            full.len() - PREVIEW_ROWS
        ),
        hint,
    ));
    let mut full = full;
    full.push(Line::styled("  click or Ctrl+B to collapse", hint));
    Out::Fold { preview, full }
}

/// What the agent is waiting on, in the user's words.
pub fn busy_label(waiting: &str) -> String {
    match waiting {
        "thinking" => "think".into(),
        "verifying" => "verify: clippy, tests".into(),
        "design" => "design_review: desktop and phone".into(),
        "agents" => "subagents working in parallel".into(),
        other => other.into(),
    }
}

/// What a tool call is doing: the tool's own name and what it works on.
pub fn action_label(name: &str, input: &serde_json::Value) -> String {
    let a = arg(input);
    let target = input["target"].as_str().unwrap_or("");
    match name {
        "grep" => format!("grep \"{a}\""),
        "investigate" | "observe_app" => format!("{name} {target}"),
        "browse" => format!(
            "browse · {} step(s)",
            input["actions"].as_array().map_or(0, Vec::len)
        ),
        "network_scan" => {
            if input["sweep"].as_bool() == Some(true) {
                "network_scan · whole subnet".into()
            } else {
                "network_scan".into()
            }
        }
        "device" => {
            let s = |k: &str| input[k].as_str().unwrap_or("");
            let value = input["value"]
                .as_u64()
                .map(|v| v.to_string())
                .unwrap_or_default();
            let detail = [
                s("app"),
                s("key"),
                s("color"),
                s("topic"),
                value.as_str(),
                s("target"),
            ]
            .into_iter()
            .find(|v| !v.is_empty())
            .unwrap_or("");
            format!("device {} {} {detail}", s("device"), s("action"))
                .trim()
                .to_string()
        }
        "device_control" => {
            let s = |k: &str| input[k].as_str().unwrap_or("");
            match s("kind") {
                "http" => format!(
                    "device_control {} {}",
                    if s("method").is_empty() {
                        "GET"
                    } else {
                        s("method")
                    },
                    s("url")
                ),
                "upnp" => format!("device_control {} → {}", s("action"), s("control_url")),
                "wake" => format!("device_control wake {}", s("mac")),
                k => format!("device_control {k}"),
            }
        }
        other => format!("{other} {a}").trim().to_string(),
    }
}

/// Tools that run the heavy engines: reverse engineering, looking at a
/// running program and the design checks. They put the snail in turbo.
fn is_heavy(tool: &str) -> bool {
    matches!(
        tool,
        "investigate" | "observe_app" | "design_review" | "design_brief" | "browse"
    )
}

/// Whether a tool result carries a design check with blocking findings
/// (`design <path>: 2 blocking, 1 warning(s)`).
pub fn design_refused(text: &str) -> bool {
    text.lines().any(|l| {
        l.starts_with("design ")
            && l.find(" blocking,").is_some_and(|i| {
                l[..i]
                    .rsplit(' ')
                    .next()
                    .and_then(|n| n.parse::<u32>().ok())
                    .is_some_and(|n| n > 0)
            })
    })
}

/// Tools that write to the project.
fn is_edit(tool: &str) -> bool {
    matches!(
        tool,
        "write_file" | "edit_file" | "replace_symbol" | "insert_symbol"
    )
}

/// The checklist the model sent through the `tasks` tool.
pub fn task_items(input: &serde_json::Value) -> Vec<TaskItem> {
    input["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|it| {
            let title = it["title"].as_str()?.trim().to_string();
            let state = match it["status"].as_str()? {
                "done" => TaskState::Done,
                "doing" => TaskState::Doing,
                _ => TaskState::Pending,
            };
            (!title.is_empty()).then_some(TaskItem { title, state })
        })
        .collect()
}

/// The result of the check that follows every edit (`check src/a.rs: 2 error(s), ...`):
/// `Some(true)` clean, `Some(false)` with errors, `None` when there is none.
pub fn edit_check(output: &str) -> Option<(bool, String)> {
    let line = output.lines().find(|l| l.starts_with("check "))?;
    let clean = line.ends_with(": clean");
    let note = if clean {
        "no problems".to_string()
    } else {
        line.split_once(": ")
            .map_or(String::new(), |(_, r)| r.to_string())
    };
    Some((clean, note))
}

pub struct TuiHost {
    out: Sender<Out>,
    pricer: Pricer,
    root: PathBuf,
    verbose: bool,
    limits: Option<String>,
    /// Session usage before this task, to show the running total.
    base: Usage,
    /// Width diffs are drawn at.
    width: usize,
    label: String,
    /// File of the tool call being written, once its path streamed in.
    composing_path: Option<String>,
    /// Model responses so far in this task, and the last tool run.
    turns: u32,
    last_step: Option<String>,
    /// Tools run in this task, whether a heavy engine ran, and the mood last
    /// sent to the screen.
    tools: u32,
    heavy: bool,
    mood: Option<Mood>,
    /// The design review ran in this task, so the gate result covers it too.
    designing: bool,
    /// The model has started to answer (text, reasoning or a tool call).
    produced: bool,
}

impl TuiHost {
    pub fn new(
        out: Sender<Out>,
        pricer: Pricer,
        root: PathBuf,
        verbose: bool,
        base: Usage,
    ) -> Self {
        let width = terminal_size::terminal_size()
            .map_or(100, |(w, _)| w.0 as usize)
            .min(140);
        let _ = out.send(Out::ClearWork);
        Self {
            out,
            pricer,
            root,
            verbose,
            limits: None,
            base,
            width,
            label: String::new(),
            composing_path: None,
            turns: 0,
            last_step: None,
            tools: 0,
            heavy: false,
            mood: None,
            designing: false,
            produced: false,
        }
    }

    fn stage(&self, name: &str, state: StageState, note: impl Into<String>) {
        self.send(Out::Stage(StageUpdate::new(name, state, note)));
    }

    fn send(&self, m: Out) {
        let _ = self.out.send(m);
    }

    fn lines(&self, lines: Vec<Line<'static>>) {
        self.send(Out::Lines(lines));
    }

    /// The snail's pace follows the effort: it runs for an answer, runs hard
    /// once tools are in play, and goes flat out on the heavy engines or a
    /// long task.
    fn working_mood(&self) -> Mood {
        if self.heavy || self.turns >= 8 {
            Mood::Turbo
        } else if self.tools > 0 {
            Mood::Hard
        } else if self.produced || self.turns > 0 {
            Mood::Run
        } else {
            // The task has begun but the model has not answered yet.
            Mood::Start
        }
    }

    fn feel(&mut self) {
        let m = self.working_mood();
        if self.mood != Some(m) {
            self.mood = Some(m);
            self.send(Out::Mood(m));
        }
    }

    /// A short flash (alert, denied) over the working mood.
    fn flash(&self, m: Mood) {
        self.send(Out::Mood(m));
    }

    fn busy(&mut self, label: String) {
        self.feel();
        if self.label != label {
            self.label.clone_from(&label);
            self.send(Out::Busy(Some(label)));
        }
    }

    async fn ask<T>(&self, make: impl FnOnce(oneshot::Sender<T>) -> Question, otherwise: T) -> T {
        let (tx, rx) = oneshot::channel();
        self.send(Out::Ask(make(tx)));
        rx.await.unwrap_or(otherwise)
    }
}

fn arg(input: &serde_json::Value) -> &str {
    ["command", "path", "symbol", "pattern"]
        .iter()
        .find_map(|k| input[*k].as_str())
        .unwrap_or("")
}

impl Host for TuiHost {
    fn on_event(&mut self, ev: Event<'_>) {
        match ev {
            Event::Waiting("thinking") if self.turns > 0 => {
                let last = self
                    .last_step
                    .clone()
                    .map(|s| format!(" · last: {s}"))
                    .unwrap_or_default();
                self.busy(format!("think · round {}{last}", self.turns + 1));
            }
            Event::Waiting(what) => {
                // The final checks are the heaviest work of a task.
                self.heavy |= matches!(what, "verifying" | "design");
                if what == "verifying" {
                    self.stage("WRITE", StageState::Done, "");
                }
                if what == "design" {
                    self.designing = true;
                    self.stage("DESIGN", StageState::Running, "desktop and phone");
                }
                self.busy(busy_label(what));
            }
            Event::Thinking(delta) => {
                self.produced = true;
                self.busy("think".into());
                self.send(Out::Stream(StreamKind::Thinking, delta.to_string()));
            }
            Event::Text(delta) => {
                self.produced = true;
                self.busy("write".into());
                self.send(Out::Stream(StreamKind::Text, delta.to_string()));
            }
            Event::Limits(headers) => self.limits = view::limits_summary(headers),
            Event::ToolStart { name, input } => {
                self.send(Out::Live(None));
                self.composing_path = None;
                self.produced = true;
                self.tools += 1;
                self.heavy |= is_heavy(name);
                if is_edit(name) {
                    self.stage("WRITE", StageState::Running, arg(input));
                }
                self.busy(action_label(name, input));
            }
            Event::ToolEnd {
                name,
                input,
                output,
                diff,
            } => {
                self.last_step = Some(format!("{name} {}", arg(input)).trim().to_string());
                if design_refused(&output.content) {
                    self.flash(Mood::Denied);
                }
                if name == "tasks" && !output.is_error {
                    self.send(Out::Tasks(task_items(input)));
                }
                if is_edit(name) && !output.is_error {
                    self.stage("WRITE", StageState::Done, arg(input));
                    if let Some((clean, note)) = edit_check(&output.content) {
                        self.stage(
                            "CHECK",
                            if clean {
                                StageState::Done
                            } else {
                                StageState::Failed
                            },
                            note,
                        );
                        if !clean {
                            let found = debugger::parse(&output.content);
                            self.send(Out::Clipped(debugger::render(
                                &found, &self.root, self.width,
                            )));
                        }
                    }
                }
                let mut lines = vec![Line::default(), ansi_line(&tool_line(name, input, output))];
                if let Some(d) = diff {
                    let (add, del) = view::diff_stat(d);
                    lines.push(ansi_line(&format!(
                        "  \x1b[32m+{add}\x1b[0m \x1b[31m-{del}\x1b[0m"
                    )));
                    self.lines(lines);
                    self.send(diff_fold(d, self.width.saturating_sub(6)));
                } else {
                    self.lines(lines);
                }
            }
            Event::Turn {
                turn,
                task,
                secs,
                window,
            } => {
                self.turns += 1;
                let cost = self.pricer.cost(turn);
                let line = view::turn_line(turn, secs, window, cost, self.limits.as_deref());
                let mut lines = vec![Line::styled(
                    format!("  {line}"),
                    Style::new().add_modifier(Modifier::DIM),
                )];
                if self.verbose {
                    lines.push(Line::styled(
                        format!("  · task {}", status_line(task, None)),
                        Style::new().add_modifier(Modifier::DIM),
                    ));
                }
                self.lines(lines);
                let mut total = self.base;
                total += *task;
                self.send(Out::Usage(status_line(&total, self.pricer.cost(&total))));
                if window > 0 {
                    self.send(Out::Context(
                        (turn.prompt_total() * 100 / window as u64) as u32,
                    ));
                }
            }
            Event::Gate { ok, summary } => {
                let color = if ok { Color::Green } else { Color::Red };
                if !ok {
                    // A refused design turns the snail red; any other failed check, yellow.
                    self.flash(if summary.to_lowercase().contains("design") {
                        Mood::Denied
                    } else {
                        Mood::Alert
                    });
                    // The debugger shows the exact lines.
                    let found = debugger::parse(summary);
                    let root = self.root.clone();
                    self.send(Out::Block(super::Dynamic(std::sync::Arc::new(move |w| {
                        debugger::render(&found, &root, w)
                    }))));
                }
                if self.designing {
                    self.designing = false;
                    let refused = design_refused(summary);
                    self.stage(
                        "DESIGN",
                        if refused {
                            StageState::Failed
                        } else {
                            StageState::Done
                        },
                        if refused { "refused" } else { "approved" },
                    );
                }
                // The report sits in a box of its own, wrapped to fit, so it reads as one piece.
                let mut summary = summary.lines();
                let mut body = vec![Line::styled(
                    summary.next().unwrap_or("").to_string(),
                    Style::new().fg(color),
                )];
                body.extend(summary.map(|l| {
                    Line::styled(l.to_string(), Style::new().add_modifier(Modifier::DIM))
                }));
                let title = vec![ratatui::text::Span::styled(
                    " verification ",
                    Style::new().fg(color).add_modifier(Modifier::BOLD),
                )];
                self.lines(vec![Line::default()]);
                self.send(Out::Block(super::Dynamic(std::sync::Arc::new(move |w| {
                    crate::tui::text::boxed(&body, w, Color::Rgb(190, 198, 212), title.clone())
                }))));
            }
            Event::Composing {
                tool,
                path,
                chars,
                code,
            } => {
                if let Some(p) = path {
                    self.composing_path = Some(p.to_string());
                }
                self.produced = true;
                if is_edit(tool) {
                    self.stage(
                        "WRITE",
                        StageState::Running,
                        self.composing_path.clone().unwrap_or_default(),
                    );
                }
                // The tool being written and what it is writing to.
                let target = self.composing_path.clone().unwrap_or_default();
                let size = if chars >= 1000 {
                    format!("{:.1}k chars", chars as f64 / 1000.0)
                } else {
                    format!("{chars} chars")
                };
                self.busy(format!("{tool} {target} · {size}").replace("  ", " "));
                // The code appears as it is written.
                if is_edit(tool)
                    && let Some(code) = code
                {
                    let lines = code
                        .lines()
                        .map(|l| {
                            Line::styled(
                                format!(
                                    "  {}",
                                    l.chars()
                                        .take(self.width.saturating_sub(8).max(20))
                                        .collect::<String>()
                                ),
                                Style::new().fg(Color::Rgb(190, 200, 215)),
                            )
                        })
                        .collect();
                    self.send(Out::Live(Some((format!("writing {target}"), lines))));
                }
            }
            Event::Step {
                name,
                phase,
                index,
                total,
                secs,
                expected,
            } => {
                let stage = name.to_uppercase();
                let upd = |state, note: String| StageUpdate {
                    name: stage.clone(),
                    state,
                    note,
                    expected,
                };
                let update = match phase {
                    StepPhase::Start => {
                        self.stage("FIX", StageState::Done, "");
                        upd(StageState::Running, format!("{}/{}", index + 1, total))
                    }
                    StepPhase::Passed => upd(StageState::Done, format!("{secs:.1}s")),
                    StepPhase::Failed => upd(StageState::Failed, format!("{secs:.1}s")),
                };
                self.send(Out::Stage(update));
            }
            Event::Fixing { attempt, of } => {
                self.stage(
                    "FIX",
                    StageState::Running,
                    format!("attempt {attempt}/{of}"),
                );
            }
            Event::Notice(msg) => {
                self.flash(Mood::Alert);
                self.lines(vec![Line::styled(
                    format!("  ! {msg}"),
                    Style::new().fg(Color::Yellow),
                )]);
            }
        }
    }

    fn approve<'a>(&'a mut self, req: &'a ApprovalRequest) -> BoxFuture<'a, Decision> {
        Box::pin(async move {
            let title = format!("Allow {} {}?", req.tool, arg(&req.input));
            if let Some(diff) = &req.preview {
                let title = format!("  proposed change: {}", arg(&req.input));
                self.lines(vec![
                    Line::default(),
                    Line::styled(title, Style::new().fg(Color::Yellow)),
                ]);
                self.send(diff_fold(diff, self.width.saturating_sub(6)));
            }
            self.label.clear();
            self.send(Out::Busy(Some("wait: your answer".into())));
            let decision = self
                .ask(
                    |reply| Question::Approve { title, reply },
                    Decision::Deny(Some("the user did not answer".into())),
                )
                .await;
            // Answered: the line shows what now runs, not the question that is gone.
            let next = if matches!(decision, Decision::Deny(_)) {
                busy_label("thinking")
            } else {
                action_label(&req.tool, &req.input)
            };
            self.busy(next);
            decision
        })
    }

    fn review_plan<'a>(&'a mut self, plan: &'a PlanReview) -> BoxFuture<'a, PlanChoice> {
        Box::pin(async move {
            let header = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
            let mut lines = vec![
                Line::default(),
                Line::styled(
                    format!(
                        "── plan: {} ({}% of the context) ──",
                        plan.path, plan.context_pct
                    ),
                    header,
                ),
            ];
            lines.extend(
                plan.content
                    .trim_end()
                    .lines()
                    .map(|l| Line::raw(l.to_string())),
            );
            lines.push(Line::styled("──", header));
            self.lines(lines);
            self.label.clear();
            self.send(Out::Busy(Some("wait: your answer".into())));
            let path = self.root.join(&plan.path);
            let keep_allowed = plan.keep_context_allowed();
            let title = "What to do with the plan?".to_string();
            self.ask(
                |reply| Question::Plan {
                    title,
                    path,
                    keep_allowed,
                    reply,
                },
                PlanChoice::SaveQuit,
            )
            .await
        })
    }

    fn ask_user<'a>(
        &'a mut self,
        question: &'a str,
        options: &'a [String],
    ) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            self.label.clear();
            self.send(Out::Busy(Some("wait: your answer".into())));
            let choices = options
                .iter()
                .map(|o| super::Choice::new(o.clone(), o.clone()))
                .collect();
            let title = format!("{question} · pick one or type your answer");
            let answer = self
                .ask(
                    |reply| Question::Select {
                        title,
                        options: choices,
                        custom: true,
                        reply,
                    },
                    None,
                )
                .await;
            self.busy(busy_label("thinking"));
            answer
        })
    }

    fn confirm_cheat<'a>(&'a mut self, finding: &'a Finding) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            self.lines(vec![
                Line::default(),
                Line::styled(
                    format!("  ! suspicious change: {}", finding.render()),
                    Style::new().fg(Color::Yellow),
                ),
            ]);
            self.label.clear();
            self.send(Out::Busy(Some("wait: your answer".into())));
            let title = "Is this change legitimate?".to_string();
            self.ask(|reply| Question::Confirm { title, reply }, false)
                .await
        })
    }
}

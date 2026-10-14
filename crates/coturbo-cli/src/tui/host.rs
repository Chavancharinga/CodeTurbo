//! The agent's [`Host`] for the interface: events become conversation
//! lines, questions go to the screen and wait for the answer.

use super::text::ansi_line;
use super::{Out, Question, StreamKind};
use crate::config::Pricer;
use crate::status::{status_line, tool_line};
use crate::view;
use coturbo_agent::plan::{PlanChoice, PlanReview};
use coturbo_agent::{ApprovalRequest, Decision, Event, Host};
use coturbo_providers::{BoxFuture, Usage};
use coturbo_verify::Finding;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use tokio::sync::oneshot;

/// What the agent is waiting on, in the user's words.
pub fn busy_label(waiting: &str) -> String {
    match waiting {
        "thinking" => "pensando".into(),
        "verifying" => "verificando: compilação, análise e testes".into(),
        other => other.into(),
    }
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
}

impl TuiHost {
    pub fn new(out: Sender<Out>, pricer: Pricer, root: PathBuf, verbose: bool, base: Usage) -> Self {
        let width = terminal_size::terminal_size().map_or(100, |(w, _)| w.0 as usize).min(140);
        Self { out, pricer, root, verbose, limits: None, base, width, label: String::new() }
    }

    fn send(&self, m: Out) {
        let _ = self.out.send(m);
    }

    fn lines(&self, lines: Vec<Line<'static>>) {
        self.send(Out::Lines(lines));
    }

    fn busy(&mut self, label: String) {
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
    ["command", "path", "symbol", "pattern"].iter().find_map(|k| input[*k].as_str()).unwrap_or("")
}

impl Host for TuiHost {
    fn on_event(&mut self, ev: Event<'_>) {
        match ev {
            Event::Waiting(what) => self.busy(busy_label(what)),
            Event::Thinking(delta) => {
                self.busy("raciocinando".into());
                self.send(Out::Stream(StreamKind::Thinking, delta.to_string()));
            }
            Event::Text(delta) => {
                self.busy("escrevendo".into());
                self.send(Out::Stream(StreamKind::Text, delta.to_string()));
            }
            Event::Limits(headers) => self.limits = view::limits_summary(headers),
            Event::ToolStart { name, input } => self.busy(format!("executando {name} {}", arg(input))),
            Event::ToolEnd { name, input, output, diff } => {
                let mut lines = vec![Line::default(), ansi_line(&tool_line(name, input, output))];
                if let Some(d) = diff {
                    let (add, del) = view::diff_stat(d);
                    lines.push(ansi_line(&format!("    \x1b[32m+{add}\x1b[0m \x1b[31m-{del}\x1b[0m")));
                    for row in view::render_diff(d, self.width.saturating_sub(6), true) {
                        lines.push(ansi_line(&format!("    {row}")));
                    }
                }
                self.lines(lines);
            }
            Event::Turn { turn, task, secs, window } => {
                let cost = self.pricer.cost(turn);
                let line = view::turn_line(turn, secs, window, cost, self.limits.as_deref());
                let mut lines = vec![Line::styled(format!("  {line}"), Style::new().add_modifier(Modifier::DIM))];
                if self.verbose {
                    lines.push(Line::styled(format!("  · tarefa {}", status_line(task, None)), Style::new().add_modifier(Modifier::DIM)));
                }
                self.lines(lines);
                let mut total = self.base;
                total += *task;
                self.send(Out::Usage(status_line(&total, self.pricer.cost(&total))));
                if window > 0 {
                    self.send(Out::Context((turn.prompt_total() * 100 / window as u64) as u32));
                }
            }
            Event::Gate { ok, summary } => {
                let color = if ok { Color::Green } else { Color::Red };
                let mut summary = summary.lines();
                let mut lines =
                    vec![Line::default(), Line::styled(summary.next().unwrap_or("").to_string(), Style::new().fg(color))];
                lines.extend(summary.map(|l| Line::styled(l.to_string(), Style::new().add_modifier(Modifier::DIM))));
                self.lines(lines);
            }
            Event::Notice(msg) => {
                self.lines(vec![Line::styled(format!("  ! {msg}"), Style::new().fg(Color::Yellow))]);
            }
        }
    }

    fn approve<'a>(&'a mut self, req: &'a ApprovalRequest) -> BoxFuture<'a, Decision> {
        Box::pin(async move {
            let title = format!("Permitir {} {}?", req.tool, arg(&req.input));
            if let Some(diff) = &req.preview {
                let mut lines = vec![Line::default(), Line::styled(format!("  alteração proposta: {}", arg(&req.input)), Style::new().fg(Color::Yellow))];
                for row in view::render_diff(diff, self.width.saturating_sub(6), true) {
                    lines.push(ansi_line(&format!("    {row}")));
                }
                self.lines(lines);
            }
            self.label.clear();
            self.send(Out::Busy(Some("aguardando sua resposta".into())));
            self.ask(|reply| Question::Approve { title, reply }, Decision::Deny(Some("o usuário não respondeu".into()))).await
        })
    }

    fn review_plan<'a>(&'a mut self, plan: &'a PlanReview) -> BoxFuture<'a, PlanChoice> {
        Box::pin(async move {
            let header = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
            let mut lines = vec![Line::default(), Line::styled(format!("── plano: {} ({}% do contexto) ──", plan.path, plan.context_pct), header)];
            lines.extend(plan.content.trim_end().lines().map(|l| Line::raw(l.to_string())));
            lines.push(Line::styled("──", header));
            self.lines(lines);
            self.label.clear();
            self.send(Out::Busy(Some("aguardando sua resposta".into())));
            let path = self.root.join(&plan.path);
            let keep_allowed = plan.keep_context_allowed();
            let title = "O que fazer com o plano?".to_string();
            self.ask(|reply| Question::Plan { title, path, keep_allowed, reply }, PlanChoice::SaveQuit).await
        })
    }

    fn confirm_cheat<'a>(&'a mut self, finding: &'a Finding) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            self.lines(vec![
                Line::default(),
                Line::styled(format!("  ! alteração suspeita: {}", finding.render()), Style::new().fg(Color::Yellow)),
            ]);
            self.label.clear();
            self.send(Out::Busy(Some("aguardando sua resposta".into())));
            let title = "Esta alteração é legítima?".to_string();
            self.ask(|reply| Question::Confirm { title, reply }, false).await
        })
    }
}

//! Terminal output: the token/cost status line and the terminal host.

use coturbo_agent::plan::{PlanChoice, PlanReview};
use coturbo_agent::{ApprovalRequest, Decision, Event, Host};
use coturbo_verify::Finding;
use coturbo_providers::{BoxFuture, Usage};
use coturbo_tools::ToolOutput;
use serde_json::Value;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

/// `950`, `12.3k`, `1.20M`.
pub fn human(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.2}M", n as f64 / 1e6),
    }
}

/// `tokens ↑in ⟳cache ↓out · $cost`. `↑` counts prompt tokens billed at
/// full or write price; `⟳` counts cache reads. No price, no dollar figure.
pub fn status_line(u: &Usage, cost: Option<f64>) -> String {
    let mut s = format!(
        "tokens ↑{} ⟳{} ↓{}",
        human(u.input + u.cache_write),
        human(u.cache_read),
        human(u.output)
    );
    if let Some(c) = cost {
        s.push_str(&format!(" · ${c:.4}"));
    }
    s
}

/// One-line, length-capped view of a tool call for the progress log.
pub fn tool_line(name: &str, input: &Value, out: &ToolOutput) -> String {
    const MAX: usize = 80;
    let arg = ["path", "symbol", "pattern", "command"]
        .iter()
        .find_map(|k| input[*k].as_str())
        .unwrap_or("");
    let first = out.content.lines().next().unwrap_or("");
    let clip = |s: &str| -> String {
        if s.chars().count() <= MAX {
            s.to_string()
        } else {
            let mut c: String = s.chars().take(MAX).collect();
            c.push('…');
            c
        }
    };
    let mark = if out.is_error { "✗" } else { "✓" };
    let mut line = format!("  {mark} {name} {}", clip(arg));
    if out.is_error {
        line.push_str(&format!(" — {}", clip(first)));
    }
    line
}

/// Parses an answer to the approval prompt: `o`/`once`/`y`, `s`/`session`,
/// `d`/`deny`/`n` (anything after the first word is the deny reason).
pub fn parse_answer(line: &str) -> Option<Decision> {
    let line = line.trim();
    let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let rest = rest.trim();
    match word.to_ascii_lowercase().as_str() {
        "o" | "once" | "y" | "yes" | "" => Some(Decision::Once),
        "s" | "session" | "a" | "always" => Some(Decision::Session),
        "d" | "deny" | "n" | "no" => Some(Decision::Deny((!rest.is_empty()).then(|| rest.to_string()))),
        _ => None,
    }
}

/// One entry of the plan review menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanMenu {
    Choice(PlanChoice),
    /// Ask for feedback, then refine.
    Refine,
    /// Open the plan in `$EDITOR`, then show the menu again.
    Edit,
}

pub const PLAN_MENU: [&str; 5] = [
    "Approve and execute (fresh context, plan only)",
    "Approve and compact context",
    "Approve and keep context",
    "Refine plan",
    "Save and quit",
];

/// `1`–`5` pick a menu entry, `e` edits. Option 3 is refused when the
/// context is nearly full.
pub fn parse_plan_choice(line: &str, keep_allowed: bool) -> Option<PlanMenu> {
    match line.trim().to_ascii_lowercase().as_str() {
        "1" => Some(PlanMenu::Choice(PlanChoice::Execute)),
        "2" => Some(PlanMenu::Choice(PlanChoice::Compact)),
        "3" if keep_allowed => Some(PlanMenu::Choice(PlanChoice::KeepContext)),
        "4" => Some(PlanMenu::Refine),
        "5" => Some(PlanMenu::Choice(PlanChoice::SaveQuit)),
        "e" | "edit" => Some(PlanMenu::Edit),
        _ => None,
    }
}

fn read_line() -> Option<String> {
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line),
    }
}

/// Colors a unified diff for a terminal.
pub fn color_diff(diff: &str) -> String {
    diff.lines()
        .map(|l| match l.as_bytes().first() {
            Some(b'+') if !l.starts_with("+++") => format!("\x1b[32m{l}\x1b[0m\n"),
            Some(b'-') if !l.starts_with("---") => format!("\x1b[31m{l}\x1b[0m\n"),
            Some(b'@') => format!("\x1b[36m{l}\x1b[0m\n"),
            _ => format!("{l}\n"),
        })
        .collect()
}

/// Terminal host: streams model text to stdout, tool calls to stderr, and
/// asks approval questions on stdin (denying when there is no terminal).
/// What the model is currently streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Streaming {
    #[default]
    Nothing,
    Thinking,
    Text,
}

pub struct TermHost {
    pub verbose: bool,
    /// Prices the per-turn line; `None` hides the dollar figure.
    pub pricer: Option<crate::config::Pricer>,
    pub streaming: Streaming,
    /// Rate limits from the last response.
    pub limits: Option<String>,
    /// Running snail while waiting on the model or the gate.
    pub spinner: Option<crate::splash::Spinner>,
    /// Project root, to open plan files in `$EDITOR`.
    pub root: PathBuf,
}

impl TermHost {
    /// Closes a thinking/text stream before other output.
    fn end_stream(&mut self) {
        match self.streaming {
            Streaming::Thinking => eprintln!("\x1b[0m"),
            Streaming::Text => println!(),
            Streaming::Nothing => {}
        }
        self.streaming = Streaming::Nothing;
    }

    fn ask(&mut self, req: &ApprovalRequest) -> Decision {
        if let Some(mut s) = self.spinner.take() {
            s.stop();
        }
        if !std::io::stdin().is_terminal() {
            return Decision::Deny(Some("no terminal to ask the user; rerun with --auto-approve".into()));
        }
        let arg = ["command", "path"].iter().find_map(|k| req.input[*k].as_str()).unwrap_or("");
        eprintln!("\n\x1b[33m? {} {}\x1b[0m", req.tool, arg);
        if let Some(diff) = &req.preview {
            eprint!("{}", color_diff(diff));
        }
        loop {
            eprint!("  [o]nce  [s]ession  [d]eny <reason> › ");
            let _ = std::io::stderr().flush();
            let Some(line) = read_line() else { return Decision::Deny(Some("cancelled".into())) };
            if let Some(d) = parse_answer(&line) {
                return d;
            }
        }
    }

    /// Yes/no question on the terminal; `false` without a terminal.
    fn yes_no(&self, question: &str) -> bool {
        if !std::io::stdin().is_terminal() {
            return false;
        }
        eprint!("{question} [y/N] › ");
        let _ = std::io::stderr().flush();
        read_line().is_some_and(|l| matches!(l.trim().to_ascii_lowercase().as_str(), "y" | "yes" | "s" | "sim"))
    }

    /// Asks once whether the project's `.codeturbo.toml` commands may run.
    pub fn trust_project(&self, commands: &[String]) -> bool {
        if commands.is_empty() {
            return true;
        }
        eprintln!("\x1b[33m? this project's .codeturbo.toml wants the verification gate to run:\x1b[0m");
        for c in commands {
            eprintln!("    {c}");
        }
        self.yes_no("  allow for this session?")
    }

    fn review(&mut self, plan: &PlanReview) -> PlanChoice {
        if let Some(mut s) = self.spinner.take() {
            s.stop();
        }
        if !std::io::stdin().is_terminal() {
            return PlanChoice::SaveQuit;
        }
        let full = self.root.join(&plan.path);
        let mut content = plan.content.clone();
        loop {
            eprintln!("\n\x1b[1;36m── plan: {} ({}% context) ──\x1b[0m\n{}", plan.path, plan.context_pct, content.trim_end());
            eprintln!("\x1b[1;36m──\x1b[0m");
            for (i, label) in PLAN_MENU.iter().enumerate() {
                let off = i == 2 && !plan.keep_context_allowed();
                let mark = if off { " (context > 95%: unavailable)" } else { "" };
                eprintln!("  {}. {label}{mark}", i + 1);
            }
            eprint!("  e. edit in $EDITOR   › ");
            let _ = std::io::stderr().flush();
            let Some(line) = read_line() else { return PlanChoice::SaveQuit };
            match parse_plan_choice(&line, plan.keep_context_allowed()) {
                Some(PlanMenu::Choice(c)) => return c,
                Some(PlanMenu::Refine) => {
                    eprint!("  what should change? › ");
                    let _ = std::io::stderr().flush();
                    let feedback = read_line().unwrap_or_default();
                    return PlanChoice::Refine(feedback.trim().to_string());
                }
                Some(PlanMenu::Edit) => {
                    let editor = std::env::var("VISUAL")
                        .or_else(|_| std::env::var("EDITOR"))
                        .unwrap_or_else(|_| if cfg!(windows) { "notepad".into() } else { "vi".into() });
                    let status = std::process::Command::new(&editor).arg(&full).status();
                    if let Err(e) = status {
                        eprintln!("  ! cannot start {editor}: {e}");
                    }
                    if let Ok(c) = std::fs::read_to_string(&full) {
                        content = c;
                    }
                }
                None => {}
            }
        }
    }
}

impl Host for TermHost {
    fn on_event(&mut self, ev: Event<'_>) {
        if let Event::Waiting(label) = ev {
            self.spinner = Some(crate::splash::Spinner::start(label));
            return;
        }
        if let Some(mut s) = self.spinner.take() {
            s.stop();
        }
        if !matches!(ev, Event::Text(_) | Event::Thinking(_)) {
            self.end_stream();
        }
        match ev {
            Event::Thinking(delta) => {
                let mut err = std::io::stderr().lock();
                if self.streaming != Streaming::Thinking {
                    let _ = write!(err, "\n\x1b[2;3m∴ ");
                    self.streaming = Streaming::Thinking;
                }
                let _ = write!(err, "{}", delta.replace('\n', "\n  "));
                let _ = err.flush();
            }
            Event::Text(delta) => {
                if self.streaming == Streaming::Thinking {
                    eprint!("\x1b[0m\n\n");
                }
                self.streaming = Streaming::Text;
                let mut out = std::io::stdout().lock();
                let _ = out.write_all(delta.as_bytes());
                let _ = out.flush();
            }
            Event::Limits(headers) => self.limits = crate::view::limits_summary(headers),
            Event::ToolEnd { name, input, output, diff } => {
                eprintln!("\n{}", tool_line(name, input, output));
                if let Some(d) = diff {
                    let (add, del) = crate::view::diff_stat(d);
                    eprintln!("    \x1b[32m+{add}\x1b[0m \x1b[31m-{del}\x1b[0m");
                    let color = std::io::stderr().is_terminal();
                    let width = terminal_size::terminal_size().map_or(100, |(w, _)| w.0 as usize).min(140);
                    for line in crate::view::render_diff(d, width.saturating_sub(4), color) {
                        eprintln!("    {line}");
                    }
                }
            }
            Event::Turn { turn, task, secs, window } => {
                let cost = self.pricer.as_ref().and_then(|p| p.cost(turn));
                let line = crate::view::turn_line(turn, secs, window, cost, self.limits.as_deref());
                eprintln!("\x1b[2m  {line}\x1b[0m");
                if self.verbose {
                    eprintln!("  · task {}", status_line(task, None));
                }
            }
            Event::Notice(msg) => eprintln!("\n  ! {msg}"),
            Event::Gate { ok, summary } => {
                let color = if ok { "32" } else { "31" };
                let mut lines = summary.lines();
                eprintln!("\n\x1b[{color}m{}\x1b[0m", lines.next().unwrap_or(""));
                for l in lines {
                    eprintln!("\x1b[2m{l}\x1b[0m");
                }
            }
            _ => {}
        }
    }

    fn approve<'a>(&'a mut self, req: &'a ApprovalRequest) -> BoxFuture<'a, Decision> {
        let d = self.ask(req);
        Box::pin(async move { d })
    }

    fn confirm_cheat<'a>(&'a mut self, finding: &'a Finding) -> BoxFuture<'a, bool> {
        if let Some(mut s) = self.spinner.take() {
            s.stop();
        }
        eprintln!("\n\x1b[33m? suspicious edit: {}\x1b[0m", finding.render());
        let ok = self.yes_no("  is this legitimate?");
        Box::pin(async move { ok })
    }

    fn review_plan<'a>(&'a mut self, plan: &'a PlanReview) -> BoxFuture<'a, PlanChoice> {
        let c = self.review(plan);
        Box::pin(async move { c })
    }
}

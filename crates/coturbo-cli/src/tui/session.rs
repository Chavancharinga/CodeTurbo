//! The agent side of the interface: reads what the user sends, runs tasks
//! and commands, and switches provider, model and settings on the fly.

use super::app::Screen;
use super::commands::{self, COMMANDS, Command, Parsed};
use super::host::TuiHost;
use super::text::ansi_line;
use super::{Choice, In, Out, Question, Status};
use crate::config::{self, Config, Pricer};
use crate::splash::Mood;
use crate::settings::{self, Credentials, PROVIDERS};
use crate::status::{status_line, stop_note};
use crate::{models, update};
use coturbo_agent::Agent;
use coturbo_providers::{PriceTable, Provider, Usage};
use coturbo_tools::Mode;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::oneshot;

/// Approval modes offered by `/aprovacao`, with what each one does.
pub const APPROVAL_MODES: [(&str, &str); 3] = [
    ("always-ask", "Ask first: confirms every change and every command"),
    ("write", "Auto-edit: changes files without asking, confirms commands (default)"),
    ("yolo", "Auto: runs everything without asking"),
];

/// Short name of an approval mode for the status bar.
pub fn approval_label(mode: &str) -> String {
    match mode {
        "always-ask" => "ask first".into(),
        "write" => "auto-edit".into(),
        "yolo" => "auto".into(),
        other => other.into(),
    }
}

/// Everything decided before the interface opens.
pub struct Launch {
    pub cfg: Config,
    pub prices: PriceTable,
    pub dir: Option<PathBuf>,
    pub root: PathBuf,
    pub mode: Mode,
    pub plan: bool,
    pub verbose: bool,
    /// A task to start with, as if typed first.
    pub prompt: Option<String>,
    /// The update check started at launch.
    pub update: Option<tokio::task::JoinHandle<Option<String>>>,
}

struct Session {
    cfg: Config,
    prices: PriceTable,
    dir: Option<PathBuf>,
    root: PathBuf,
    mode: Mode,
    plan: bool,
    verbose: bool,
    creds: Credentials,
    /// Sent before anything the user types (the task given at launch).
    pending: Option<String>,
    out: std::sync::mpsc::Sender<Out>,
    /// The last task sent and the model's last answer, for /repetir and /copiar.
    last_task: Option<String>,
    last_answer: Option<String>,
    /// Usage of the agents replaced during the session (provider/model changes).
    spent: Usage,
    closing: Arc<AtomicBool>,
}

fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

fn usage_of(agent: &Option<Agent<'_>>) -> Usage {
    agent.as_ref().map(Agent::session_usage).unwrap_or_default()
}

/// Puts `text` on the system clipboard.
fn copy_to_clipboard(text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let (program, args, bytes): (&str, &[&str], Vec<u8>) = if cfg!(windows) {
        // clip.exe reads UTF-16 with a byte order mark.
        let mut b = vec![0xFF, 0xFE];
        b.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        ("clip", &[], b)
    } else if cfg!(target_os = "macos") {
        ("pbcopy", &[], text.as_bytes().to_vec())
    } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        ("wl-copy", &[], text.as_bytes().to_vec())
    } else {
        ("xclip", &["-selection", "clipboard"], text.as_bytes().to_vec())
    };
    let mut child = Command::new(program).args(args).stdin(Stdio::piped()).spawn().map_err(|e| format!("{program}: {e}"))?;
    child.stdin.take().ok_or("sem entrada")?.write_all(&bytes).map_err(|e| e.to_string())?;
    child.wait().map_err(|e| e.to_string())?;
    Ok(())
}

/// Opens the interface and runs it until the user quits.
pub async fn run(launch: Launch) -> Result<ExitCode, String> {
    let closing = Arc::new(AtomicBool::new(false));
    let initial = Status {
        provider: launch.cfg.provider.clone(),
        model: launch.cfg.model.clone(),
        approval: approval_label(&launch.cfg.approval_mode),
        plan: launch.plan,
        usage: status_line(&Usage::default(), None),
        context: None,
        folder: super::path::folder_text(&launch.root),
    };
    let (screen, rx, cancel) = Screen::spawn(initial, closing.clone());
    let channels = Channels { out: screen.out.clone(), rx, cancel, closing: closing.clone() };
    let result = drive(launch, channels).await;
    closing.store(true, Ordering::SeqCst);
    screen.join().map_err(|e| e.to_string())?;
    let total = result?;
    eprintln!("CodeTurbo closed · session: {}", status_line(&total, None));
    Ok(ExitCode::SUCCESS)
}

/// The two ends a session talks to the screen through.
pub struct Channels {
    pub out: std::sync::mpsc::Sender<Out>,
    pub rx: UnboundedReceiver<In>,
    pub cancel: UnboundedReceiver<()>,
    /// Set by the screen when the user quits.
    pub closing: Arc<AtomicBool>,
}

/// Runs a session against any screen until the user quits. Returns the
/// tokens the session used.
pub async fn drive(launch: Launch, ch: Channels) -> Result<Usage, String> {
    let Channels { out, mut rx, mut cancel, closing } = ch;
    let mut s = Session {
        creds: Credentials::load(launch.dir.as_deref()),
        cfg: launch.cfg,
        prices: launch.prices,
        dir: launch.dir,
        root: launch.root,
        mode: launch.mode,
        plan: launch.plan,
        verbose: launch.verbose,
        pending: launch.prompt.filter(|p| !p.trim().is_empty()),
        last_task: None,
        last_answer: None,
        out,
        spent: Usage::default(),
        closing,
    };
    if let Some(check) = launch.update {
        let out = s.out.clone();
        tokio::spawn(async move {
            if let Ok(Some(tag)) = check.await {
                let line = Line::styled(update::notice(&tag), Style::new().fg(Color::Yellow));
                let _ = out.send(Out::Lines(vec![line]));
            }
        });
    }
    s.main_loop(&mut rx, &mut cancel).await?;
    Ok(s.spent)
}

impl Session {
    fn send(&self, m: Out) {
        let _ = self.out.send(m);
    }

    fn info(&self, text: impl Into<String>) {
        self.send(Out::Lines(vec![Line::styled(format!("  {}", text.into()), dim())]));
    }

    fn warn(&self, text: impl Into<String>) {
        self.send(Out::Lines(vec![Line::styled(format!("  ! {}", text.into()), Style::new().fg(Color::Yellow))]));
    }

    fn pricer(&self) -> Pricer {
        Pricer { table: self.prices.clone(), model: self.cfg.model.clone(), provider: self.cfg.provider.clone() }
    }

    fn status(&self, agent_usage: Usage) {
        let mut total = self.spent;
        total += agent_usage;
        self.send(Out::Status(Status {
            provider: self.cfg.provider.clone(),
            model: self.cfg.model.clone(),
            approval: approval_label(&self.cfg.approval_mode),
            plan: self.plan,
            usage: status_line(&total, self.pricer().cost(&total)),
            context: None,
            folder: super::path::folder_text(&self.root),
        }));
    }

    async fn ask<T>(&self, make: impl FnOnce(oneshot::Sender<T>) -> Question, otherwise: T) -> T {
        let (tx, rx) = oneshot::channel();
        self.send(Out::Ask(make(tx)));
        rx.await.unwrap_or(otherwise)
    }

    async fn select(&self, title: String, options: Vec<Choice>, custom: bool) -> Option<String> {
        self.ask(|reply| Question::Select { title, options, custom, reply }, None).await
    }

    fn save(&self, values: &[(&str, Option<&str>)]) {
        if let Some(dir) = &self.dir
            && let Err(e) = settings::save_config(dir, values)
        {
            self.warn(format!("could not save the preference: {e}"));
        }
    }

    fn provider(&self) -> Result<Box<dyn Provider>, String> {
        config::make_provider(&self.cfg, settings::api_key(&self.cfg.provider, &self.creds))
    }

    fn welcome(&self) {
        let title = Style::new().fg(Color::Rgb(225, 232, 245)).add_modifier(Modifier::BOLD);
        self.send(Out::Lines(vec![
            Line::styled(format!("CodeTurbo {}", update::VERSION), title),
            Line::styled(format!("Project: {}", self.root.display()), dim()),
            Line::styled("Describe what to build or change. Type /help for the commands.", dim()),
        ]));
    }

    /// Asks once whether the project's `.codeturbo.toml` may add commands
    /// to the verification.
    async fn trust_project(&mut self) {
        let project = match config::load_project(&self.root) {
            Ok(Some(p)) => p,
            Ok(None) => return,
            Err(e) => return self.warn(e),
        };
        let commands = project.commands();
        if !commands.is_empty() && self.cfg.approval_mode != "yolo" {
            let mut lines = vec![
                Line::default(),
                Line::styled("This project's .codeturbo.toml wants the verification gate to run:", Style::new().fg(Color::Yellow)),
            ];
            lines.extend(commands.iter().map(|c| Line::raw(format!("    {c}"))));
            self.send(Out::Lines(lines));
            let title = "Allow these commands in this session?".to_string();
            if !self.ask(|reply| Question::Confirm { title, reply }, false).await {
                self.warn("comandos do .codeturbo.toml ignorados");
                return;
            }
        }
        self.cfg.verify = project.verify.or(self.cfg.verify.take());
        self.cfg.run = project.run.or(self.cfg.run.take());
    }

    /// Asks before working in a folder for the first time. `false`: the
    /// user did not allow it.
    async fn trust_folder(&mut self) -> bool {
        let mut trust = settings::Trust::load(self.dir.as_deref());
        if trust.covers(&self.root) {
            if settings::known_project(&self.root) {
                self.info("CodeTurbo project recognised in this folder.");
            }
            return true;
        }
        let mut lines = vec![
            Line::default(),
            Line::styled(format!("First time in this folder: {}", super::path::folder_text(&self.root)), Style::new().fg(Color::Yellow)),
            Line::raw("  CodeTurbo will read and change files here and may run commands, depending on the approval mode."),
            Line::raw("  Only trust folders whose contents you know."),
        ];
        if settings::too_broad(&self.root) {
            lines.push(Line::styled(
                "  Warning: this is your home folder or the root of a drive. Prefer opening CodeTurbo inside the project folder.",
                Style::new().fg(Color::Red),
            ));
        }
        self.send(Out::Lines(lines));
        // The folder goes in the question itself, so it is the last thing read before answering.
        let title = format!("Trust this folder? {}", super::path::short_path(&super::path::folder_text(&self.root), 60));
        if !self.ask(|reply| Question::Confirm { title, reply }, false).await {
            self.warn("folder not trusted: nothing will be read or changed. Open CodeTurbo in your project folder.");
            return false;
        }
        if let Err(e) = trust.add(self.dir.as_deref(), &self.root) {
            self.warn(format!("the trust only lasts for this session: {e}"));
        }
        self.info("Folder trusted. It is recognised automatically next time.");
        true
    }

    async fn main_loop(&mut self, rx: &mut UnboundedReceiver<In>, cancel: &mut UnboundedReceiver<()>) -> Result<(), String> {
        self.welcome();
        if !self.trust_folder().await {
            // Wait for the user to quit; nothing runs here.
            while let Some(In::Submit(_)) = rx.recv().await {
                self.warn("this folder is not trusted; quit with /exit and open CodeTurbo in the project folder");
            }
            return Ok(());
        }
        self.trust_project().await;
        // The pages the agent uses in `browse` are shown in the mini browser as it goes.
        let screen = self.out.clone();
        let tools = config::make_tools(&self.root, self.mode, &self.cfg)
            .with_browse_window_when(config::browser_window_enabled)
            .with_browse_sink(Arc::new(move |frame| {
                let _ = screen.send(Out::Browser(frame));
            }));
        let mut first = true;
        'session: loop {
            let provider = match self.provider() {
                Ok(p) => {
                    p.warm(&self.cfg.model, self.cfg.effort.as_deref());
                    Some(p)
                }
                Err(e) => {
                    self.warn(e);
                    None
                }
            };
            let mut agent = match &provider {
                Some(p) => Some(Agent::new(p.as_ref(), &tools, config::agent_config(&self.cfg)?)),
                None => None,
            };
            if let Some(a) = agent.as_mut() {
                a.set_plan_mode(self.plan);
            }
            self.status(usage_of(&agent));
            if std::mem::take(&mut first) && agent.is_none() {
                // First run without a usable provider: go straight to choosing one.
                self.info("Choose a provider to start. Ollama runs local models without an API key.");
                if self.choose_provider(false).await {
                    continue 'session;
                }
            }
            let restart = loop {
                let text = match self.pending.take() {
                    Some(task) => {
                        self.send(Out::Lines(super::app::task_lines(&task)));
                        task
                    }
                    None => match rx.recv().await {
                        Some(In::Submit(text)) => text,
                        _ => break false,
                    },
                };
                if self.closing.load(Ordering::SeqCst) {
                    break false;
                }
                let restart = match commands::parse(&text) {
                    Parsed::Task => {
                        self.task(agent.as_mut(), &text, cancel).await;
                        false
                    }
                    Parsed::Unknown(w) => {
                        self.warn(format!("unknown command: {w}. Type /help for the commands."));
                        false
                    }
                    Parsed::Command(cmd, arg) => match cmd {
                        Command::Provider => self.choose_provider(agent.is_some()).await,
                        Command::Model => self.choose_model(&arg).await,
                        Command::Key => self.set_key().await,
                        Command::Approval => self.choose_approval(&arg).await,
                        Command::Effort => self.choose_effort(&arg).await,
                        Command::Plan => {
                            self.plan = !self.plan;
                            if let Some(a) = agent.as_mut() {
                                a.set_plan_mode(self.plan);
                            }
                            self.info(if self.plan {
                                "Plan mode on: read-only until you approve a plan."
                            } else {
                                "Plan mode off."
                            });
                            self.status(usage_of(&agent));
                            false
                        }
                        Command::Usage => {
                            let mut total = self.spent;
                            total += usage_of(&agent);
                            self.info(format!("Session: {}", status_line(&total, self.pricer().cost(&total))));
                            false
                        }
                        Command::Retry => {
                            match self.last_task.clone() {
                                Some(task) => {
                                    self.send(Out::Lines(super::app::task_lines(&task)));
                                    self.task(agent.as_mut(), &task, cancel).await;
                                }
                                None => self.warn("no task to repeat"),
                            }
                            false
                        }
                        Command::Copy => {
                            match &self.last_answer {
                                Some(text) => match copy_to_clipboard(text) {
                                    Ok(()) => self.info("Last answer copied."),
                                    Err(e) => self.warn(format!("could not copy: {e}")),
                                },
                                None => self.warn("no answer to copy yet"),
                            }
                            false
                        }
                        Command::Export => {
                            let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                            self.send(Out::Export(self.root.join(".codeturbo/sessoes").join(format!("conversa-{secs}.md"))));
                            false
                        }
                        Command::Shake => {
                            match agent.as_mut().map(|a| a.shake(2)) {
                                Some(0) | None => self.info("Nada para enxugar."),
                                Some(n) => self.info(format!("{n} old result(s) shrunk: fewer tokens on the next message.")),
                            }
                            false
                        }
                        Command::Rule => {
                            if arg.trim().is_empty() {
                                self.warn("use /rule followed by what the model must always do, e.g. /rule always run the tests before finishing");
                            } else {
                                match coturbo_agent::add_rule(&self.root, &arg) {
                                    Ok(()) => {
                                        if let Some(a) = agent.as_mut() {
                                            a.reload_rules(&self.root);
                                        }
                                        self.info(format!("Rule pinned (applies from the next message): {}", arg.trim()));
                                    }
                                    Err(e) => self.warn(e),
                                }
                            }
                            false
                        }
                        Command::Undo => {
                            match agent.as_mut().map(Agent::undo) {
                                Some(Ok(files)) => {
                                    self.info(format!("Changes undone in: {}", files.join(", ")));
                                }
                                Some(Err(e)) => self.warn(e),
                                None => self.warn("no provider ready"),
                            }
                            false
                        }
                        Command::Clear => {
                            self.send(Out::Clear);
                            self.info("Nova conversa iniciada.");
                            true
                        }
                        Command::Update => {
                            self.update().await;
                            false
                        }
                        Command::Help => {
                            self.help();
                            false
                        }
                        Command::Exit => break false,
                    },
                };
                if restart {
                    break true;
                }
            };
            self.spent += usage_of(&agent);
            if !restart {
                break 'session;
            }
        }
        // The browser `browse` kept open goes away with the session.
        tools.shutdown().await;
        Ok(())
    }

    async fn task(&mut self, agent: Option<&mut Agent<'_>>, text: &str, cancel: &mut UnboundedReceiver<()>) {
        let Some(agent) = agent else {
            self.warn("no provider ready. Use /provider to choose one or /key to set the API key.");
            return;
        };
        self.last_task = Some(text.to_string());
        while cancel.try_recv().is_ok() {}
        let mut base = self.spent;
        base += agent.session_usage();
        let mut host = TuiHost::new(self.out.clone(), self.pricer(), self.root.clone(), self.verbose, base);
        self.send(Out::Busy(Some("think".into())));
        let started = std::time::Instant::now();
        let result = tokio::select! {
            r = agent.run_task(text, &mut host) => Some(r),
            _ = cancel.recv() => None,
        };
        drop(host);
        self.send(Out::Busy(None));
        match result {
            Some(Ok(report)) => {
                let cost = self.pricer().cost(&report.usage);
                self.send(Out::Lines(vec![
                    Line::default(),
                    ansi_line(&format!(
                        "{} · {} · task {}",
                        stop_note(&report),
                        crate::status::elapsed(started.elapsed()),
                        status_line(&report.usage, cost)
                    )),
                ]));
                if !report.final_text.trim().is_empty() {
                    self.last_answer = Some(report.final_text.clone());
                }
                if report.verified == Some(false) {
                    self.send(Out::Ended(Mood::Denied));
                } else if !matches!(report.stop, coturbo_agent::Stop::Done | coturbo_agent::Stop::PlanSaved) {
                    self.send(Out::Ended(Mood::Alert));
                }
                agent.compact();
            }
            Some(Err(e)) => {
                self.send(Out::Ended(Mood::Alert));
                self.warn(format!("error: {e}"));
            }
            None => {
                agent.interrupted();
                self.send(Out::CloseQuestion);
                self.send(Out::Ended(Mood::Alert));
                self.warn("task interrupted");
            }
        }
        self.status(agent.session_usage());
    }

    /// Picks a provider. `ready`: the current one is actually working.
    async fn choose_provider(&mut self, ready: bool) -> bool {
        // Only what this computer has: installed plans, local servers, keys already given.
        let here: Vec<&settings::ProviderInfo> =
            PROVIDERS.iter().filter(|p| settings::detected(p.id, &self.creds) || (ready && p.id == self.cfg.provider)).collect();
        let mut options: Vec<Choice> = here
            .iter()
            .map(|p| {
                let note = if p.id == self.cfg.provider && ready {
                    format!("{} · in use ({})", settings::standing(p.id, &self.creds), self.cfg.model)
                } else {
                    settings::standing(p.id, &self.creds)
                };
                Choice::new(p.id, format!("{}{note}", p.label))
            })
            .collect();
        options.push(Choice::new("__outro", "Connect another provider with an API key…"));
        let Some(mut id) = self.select("Sign in to your providers · pick one to use".into(), options, false).await else { return false };
        if id == "__outro" {
            let rest: Vec<Choice> = PROVIDERS
                .iter()
                .filter(|p| !here.iter().any(|h| h.id == p.id) && (p.key_var.is_some() || p.base_url.is_some() || p.id == "ollama"))
                .map(|p| Choice::new(p.id, p.label))
                .collect();
            let Some(other) = self.select("Connect a provider".into(), rest, false).await else { return false };
            id = other;
        }
        let Some(info) = settings::provider(&id) else { return false };
        if info.key_var.is_some() && settings::api_key(&id, &self.creds).is_none() && !self.ask_key(&id).await {
            return false;
        }
        let Some(model) = self.pick_model(&id, "").await else { return false };
        let changed = id != self.cfg.provider;
        self.cfg.provider.clone_from(&id);
        self.cfg.model.clone_from(&model);
        if changed {
            // An endpoint set for the old provider would point the new one at the wrong place.
            self.cfg.base_url = None;
            self.save(&[("provider", Some(&id)), ("model", Some(&model)), ("base_url", None)]);
        } else {
            self.save(&[("provider", Some(&id)), ("model", Some(&model))]);
        }
        self.info(format!("Provider: {} · model: {model}", info.label));
        true
    }

    async fn choose_model(&mut self, typed: &str) -> bool {
        let provider = self.cfg.provider.clone();
        let Some(model) = self.pick_model(&provider, typed).await else { return false };
        self.cfg.model.clone_from(&model);
        self.save(&[("model", Some(&model))]);
        self.info(format!("Model: {model}"));
        true
    }

    /// The model to use with `provider`: `typed` when given, otherwise
    /// picked from the provider's list.
    async fn pick_model(&mut self, provider: &str, typed: &str) -> Option<String> {
        if !typed.is_empty() {
            return Some(typed.to_string());
        }
        self.send(Out::Busy(Some("listing the available models".into())));
        let key = settings::api_key(provider, &self.creds);
        let base = if provider == self.cfg.provider { self.cfg.base_url.clone() } else { None };
        let listed = models::list(provider, key.as_deref(), base.as_deref()).await;
        self.send(Out::Busy(None));
        let mut named = listed.unwrap_or_else(|e| {
            self.warn(format!("could not list the models: {e}"));
            Vec::new()
        });
        if provider == "anthropic" && named.is_empty() {
            named = models::ANTHROPIC_FALLBACK.map(|m| (m.to_string(), m.to_string())).to_vec();
        }
        let labels: std::collections::HashMap<String, String> = named.iter().cloned().collect();
        let mut ids: Vec<String> = named.into_iter().map(|(id, _)| id).collect();
        if provider == "ollama" && ids.is_empty() {
            self.info("No local model found. To install one: ollama pull qwen2.5:7b");
        }
        // The current model first, so Enter keeps it.
        let current = (provider == self.cfg.provider).then(|| self.cfg.model.clone());
        if let Some(c) = &current
            && let Some(i) = ids.iter().position(|id| id == c)
        {
            let id = ids.remove(i);
            ids.insert(0, id);
        }
        let count = ids.len();
        let options = ids
            .into_iter()
            .map(|id| {
                let base = labels.get(&id).cloned().unwrap_or_else(|| id.clone());
                let label = if Some(&id) == current.as_ref() { format!("{base} · in use") } else { base };
                Choice::new(id, label)
            })
            .collect();
        let label = settings::provider(provider).map_or(provider, |p| p.label);
        self.select(format!("Model · {label} · {count} available · type to search"), options, true).await
    }

    async fn set_key(&mut self) -> bool {
        let id = self.cfg.provider.clone();
        match settings::provider(&id) {
            Some(p) if p.key_var.is_none() => {
                self.info(format!("The provider {} does not use an API key.", p.label));
                false
            }
            _ => self.ask_key(&id).await,
        }
    }

    async fn ask_key(&mut self, id: &str) -> bool {
        let label = settings::provider(id).map_or(id, |p| p.label);
        let title = format!("API key · {label}");
        let Some(key) = self.ask(|reply| Question::Secret { title, reply }, None).await else { return false };
        self.creds.set(id, &key);
        match &self.dir {
            Some(dir) => match self.creds.save(dir) {
                Ok(()) => self.info(format!("Key saved for {label}.")),
                Err(e) => self.warn(format!("the key only lasts for this session: {e}")),
            },
            None => self.info(format!("Key set for {label} for this session.")),
        }
        true
    }

    async fn choose_approval(&mut self, typed: &str) -> bool {
        let mode = if typed.is_empty() {
            let options = APPROVAL_MODES
                .iter()
                .map(|(id, label)| {
                    let current = if *id == self.cfg.approval_mode { " (current)" } else { "" };
                    Choice::new(*id, format!("{label}{current}"))
                })
                .collect();
            let Some(m) = self.select("When to ask for confirmation?".into(), options, false).await else { return false };
            m
        } else {
            typed.to_string()
        };
        let mut next = self.cfg.clone();
        next.approval_mode.clone_from(&mode);
        if let Err(e) = next.approval_config() {
            self.warn(e);
            return false;
        }
        self.cfg = next;
        self.save(&[("approval_mode", Some(&mode))]);
        self.info(format!("Approval: {}", approval_label(&mode)));
        true
    }

    /// How hard the model thinks, for the providers and models that take it.
    async fn choose_effort(&mut self, typed: &str) -> bool {
        const LEVELS: [(&str, &str); 6] = [
            ("default", "Default: what the model does on its own"),
            ("low", "Low: quick answers, fewer tokens"),
            ("medium", "Medium"),
            ("high", "High: thinks longer before acting"),
            ("xhigh", "Extra high"),
            ("max", "Max: the deepest thinking the model offers"),
        ];
        let level = if typed.is_empty() {
            let now = self.cfg.effort.clone().unwrap_or_else(|| "default".into());
            let options = LEVELS
                .iter()
                .map(|(id, label)| Choice::new(*id, format!("{label}{}", if *id == now { " (current)" } else { "" })))
                .collect();
            let Some(l) = self.select("Reasoning effort".into(), options, false).await else { return false };
            l
        } else {
            typed.trim().to_lowercase()
        };
        if !LEVELS.iter().any(|(id, _)| *id == level) {
            self.warn(format!("unknown effort `{level}`: use default, low, medium, high, xhigh or max"));
            return false;
        }
        let value = (level != "default").then_some(level.as_str());
        self.cfg.effort = value.map(str::to_string);
        self.save(&[("effort", value)]);
        let takes = matches!(self.cfg.provider.as_str(), "anthropic" | "claude-code" | "codex" | "antigravity" | "openrouter" | "openai");
        let note = if takes { "" } else { " (this provider does not take an effort setting; it applies when you switch to one that does)" };
        self.info(format!("Effort: {level}{note}"));
        true
    }

    async fn update(&self) {
        self.send(Out::Busy(Some("looking for a new version".into())));
        let result = update::self_update().await;
        self.send(Out::Busy(None));
        match result {
            Ok(msg) => {
                self.info(&msg);
                if msg.starts_with("CodeTurbo updated") {
                    self.info("Close and reopen CodeTurbo to use the new version.");
                }
            }
            Err(e) => self.warn(format!("could not update: {e}")),
        }
    }

    fn help(&self) {
        let mut lines = vec![Line::default(), Line::styled("Commands", Style::new().add_modifier(Modifier::BOLD))];
        lines.extend(COMMANDS.iter().map(|c| Line::raw(format!("  {:<12} {}", c.name, c.about))));
        lines.push(Line::default());
        lines.push(Line::styled("Shortcuts", Style::new().add_modifier(Modifier::BOLD)));
        for (keys, what) in [
            ("Enter", "sends the message"),
            ("Shift+Enter", "new line"),
            ("↑ ↓", "scroll the conversation"),
            ("PageUp PageDown", "scroll a page"),
            ("Ctrl+↑ Ctrl+↓", "previous messages"),
            ("Tab", "completes the command"),
            ("Esc", "interrupts the running task"),
            ("Ctrl+C", "interrupts; quits when the box is empty"),
        ] {
            lines.push(Line::raw(format!("  {keys:<16} {what}")));
        }
        self.send(Out::Lines(lines));
    }
}

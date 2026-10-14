//! The agent side of the interface: reads what the user sends, runs tasks
//! and commands, and switches provider, model and settings on the fly.

use super::app::Screen;
use super::commands::{self, COMMANDS, Command, Parsed};
use super::host::TuiHost;
use super::text::ansi_line;
use super::{Choice, In, Out, Question, Status};
use crate::config::{self, Config, Pricer};
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
    ("always-ask", "Sempre perguntar: confirma cada alteração e cada comando"),
    ("write", "Edição livre: altera arquivos sem perguntar, confirma comandos (padrão)"),
    ("yolo", "Automático: executa tudo sem perguntar"),
];

/// Short name of an approval mode for the status bar.
pub fn approval_label(mode: &str) -> String {
    match mode {
        "always-ask" => "sempre perguntar".into(),
        "write" => "edição livre".into(),
        "yolo" => "automático".into(),
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
    };
    let (screen, rx, cancel) = Screen::spawn(initial, closing.clone());
    let channels = Channels { out: screen.out.clone(), rx, cancel, closing: closing.clone() };
    let result = drive(launch, channels).await;
    closing.store(true, Ordering::SeqCst);
    screen.join().map_err(|e| e.to_string())?;
    let total = result?;
    eprintln!("CodeTurbo encerrado · sessão: {}", status_line(&total, None));
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
            self.warn(format!("não foi possível salvar a preferência: {e}"));
        }
    }

    fn provider(&self) -> Result<Box<dyn Provider>, String> {
        config::make_provider(&self.cfg, settings::api_key(&self.cfg.provider, &self.creds))
    }

    fn welcome(&self) {
        let title = Style::new().fg(Color::Rgb(225, 232, 245)).add_modifier(Modifier::BOLD);
        self.send(Out::Lines(vec![
            Line::styled(format!("CodeTurbo {}", update::VERSION), title),
            Line::styled(format!("Projeto: {}", self.root.display()), dim()),
            Line::styled("Descreva o que deseja criar ou alterar. Digite /ajuda para ver os comandos.", dim()),
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
                Line::styled("O arquivo .codeturbo.toml deste projeto quer rodar na verificação:", Style::new().fg(Color::Yellow)),
            ];
            lines.extend(commands.iter().map(|c| Line::raw(format!("    {c}"))));
            self.send(Out::Lines(lines));
            let title = "Permitir estes comandos nesta sessão?".to_string();
            if !self.ask(|reply| Question::Confirm { title, reply }, false).await {
                self.warn("comandos do .codeturbo.toml ignorados");
                return;
            }
        }
        self.cfg.verify = project.verify.or(self.cfg.verify.take());
        self.cfg.run = project.run.or(self.cfg.run.take());
    }

    async fn main_loop(&mut self, rx: &mut UnboundedReceiver<In>, cancel: &mut UnboundedReceiver<()>) -> Result<(), String> {
        self.welcome();
        self.trust_project().await;
        let tools = config::make_tools(&self.root, self.mode, &self.cfg);
        let mut first = true;
        'session: loop {
            let provider = match self.provider() {
                Ok(p) => Some(p),
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
                self.info("Escolha um provedor para começar. O Ollama roda modelos locais sem chave de API.");
                if self.choose_provider().await {
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
                        self.warn(format!("comando desconhecido: {w}. Digite /ajuda para ver os comandos."));
                        false
                    }
                    Parsed::Command(cmd, arg) => match cmd {
                        Command::Provider => self.choose_provider().await,
                        Command::Model => self.choose_model(&arg).await,
                        Command::Key => self.set_key().await,
                        Command::Approval => self.choose_approval(&arg).await,
                        Command::Plan => {
                            self.plan = !self.plan;
                            if let Some(a) = agent.as_mut() {
                                a.set_plan_mode(self.plan);
                            }
                            self.info(if self.plan {
                                "Modo plano ativado: somente leitura até você aprovar um plano."
                            } else {
                                "Modo plano desativado."
                            });
                            self.status(usage_of(&agent));
                            false
                        }
                        Command::Usage => {
                            let mut total = self.spent;
                            total += usage_of(&agent);
                            self.info(format!("Sessão: {}", status_line(&total, self.pricer().cost(&total))));
                            false
                        }
                        Command::Undo => {
                            match agent.as_mut().map(Agent::undo) {
                                Some(Ok(files)) => {
                                    self.info(format!("Alterações desfeitas em: {}", files.join(", ")));
                                }
                                Some(Err(e)) => self.warn(e),
                                None => self.warn("nenhum provedor pronto"),
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
        Ok(())
    }

    async fn task(&mut self, agent: Option<&mut Agent<'_>>, text: &str, cancel: &mut UnboundedReceiver<()>) {
        let Some(agent) = agent else {
            self.warn("nenhum provedor pronto. Use /provedor para escolher um ou /chave para informar a chave de API.");
            return;
        };
        while cancel.try_recv().is_ok() {}
        let mut base = self.spent;
        base += agent.session_usage();
        let mut host = TuiHost::new(self.out.clone(), self.pricer(), self.root.clone(), self.verbose, base);
        self.send(Out::Busy(Some("pensando".into())));
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
                        "{} · {} · tarefa {}",
                        stop_note(&report),
                        crate::status::elapsed(started.elapsed()),
                        status_line(&report.usage, cost)
                    )),
                ]));
                agent.compact();
            }
            Some(Err(e)) => self.warn(format!("erro: {e}")),
            None => {
                agent.interrupted();
                self.send(Out::CloseQuestion);
                self.warn("tarefa interrompida");
            }
        }
        self.status(agent.session_usage());
    }

    async fn choose_provider(&mut self) -> bool {
        let options = PROVIDERS
            .iter()
            .map(|p| {
                let current = if p.id == self.cfg.provider { " (atual)" } else { "" };
                Choice::new(p.id, format!("{}{current}", p.label))
            })
            .collect();
        let Some(id) = self.select("Escolha o provedor".into(), options, false).await else { return false };
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
        self.info(format!("Provedor: {} · modelo: {model}", info.label));
        true
    }

    async fn choose_model(&mut self, typed: &str) -> bool {
        let provider = self.cfg.provider.clone();
        let Some(model) = self.pick_model(&provider, typed).await else { return false };
        self.cfg.model.clone_from(&model);
        self.save(&[("model", Some(&model))]);
        self.info(format!("Modelo: {model}"));
        true
    }

    /// The model to use with `provider`: `typed` when given, otherwise
    /// picked from the provider's list.
    async fn pick_model(&mut self, provider: &str, typed: &str) -> Option<String> {
        if !typed.is_empty() {
            return Some(typed.to_string());
        }
        self.send(Out::Busy(Some("buscando os modelos disponíveis".into())));
        let key = settings::api_key(provider, &self.creds);
        let base = if provider == self.cfg.provider { self.cfg.base_url.clone() } else { None };
        let listed = models::list(provider, key.as_deref(), base.as_deref()).await;
        self.send(Out::Busy(None));
        let mut ids = listed.unwrap_or_else(|e| {
            self.warn(format!("não foi possível listar os modelos: {e}"));
            Vec::new()
        });
        if provider == "anthropic" && ids.is_empty() {
            ids = models::ANTHROPIC_FALLBACK.map(String::from).to_vec();
        }
        if provider == "ollama" && ids.is_empty() {
            self.info("Nenhum modelo local encontrado. Para instalar um: ollama pull qwen2.5:7b");
        }
        // The current model first, so Enter keeps it.
        let current = (provider == self.cfg.provider).then(|| self.cfg.model.clone());
        if let Some(c) = &current
            && let Some(i) = ids.iter().position(|id| id == c)
        {
            let id = ids.remove(i);
            ids.insert(0, id);
        }
        let options = ids
            .into_iter()
            .map(|id| {
                let label = if Some(&id) == current.as_ref() { format!("{id} (atual)") } else { id.clone() };
                Choice::new(id, label)
            })
            .collect();
        let label = settings::provider(provider).map_or(provider, |p| p.label);
        self.select(format!("Escolha o modelo · {label}"), options, true).await
    }

    async fn set_key(&mut self) -> bool {
        let id = self.cfg.provider.clone();
        match settings::provider(&id) {
            Some(p) if p.key_var.is_none() => {
                self.info(format!("O provedor {} não usa chave de API.", p.label));
                false
            }
            _ => self.ask_key(&id).await,
        }
    }

    async fn ask_key(&mut self, id: &str) -> bool {
        let label = settings::provider(id).map_or(id, |p| p.label);
        let title = format!("Chave de API · {label}");
        let Some(key) = self.ask(|reply| Question::Secret { title, reply }, None).await else { return false };
        self.creds.set(id, &key);
        match &self.dir {
            Some(dir) => match self.creds.save(dir) {
                Ok(()) => self.info(format!("Chave salva para {label}.")),
                Err(e) => self.warn(format!("a chave vale só nesta sessão: {e}")),
            },
            None => self.info(format!("Chave definida para {label} nesta sessão.")),
        }
        true
    }

    async fn choose_approval(&mut self, typed: &str) -> bool {
        let mode = if typed.is_empty() {
            let options = APPROVAL_MODES
                .iter()
                .map(|(id, label)| {
                    let current = if *id == self.cfg.approval_mode { " (atual)" } else { "" };
                    Choice::new(*id, format!("{label}{current}"))
                })
                .collect();
            let Some(m) = self.select("Quando pedir confirmação?".into(), options, false).await else { return false };
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
        self.info(format!("Aprovação: {}", approval_label(&mode)));
        true
    }

    async fn update(&self) {
        self.send(Out::Busy(Some("procurando uma nova versão".into())));
        let result = update::self_update().await;
        self.send(Out::Busy(None));
        match result {
            Ok(msg) => {
                self.info(&msg);
                if msg.starts_with("CodeTurbo atualizado") {
                    self.info("Feche e abra o CodeTurbo para usar a nova versão.");
                }
            }
            Err(e) => self.warn(format!("não foi possível atualizar: {e}")),
        }
    }

    fn help(&self) {
        let mut lines = vec![Line::default(), Line::styled("Comandos", Style::new().add_modifier(Modifier::BOLD))];
        lines.extend(COMMANDS.iter().map(|c| Line::raw(format!("  {:<12} {}", c.name, c.about))));
        lines.push(Line::default());
        lines.push(Line::styled("Atalhos", Style::new().add_modifier(Modifier::BOLD)));
        for (keys, what) in [
            ("Enter", "envia a mensagem"),
            ("Shift+Enter", "quebra a linha"),
            ("↑ ↓", "rolam a conversa"),
            ("PageUp PageDown", "rolam uma página"),
            ("Ctrl+↑ Ctrl+↓", "mensagens anteriores"),
            ("Tab", "completa o comando"),
            ("Esc", "interrompe a tarefa em andamento"),
            ("Ctrl+C", "interrompe; com o campo vazio, encerra"),
        ] {
            lines.push(Line::raw(format!("  {keys:<16} {what}")));
        }
        self.send(Out::Lines(lines));
    }
}

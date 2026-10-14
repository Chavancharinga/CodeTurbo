use clap::{Args, Parser, Subcommand};
use coturbo_agent::{Agent, AgentConfig, Stop, approval::ApprovalConfig};
use coturbo_cli::config::{self, Config, Pricer, agent_config, make_tools, mode_name, parse_mode};
use coturbo_cli::status::{TermHost, status_line, stop_note};
use coturbo_cli::tui::session::{self, Launch};
use coturbo_cli::{bench, settings, splash, update};
use coturbo_tools::Mode;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "codeturbo", version, about = "Agente de código que entrega código verificado gastando menos tokens. Sem subcomando, abre a interface.", disable_help_subcommand = true)]
struct Cli {
    /// Tarefa enviada assim que a interface abre.
    prompt: Option<String>,
    #[command(flatten)]
    opts: Opts,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

/// Substituem os valores do config.toml.
#[derive(Args)]
struct Opts {
    /// anthropic | openai | openrouter | ollama
    #[arg(long, global = true)]
    provider: Option<String>,
    #[arg(long, global = true)]
    model: Option<String>,
    /// naive | ast
    #[arg(long, global = true)]
    tools: Option<String>,
    #[arg(long, global = true)]
    max_turns: Option<u32>,
    /// low | medium | high | xhigh | max
    #[arg(long, global = true)]
    effort: Option<String>,
    #[arg(long, global = true)]
    base_url: Option<String>,
    /// Pasta do projeto onde as ferramentas atuam (padrão: a pasta atual).
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    /// always-ask | write | yolo
    #[arg(long, global = true)]
    approval_mode: Option<String>,
    /// Começa no modo plano: somente leitura até você aprovar um plano.
    #[arg(long, global = true)]
    plan: bool,
    /// Modelo usado no planejamento (por exemplo, um mais forte que o --model).
    #[arg(long, global = true)]
    plan_model: Option<String>,
    /// Aprova todas as ações (o mesmo que --approval-mode yolo).
    #[arg(long, global = true)]
    auto_approve: bool,
    /// Mostra a contagem de tokens após cada rodada.
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Executa uma tarefa e sai.
    Run { task: String },
    /// Mostra a animação de abertura do CodeTurbo.
    Splash,
    /// Atualiza o CodeTurbo para a versão mais recente.
    Update,
    /// Comparação A/B dos conjuntos de ferramentas sobre um projeto de teste.
    Bench {
        #[arg(long, default_value = "bench/tasks")]
        tasks: PathBuf,
        #[arg(long, default_value = "bench/fixture")]
        fixture: PathBuf,
        /// Conjuntos de ferramentas separados por vírgula.
        #[arg(long, default_value = "naive,ast")]
        modes: String,
        #[arg(long, default_value = "bench/runs")]
        out: PathBuf,
        /// Apenas as tarefas cujo id contém este texto.
        #[arg(long)]
        only: Option<String>,
    },
}

fn apply(mut c: Config, o: &Opts) -> Config {
    if let Some(v) = &o.provider {
        c.provider = v.clone();
    }
    if let Some(v) = &o.model {
        c.model = v.clone();
    }
    if let Some(v) = &o.tools {
        c.tools = v.clone();
    }
    if let Some(v) = o.max_turns {
        c.max_turns = v;
    }
    if o.effort.is_some() {
        c.effort = o.effort.clone();
    }
    if o.base_url.is_some() {
        c.base_url = o.base_url.clone();
    }
    if let Some(v) = &o.approval_mode {
        c.approval_mode = v.clone();
    }
    if o.auto_approve {
        c.approval_mode = "yolo".into();
    }
    if o.plan_model.is_some() {
        c.plan_model = o.plan_model.clone();
    }
    c
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match real_main(cli).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("erro: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The update check's result, waiting at most a moment for it: it runs
/// while the intro plays, so it is usually ready.
async fn update_notice(check: Option<tokio::task::JoinHandle<Option<String>>>) -> Option<String> {
    let tag = tokio::time::timeout(std::time::Duration::from_millis(1500), check?).await.ok()?.ok()??;
    Some(update::notice(&tag))
}

async fn real_main(cli: Cli) -> Result<ExitCode, String> {
    update::cleanup_old_binary();
    if matches!(cli.cmd, Some(Cmd::Update)) {
        eprintln!("{}", update::self_update().await?);
        return Ok(ExitCode::SUCCESS);
    }
    if matches!(cli.cmd, Some(Cmd::Splash)) {
        // Runs until Enter.
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            let _ = std::io::stdin().read_line(&mut String::new());
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        let term = std::io::stderr().is_terminal() && std::io::stdin().is_terminal();
        if !term {
            stop.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        splash::play(&mut std::io::stderr(), term, &stop);
        return Ok(ExitCode::SUCCESS);
    }
    let dir = config::config_dir();
    let (cfg, prices) = config::load(dir.as_deref())?;
    let cfg = apply(cfg, &cli.opts);
    let mode = parse_mode(&cfg.tools)?;
    cfg.approval_config()?; // config errors before credential errors
    let wants_check = cfg.update_check
        && std::env::var_os("CODETURBO_NO_UPDATE_CHECK").is_none()
        && matches!(cli.cmd, None | Some(Cmd::Run { .. }));
    let mut check = wants_check.then(|| tokio::spawn(update::check_now(dir.clone())));
    let root = match &cli.opts.root {
        Some(r) => r.clone(),
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    if cli.cmd.is_none() {
        if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
            return Err("a interface precisa de um terminal; para uma tarefa sem interface use: codeturbo run \"...\"".into());
        }
        let (plan, verbose, prompt) = (cli.opts.plan, cli.opts.verbose, cli.prompt);
        return session::run(Launch { cfg, prices, dir, root, mode, plan, verbose, prompt, update: check }).await;
    }
    let creds = settings::Credentials::load(dir.as_deref());
    let provider = config::make_provider(&cfg, settings::api_key(&cfg.provider, &creds))?;
    let pricer = Pricer { table: prices, model: cfg.model.clone(), provider: cfg.provider.clone() };
    let mut host = TermHost {
        verbose: cli.opts.verbose,
        pricer: Some(pricer.clone()),
        streaming: Default::default(),
        limits: None,
        spinner: None,
        root: root.clone(),
    };
    let mut cfg = cfg;
    if let Some(project) = config::load_project(&root)? {
        let trusted = cfg.approval_mode == "yolo" || host.trust_project(&project.commands());
        if trusted {
            cfg.verify = project.verify.or(cfg.verify);
            cfg.run = project.run.or(cfg.run);
        } else {
            eprintln!("  ! comandos do .codeturbo.toml ignorados (não autorizados)");
        }
    }

    match cli.cmd {
        Some(Cmd::Run { task }) => {
            let tools = make_tools(&root, mode, &cfg);
            let mut agent = Agent::new(provider.as_ref(), &tools, agent_config(&cfg)?);
            agent.set_plan_mode(cli.opts.plan);
            let report = agent.run_task(&task, &mut host).await.map_err(|e| e.to_string())?;
            println!();
            eprintln!("{}", stop_note(&report));
            eprintln!("{}", status_line(&report.usage, pricer.cost(&report.usage)));
            if let Some(msg) = update_notice(check.take()).await {
                eprintln!("\x1b[33m{msg}\x1b[0m");
            }
            let ok = matches!(report.stop, Stop::Done | Stop::PlanSaved) && report.verified != Some(false);
            Ok(if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE })
        }
        Some(Cmd::Bench { tasks, fixture, modes, out, only }) => {
            let modes = modes.split(',').map(|m| parse_mode(m.trim())).collect::<Result<Vec<Mode>, _>>()?;
            let mut tasks = bench::load_tasks(&tasks)?;
            if let Some(f) = &only {
                tasks.retain(|t| t.id.contains(f.as_str()));
            }
            if tasks.is_empty() {
                return Err("nenhuma tarefa para executar".into());
            }
            // Bench runs unattended in throwaway copies of the fixture.
            let acfg = AgentConfig { approval: ApprovalConfig::yolo(), ..agent_config(&cfg)? };
            let mut runs = Vec::new();
            for task in &tasks {
                for &m in &modes {
                    eprint!("{} [{}] … ", task.id, mode_name(m));
                    let mut r = bench::run_one(provider.as_ref(), task, &fixture, m, &acfg).await;
                    r.cost = pricer.cost(&r.usage);
                    eprintln!(
                        "{} {} turns, {}",
                        if r.passed { "✓" } else { "✗" },
                        r.turns,
                        status_line(&r.usage, r.cost)
                    );
                    if let Some(e) = &r.error {
                        eprintln!("    {}", e.lines().last().unwrap_or(""));
                    }
                    runs.push(r);
                }
            }
            std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let json = serde_json::to_string_pretty(&bench::to_json(&cfg.model, &runs)).map_err(|e| e.to_string())?;
            let md = bench::markdown(&cfg.model, &runs);
            let (jp, mp) = (out.join(format!("{ts}.json")), out.join(format!("{ts}.md")));
            std::fs::write(&jp, json).map_err(|e| format!("{}: {e}", jp.display()))?;
            std::fs::write(&mp, &md).map_err(|e| format!("{}: {e}", mp.display()))?;
            println!("{md}");
            eprintln!("gravados {} e {}", jp.display(), mp.display());
            Ok(ExitCode::SUCCESS)
        }
        Some(Cmd::Splash | Cmd::Update) | None => unreachable!("handled before the provider is built"),
    }
}

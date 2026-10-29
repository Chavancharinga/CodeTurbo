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
#[command(
    name = "codeturbo",
    version,
    about = "A coding agent that delivers verified code for fewer tokens. With no subcommand it opens the interface.",
    disable_help_subcommand = true
)]
struct Cli {
    /// Task sent as soon as the interface opens.
    prompt: Option<String>,
    #[command(flatten)]
    opts: Opts,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

/// Override the values in config.toml.
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
    /// Project folder the tools work in (default: the current folder).
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    /// always-ask | write | yolo
    #[arg(long, global = true)]
    approval_mode: Option<String>,
    /// Start in plan mode: read-only until you approve a plan.
    #[arg(long, global = true)]
    plan: bool,
    /// Model used for planning (for example, a stronger one than --model).
    #[arg(long, global = true)]
    plan_model: Option<String>,
    /// Approve every action (same as --approval-mode yolo).
    #[arg(long, global = true)]
    auto_approve: bool,
    /// Show the token count after each round.
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run one task and exit.
    Run { task: String },
    /// Show CodeTurbo's opening animation.
    Splash,
    /// Update CodeTurbo to the latest version.
    Update,
    /// A/B comparison of the tool sets on a test project.
    Bench {
        #[arg(long, default_value = "bench/tasks")]
        tasks: PathBuf,
        #[arg(long, default_value = "bench/fixture")]
        fixture: PathBuf,
        /// Comma-separated tool sets.
        #[arg(long, default_value = "naive,ast")]
        modes: String,
        #[arg(long, default_value = "bench/runs")]
        out: PathBuf,
        /// Only the tasks whose id contains this text.
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

/// Stack for the thread that drives the agent and for tokio's workers. The
/// agent's futures are large (in a debug build above all) and Windows gives
/// the main thread only 1 MiB, so a session could overflow it mid-task.
const STACK: usize = 32 * 1024 * 1024;

fn main() -> ExitCode {
    let run = || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_stack_size(STACK)
            .build();
        match rt {
            Ok(rt) => rt.block_on(start()),
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        }
    };
    match std::thread::Builder::new()
        .name("codeturbo".into())
        .stack_size(STACK)
        .spawn(run)
    {
        Ok(t) => t.join().unwrap_or(ExitCode::FAILURE),
        Err(_) => run(),
    }
}

async fn start() -> ExitCode {
    let cli = Cli::parse();
    match real_main(cli).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The update check's result, waiting at most a moment for it: it runs
/// while the intro plays, so it is usually ready.
async fn update_notice(check: Option<tokio::task::JoinHandle<Option<String>>>) -> Option<String> {
    let tag = tokio::time::timeout(std::time::Duration::from_millis(1500), check?)
        .await
        .ok()?
        .ok()??;
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
            return Err(
                "the interface needs a terminal; for a task without it use: codeturbo run \"...\""
                    .into(),
            );
        }
        let (plan, verbose, prompt) = (cli.opts.plan, cli.opts.verbose, cli.prompt);
        return session::run(Launch {
            cfg,
            prices,
            dir,
            root,
            mode,
            plan,
            verbose,
            prompt,
            update: check,
        })
        .await;
    }
    if matches!(cli.cmd, Some(Cmd::Run { .. })) {
        let mut trust = settings::Trust::load(dir.as_deref());
        if !trust.covers(&root) {
            let interactive = std::io::stdin().is_terminal();
            if !interactive {
                return Err(format!(
                    "the folder {} is not trusted yet; open `codeturbo` in it once to trust it",
                    root.display()
                ));
            }
            eprintln!("First time in this folder: {}", root.display());
            eprint!(
                "CodeTurbo will read, change files and run commands here. Trust this folder? [y/N] "
            );
            let mut answer = String::new();
            let _ = std::io::stdin().read_line(&mut answer);
            if !matches!(
                answer.trim().to_lowercase().as_str(),
                "s" | "sim" | "y" | "yes"
            ) {
                return Err("folder not trusted".into());
            }
            trust.add(dir.as_deref(), &root)?;
        }
    }
    let creds = settings::Credentials::load(dir.as_deref());
    let provider = config::make_provider(&cfg, settings::api_key(&cfg.provider, &creds))?;
    provider.warm(&cfg.model, cfg.effort.as_deref());
    let pricer = Pricer {
        table: prices,
        model: cfg.model.clone(),
        provider: cfg.provider.clone(),
    };
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
            eprintln!("  ! commands from .codeturbo.toml ignored (not allowed)");
        }
    }

    match cli.cmd {
        Some(Cmd::Run { task }) => {
            let tools = make_tools(&root, mode, &cfg);
            let mut agent = Agent::new(provider.as_ref(), &tools, agent_config(&cfg)?);
            agent.set_plan_mode(cli.opts.plan);
            let result = agent.run_task(&task, &mut host).await;
            // The browser `browse` kept open goes away with the task.
            tools.shutdown().await;
            let report = result.map_err(|e| e.to_string())?;
            println!();
            eprintln!("{}", stop_note(&report));
            eprintln!("{}", status_line(&report.usage, pricer.cost(&report.usage)));
            if let Some(msg) = update_notice(check.take()).await {
                eprintln!("\x1b[33m{msg}\x1b[0m");
            }
            let ok = matches!(report.stop, Stop::Done | Stop::PlanSaved)
                && report.verified != Some(false);
            Ok(if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        Some(Cmd::Bench {
            tasks,
            fixture,
            modes,
            out,
            only,
        }) => {
            let modes = modes
                .split(',')
                .map(|m| parse_mode(m.trim()))
                .collect::<Result<Vec<Mode>, _>>()?;
            let mut tasks = bench::load_tasks(&tasks)?;
            if let Some(f) = &only {
                tasks.retain(|t| t.id.contains(f.as_str()));
            }
            if tasks.is_empty() {
                return Err("no task to run".into());
            }
            // Bench runs unattended in throwaway copies of the fixture.
            let acfg = AgentConfig {
                approval: ApprovalConfig::yolo(),
                ..agent_config(&cfg)?
            };
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
            let json = serde_json::to_string_pretty(&bench::to_json(&cfg.model, &runs))
                .map_err(|e| e.to_string())?;
            let md = bench::markdown(&cfg.model, &runs);
            let (jp, mp) = (out.join(format!("{ts}.json")), out.join(format!("{ts}.md")));
            std::fs::write(&jp, json).map_err(|e| format!("{}: {e}", jp.display()))?;
            std::fs::write(&mp, &md).map_err(|e| format!("{}: {e}", mp.display()))?;
            println!("{md}");
            eprintln!("gravados {} e {}", jp.display(), mp.display());
            Ok(ExitCode::SUCCESS)
        }
        Some(Cmd::Splash | Cmd::Update) | None => {
            unreachable!("handled before the provider is built")
        }
    }
}

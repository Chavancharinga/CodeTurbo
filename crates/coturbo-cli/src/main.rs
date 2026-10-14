use clap::{Args, Parser, Subcommand};
use coturbo_agent::{Agent, AgentConfig, Stop, TaskReport, approval::ApprovalConfig};
use coturbo_cli::{
    bench,
    config::{self, Config, Pricer, mode_name, parse_mode},
    status::{TermHost, status_line},
};
use coturbo_tools::{Mode, Tools};
use coturbo_cli::splash;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "coturbo", version, about = "Token-frugal coding agent. No subcommand starts the REPL.")]
struct Cli {
    #[command(flatten)]
    opts: Opts,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

/// Overrides for values in config.toml.
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
    /// Project root the tools are confined to (default: current dir).
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    /// always-ask | write | yolo
    #[arg(long, global = true)]
    approval_mode: Option<String>,
    /// Start in plan mode: read-only until you approve a plan.
    #[arg(long, global = true)]
    plan: bool,
    /// Model used while planning (e.g. a stronger one than --model).
    #[arg(long, global = true)]
    plan_model: Option<String>,
    /// Approve every tool call (same as --approval-mode yolo).
    #[arg(long, global = true)]
    auto_approve: bool,
    /// Print the running token count after every turn.
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run one task and exit.
    Run { task: String },
    /// Play the CodeTurbo intro animation.
    Splash,
    /// A/B benchmark of the tool sets over a fixture.
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
        /// Only tasks whose id contains this.
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

fn make_tools(root: &std::path::Path, mode: Mode, cfg: &Config) -> Tools {
    let tools = Tools::new(root, mode).with_native_checks(cfg.diagnostics_on_write);
    match cfg.lsp_config() {
        Some(lsp) => {
            let root = tools.root().to_path_buf();
            tools.with_lsp(std::sync::Arc::new(coturbo_verify::lsp::LspManager::new(&root, lsp)))
        }
        None => tools,
    }
}

fn agent_config(c: &Config) -> Result<AgentConfig, String> {
    Ok(AgentConfig {
        model: c.model.clone(),
        max_turns: c.max_turns,
        max_tokens: c.max_tokens,
        effort: c.effort.clone(),
        approval: c.approval_config()?,
        plan_model: c.plan_model.clone(),
        context_window: c.context_window,
        gate: c.gate_config(),
        max_fix_rounds: c.max_fix_rounds,
    })
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match real_main(cli).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn real_main(cli: Cli) -> Result<ExitCode, String> {
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
    // The REPL opens with the snail, before anything that can fail on a
    // missing key, so the intro always shows.
    let animated = cli.cmd.is_none() && splash::can_animate(std::io::stderr().is_terminal());
    if animated {
        splash::play(&mut std::io::stderr(), true, &std::sync::atomic::AtomicBool::new(true));
    }
    let provider = config::make_provider(&cfg)?;
    let pricer = Pricer { table: prices, model: cfg.model.clone(), provider: cfg.provider.clone() };
    let root = match &cli.opts.root {
        Some(r) => r.clone(),
        None => std::env::current_dir().map_err(|e| e.to_string())?,
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
            eprintln!("  ! ignoring .coturbo.toml commands (not trusted)");
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
                return Err("no tasks to run".into());
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
            eprintln!("wrote {} and {}", jp.display(), mp.display());
            Ok(ExitCode::SUCCESS)
        }
        Some(Cmd::Splash) => unreachable!("handled before the provider is built"),
        None => repl(provider.as_ref(), &root, mode, &cfg, &pricer, &mut host, cli.opts.plan).await,
    }
}

fn stop_note(report: &TaskReport) -> String {
    let turns = report.turns;
    let stop = match report.stop {
        Stop::Done => format!("done in {turns} turns"),
        Stop::MaxTurns => format!("stopped: hit max_turns ({turns})"),
        Stop::Refused => "stopped: the model refused".into(),
        Stop::PlanSaved => "plan saved, not implemented".into(),
    };
    match report.verified {
        Some(true) => format!("{stop} · \x1b[32m✓ verified\x1b[0m"),
        Some(false) => format!("{stop} · \x1b[31m✗ NOT verified\x1b[0m"),
        None => stop,
    }
}

async fn repl(
    provider: &dyn coturbo_providers::Provider,
    root: &std::path::Path,
    mode: Mode,
    cfg: &Config,
    pricer: &Pricer,
    host: &mut TermHost,
    plan: bool,
) -> Result<ExitCode, String> {
    use rustyline::error::ReadlineError;
    let tools = make_tools(root, mode, cfg);
    let mut agent = Agent::new(provider, &tools, agent_config(cfg)?);
    agent.set_plan_mode(plan);
    // The intro already played (in `real_main`) under this same condition.
    let animated = splash::can_animate(std::io::stderr().is_terminal());
    let mut rl = rustyline::DefaultEditor::new().map_err(|e| e.to_string())?;
    eprintln!(
        "coturbo · {} · {} · tools {} · approval {} · /plan, /usage, /exit",
        cfg.provider,
        cfg.model,
        mode_name(mode),
        cfg.approval_mode
    );
    // The snail keeps running above the prompt until the first task.
    // From the prompt: header (1) + tagline (1) + logo rows.
    let mut live = animated.then(|| splash::LiveLogo::start(2 + splash::ROWS));
    loop {
        let prompt = if agent.plan_mode() { "plan › " } else { "› " };
        let read = rl.readline(prompt);
        if let Some(mut l) = live.take() {
            l.stop();
        }
        let line = match read {
            Ok(l) => l,
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => break,
            Err(e) => return Err(e.to_string()),
        };
        let line = line.trim();
        match line {
            "" => continue,
            "/exit" | "/quit" => break,
            "/plan" => {
                agent.set_plan_mode(!agent.plan_mode());
                eprintln!("plan mode {}", if agent.plan_mode() { "on: read-only until you approve a plan" } else { "off" });
                continue;
            }
            "/usage" => {
                let u = agent.session_usage();
                eprintln!("session: {}", status_line(&u, pricer.cost(&u)));
                continue;
            }
            _ => {}
        }
        let _ = rl.add_history_entry(line);
        match agent.run_task(line, host).await {
            Ok(report) => {
                println!();
                let s = agent.session_usage();
                eprintln!(
                    "{} · task {} · session {}",
                    stop_note(&report),
                    status_line(&report.usage, pricer.cost(&report.usage)),
                    status_line(&s, pricer.cost(&s)),
                );
            }
            Err(e) => eprintln!("\nerror: {e}"),
        }
        // Each task ends with the history reset to a short summary.
        agent.compact();
    }
    Ok(ExitCode::SUCCESS)
}

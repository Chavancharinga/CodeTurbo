use clap::{Args, Parser, Subcommand};
use coturbo_agent::{Agent, AgentConfig, Stop};
use coturbo_cli::{
    bench,
    config::{self, Config, Pricer, mode_name, parse_mode},
    status::{TermObserver, status_line},
};
use coturbo_tools::{Mode, Tools};
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
    /// Print the running token count after every turn.
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run one task and exit.
    Run { task: String },
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
    c
}

fn agent_config(c: &Config) -> AgentConfig {
    AgentConfig { model: c.model.clone(), max_turns: c.max_turns, max_tokens: c.max_tokens, effort: c.effort.clone() }
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
    let dir = config::config_dir();
    let (cfg, prices) = config::load(dir.as_deref())?;
    let cfg = apply(cfg, &cli.opts);
    let mode = parse_mode(&cfg.tools)?;
    let provider = config::make_provider(&cfg)?;
    let pricer = Pricer { table: prices, model: cfg.model.clone(), provider: cfg.provider.clone() };
    let root = match &cli.opts.root {
        Some(r) => r.clone(),
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let mut obs = TermObserver { verbose: cli.opts.verbose };

    match cli.cmd {
        Some(Cmd::Run { task }) => {
            let tools = Tools::new(&root, mode);
            let mut agent = Agent::new(provider.as_ref(), &tools, agent_config(&cfg));
            let report = agent.run_task(&task, &mut obs).await.map_err(|e| e.to_string())?;
            println!();
            eprintln!("{}", stop_note(report.stop, report.turns));
            eprintln!("{}", status_line(&report.usage, pricer.cost(&report.usage)));
            Ok(if report.stop == Stop::Done { ExitCode::SUCCESS } else { ExitCode::FAILURE })
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
            let acfg = agent_config(&cfg);
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
        None => repl(provider.as_ref(), &root, mode, &cfg, &pricer, &mut obs).await,
    }
}

fn stop_note(stop: Stop, turns: u32) -> String {
    match stop {
        Stop::Done => format!("done in {turns} turns"),
        Stop::MaxTurns => format!("stopped: hit max_turns ({turns})"),
        Stop::Refused => "stopped: the model refused".into(),
    }
}

async fn repl(
    provider: &dyn coturbo_providers::Provider,
    root: &std::path::Path,
    mode: Mode,
    cfg: &Config,
    pricer: &Pricer,
    obs: &mut TermObserver,
) -> Result<ExitCode, String> {
    use rustyline::error::ReadlineError;
    let tools = Tools::new(root, mode);
    let mut agent = Agent::new(provider, &tools, agent_config(cfg));
    let mut rl = rustyline::DefaultEditor::new().map_err(|e| e.to_string())?;
    eprintln!(
        "coturbo · {} · {} · tools {} · /usage, /exit",
        cfg.provider,
        cfg.model,
        mode_name(mode)
    );
    loop {
        let line = match rl.readline("› ") {
            Ok(l) => l,
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => break,
            Err(e) => return Err(e.to_string()),
        };
        let line = line.trim();
        match line {
            "" => continue,
            "/exit" | "/quit" => break,
            "/usage" => {
                let u = agent.session_usage();
                eprintln!("session: {}", status_line(&u, pricer.cost(&u)));
                continue;
            }
            _ => {}
        }
        let _ = rl.add_history_entry(line);
        match agent.run_task(line, obs).await {
            Ok(report) => {
                println!();
                let s = agent.session_usage();
                eprintln!(
                    "{} · task {} · session {}",
                    stop_note(report.stop, report.turns),
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

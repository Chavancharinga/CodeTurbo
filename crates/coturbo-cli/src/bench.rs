//! A/B benchmark: every task runs once per tool set on a fresh copy of the
//! fixture; `verify` decides pass/fail. Results go to JSON + a Markdown table.

use crate::config::mode_name;
use coturbo_agent::{Agent, AgentConfig, NoopObserver, Stop};
use coturbo_providers::{Provider, Usage};
use coturbo_tools::{Mode, Tools};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;
use std::time::Instant;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub prompt: String,
    /// Shell command run in the task's copy of the fixture; exit 0 = pass.
    pub verify: String,
}

/// Loads `*.toml` tasks from `dir`, sorted by file name.
pub fn load_tasks(dir: &Path) -> Result<Vec<Task>, String> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();
    files
        .iter()
        .map(|p| {
            let s = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
            toml::from_str(&s).map_err(|e| format!("{}: {e}", p.display()))
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct BenchRun {
    pub task: String,
    pub mode: Mode,
    pub stop: Option<Stop>,
    pub turns: u32,
    pub usage: Usage,
    pub cost: Option<f64>,
    pub passed: bool,
    /// Provider error or verify output tail when the run failed.
    pub error: Option<String>,
    pub seconds: f64,
}

/// Copies `src` into `dst`, skipping build output and VCS metadata.
pub fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == "target" || name == ".git" {
            continue;
        }
        let to = dst.join(&name);
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

pub async fn run_one(
    provider: &dyn Provider,
    task: &Task,
    fixture: &Path,
    mode: Mode,
    config: &AgentConfig,
) -> BenchRun {
    let started = Instant::now();
    let mut run = BenchRun {
        task: task.id.clone(),
        mode,
        stop: None,
        turns: 0,
        usage: Usage::default(),
        cost: None,
        passed: false,
        error: None,
        seconds: 0.0,
    };
    let dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => {
            run.error = Some(format!("tempdir: {e}"));
            return run;
        }
    };
    if let Err(e) = copy_tree(fixture, dir.path()) {
        run.error = Some(format!("copy fixture: {e}"));
        return run;
    }
    let tools = Tools::new(dir.path(), mode);
    let mut agent = Agent::new(provider, &tools, config.clone());
    match agent.run_task(&task.prompt, &mut NoopObserver).await {
        Ok(report) => {
            run.stop = Some(report.stop);
            run.turns = report.turns;
            run.usage = report.usage;
            let out = tools.call("bash", &json!({ "command": task.verify })).await;
            run.passed = !out.is_error;
            if out.is_error {
                run.error = Some(tail(&out.content, 600));
            }
        }
        Err(e) => run.error = Some(e.to_string()),
    }
    run.seconds = started.elapsed().as_secs_f64();
    run
}

fn tail(s: &str, max: usize) -> String {
    let n = s.chars().count();
    s.chars().skip(n.saturating_sub(max)).collect()
}

fn stop_name(s: Option<Stop>) -> &'static str {
    match s {
        Some(Stop::Done) => "done",
        Some(Stop::MaxTurns) => "max_turns",
        Some(Stop::Refused) => "refused",
        None => "error",
    }
}

pub fn to_json(model: &str, runs: &[BenchRun]) -> Value {
    json!({
        "model": model,
        "runs": runs.iter().map(|r| json!({
            "task": r.task,
            "mode": mode_name(r.mode),
            "stop": stop_name(r.stop),
            "turns": r.turns,
            "usage": {
                "input": r.usage.input,
                "cache_write": r.usage.cache_write,
                "cache_read": r.usage.cache_read,
                "output": r.usage.output,
            },
            "cost": r.cost,
            "passed": r.passed,
            "error": r.error,
            "seconds": r.seconds,
        })).collect::<Vec<_>>(),
    })
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Totals {
    pub runs: u32,
    pub passed: u32,
    pub turns: u32,
    pub tokens: u64,
    /// `None` if any run lacked a price.
    pub cost: Option<f64>,
}

pub fn totals(runs: &[BenchRun], mode: Mode) -> Totals {
    let mut t = Totals { cost: Some(0.0), ..Default::default() };
    for r in runs.iter().filter(|r| r.mode == mode) {
        t.runs += 1;
        t.passed += r.passed as u32;
        t.turns += r.turns;
        t.tokens += r.usage.prompt_total() + r.usage.output;
        t.cost = t.cost.zip(r.cost).map(|(a, b)| a + b);
    }
    t
}

fn money(c: Option<f64>) -> String {
    c.map_or("—".into(), |c| format!("${c:.4}"))
}

pub fn markdown(model: &str, runs: &[BenchRun]) -> String {
    let mut s = format!(
        "# CoTurbo bench — {model}\n\n\
         | task | mode | passed | turns | ↑in | ⟳cache | ↓out | cost | s |\n\
         |---|---|---|---|---|---|---|---|---|\n"
    );
    for r in runs {
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {:.0} |\n",
            r.task,
            mode_name(r.mode),
            if r.passed { "✓" } else { "✗" },
            r.turns,
            r.usage.input + r.usage.cache_write,
            r.usage.cache_read,
            r.usage.output,
            money(r.cost),
            r.seconds,
        ));
    }
    s.push_str("\n| mode | pass rate | avg turns | tokens | cost |\n|---|---|---|---|---|\n");
    let modes: Vec<Mode> = [Mode::Naive, Mode::Ast].into_iter().filter(|m| runs.iter().any(|r| r.mode == *m)).collect();
    for &m in &modes {
        let t = totals(runs, m);
        s.push_str(&format!(
            "| {} | {}/{} | {:.1} | {} | {} |\n",
            mode_name(m),
            t.passed,
            t.runs,
            t.turns as f64 / t.runs.max(1) as f64,
            t.tokens,
            money(t.cost),
        ));
    }
    if modes.len() == 2 {
        let (n, a) = (totals(runs, Mode::Naive), totals(runs, Mode::Ast));
        if n.tokens > 0 {
            let saved = 100.0 * (1.0 - a.tokens as f64 / n.tokens as f64);
            s.push_str(&format!("\nast vs naive: {saved:+.1}% tokens saved"));
            if let (Some(nc), Some(ac)) = (n.cost, a.cost)
                && nc > 0.0
            {
                s.push_str(&format!(", {:+.1}% cost saved", 100.0 * (1.0 - ac / nc)));
            }
            s.push('\n');
        }
    }
    s
}

//! The final gate: before a task that edited code counts as done, the
//! harness builds, lints and tests the project. Zero errors and zero
//! warnings, or the model gets the compact diagnostics and keeps working.

use crate::cmd::{self, CmdOutput};
use crate::diag::{self, Level};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

const MAX_DIAGS: usize = 20;
const TAIL: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub struct GateConfig {
    /// Overrides detection: shell commands run in order.
    pub verify: Option<Vec<String>>,
    /// Optional smoke run of the program, e.g. `cargo run -- --help`.
    pub run: Option<String>,
    pub step_timeout: Duration,
    pub run_timeout: Duration,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            verify: None,
            run: None,
            step_timeout: Duration::from_secs(600),
            run_timeout: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `cargo <args> --message-format=json`; diagnostics are parsed.
    Cargo {
        name: String,
        args: Vec<String>,
    },
    CargoTest,
    /// Shell command: exit code and warning lines decide. `optional` steps
    /// are skipped when the tool is not installed.
    Shell {
        name: String,
        command: String,
        optional: bool,
    },
    /// Smoke run: fails on non-zero exit, a panic or a stack trace.
    Run {
        command: String,
    },
}

impl Step {
    pub fn name(&self) -> String {
        match self {
            Step::Cargo { name, .. } | Step::Shell { name, .. } => name.clone(),
            Step::CargoTest => "test".into(),
            Step::Run { .. } => "run".into(),
        }
    }

    pub fn command(&self) -> String {
        match self {
            Step::Cargo { args, .. } => format!("cargo {}", args.join(" ")),
            Step::CargoTest => "cargo test".into(),
            Step::Shell { command, .. } | Step::Run { command } => command.clone(),
        }
    }
}

fn shell_step(name: &str, command: &str, optional: bool) -> Step {
    Step::Shell {
        name: name.into(),
        command: command.into(),
        optional,
    }
}

/// Package manager used to run `package.json` scripts, from the lockfile.
fn js_runner(root: &Path) -> &'static str {
    if root.join("bun.lockb").exists() || root.join("bun.lock").exists() {
        "bun run"
    } else if root.join("pnpm-lock.yaml").exists() {
        "pnpm run"
    } else if root.join("yarn.lock").exists() {
        "yarn run"
    } else {
        "npm run"
    }
}

fn js_steps(root: &Path) -> Vec<Step> {
    let scripts: Value = std::fs::read_to_string(root.join("package.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .map(|v| v["scripts"].clone())
        .unwrap_or(Value::Null);
    let has = |s: &str| {
        scripts[s]
            .as_str()
            .is_some_and(|c| !c.contains("no test specified"))
    };
    let run = js_runner(root);
    let mut steps = Vec::new();
    if let Some(s) = ["typecheck", "type-check", "tsc"]
        .into_iter()
        .find(|s| has(s))
    {
        steps.push(shell_step("typecheck", &format!("{run} {s}"), false));
    } else if root.join("tsconfig.json").exists() {
        steps.push(shell_step("typecheck", "npx tsc --noEmit", false));
    }
    for s in ["lint", "build", "test"] {
        if has(s) {
            steps.push(shell_step(s, &format!("{run} {s}"), false));
        }
    }
    steps
}

/// Steps for every toolchain found at the root, in a fixed order.
pub fn detect(root: &Path, cfg: &GateConfig) -> Vec<Step> {
    let mut steps = Vec::new();
    if let Some(cmds) = &cfg.verify {
        steps.extend(cmds.iter().map(|c| shell_step(c, c, false)));
    } else {
        if root.join("Cargo.toml").exists() {
            let cargo = |name: &str, args: &[&str]| Step::Cargo {
                name: name.into(),
                args: args.iter().map(|s| s.to_string()).collect(),
            };
            // No separate `cargo build`: clippy over all targets reports every
            // compile error, and `cargo test` builds everything again anyway.
            steps.push(cargo(
                "clippy",
                &[
                    "clippy",
                    "--all-targets",
                    "--message-format=json",
                    "--",
                    "-D",
                    "warnings",
                ],
            ));
            steps.push(Step::CargoTest);
        }
        if root.join("package.json").exists() {
            steps.extend(js_steps(root));
        }
        if root.join("go.mod").exists() || root.join("go.work").exists() {
            steps.push(shell_step("go build", "go build ./...", false));
            steps.push(shell_step("go vet", "go vet ./...", false));
            steps.push(shell_step("go test", "go test ./...", false));
        }
        if root.join("pyproject.toml").exists() {
            steps.push(shell_step("pyright", "pyright", true));
            steps.push(shell_step("pytest", "python -m pytest -q", true));
        }
    }
    if let Some(r) = &cfg.run {
        steps.push(Step::Run { command: r.clone() });
    }
    steps
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepResult {
    pub name: String,
    pub command: String,
    pub ok: bool,
    pub skipped: bool,
    pub errors: usize,
    pub warnings: usize,
    /// Compact diagnostics or output tail; empty on success.
    pub details: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateReport {
    pub steps: Vec<StepResult>,
}

impl GateReport {
    pub fn ok(&self) -> bool {
        self.steps.iter().all(|s| s.ok)
    }

    /// No checker ran: nothing was verified either way.
    pub fn nothing_ran(&self) -> bool {
        self.steps.iter().all(|s| s.skipped)
    }

    /// What the model sees: one line on success, compact diagnostics on failure.
    pub fn summary(&self) -> String {
        let ran: Vec<&str> = self
            .steps
            .iter()
            .filter(|s| !s.skipped)
            .map(|s| s.name.as_str())
            .collect();
        match self.steps.iter().find(|s| !s.ok) {
            None if ran.is_empty() => {
                "verification skipped: no build, lint or test command found".into()
            }
            None => format!(
                "✓ verification passed ({}): 0 errors, 0 warnings",
                ran.join(", ")
            ),
            Some(f) => format!(
                "✗ verification failed at `{}` (`{}`): {} error(s), {} warning(s). \
                 Warnings count as failures. Fix every item, then finish.\n{}",
                f.name, f.command, f.errors, f.warnings, f.details
            ),
        }
    }
}

fn not_installed(out: &CmdOutput) -> bool {
    let all = out.combined().to_ascii_lowercase();
    out.code == Some(127)
        || all.contains("command not found")
        || all.contains("is not recognized as an internal or external command")
        || all.contains("no module named")
}

async fn run_step(root: &Path, step: &Step, cfg: &GateConfig) -> StepResult {
    let mut r = StepResult {
        name: step.name(),
        command: step.command(),
        ok: false,
        skipped: false,
        errors: 0,
        warnings: 0,
        details: String::new(),
    };
    let timeout = if matches!(step, Step::Run { .. }) {
        cfg.run_timeout
    } else {
        cfg.step_timeout
    };
    let command = match step {
        Step::Cargo { args, .. } => {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            cmd::program("cargo", &args)
        }
        Step::CargoTest => cmd::program("cargo", &["test", "--quiet", "--color", "never"]),
        Step::Shell { command, .. } | Step::Run { command } => cmd::shell(command),
    };
    let out = match cmd::run(command, root, timeout).await {
        Ok(o) => o,
        Err(e) => {
            let optional = matches!(step, Step::Shell { optional: true, .. });
            r.skipped = optional && e.starts_with("cannot start");
            r.ok = r.skipped;
            r.errors = usize::from(!r.ok);
            r.details = e;
            return r;
        }
    };
    match step {
        Step::Cargo { .. } => {
            let diags = diag::cargo_json(&out.stdout);
            r.errors = diag::count(&diags, Level::Error);
            r.warnings = diag::count(&diags, Level::Warning);
            r.ok = out.ok && diags.is_empty();
            r.details = if diags.is_empty() {
                cmd::tail_lines(&out.stderr, TAIL)
            } else {
                diag::render_list(&diags, MAX_DIAGS)
            };
            if !out.ok && r.errors == 0 {
                r.errors = 1;
            }
        }
        Step::CargoTest => {
            r.ok = out.ok;
            if !out.ok {
                let failures = diag::cargo_test_failures(&out.combined());
                r.errors = failures.len().max(1);
                r.details = if failures.is_empty() {
                    cmd::tail_lines(&out.combined(), TAIL)
                } else {
                    failures
                        .into_iter()
                        .take(MAX_DIAGS)
                        .collect::<Vec<_>>()
                        .join("\n")
                };
            }
        }
        Step::Shell { optional, .. } => {
            if *optional && !out.ok && not_installed(&out) {
                r.skipped = true;
                r.ok = true;
                return r;
            }
            let warnings = diag::generic_warnings(&out.combined());
            r.warnings = warnings.len();
            r.ok = out.ok && warnings.is_empty();
            if !out.ok {
                r.errors = 1;
                r.details = cmd::tail_lines(&out.combined(), TAIL);
            } else if !warnings.is_empty() {
                r.details = warnings
                    .into_iter()
                    .take(MAX_DIAGS)
                    .collect::<Vec<_>>()
                    .join("\n");
            }
        }
        Step::Run { .. } => {
            let all = out.combined();
            let crashed =
                all.contains("panicked at") || all.contains("Traceback (most recent call last)");
            r.ok = out.ok && !crashed;
            if !r.ok {
                r.errors = 1;
                r.details = format!(
                    "exit {}\n{}",
                    out.code.map_or("signal".into(), |c| c.to_string()),
                    cmd::tail_lines(&all, TAIL)
                );
            }
        }
    }
    r
}

/// What the gate is doing, for a screen that shows progress.
#[derive(Debug, Clone, Copy)]
pub enum GateProgress<'a> {
    /// Step `index` of `total` is about to run.
    Started {
        name: &'a str,
        index: usize,
        total: usize,
    },
    /// It ran, in `secs` seconds.
    Finished {
        result: &'a StepResult,
        index: usize,
        total: usize,
        secs: f32,
    },
}

/// Runs the steps in order and stops at the first failure.
pub async fn run_gate(root: &Path, steps: &[Step], cfg: &GateConfig) -> GateReport {
    run_gate_with(root, steps, cfg, &mut |_| {}).await
}

/// [`run_gate`], telling `progress` when each step starts and ends.
pub async fn run_gate_with(
    root: &Path,
    steps: &[Step],
    cfg: &GateConfig,
    progress: &mut (dyn FnMut(GateProgress<'_>) + Send),
) -> GateReport {
    let mut report = GateReport { steps: Vec::new() };
    let total = steps.len();
    for (index, step) in steps.iter().enumerate() {
        progress(GateProgress::Started {
            name: &step.name(),
            index,
            total,
        });
        let started = std::time::Instant::now();
        let r = run_step(root, step, cfg).await;
        progress(GateProgress::Finished {
            result: &r,
            index,
            total,
            secs: started.elapsed().as_secs_f32(),
        });
        let failed = !r.ok;
        report.steps.push(r);
        if failed {
            break;
        }
    }
    report
}

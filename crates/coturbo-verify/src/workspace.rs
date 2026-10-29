//! Whole-project diagnostics, modelled on oh-my-pi's workspace diagnostics:
//! every toolchain present at the root is checked (not just the first one
//! found), at most two at a time, and an empty result from a failed checker
//! is reported as a failure, never as a clean project.

use crate::cmd::{self, CmdOutput};
use crate::diag::{self, Level};
use futures_util::{StreamExt, stream};
use std::path::Path;
use std::time::Duration;

const MAX_LINES: usize = 20;
/// Checkers like `cargo check` and `tsc` already use every core.
const PARALLEL: usize = 2;
const TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checker {
    pub lang: &'static str,
    pub description: String,
    pub program: String,
    pub args: Vec<String>,
}

/// `use` directories of a `go.work` file as `./dir/...` package patterns.
pub fn go_work_patterns(src: &str) -> Vec<String> {
    let mut dirs = Vec::new();
    let mut in_block = false;
    for line in src.lines() {
        let line = line.split("//").next().unwrap_or("").trim();
        if in_block {
            if line == ")" {
                in_block = false;
            } else if !line.is_empty() {
                dirs.push(line.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("use") {
            let rest = rest.trim();
            if rest == "(" {
                in_block = true;
            } else if !rest.is_empty() {
                dirs.push(rest.to_string());
            }
        }
    }
    dirs.into_iter()
        .map(|d| format!("{}/...", d.trim_end_matches('/')))
        .collect()
}

fn checker(lang: &'static str, description: &str, program: &str, args: &[&str]) -> Checker {
    Checker {
        lang,
        description: description.into(),
        program: program.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
    }
}

/// Every checker that applies at `root`, in a fixed priority order.
/// `go.work` wins over `go.mod`; `pyproject.toml` over `pyrightconfig.json`.
pub fn detect(root: &Path) -> Vec<Checker> {
    let has = |f: &str| root.join(f).exists();
    let mut out = Vec::new();
    if has("Cargo.toml") {
        out.push(checker(
            "rust",
            "Rust (cargo check)",
            "cargo",
            &["check", "--all-targets", "--message-format=json"],
        ));
    }
    if has("tsconfig.json") {
        out.push(checker(
            "typescript",
            "TypeScript (tsc)",
            "npx",
            &["tsc", "--noEmit", "--pretty", "false"],
        ));
    }
    if has("go.work") {
        let patterns = std::fs::read_to_string(root.join("go.work"))
            .map(|s| go_work_patterns(&s))
            .unwrap_or_default();
        let mut args = vec!["build".to_string()];
        if patterns.is_empty() {
            args.push("./...".into());
        } else {
            args.extend(patterns);
        }
        out.push(Checker {
            lang: "go",
            description: "Go (go build)".into(),
            program: "go".into(),
            args,
        });
    } else if has("go.mod") {
        out.push(checker("go", "Go (go build)", "go", &["build", "./..."]));
    }
    if has("pyproject.toml") || has("pyrightconfig.json") {
        out.push(checker("python", "Python (pyright)", "pyright", &[]));
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub description: String,
    pub errors: usize,
    pub warnings: usize,
    /// Compact problem list, or why the checker itself failed.
    pub body: String,
    /// The checker crashed or could not start.
    pub failed: bool,
}

impl CheckResult {
    pub fn clean(&self) -> bool {
        !self.failed && self.errors == 0 && self.warnings == 0
    }
}

/// Lines that look like problems in a non-JSON checker's output.
fn problem_lines(output: &str) -> (Vec<String>, usize, usize) {
    // `go build` prints `./x.go:5:2: undefined: y` without the word "error".
    let error = regex::Regex::new(r"(?i)\berror\b|\.go:\d+:\d+:").expect("valid regex");
    let warning = regex::Regex::new(r"(?i)\bwarning\b").expect("valid regex");
    let zero = regex::Regex::new(r"(?i)\b0 (errors?|warnings?)\b").expect("valid regex");
    let mut lines = Vec::new();
    let (mut errors, mut warnings) = (0, 0);
    for l in output.lines() {
        if zero.is_match(l) {
            continue;
        }
        if error.is_match(l) {
            errors += 1;
        } else if warning.is_match(l) {
            warnings += 1;
        } else {
            continue;
        }
        lines.push(l.trim().replace('\\', "/"));
    }
    (lines, errors, warnings)
}

fn interpret(c: &Checker, out: Result<CmdOutput, String>) -> CheckResult {
    let mut r = CheckResult {
        description: c.description.clone(),
        errors: 0,
        warnings: 0,
        body: String::new(),
        failed: false,
    };
    let out = match out {
        Ok(o) => o,
        Err(e) => {
            r.failed = true;
            r.body = format!("checker failed: {e}");
            return r;
        }
    };
    if c.lang == "rust" {
        let diags = diag::cargo_json(&out.stdout);
        r.errors = diag::count(&diags, Level::Error);
        r.warnings = diag::count(&diags, Level::Warning);
        r.body = diag::render_list(&diags, MAX_LINES);
    } else {
        let (lines, errors, warnings) = problem_lines(&out.combined());
        r.errors = errors;
        r.warnings = warnings;
        let total = lines.len();
        r.body = lines
            .into_iter()
            .take(MAX_LINES)
            .collect::<Vec<_>>()
            .join("\n");
        if total > MAX_LINES {
            r.body.push_str(&format!("\n[{} more]", total - MAX_LINES));
        }
    }
    if !out.ok && r.errors == 0 {
        // Non-zero exit without a parsable problem: the checker itself broke.
        r.failed = true;
        let tail = cmd::tail_lines(&out.combined(), 15);
        r.body = if tail.is_empty() {
            format!(
                "checker exited with {} and printed nothing",
                out.code.map_or("a signal".into(), |c| c.to_string())
            )
        } else {
            tail
        };
    }
    r
}

pub async fn check(root: &Path) -> Vec<CheckResult> {
    let checkers = detect(root);
    stream::iter(checkers.into_iter().map(|c| async move {
        let args: Vec<&str> = c.args.iter().map(String::as_str).collect();
        let out = cmd::run(cmd::program(&c.program, &args), root, TIMEOUT).await;
        interpret(&c, out)
    }))
    .buffered(PARALLEL)
    .collect()
    .await
}

/// Tool output: `(text, clean)`. A single language keeps a bare body; more
/// than one get a header each.
pub fn render(results: &[CheckResult]) -> (String, bool) {
    if results.is_empty() {
        return (
            "no supported project found (Cargo.toml, tsconfig.json, go.mod, pyproject.toml)".into(),
            true,
        );
    }
    let clean = results.iter().all(CheckResult::clean);
    let section = |r: &CheckResult| {
        if r.clean() {
            "ok: 0 errors, 0 warnings".to_string()
        } else if r.failed {
            r.body.clone()
        } else {
            format!(
                "{} error(s), {} warning(s)\n{}",
                r.errors, r.warnings, r.body
            )
        }
    };
    let text = if results.len() == 1 {
        section(&results[0])
    } else {
        results
            .iter()
            .map(|r| format!("## {}\n{}", r.description, section(r)))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    (text, clean)
}

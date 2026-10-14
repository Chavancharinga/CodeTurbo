//! Compact diagnostics shared by every checker: one `path:line:col` line per
//! problem plus the offending source line, never the raw log.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diag {
    pub level: Level,
    pub path: String,
    pub line: u32,
    pub col: u32,
    pub message: String,
    /// `E0308`, `unused_variables`, `clippy::needless_return`, ...
    pub code: Option<String>,
    /// The source line the problem points at.
    pub source: Option<String>,
}

impl Diag {
    pub fn render(&self) -> String {
        let level = match self.level {
            Level::Error => "error",
            Level::Warning => "warning",
        };
        let code = self.code.as_ref().map(|c| format!("[{c}]")).unwrap_or_default();
        let mut s = format!("{level}{code} {}:{}:{}: {}", self.path, self.line, self.col, self.message);
        if let Some(src) = &self.source {
            s.push_str(&format!("\n    {}", src.trim()));
        }
        s
    }
}

/// Parses `cargo … --message-format=json` output: errors and warnings with a
/// primary span, deduplicated, in output order.
pub fn cargo_json(stdout: &str) -> Vec<Diag> {
    let mut out: Vec<Diag> = Vec::new();
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v["reason"] != "compiler-message" {
            continue;
        }
        let m = &v["message"];
        let level = match m["level"].as_str() {
            Some("error") => Level::Error,
            Some("warning") => Level::Warning,
            _ => continue,
        };
        let Some(span) = m["spans"].as_array().and_then(|s| s.iter().find(|s| s["is_primary"] == true)) else {
            continue;
        };
        let label = span["label"].as_str().filter(|l| !l.is_empty());
        let message = match label {
            Some(l) => format!("{} — {l}", m["message"].as_str().unwrap_or("")),
            None => m["message"].as_str().unwrap_or("").to_string(),
        };
        let d = Diag {
            level,
            path: span["file_name"].as_str().unwrap_or("?").replace('\\', "/"),
            line: span["line_start"].as_u64().unwrap_or(0) as u32,
            col: span["column_start"].as_u64().unwrap_or(0) as u32,
            message,
            code: m["code"]["code"].as_str().map(str::to_string),
            source: span["text"][0]["text"].as_str().map(str::to_string),
        };
        if !out.contains(&d) {
            out.push(d);
        }
    }
    out
}

/// Errors first, at most `max` entries, with a count of the rest.
pub fn render_list(diags: &[Diag], max: usize) -> String {
    let mut sorted: Vec<&Diag> = diags.iter().collect();
    sorted.sort_by_key(|d| d.level);
    let mut s: Vec<String> = sorted.iter().take(max).map(|d| d.render()).collect();
    if diags.len() > max {
        s.push(format!("[{} more]", diags.len() - max));
    }
    s.join("\n")
}

pub fn count(diags: &[Diag], level: Level) -> usize {
    diags.iter().filter(|d| d.level == level).count()
}

/// Failed test names and their panic messages from `cargo test` output.
pub fn cargo_test_failures(output: &str) -> Vec<String> {
    let mut failures = Vec::new();
    let mut current: Option<String> = None;
    for line in output.lines() {
        if let Some(name) = line.strip_prefix("---- ").and_then(|l| l.split(" stdout ----").next())
            && line.ends_with("stdout ----")
        {
            current = Some(name.to_string());
            continue;
        }
        if let Some(name) = &current
            && (line.contains("panicked at") || line.starts_with("assertion") || line.trim_start().starts_with("left:") || line.trim_start().starts_with("right:"))
        {
            failures.push(format!("{name}: {}", line.trim()));
        }
    }
    if failures.is_empty() {
        // No captured output: fall back to the result lines.
        failures = output
            .lines()
            .filter(|l| l.starts_with("test ") && l.ends_with("FAILED"))
            .map(|l| l.trim().to_string())
            .collect();
    }
    failures.dedup();
    failures
}

/// Lines a generic tool printed as warnings (eslint, tsc plugins, gcc, ...).
/// `npm warn` notices about the package manager itself are ignored.
pub fn generic_warnings(output: &str) -> Vec<String> {
    let re = regex::Regex::new(r"(?i)\bwarn(ing)?\b").expect("valid regex");
    output
        .lines()
        .filter(|l| re.is_match(l))
        .filter(|l| {
            let t = l.trim_start().to_ascii_lowercase();
            !(t.starts_with("npm warn") || t.starts_with("yarn warn") || t.contains(" 0 warnings") || t.starts_with("0 warnings"))
        })
        .map(|l| l.trim().to_string())
        .collect()
}

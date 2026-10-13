//! `cargo_check`: runs `cargo check` and condenses diagnostics to the
//! primary location, message and source line of each error.

use crate::{Res, ToolOutput};
use serde_json::Value;
use std::path::Path;
use tokio::process::Command;

const MAX_ERRORS: usize = 20;

/// One line per error: `error[E0308] src/lib.rs:2:21: mismatched types — label`
/// followed by the offending source line.
pub(crate) fn summarize(json_lines: &str) -> (Vec<String>, usize) {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings = 0;
    for line in json_lines.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v["reason"] != "compiler-message" {
            continue;
        }
        let m = &v["message"];
        let Some(span) = m["spans"].as_array().and_then(|s| s.iter().find(|s| s["is_primary"] == true)) else {
            continue;
        };
        match m["level"].as_str() {
            Some("warning") => warnings += 1,
            Some("error") => {
                let code = m["code"]["code"].as_str().map(|c| format!("[{c}]")).unwrap_or_default();
                let label = span["label"].as_str().map(|l| format!(" — {l}")).unwrap_or_default();
                let src = span["text"][0]["text"].as_str().unwrap_or("").trim();
                let entry = format!(
                    "error{code} {}:{}:{}: {}{label}\n    {src}",
                    span["file_name"].as_str().unwrap_or("?").replace('\\', "/"),
                    span["line_start"],
                    span["column_start"],
                    m["message"].as_str().unwrap_or(""),
                );
                if !errors.contains(&entry) {
                    errors.push(entry);
                }
            }
            _ => {}
        }
    }
    (errors, warnings)
}

pub(crate) async fn check(root: &Path) -> Res {
    let out = Command::new("cargo")
        .args(["check", "--message-format=json", "--quiet", "--all-targets"])
        .current_dir(root)
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| format!("failed to run cargo: {e}"))?;
    let (mut errors, warnings) = summarize(&String::from_utf8_lossy(&out.stdout));
    let warn = if warnings > 0 { format!(" ({warnings} warnings)") } else { String::new() };
    if out.status.success() {
        return Ok(ToolOutput::ok(format!("ok{warn}")));
    }
    if errors.is_empty() {
        // Failure without compiler messages (bad manifest, resolver error, ...).
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(15).collect();
        return Ok(ToolOutput::err(tail.into_iter().rev().collect::<Vec<_>>().join("\n")));
    }
    let total = errors.len();
    errors.truncate(MAX_ERRORS);
    let more = if total > MAX_ERRORS { format!("\n[{} more errors]", total - MAX_ERRORS) } else { String::new() };
    Ok(ToolOutput::err(format!("{total} error(s){warn}\n{}{more}", errors.join("\n"))))
}

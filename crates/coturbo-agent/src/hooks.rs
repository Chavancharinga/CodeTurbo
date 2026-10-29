//! Hooks: the user's own commands, run around the agent's work.
//!
//! - `pre_tool`: before a tool runs. Gets `{"tool", "input"}` as JSON on
//!   stdin; exit code 2 blocks the call, and its message goes to the model.
//! - `post_tool`: after a tool ran. What it prints is added to the result
//!   (a formatter's or linter's verdict, for example).
//! - `task_start` / `task_end`: when a task begins and ends (notify, log...).
//!
//! `matcher` picks tools by name: `*`, `write_file|edit_file`, `bash*`.

use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, PartialEq)]
pub struct Hook {
    pub event: String,
    pub matcher: String,
    pub command: String,
}

/// What a hook decided.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Go,
    /// Exit code 2: do not run the tool; the reason goes to the model.
    Block(String),
}

pub fn matches(matcher: &str, tool: &str) -> bool {
    matcher
        .split('|')
        .map(str::trim)
        .any(|m| m.is_empty() || m == "*" || crate::approval::glob_match(m, tool))
}

const LIMIT: Duration = Duration::from_secs(60);

async fn run(command: &str, root: &Path, stdin: &Value) -> Option<(i32, String)> {
    let use_cmd = cfg!(windows)
        && std::process::Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .output()
            .is_err();
    let mut cmd = if use_cmd {
        tokio::process::Command::new("cmd")
    } else {
        tokio::process::Command::new("sh")
    };
    cmd.arg(if use_cmd { "/C" } else { "-c" })
        .arg(command)
        .current_dir(root);
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().ok()?;
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(stdin.to_string().as_bytes()).await;
    }
    let out = tokio::time::timeout(LIMIT, child.wait_with_output())
        .await
        .ok()?
        .ok()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
    .trim()
    .to_string();
    Some((out.status.code().unwrap_or(-1), text))
}

/// Runs the `pre_tool` hooks for `tool`; the first that blocks wins.
pub async fn before(hooks: &[Hook], root: &Path, tool: &str, input: &Value) -> Verdict {
    for h in hooks
        .iter()
        .filter(|h| h.event == "pre_tool" && matches(&h.matcher, tool))
    {
        if let Some((2, msg)) = run(
            &h.command,
            root,
            &json!({"event": "pre_tool", "tool": tool, "input": input}),
        )
        .await
        {
            return Verdict::Block(if msg.is_empty() {
                format!("blocked by the hook `{}`", h.command)
            } else {
                msg
            });
        }
    }
    Verdict::Go
}

/// Runs the `post_tool` hooks; what they print, to add to the result.
pub async fn after(
    hooks: &[Hook],
    root: &Path,
    tool: &str,
    input: &Value,
    output: &str,
    is_error: bool,
) -> String {
    let mut notes = Vec::new();
    for h in hooks
        .iter()
        .filter(|h| h.event == "post_tool" && matches(&h.matcher, tool))
    {
        let payload = json!({"event": "post_tool", "tool": tool, "input": input, "output": output, "is_error": is_error});
        if let Some((_, msg)) = run(&h.command, root, &payload).await
            && !msg.is_empty()
        {
            notes.push(format!("[hook] {msg}"));
        }
    }
    notes.join("\n")
}

/// Runs the hooks of a task event (`task_start`, `task_end`).
pub async fn event(hooks: &[Hook], root: &Path, event: &str, detail: &Value) {
    for h in hooks.iter().filter(|h| h.event == event) {
        let _ = run(&h.command, root, &json!({"event": event, "detail": detail})).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matchers_pick_tools() {
        assert!(matches("*", "bash"));
        assert!(matches("write_file|edit_file", "edit_file"));
        assert!(matches("mcp__*", "mcp__gh__create_issue"));
        assert!(!matches("write_file", "bash"));
    }
}

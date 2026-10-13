//! `bash` tool: runs a command in the project root with a timeout.

use crate::{Res, ToolOutput, Tools, arg};
use serde_json::Value;
use std::time::Duration;
use tokio::process::Command;

const TIMEOUT: Duration = Duration::from_secs(120);
const MAX_OUTPUT: usize = 30_000;

/// Prefers a POSIX `sh` (Git Bash on Windows); falls back to `cmd /C`.
fn shell(command: &str) -> Command {
    let use_cmd = cfg!(windows) && std::process::Command::new("sh").arg("-c").arg("exit 0").output().is_err();
    let mut c = if use_cmd { Command::new("cmd") } else { Command::new("sh") };
    c.arg(if use_cmd { "/C" } else { "-c" }).arg(command);
    c
}

/// Keeps the tail: errors are usually at the end of build output.
fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("[{} bytes omitted]\n{}", start, &s[start..])
}

impl Tools {
    pub(crate) async fn bash(&self, input: &Value) -> Res {
        let command = arg(input, "command")?;
        let mut cmd = shell(command);
        cmd.current_dir(self.root()).kill_on_drop(true);
        let out = tokio::time::timeout(TIMEOUT, cmd.output())
            .await
            .map_err(|_| format!("timed out after {}s", TIMEOUT.as_secs()))?
            .map_err(|e| format!("failed to start shell: {e}"))?;
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        let text = tail(&text, MAX_OUTPUT);
        Ok(match out.status.code() {
            Some(0) => ToolOutput::ok(text),
            code => ToolOutput::err(format!("exit {}\n{text}", code.map_or("signal".into(), |c| c.to_string()))),
        })
    }
}

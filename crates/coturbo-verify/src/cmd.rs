//! Running checker commands with a timeout.

use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct CmdOutput {
    pub ok: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CmdOutput {
    pub fn combined(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

/// Prefers a POSIX `sh` (Git Bash on Windows); falls back to `cmd /C`.
pub fn shell(command: &str) -> Command {
    let use_cmd =
        cfg!(windows) && std::process::Command::new("sh").arg("-c").arg("exit 0").output().is_err();
    let mut c = if use_cmd { Command::new("cmd") } else { Command::new("sh") };
    c.arg(if use_cmd { "/C" } else { "-c" }).arg(command);
    c
}

pub fn program(prog: &str, args: &[&str]) -> Command {
    let mut c = Command::new(prog);
    c.args(args);
    c
}

/// Runs `cmd` in `root`. A missing program or a timeout is an `Err`.
pub async fn run(mut cmd: Command, root: &Path, timeout: Duration) -> Result<CmdOutput, String> {
    cmd.current_dir(root).kill_on_drop(true);
    let out = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| format!("timed out after {}s", timeout.as_secs()))?
        .map_err(|e| format!("cannot start: {e}"))?;
    Ok(CmdOutput {
        ok: out.status.success(),
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// Last `n` non-empty lines.
pub fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

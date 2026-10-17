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

/// Commands that serve or watch and never end on their own.
pub fn keeps_running(command: &str) -> bool {
    let c = command.to_lowercase();
    ["http.server", "run_server", "runserver", "npm run dev", "npm start", "yarn dev", "pnpm dev", "vite", "next dev", "flask run", "uvicorn", "gunicorn", "nodemon", "live-server", "serve -", "php -s", "jekyll serve", "hugo server", "cargo watch", "--watch"]
        .iter()
        .any(|k| c.contains(k))
        || c.trim_end().ends_with(" serve")
        || c.trim_end().ends_with('&')
}

/// How long a background command is watched before the call returns.
const FIRST_OUTPUT: Duration = Duration::from_secs(4);

impl Tools {
    /// Starts `command` and leaves it running; returns what it printed first.
    async fn background(&self, command: &str) -> Res {
        let log = std::env::temp_dir().join(format!("codeturbo-bg-{}-{}.log", std::process::id(), self.background.lock().map(|b| b.len()).unwrap_or(0)));
        let file = std::fs::File::create(&log).map_err(|e| e.to_string())?;
        let err = file.try_clone().map_err(|e| e.to_string())?;
        let mut cmd = shell(command.trim_end().trim_end_matches('&'));
        cmd.current_dir(self.root()).stdin(std::process::Stdio::null()).stdout(file).stderr(err).kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| format!("failed to start shell: {e}"))?;
        tokio::time::sleep(FIRST_OUTPUT).await;
        let first = tail(&std::fs::read_to_string(&log).unwrap_or_default(), 4_000);
        if let Ok(Some(status)) = child.try_wait() {
            // It ended by itself: an ordinary command after all.
            let text = format!("exited ({status}) within {}s\n{first}", FIRST_OUTPUT.as_secs());
            return Ok(if status.success() { ToolOutput::ok(text) } else { ToolOutput::err(text) });
        }
        let pid = child.id().map(|p| p.to_string()).unwrap_or_else(|| "?".into());
        if let Ok(mut b) = self.background.lock() {
            b.push(child);
        }
        Ok(ToolOutput::ok(format!(
            "running in the background (pid {pid}); it stops when the session ends. Full output: {}\nFirst output:\n{}",
            log.display(),
            if first.trim().is_empty() { "(nothing yet)" } else { first.trim() }
        )))
    }

    pub(crate) async fn bash(&self, input: &Value) -> Res {
        let command = arg(input, "command")?;
        if input["background"].as_bool() == Some(true) || keeps_running(command) {
            return self.background(command).await;
        }
        let mut cmd = shell(command);
        cmd.current_dir(self.root()).kill_on_drop(true);
        let out = tokio::time::timeout(TIMEOUT, cmd.output())
            .await
            .map_err(|_| format!("timed out after {}s; a command that keeps running (a server) needs background=true", TIMEOUT.as_secs()))?
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

#[cfg(test)]
mod tests {
    use super::keeps_running;

    #[test]
    fn servers_are_recognised() {
        for c in ["python -m http.server 8000", "python run_server.py", "npm run dev", "flask run", "php -S localhost:8000", "node app.js &"] {
            assert!(keeps_running(c), "{c}");
        }
        for c in ["cargo test", "python script.py", "ls -la", "npm install"] {
            assert!(!keeps_running(c), "{c}");
        }
    }
}

//! Subagents: the main agent hands a piece of work to a fresh agent with its
//! own clean context, and gets back only its report. Several run at once.
//! `explore`, `plan` and `review` only read; `general` works in its own git
//! worktree (a copy of the project) and hands back a branch.

use crate::{Agent, AgentConfig, Event, Host};
use coturbo_tools::{ToolOutput, Tools};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// One piece of work for a subagent.
#[derive(Debug, Clone)]
pub struct SubTask {
    pub id: String,
    pub kind: String,
    pub description: String,
    pub prompt: String,
    pub worktree: bool,
}

impl SubTask {
    pub fn from_input(id: &str, input: &Value) -> SubTask {
        let kind = input["kind"].as_str().unwrap_or("explore").to_lowercase();
        SubTask {
            id: id.to_string(),
            worktree: kind == "general" || input["worktree"].as_bool() == Some(true),
            kind,
            description: input["description"].as_str().unwrap_or("subtask").to_string(),
            prompt: input["prompt"].as_str().unwrap_or("").to_string(),
        }
    }
}

fn role(kind: &str) -> &'static str {
    match kind {
        "plan" => "\n\nYou are a planning subagent. Read what you need and change nothing. Reply with a concrete step-by-step plan: \
                   the files and functions to change, in order, the risks, and how to verify each step.",
        "review" => "\n\nYou are a code review subagent. Read the code in question and change nothing. Reply with the real defects only \
                     (bugs, broken edge cases, security issues, missing tests), each with file:line, why it is wrong and the fix; say so when there are none.",
        "general" => "\n\nYou are a subagent working in your own copy of the project (a git worktree): change files freely, nothing \
                      reaches the user's copy until they merge your branch. Do the task completely, then reply with what you changed and why.",
        _ => "\n\nYou are a research subagent. Find the answer by reading the code (and the web if needed) and change nothing. \
              Reply with a concise report: the files and lines that matter, what they do, and the direct answer.",
    }
}

/// Collects what the subagent did, for its report; asks nothing (a
/// subagent never waits for the user: what needs approval is refused).
struct Quiet {
    steps: Arc<Mutex<Vec<String>>>,
}

impl Host for Quiet {
    fn on_event(&mut self, ev: Event<'_>) {
        if let Event::ToolEnd { name, input, output, .. } = ev {
            let what = ["path", "pattern", "query", "url", "command"].iter().find_map(|k| input[*k].as_str()).unwrap_or("");
            let mark = if output.is_error { "✗" } else { "✓" };
            if let Ok(mut s) = self.steps.lock() {
                s.push(format!("{mark} {name} {}", what.chars().take(60).collect::<String>()));
            }
        }
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().map_err(|e| format!("git: {e}"))?;
    if out.status.success() { Ok(String::from_utf8_lossy(&out.stdout).trim().to_string()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

/// A new worktree of `root` on its own branch.
fn make_worktree(root: &Path) -> Result<(PathBuf, String), String> {
    git(root, &["rev-parse", "--is-inside-work-tree"]).map_err(|_| "a worktree needs a git repository; run the subagent as explore/plan/review".to_string())?;
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let branch = format!("codeturbo/agent-{stamp}-{n}");
    let path = std::env::temp_dir().join(format!("codeturbo-worktree-{stamp}-{n}"));
    git(root, &["worktree", "add", "-b", &branch, &path.to_string_lossy(), "HEAD"])?;
    Ok((path, branch))
}

/// Commits the subagent's work on its branch and removes the copy.
fn close_worktree(root: &Path, path: &Path, branch: &str) -> String {
    let changed = git(path, &["status", "--porcelain"]).unwrap_or_default();
    let mut note = if changed.is_empty() {
        format!("No files changed; branch {branch} removed.")
    } else {
        let _ = git(path, &["add", "-A"]);
        let _ = git(path, &["-c", "user.name=CodeTurbo", "-c", "user.email=codeturbo@localhost", "commit", "-q", "-m", "work of a CodeTurbo subagent"]);
        let stat = git(root, &["diff", "--stat", &format!("HEAD...{branch}")]).unwrap_or_default();
        format!("Changes are on branch `{branch}` (the user's files are untouched):\n{stat}\nTo bring them in: `git merge {branch}`.")
    };
    let _ = git(root, &["worktree", "remove", "--force", &path.to_string_lossy()]);
    if changed.is_empty() {
        let _ = git(root, &["branch", "-D", branch]);
    }
    if note.is_empty() {
        note.push_str("done");
    }
    note
}

async fn run_one(provider: &dyn coturbo_providers::Provider, tools: &Tools, config: AgentConfig, task: SubTask) -> (String, ToolOutput) {
    if task.prompt.trim().is_empty() {
        return (task.id, ToolOutput::err("the subagent needs a `prompt` with the whole task"));
    }
    let steps = Arc::new(Mutex::new(Vec::new()));
    let mut host = Quiet { steps: steps.clone() };
    let mut config = config;
    config.gate = None;
    config.max_turns = config.max_turns.min(40);
    let read_only = !task.worktree;
    // A copy of the project for a subagent that writes.
    let place = if task.worktree {
        match make_worktree(tools.root()) {
            Ok(p) => Some(p),
            Err(e) => return (task.id, ToolOutput::err(e)),
        }
    } else {
        None
    };
    let own_tools = place.as_ref().map(|(path, _)| Tools::new(path, tools.mode()));
    let mut agent = Agent::new(provider, own_tools.as_ref().unwrap_or(tools), config);
    agent.depth = 1;
    agent.system.push_str(role(&task.kind));
    agent.plan_mode = read_only;
    let result = agent.run_task(&task.prompt, &mut host).await;
    let worktree_note = place.as_ref().map(|(path, branch)| close_worktree(tools.root(), path, branch));
    let steps = steps.lock().map(|s| s.clone()).unwrap_or_default();
    let trail = if steps.is_empty() { String::new() } else { format!("\n\nSteps: {}", steps.join(" · ")) };
    let out = match result {
        Ok(report) => {
            let mut text = format!("[{} · {}] {}", task.kind, task.description, report.final_text.trim());
            if let Some(n) = worktree_note {
                text.push_str(&format!("\n\n{n}"));
            }
            text.push_str(&trail);
            ToolOutput::ok(text)
        }
        Err(e) => ToolOutput::err(format!("the subagent stopped: {e}{trail}")),
    };
    (task.id, out)
}

/// Runs every subtask at the same time; results by tool-call id.
pub async fn run_all(provider: &dyn coturbo_providers::Provider, tools: &Tools, config: &AgentConfig, tasks: Vec<SubTask>) -> Vec<(String, ToolOutput)> {
    futures_util::future::join_all(tasks.into_iter().map(|t| run_one(provider, tools, config.clone(), t))).await
}

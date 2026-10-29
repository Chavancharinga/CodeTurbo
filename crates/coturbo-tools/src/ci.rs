//! `ci`: the GitHub Actions runs of the current branch through the `gh`
//! CLI: their state, the logs of what failed, or waiting for a run to end.

use crate::{Res, ToolOutput};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

async fn run(root: &Path, program: &str, args: &[&str]) -> Result<String, String> {
    let out = tokio::process::Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .await
        .map_err(|e| format!("could not run {program} ({e}); install the GitHub CLI and run `gh auth login`"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

async fn runs(root: &Path, branch: &str) -> Result<Vec<Value>, String> {
    let text = run(root, "gh", &["run", "list", "--branch", branch, "--limit", "6", "--json", "databaseId,status,conclusion,workflowName,displayTitle,createdAt"]).await?;
    Ok(serde_json::from_str::<Value>(&text).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default())
}

fn line(r: &Value) -> String {
    let state = match (r["status"].as_str(), r["conclusion"].as_str()) {
        (Some("completed"), Some(c)) => c.to_string(),
        (Some(s), _) => s.to_string(),
        _ => "?".into(),
    };
    format!("#{} {} · {} · {}", r["databaseId"], r["workflowName"].as_str().unwrap_or("?"), state, r["displayTitle"].as_str().unwrap_or(""))
}

async fn failed_logs(root: &Path, id: &str) -> String {
    let logs = run(root, "gh", &["run", "view", id, "--log-failed"]).await.unwrap_or_default();
    // The end of a failed log holds the error.
    let tail: Vec<&str> = logs.lines().rev().take(120).collect();
    tail.into_iter().rev().map(|l| l.split('\t').next_back().unwrap_or(l)).collect::<Vec<_>>().join("\n")
}

pub(crate) async fn tool(root: &Path, input: &Value) -> Res {
    let branch = match input["branch"].as_str() {
        Some(b) => b.to_string(),
        None => run(root, "git", &["branch", "--show-current"]).await?.trim().to_string(),
    };
    match input["action"].as_str().unwrap_or("status") {
        "logs" => {
            let id = match input["run"].as_u64() {
                Some(id) => id.to_string(),
                None => runs(root, &branch).await?.iter().find(|r| r["conclusion"] == "failure").map(|r| r["databaseId"].to_string()).ok_or("no failed run on this branch")?,
            };
            Ok(ToolOutput::ok(format!("Failed steps of run #{id}:\n{}", failed_logs(root, &id).await)))
        }
        "wait" => {
            // Until the newest run of the branch ends (at most 20 minutes).
            for _ in 0..60 {
                let list = runs(root, &branch).await?;
                let Some(latest) = list.first() else { return Err(format!("no runs on {branch}")) };
                if latest["status"] == "completed" {
                    let mut text = format!("Run finished: {}", line(latest));
                    if latest["conclusion"] == "failure" {
                        text.push_str(&format!("\n\nFailed steps:\n{}", failed_logs(root, &latest["databaseId"].to_string()).await));
                    }
                    return Ok(if latest["conclusion"] == "success" { ToolOutput::ok(text) } else { ToolOutput::err(text) });
                }
                tokio::time::sleep(Duration::from_secs(20)).await;
            }
            Err("the run did not finish in 20 minutes".into())
        }
        _ => {
            let list = runs(root, &branch).await?;
            if list.is_empty() {
                return Ok(ToolOutput::ok(format!("No CI runs on {branch} yet.")));
            }
            Ok(ToolOutput::ok(format!("CI on {branch}:\n{}", list.iter().map(line).collect::<Vec<_>>().join("\n"))))
        }
    }
}

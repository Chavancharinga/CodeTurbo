//! Memory that lasts between sessions, one folder per project: facts about
//! the user, their corrections and preferences, the project's goals and
//! where things live outside the code. One file per fact, plus an index the
//! agent reads at the start of every session.

use crate::{Res, ToolOutput, arg};
use serde_json::Value;
use std::path::{Path, PathBuf};

const INDEX: &str = "MEMORY.md";
/// The index read into every session stays this small.
const INDEX_MAX: usize = 6_000;

/// `<config>/codeturbo/memory/<project>`.
pub fn dir(root: &Path) -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    let project: String = root
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    base.unwrap_or_else(std::env::temp_dir)
        .join("codeturbo")
        .join("memory")
        .join(project.trim_matches('-'))
}

fn slug(s: &str) -> String {
    let s: String = s
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    s.split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// The index, for the system prompt: what was remembered, one line each.
pub fn index(root: &Path) -> String {
    let text = std::fs::read_to_string(dir(root).join(INDEX)).unwrap_or_default();
    let mut cut = text.len().min(INDEX_MAX);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text[..cut].trim().to_string()
}

fn write_index(d: &Path) -> Result<(), String> {
    let mut lines = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(d)
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        if name == INDEX || !name.ends_with(".md") {
            continue;
        }
        let text = std::fs::read_to_string(e.path()).unwrap_or_default();
        let field = |k: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(&format!("{k}: ")))
                .unwrap_or("")
                .to_string()
        };
        lines.push(format!(
            "- [{}] {} ({name}) — {}",
            field("type"),
            field("name"),
            field("description")
        ));
    }
    std::fs::write(d.join(INDEX), lines.join("\n") + "\n").map_err(|e| e.to_string())
}

/// `remember`: saves (or replaces) one fact.
pub(crate) fn remember(root: &Path, input: &Value) -> Res {
    let name = slug(arg(input, "name")?);
    if name.is_empty() {
        return Err("`name` needs letters or digits".into());
    }
    let kind = input["type"].as_str().unwrap_or("project");
    if !matches!(kind, "user" | "feedback" | "project" | "reference") {
        return Err("`type` is user, feedback, project or reference".into());
    }
    let description = arg(input, "description")?.replace('\n', " ");
    let content = arg(input, "content")?;
    let d = dir(root);
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let replaced = d.join(format!("{name}.md")).exists();
    std::fs::write(
        d.join(format!("{name}.md")),
        format!(
            "---\nname: {name}\ndescription: {description}\ntype: {kind}\n---\n\n{}\n",
            content.trim()
        ),
    )
    .map_err(|e| e.to_string())?;
    write_index(&d)?;
    Ok(ToolOutput::ok(format!(
        "{} memory `{name}`",
        if replaced { "Updated" } else { "Saved" }
    )))
}

/// `recall`: one fact in full, or the index.
pub(crate) fn recall(root: &Path, input: &Value) -> Res {
    let d = dir(root);
    match input["name"].as_str().map(slug).filter(|n| !n.is_empty()) {
        Some(n) => std::fs::read_to_string(d.join(format!("{n}.md")))
            .map(ToolOutput::ok)
            .map_err(|_| format!("no memory called `{n}`")),
        None => {
            let i = index(root);
            Ok(ToolOutput::ok(if i.is_empty() {
                "Nothing remembered for this project yet.".to_string()
            } else {
                i
            }))
        }
    }
}

/// `forget`: removes a fact that turned out wrong or no longer applies.
pub(crate) fn forget(root: &Path, input: &Value) -> Res {
    let n = slug(arg(input, "name")?);
    let d = dir(root);
    std::fs::remove_file(d.join(format!("{n}.md")))
        .map_err(|_| format!("no memory called `{n}`"))?;
    write_index(&d)?;
    Ok(ToolOutput::ok(format!("Forgot `{n}`")))
}

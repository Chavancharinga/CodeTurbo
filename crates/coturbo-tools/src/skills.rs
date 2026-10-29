//! Skills and custom slash commands, in CodeTurbo's folders and in Claude
//! Code's (so what the user already has there works here too).
//!
//! - A skill is a folder with a `SKILL.md` (front matter `name` and
//!   `description`, then instructions) and any files it needs; the agent
//!   loads it with the `skill` tool when a task matches its description.
//! - Some skills ship inside CodeTurbo (`builtin/`: Hallmark and
//!   last30days) and are written out to the config folder on first use; a copy
//!   the user installed wins over them, and their mechanisms run natively
//!   (see [`ENGINE_NOTES`]).
//! - A command is a `<name>.md` file; typing `/<name> words` sends its text
//!   as the task, with `$ARGUMENTS` replaced by the words.

use crate::{Res, ToolOutput, arg};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub dir: PathBuf,
    /// Ships with CodeTurbo.
    pub builtin: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    pub name: String,
    pub description: String,
    pub body: String,
}

fn config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("codeturbo"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|c| c.join("codeturbo"))
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

/// The skills embedded in the binary: `(path inside builtin/, text)`.
const BUILTIN: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/builtin.rs"));

/// What CodeTurbo does natively for a built-in skill, put before its text.
pub const ENGINE_NOTES: [(&str, &str); 2] = [
    (
        "last30days",
        "CodeTurbo runs this research natively: call the `last30days` tool (Hacker News, GitHub, Polymarket, dev.to and arXiv, plus Reddit when REDDIT_CLIENT_ID and REDDIT_CLIENT_SECRET are set), then write the brief as below. The Python scripts are not needed; only when they are in this skill's folder and the user wants X, YouTube or TikTok as well, run them as the skill says.",
    ),
    (
        "hallmark",
        "CodeTurbo's design inspector runs Hallmark's static gates after every edit of an interface file (each finding names its gate), and design_review renders the page at desktop and phone widths: use them wherever the skill says sloplint.mjs. The themes are references/themes/<name>.md (list them in references/themes/README.md and read one with skill + file). scripts/*.mjs exist only when the user installed the skill with them and Node is present.",
    ),
];

fn builtin_dir() -> Option<PathBuf> {
    config_dir().map(|c| c.join("builtin-skills"))
}

/// Writes the built-in skills out once per version, so they load like any other.
fn install_builtin() {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| {
        let Some(dir) = builtin_dir() else { return };
        let stamp = format!(
            "{} {}",
            env!("CARGO_PKG_VERSION"),
            BUILTIN.iter().map(|(_, t)| t.len()).sum::<usize>()
        );
        if std::fs::read_to_string(dir.join(".version")).is_ok_and(|v| v == stamp) {
            return;
        }
        let _ = std::fs::remove_dir_all(&dir);
        for (rel, text) in BUILTIN {
            let p = dir.join(rel);
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&p, text);
        }
        let _ = std::fs::write(dir.join(".version"), stamp);
    });
}

/// Where to look, the project's first (it wins on a name clash) and the
/// built-in skills last.
fn places(root: &Path, kind: &str) -> Vec<PathBuf> {
    let mut v = vec![
        root.join(".codeturbo").join(kind),
        root.join(".claude").join(kind),
    ];
    v.extend(config_dir().map(|c| c.join(kind)));
    v.extend(home().map(|h| h.join(".claude").join(kind)));
    if kind == "skills" {
        install_builtin();
        v.extend(builtin_dir());
    }
    v
}

/// `(front matter fields, body)` of a Markdown file.
pub fn front_matter(text: &str) -> (Vec<(String, String)>, String) {
    let t = text.trim_start_matches('\u{feff}');
    let Some(rest) = t.strip_prefix("---") else {
        return (Vec::new(), t.to_string());
    };
    let Some(end) = rest.find("\n---") else {
        return (Vec::new(), t.to_string());
    };
    let fields = rest[..end]
        .lines()
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().trim_matches('"').to_string()))
        .collect();
    (
        fields,
        rest[end + 4..]
            .trim_start_matches(['-', '\r', '\n'])
            .to_string(),
    )
}

fn field(fields: &[(String, String)], k: &str) -> String {
    fields
        .iter()
        .find(|(a, _)| a == k)
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

pub fn skills(root: &Path) -> Vec<Skill> {
    let mut out: Vec<Skill> = Vec::new();
    let builtin = builtin_dir();
    for place in places(root, "skills") {
        for e in std::fs::read_dir(&place).into_iter().flatten().flatten() {
            let file = e.path().join("SKILL.md");
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let (fields, _) = front_matter(&text);
            let name = Some(field(&fields, "name"))
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| e.file_name().to_string_lossy().into_owned());
            if !out.iter().any(|s| s.name == name) {
                let builtin = builtin.as_ref().is_some_and(|b| e.path().starts_with(b))
                    || ENGINE_NOTES.iter().any(|(n, _)| *n == name);
                out.push(Skill {
                    name,
                    description: field(&fields, "description"),
                    dir: e.path(),
                    builtin,
                });
            }
        }
    }
    // The built-in ones first, so the catalogue never cuts them.
    out.sort_by(|a, b| (!a.builtin, &a.name).cmp(&(!b.builtin, &b.name)));
    out
}

pub fn commands(root: &Path) -> Vec<Command> {
    let mut out: Vec<Command> = Vec::new();
    for place in places(root, "commands") {
        for e in std::fs::read_dir(&place).into_iter().flatten().flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("md") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            let name = p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let (fields, body) = front_matter(&text);
            if !out.iter().any(|c| c.name == name) {
                let description = Some(field(&fields, "description"))
                    .filter(|d| !d.is_empty())
                    .unwrap_or_else(|| {
                        body.lines()
                            .find(|l| !l.trim().is_empty())
                            .unwrap_or("")
                            .chars()
                            .take(80)
                            .collect()
                    });
                out.push(Command {
                    name,
                    description,
                    body,
                });
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The task a custom command or skill typed as `/<name> <args>` becomes.
pub fn expand(root: &Path, name: &str, args: &str) -> Option<String> {
    if let Some(c) = commands(root).into_iter().find(|c| c.name == name) {
        return Some(if c.body.contains("$ARGUMENTS") {
            c.body.replace("$ARGUMENTS", args.trim())
        } else {
            format!("{}\n\n{}", c.body.trim(), args.trim())
                .trim()
                .to_string()
        });
    }
    skills(root).into_iter().find(|s| s.name == name).map(|s| {
        format!(
            "Use the skill `{}` (load it with the skill tool first). {}",
            s.name,
            args.trim()
        )
        .trim()
        .to_string()
    })
}

/// The list for the system prompt.
pub fn catalogue(root: &Path) -> String {
    // A short line each, and not hundreds of them: this goes into every request.
    let all = skills(root);
    let mut list: Vec<String> = all
        .iter()
        .take(40)
        .map(|s| {
            format!(
                "- {}: {}",
                s.name,
                s.description.chars().take(140).collect::<String>()
            )
        })
        .collect();
    if list.is_empty() {
        return String::new();
    }
    if all.len() > 40 {
        list.push(format!(
            "- and {} more: ask the skill tool for one by name",
            all.len() - 40
        ));
    }
    format!(
        "\n\nSkills installed (instructions for particular kinds of work). When a task matches one, load it with the skill tool before starting and follow it:\n{}",
        list.join("\n")
    )
}

/// `skill`: a skill's instructions, or one of its files.
pub(crate) fn load(root: &Path, input: &Value) -> Res {
    let name = arg(input, "name")?;
    let s = skills(root)
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("no skill called `{name}`"))?;
    if let Some(f) = input["file"].as_str() {
        let p = s.dir.join(f);
        if !p.starts_with(&s.dir) || f.contains("..") {
            return Err("the file must be inside the skill's folder".into());
        }
        return std::fs::read_to_string(&p)
            .map(ToolOutput::ok)
            .map_err(|e| format!("{f}: {e}"));
    }
    let text = std::fs::read_to_string(s.dir.join("SKILL.md")).map_err(|e| e.to_string())?;
    let (_, body) = front_matter(&text);
    let mut files: Vec<String> = Vec::new();
    for e in walk(&s.dir) {
        if let Ok(rel) = e.strip_prefix(&s.dir)
            && rel != Path::new("SKILL.md")
        {
            files.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    let extra = if files.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nFiles of this skill (read one with skill + file): {}",
            files.join(", ")
        )
    };
    let note = ENGINE_NOTES
        .iter()
        .find(|(n, _)| *n == s.name)
        .map(|(_, t)| format!("> In CodeTurbo: {t}\n\n"))
        .unwrap_or_default();
    Ok(ToolOutput::ok(format!(
        "# Skill: {}\n\n{note}{}{extra}",
        s.name,
        body.trim()
    )))
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            if out.len() < 200 {
                out.extend(walk(&p));
            }
        } else {
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_and_commands_from_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let s = dir.path().join(".claude/skills/release");
        std::fs::create_dir_all(s.join("scripts")).unwrap();
        std::fs::write(s.join("SKILL.md"), "---\nname: release\ndescription: Cut a release with a changelog\n---\n\n1. Bump the version.\n").unwrap();
        std::fs::write(s.join("scripts/bump.sh"), "echo bump").unwrap();
        let c = dir.path().join(".codeturbo/commands");
        std::fs::create_dir_all(&c).unwrap();
        std::fs::write(
            c.join("fix-issue.md"),
            "---\ndescription: Fix a GitHub issue\n---\nFix issue #$ARGUMENTS and add a test.",
        )
        .unwrap();
        let root = dir.path();
        assert!(
            skills(root)
                .iter()
                .any(|s| s.name == "release" && s.description.contains("changelog"))
        );
        assert!(catalogue(root).contains("- release: Cut a release with a changelog"));
        assert_eq!(
            expand(root, "fix-issue", "42").as_deref(),
            Some("Fix issue #42 and add a test.")
        );
        assert!(
            expand(root, "release", "v2")
                .unwrap()
                .contains("skill `release`")
        );
        let loaded = load(root, &serde_json::json!({"name": "release"}))
            .unwrap()
            .content;
        assert!(
            loaded.contains("Bump the version") && loaded.contains("scripts/bump.sh"),
            "{loaded}"
        );
        let script = load(
            root,
            &serde_json::json!({"name": "release", "file": "scripts/bump.sh"}),
        )
        .unwrap()
        .content;
        assert_eq!(script, "echo bump");
        assert!(
            load(
                root,
                &serde_json::json!({"name": "release", "file": "../../x"})
            )
            .is_err()
        );
    }

    #[test]
    fn the_built_in_skills_ship_with_their_native_notes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let all = skills(root);
        for n in ["hallmark", "last30days"] {
            assert!(all.iter().any(|s| s.name == n && s.builtin), "{n} missing");
        }
        assert!(catalogue(root).contains("- hallmark: Design skill"));
        let l = load(root, &serde_json::json!({"name": "last30days"}))
            .unwrap()
            .content;
        assert!(
            l.contains("In CodeTurbo: CodeTurbo runs this research natively"),
            "{}",
            &l[..300]
        );
        // Hallmark's themes are readable files.
        let g = load(
            root,
            &serde_json::json!({"name": "hallmark", "file": "references/gates.md"}),
        )
        .unwrap()
        .content;
        assert!(g.contains("# Gates"));
        assert!(
            BUILTIN
                .iter()
                .any(|(p, _)| p.starts_with("hallmark/references/themes/") && p.ends_with(".md"))
        );
        assert!(
            BUILTIN
                .iter()
                .filter(|(p, _)| p.ends_with("LICENSE"))
                .count()
                >= 2
        );
    }
}

//! The project's knowledge graph, when graphify (`pip install graphifyy`) is
//! installed: [`refresh`] rebuilds it in the background at session start
//! (code only, from the syntax tree: no model, no tokens), and [`note`] tells
//! the model to ask the graph before searching files.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The folder graphify writes.
pub const OUT: &str = "graphify-out";
/// Sessions started closer together than this reuse the graph as it is.
const THROTTLE: Duration = Duration::from_secs(120);

fn graphify() -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) { &["graphify.exe", "graphify.cmd", "graphify"] } else { &["graphify"] };
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
}

fn recent(p: &Path) -> bool {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok()).is_some_and(|age| age < THROTTLE)
}

/// Keeps the graph's output out of git without touching the project's files.
fn exclude(root: &Path) {
    let file = root.join(".git").join("info").join("exclude");
    if !root.join(".git").is_dir() {
        return;
    }
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    if text.contains(OUT) {
        return;
    }
    let sep = if text.is_empty() || text.ends_with('\n') { "" } else { "\n" };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&file, format!("{text}{sep}{OUT}/\n"));
}

/// Starts `graphify update` for a git project, detached, so the session never
/// waits for it. Does nothing without graphify, outside git, when run
/// moments ago, or with `CODETURBO_NO_GRAPH` set.
pub fn refresh(root: &Path) {
    if std::env::var_os("CODETURBO_NO_GRAPH").is_some() || !root.join(".git").exists() {
        return;
    }
    let Some(exe) = graphify() else { return };
    let stamp = root.join(OUT).join(".codeturbo-refresh");
    if recent(&stamp) || recent(&root.join(OUT).join("graph.json")) {
        return;
    }
    exclude(root);
    let _ = std::fs::create_dir_all(root.join(OUT));
    let _ = std::fs::write(&stamp, "");
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("update").arg(root).current_dir(root).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NO_WINDOW
        cmd.creation_flags(0x0000_0008 | 0x0800_0000);
    }
    let _ = cmd.spawn();
}

/// For the system prompt, when the project has a graph.
pub fn note(root: &Path) -> String {
    if !root.join(OUT).join("GRAPH_REPORT.md").is_file() {
        return String::new();
    }
    format!(
        "\n\nKnowledge graph: this project has one in {OUT}/ (kept fresh automatically). Before searching or reading many files \
         to understand how the code fits together, read {OUT}/GRAPH_REPORT.md (central modules and communities) or ask it through \
         bash: `graphify query \"<question>\"`, `graphify path \"<A>\" \"<B>\"`, `graphify explain \"<symbol>\"`, \
         `graphify affected \"<symbol>\"` (what a change would touch). Then read only the lines it points to."
    )
}

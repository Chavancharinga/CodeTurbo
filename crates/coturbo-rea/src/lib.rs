//! Reverse engineering built into CodeTurbo: study how existing software
//! does something, from evidence, to explain it or build something like
//! it. Native Rust, read-only, no external tools: websites (rendered in the
//! installed browser), JavaScript and Electron apps (folders, `.asar`,
//! bundles) and native binaries (PE, ELF, Mach-O, WebAssembly). If the
//! external REA is installed, the agent can still call it through bash.

pub mod apk;
pub mod binary;
pub mod disasm;
pub mod js;
pub mod site;
pub mod sqlite;

use std::path::{Path, PathBuf};

/// The report of an investigation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub text: String,
    /// Screenshots taken (websites), to show the model when it sees images.
    pub images: Vec<PathBuf>,
}

/// Words of the question worth searching for (3+ letters, lower case).
pub fn terms(question: &str) -> Vec<String> {
    const STOP: [&str; 24] = [
        "the", "and", "how", "what", "does", "this", "that", "with", "como", "que", "para", "uma", "dos", "das", "por",
        "funciona", "faz", "isso", "esse", "essa", "onde", "qual", "from", "into",
    ];
    let mut out: Vec<String> = question
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .map(str::to_lowercase)
        .filter(|w| w.chars().count() >= 3 && !STOP.contains(&w.as_str()))
        .collect();
    out.dedup();
    out.truncate(8);
    out
}

/// Investigates `target` (a URL, or a path relative to `root` or absolute)
/// with `question` in mind.
pub async fn investigate(root: &Path, target: &str, question: &str) -> Result<Report, String> {
    let t = target.trim();
    if t.starts_with("http://") || t.starts_with("https://") {
        return site::inspect(root, t, question).await;
    }
    let path = if Path::new(t).is_absolute() { PathBuf::from(t) } else { root.join(t) };
    if path.is_dir() {
        return Ok(Report { text: js::inspect_dir(&path, question), images: Vec::new() });
    }
    let data = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let lower = path.to_string_lossy().to_lowercase();
    let text = if lower.ends_with(".asar") {
        js::inspect_asar(&data, question)?
    } else if [".js", ".mjs", ".cjs", ".jsx", ".ts", ".tsx"].iter().any(|e| lower.ends_with(e)) {
        js::inspect_files(&[(path.to_string_lossy().into_owned(), String::from_utf8_lossy(&data).into_owned())], None, question)
    } else if lower.ends_with(".apk") {
        apk::inspect(&data, question)?
    } else if data.len() >= 16 && &data[..16] == b"SQLite format 3\0" {
        sqlite::inspect(&data)?
    } else {
        // A native binary: static facts, plus the disassembly of the
        // functions related to the question when the symbols allow it.
        let mut text = binary::inspect(&data, question)?;
        if let Ok(file) = object::File::parse(&data[..]) {
            let code = disasm::functions(&file, question);
            if !code.is_empty() {
                text.push_str("\n\n");
                text.push_str(&code);
            }
        }
        text
    };
    Ok(Report { text, images: Vec::new() })
}

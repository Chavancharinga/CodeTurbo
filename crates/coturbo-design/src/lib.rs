//! The CodeTurbo design method, in code: the model writes a brief, the
//! [`compiler`] turns it into tokens locked in a [`contract`], the
//! [`fiscal`] rejects generic patterns after every edit, and [`render`]
//! measures the page in the installed browser. The model keeps the ideas;
//! the code keeps it away from the average.

pub mod color;
pub mod compiler;
pub mod contract;
pub mod fiscal;
pub mod kit;
pub mod render;

use contract::Contract;
use fiscal::Finding;
use std::path::Path;

/// Result of reviewing a task's interface files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    pub ok: bool,
    pub summary: String,
}

/// Checks the interface files a task changed: the static rules on each,
/// and, for HTML pages, the rendered measurements. `None` when none of
/// `files` is an interface file.
pub async fn review(root: &Path, files: &[String]) -> Option<Review> {
    let ui: Vec<&String> = files.iter().filter(|f| fiscal::is_ui_file(f) && root.join(f).is_file()).collect();
    if ui.is_empty() {
        return None;
    }
    let contract = Contract::load(root);
    let browser = render::find_browser();
    let mut blocking = 0;
    let mut report = Vec::new();
    let mut rendered = 0;
    for rel in ui {
        let Ok(text) = std::fs::read_to_string(root.join(rel)) else { continue };
        let mut findings: Vec<Finding> = fiscal::check(rel, &text, contract.as_ref());
        let lower = rel.to_ascii_lowercase();
        if let Some(b) = &browser
            && (lower.ends_with(".html") || lower.ends_with(".htm"))
        {
            let stem = Path::new(rel).file_stem().map_or("page".into(), |s| s.to_string_lossy().into_owned());
            let shot = root.join(".codeturbo/design").join(format!("{stem}.png"));
            if let Ok(m) = render::measure(b, &root.join(rel), &shot).await {
                rendered += 1;
                findings.extend(render::judge(&m));
            }
        }
        blocking += findings.iter().filter(|f| f.blocking()).count();
        if !findings.is_empty() {
            report.push(fiscal::render(rel, &findings));
        }
    }
    let how = if rendered > 0 { "regras e página renderizada" } else { "regras" };
    let summary = if blocking == 0 {
        let mut s = format!("✓ design aprovado ({how})");
        if !report.is_empty() {
            s.push('\n');
            s.push_str(&report.join("\n"));
        }
        s
    } else {
        format!(
            "✗ design recusado ({how}): {blocking} problema(s) bloqueante(s). Corrija mantendo o contrato do DESIGN.md.\n{}",
            report.join("\n")
        )
    };
    Some(Review { ok: blocking == 0, summary })
}

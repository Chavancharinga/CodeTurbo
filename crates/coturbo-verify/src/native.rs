//! Diagnostics computed inside CoTurbo, in Rust, with no language server:
//! Rust (rust-analyzer's parser), JS/TS/JSX/TSX (oxc parser + semantic
//! checks), CSS (lightningcss), JSON and TOML. Instant and always available;
//! an external language server, when installed, adds type-level checks.

use crate::diag::{Diag, Level};
use std::sync::{Arc, RwLock};

/// `(line, col)`, both 1-based, of a byte offset.
fn position(text: &str, offset: usize) -> (u32, u32) {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let line = before.matches('\n').count() as u32 + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) as u32 + 1;
    (line, col)
}

fn diag(rel: &str, text: &str, level: Level, line: u32, col: u32, message: String) -> Diag {
    Diag {
        level,
        path: rel.to_string(),
        line,
        col,
        message: message.lines().next().unwrap_or("").to_string(),
        code: None,
        source: text.lines().nth(line.saturating_sub(1) as usize).map(str::to_string),
    }
}

fn at(rel: &str, text: &str, level: Level, offset: usize, message: String) -> Diag {
    let (line, col) = position(text, offset);
    diag(rel, text, level, line, col, message)
}

fn rust(rel: &str, text: &str) -> Vec<Diag> {
    use ra_ap_syntax::{Edition, SourceFile};
    SourceFile::parse(text, Edition::Edition2024)
        .errors()
        .iter()
        .map(|e| at(rel, text, Level::Error, usize::from(e.range().start()), e.to_string()))
        .collect()
}

fn js(rel: &str, text: &str) -> Vec<Diag> {
    use oxc_allocator::Allocator;
    use oxc_parser::Parser;
    use oxc_semantic::SemanticBuilder;
    use oxc_span::SourceType;
    let source_type = SourceType::from_path(rel).unwrap_or_default();
    let alloc = Allocator::default();
    let parsed = Parser::new(&alloc, text, source_type).parse();
    let mut errors: Vec<_> = parsed.diagnostics.iter().cloned().collect();
    if !parsed.panicked {
        errors.extend(SemanticBuilder::new().with_check_syntax_error(true).build(&parsed.program).diagnostics.iter().cloned());
    }
    errors
        .iter()
        .map(|e| {
            // The primary label marks the offending spot (e.g. the redeclaration,
            // not the original declaration); otherwise the last one.
            let label = e.labels.iter().find(|l| l.primary()).or_else(|| e.labels.last());
            let offset = label.map_or(0, |l| l.offset());
            // miette's Severity is not re-exported by oxc: compare by name.
            let level = if format!("{:?}", e.severity) == "Error" { Level::Error } else { Level::Warning };
            at(rel, text, level, offset as usize, e.message.to_string())
        })
        .collect()
}

fn css(rel: &str, text: &str) -> Vec<Diag> {
    use lightningcss::stylesheet::{ParserOptions, StyleSheet};
    let warnings = Arc::new(RwLock::new(Vec::new()));
    let opts = ParserOptions {
        filename: rel.to_string(),
        error_recovery: true,
        warnings: Some(warnings.clone()),
        ..ParserOptions::default()
    };
    let mut out = Vec::new();
    let to_diag = |level: Level, loc: Option<lightningcss::error::ErrorLocation>, msg: String| {
        let (line, col) = loc.map_or((1, 1), |l| (l.line + 1, l.column));
        diag(rel, text, level, line, col, msg)
    };
    if let Err(e) = StyleSheet::parse(text, opts) {
        out.push(to_diag(Level::Error, e.loc.clone(), e.kind.to_string()));
    }
    for w in warnings.read().map(|w| w.clone()).unwrap_or_default() {
        out.push(to_diag(Level::Warning, w.loc.clone(), w.kind.to_string()));
    }
    out
}

fn json(rel: &str, text: &str) -> Vec<Diag> {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(_) => Vec::new(),
        Err(e) => vec![diag(rel, text, Level::Error, e.line() as u32, e.column() as u32, e.to_string())],
    }
}

fn toml_file(rel: &str, text: &str) -> Vec<Diag> {
    match toml::from_str::<toml::Value>(text) {
        Ok(_) => Vec::new(),
        Err(e) => {
            let offset = e.span().map_or(0, |s| s.start);
            vec![at(rel, text, Level::Error, offset, e.message().to_string())]
        }
    }
}

/// Diagnostics for a file, or `None` when its language is not covered.
pub fn check(rel: &str, text: &str) -> Option<Vec<Diag>> {
    let ext = rel.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    Some(match ext.as_str() {
        "rs" => rust(rel, text),
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts" => js(rel, text),
        "css" => css(rel, text),
        "json" => json(rel, text),
        "toml" => toml_file(rel, text),
        _ => return None,
    })
}

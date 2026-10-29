//! HTML in Rust, no external tools: formatting (so a page written on one
//! line becomes readable), syntax errors, the embedded `<style>` and
//! `<script>` checked by the CSS and JS checkers at their real line numbers,
//! and warnings for what every page needs (doctype, charset, viewport, lang,
//! image alt text).

use crate::diag::{Diag, Level};
use crate::native;
use markup_fmt::config::FormatOptions;
use markup_fmt::{FormatError, Language};
use std::borrow::Cow;

/// Pretty-prints embedded CSS that has no comments (the CSS printer drops
/// them, so commented CSS is left as written).
fn format_css(code: &str) -> Option<String> {
    use lightningcss::stylesheet::{ParserOptions, PrinterOptions, StyleSheet};
    if code.contains("/*") {
        return None;
    }
    let sheet = StyleSheet::parse(code, ParserOptions::default()).ok()?;
    sheet.to_css(PrinterOptions::default()).ok().map(|r| r.code)
}

/// The page formatted, or `None` if it does not parse.
pub fn format(text: &str) -> Option<String> {
    markup_fmt::format_text(
        text,
        Language::Html,
        &FormatOptions::default(),
        |code, hints| {
            let out = match hints.ext {
                "css" => format_css(code)
                    .map(Cow::Owned)
                    .unwrap_or(Cow::Borrowed(code)),
                _ => Cow::Borrowed(code),
            };
            Ok(out)
        },
    )
    .ok()
}

/// `(line, col)`, both 1-based, of a byte offset.
fn position(text: &str, offset: usize) -> (u32, u32) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() as u32 + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) as u32 + 1;
    (line, col)
}

fn warn(rel: &str, text: &str, offset: usize, message: &str) -> Diag {
    let (line, col) = position(text, offset);
    Diag {
        level: Level::Warning,
        path: rel.to_string(),
        line,
        col,
        message: message.to_string(),
        code: None,
        source: text.lines().nth(line as usize - 1).map(str::to_string),
    }
}

/// Embedded blocks as `(language extension, body start offset, body)`.
/// Scripts with a `src` or a non-JavaScript `type` are skipped.
fn embedded(text: &str) -> Vec<(&'static str, usize, &str)> {
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    for (tag, ext) in [("style", "css"), ("script", "js")] {
        let (open, close) = (format!("<{tag}"), format!("</{tag}"));
        let mut from = 0;
        while let Some(i) = lower[from..].find(&open).map(|i| from + i) {
            let Some(gt) = lower[i..].find('>').map(|g| i + g) else {
                break;
            };
            let attrs = &lower[i + open.len()..gt];
            let Some(end) = lower[gt..].find(&close).map(|e| gt + e) else {
                break;
            };
            let js_type = !attrs.contains("type=")
                || ["module", "javascript", "text/babel"]
                    .iter()
                    .any(|t| attrs.contains(t));
            if ext == "css" || (!attrs.contains("src=") && js_type) {
                out.push((ext, gt + 1, &text[gt + 1..end]));
            }
            from = end + close.len();
        }
    }
    out
}

/// Errors and warnings for an HTML page.
pub fn check(rel: &str, text: &str) -> Vec<Diag> {
    let mut out = Vec::new();
    let parsed = markup_fmt::format_text(
        text,
        Language::Html,
        &FormatOptions::default(),
        |code, _| Ok(Cow::Borrowed(code)),
    );
    if let Err(FormatError::Syntax(e)) = parsed {
        out.push(Diag {
            level: Level::Error,
            path: rel.to_string(),
            line: e.line as u32,
            col: e.column as u32,
            message: e.kind.to_string(),
            code: None,
            source: text
                .lines()
                .nth(e.line.saturating_sub(1))
                .map(str::to_string),
        });
    }
    for (ext, start, body) in embedded(text) {
        let fake = format!("{rel}.{ext}");
        let (line0, col0) = position(text, start);
        for mut d in native::check(&fake, body).unwrap_or_default() {
            if d.line == 1 {
                d.col += col0 - 1;
            }
            d.line += line0 - 1;
            d.path = rel.to_string();
            d.source = text.lines().nth(d.line as usize - 1).map(str::to_string);
            out.push(d);
        }
    }
    let lower = text.to_ascii_lowercase();
    if !lower.trim_start().starts_with("<!doctype html") {
        out.push(warn(rel, text, 0, "missing <!doctype html>"));
    }
    if !lower.contains("<meta charset") {
        out.push(warn(
            rel,
            text,
            lower.find("<head").unwrap_or(0),
            "missing <meta charset=\"utf-8\">",
        ));
    }
    if !lower.contains("name=\"viewport\"") && !lower.contains("name=viewport") {
        out.push(warn(
            rel,
            text,
            lower.find("<head").unwrap_or(0),
            "missing <meta name=\"viewport\">: the page won't scale on phones",
        ));
    }
    if let Some(i) = lower.find("<html")
        && !lower[i..].split('>').next().unwrap_or("").contains("lang=")
    {
        out.push(warn(rel, text, i, "<html> without lang"));
    }
    let mut from = 0;
    while let Some(i) = lower[from..].find("<img").map(|i| from + i) {
        let tag = lower[i..].split('>').next().unwrap_or("");
        if !tag.contains("alt=") {
            out.push(warn(rel, text, i, "<img> without alt text"));
        }
        from = i + 4;
    }
    out
}

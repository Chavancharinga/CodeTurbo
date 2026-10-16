//! O depurador: de um relatório de erros (compilação, análise, testes) mostra
//! exatamente em qual linha de qual arquivo está o problema, com o trecho do
//! código, a linha em vermelho e uma seta na coluna.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::path::Path;

const RED: Color = Color::Rgb(255, 92, 92);
const RED_BG: Color = Color::Rgb(74, 18, 22);
const YELLOW: Color = Color::Rgb(240, 190, 80);
const DIM: Color = Color::Rgb(110, 125, 150);
const MAX_FINDINGS: usize = 4;

/// One problem pointing at a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub error: bool,
    pub path: String,
    pub line: u32,
    pub col: u32,
    pub message: String,
}

/// `path:line:col` split from the right, so a drive letter's colon is safe.
fn location(s: &str) -> Option<(String, u32, u32)> {
    let s = s.trim().trim_end_matches(':');
    let (rest, col) = s.rsplit_once(':')?;
    let (path, line) = rest.rsplit_once(':')?;
    Some((path.trim().to_string(), line.trim().parse().ok()?, col.trim().parse().ok()?))
}

/// The problems named in `text`: compiler and linter lines
/// (`error[E0308] src/a.rs:4:9: message`) and test panics
/// (`panicked at src/a.rs:4:9:`).
pub fn parse(text: &str) -> Vec<Finding> {
    let mut out: Vec<Finding> = Vec::new();
    let mut push = |f: Finding| {
        if !out.iter().any(|o| o.path == f.path && o.line == f.line) {
            out.push(f);
        }
    };
    let lines: Vec<&str> = text.lines().collect();
    for (i, raw) in lines.iter().enumerate() {
        let l = raw.trim_start().trim_start_matches("- ");
        let level = if l.starts_with("error") {
            Some(true)
        } else if l.starts_with("warning") {
            Some(false)
        } else {
            None
        };
        if let Some(error) = level {
            // After the level and an optional [code], up to `path:line:col: `.
            let after = &l[if error { 5 } else { 7 }..];
            let after = match after.strip_prefix('[') {
                Some(rest) => rest.split_once(']').map_or("", |(_, r)| r),
                None => after,
            };
            let after = after.trim_start();
            let at = after.char_indices().find_map(|(i, c)| {
                (c == ':').then(|| &after[i..]).filter(|t| {
                    let d: String = t[1..].chars().take_while(char::is_ascii_digit).collect();
                    !d.is_empty() && t[1 + d.len()..].starts_with(':')
                }).map(|_| i)
            });
            if let Some(i) = at
                && let Some(sep) = after[i..].find(": ")
                && let Some((path, line, col)) = location(&after[..i + sep])
            {
                push(Finding { error, path, line, col, message: after[i + sep + 2..].trim().to_string() });
                continue;
            }
        }
        if let Some(p) = l.split("panicked at ").nth(1)
            && let Some((path, line, col)) = location(p)
        {
            let message = lines.get(i + 1).map_or(String::new(), |m| m.trim().to_string());
            push(Finding { error: true, path, line, col, message: format!("panicked: {message}").trim_end_matches(": ").to_string() });
        }
    }
    out
}

fn expand(s: &str) -> String {
    s.replace('\t', "    ")
}

/// Cuts `s` to `width` columns.
fn fit(s: &str, width: usize) -> String {
    let s = expand(s);
    if s.chars().count() <= width {
        s
    } else {
        format!("{}…", s.chars().take(width.saturating_sub(1)).collect::<String>())
    }
}

/// The debugger panel for `findings`, reading the code from `root`.
pub fn render(findings: &[Finding], root: &Path, width: usize) -> Vec<Line<'static>> {
    if findings.is_empty() {
        return Vec::new();
    }
    let width = width.max(30);
    let errors: Vec<u32> = findings.iter().filter(|f| f.error).map(|f| f.line).collect();
    let all: Vec<u32> = findings.iter().map(|f| f.line).collect();
    let (kind, lines) = if errors.is_empty() { ("avisos", &all) } else { ("erros", &errors) };
    let list = lines.iter().take(8).map(u32::to_string).collect::<Vec<_>>().join(", ");
    let header_color = if errors.is_empty() { YELLOW } else { RED };
    let mut out = vec![
        Line::default(),
        Line::from(vec![
            Span::styled(" DEBUGGER ", Style::new().fg(Color::Black).bg(header_color).add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {kind} nas linhas {list}"), Style::new().fg(header_color).add_modifier(Modifier::BOLD)),
        ]),
    ];
    for f in findings.iter().take(MAX_FINDINGS) {
        let color = if f.error { RED } else { YELLOW };
        out.push(Line::styled(format!("  {}:{}:{}", f.path, f.line, f.col), Style::new().fg(DIM)));
        let file = if Path::new(&f.path).is_absolute() { Path::new(&f.path).to_path_buf() } else { root.join(&f.path) };
        let src = std::fs::read_to_string(file).ok();
        let code: Vec<&str> = src.as_deref().map(|s| s.lines().collect()).unwrap_or_default();
        let at = f.line as usize;
        let gutter = format!("{}", at + 2).len().max(3);
        let room = width.saturating_sub(gutter + 9);
        if code.len() >= at && at > 0 {
            for n in at.saturating_sub(2).max(1)..=(at + 1).min(code.len()) {
                let text = fit(code[n - 1], room);
                if n == at {
                    out.push(Line::from(vec![
                        Span::styled(format!("  ▶ {n:>gutter$} │ "), Style::new().fg(color).add_modifier(Modifier::BOLD)),
                        Span::styled(format!("{text:<room$}"), Style::new().fg(Color::White).bg(RED_BG)),
                    ]));
                    // The arrow under the column, then what is wrong.
                    let col = (f.col as usize).saturating_sub(1).min(room.saturating_sub(1));
                    let col = expand(&code[n - 1].chars().take(col).collect::<String>()).chars().count().min(room.saturating_sub(1));
                    out.push(Line::from(vec![
                        Span::styled(format!("    {:>gutter$} │ ", ""), Style::new().fg(DIM)),
                        Span::styled(format!("{}^ {}", " ".repeat(col), fit(&f.message, room.saturating_sub(col + 2))), Style::new().fg(color).add_modifier(Modifier::BOLD)),
                    ]));
                } else {
                    out.push(Line::styled(format!("    {n:>gutter$} │ {text}"), Style::new().fg(DIM)));
                }
            }
        } else {
            // The file is not there to read: at least the message.
            out.push(Line::styled(format!("    {}", fit(&f.message, width.saturating_sub(6))), Style::new().fg(color)));
        }
    }
    if findings.len() > MAX_FINDINGS {
        out.push(Line::styled(format!("  … e mais {} problema(s)", findings.len() - MAX_FINDINGS), Style::new().fg(DIM)));
    }
    out
}

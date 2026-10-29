//! What the terminal shows for agent output: line-numbered red/green diffs,
//! the per-turn token line (tokens, tok/s, context, cost, rate limits).
//! Pure functions so they can be tested; `status::TermHost` prints them.

use crate::status::human;
use coturbo_providers::Usage;

const MAX_DIFF_LINES: usize = 80;

struct Style {
    num: (u8, u8, u8),
    bg: (u8, u8, u8),
    fg: (u8, u8, u8),
}

const REMOVED: Style = Style { num: (255, 95, 95), bg: (70, 18, 24), fg: (255, 205, 205) };
const ADDED: Style = Style { num: (95, 215, 120), bg: (16, 58, 32), fg: (205, 255, 215) };

fn clip(s: &str, max: usize) -> String {
    let s = s.replace('\t', "    ");
    if s.chars().count() <= max { s } else { s.chars().take(max.saturating_sub(1)).chain(['…']).collect() }
}

/// `@@ -12,7 +12,8 @@` → (12, 12).
fn hunk_start(line: &str) -> Option<(usize, usize)> {
    let mut parts = line.split_whitespace().skip(1);
    let num = |p: Option<&str>| p?.trim_start_matches(['-', '+']).split(',').next()?.parse().ok();
    Some((num(parts.next())?, num(parts.next())?))
}

/// A unified diff as numbered lines: removed lines in red with their old
/// number, added lines in green with their new number, context dimmed.
/// `width` is the row width the colored background fills; `color` off
/// gives plain text.
pub fn render_diff(diff: &str, width: usize, color: bool) -> Vec<String> {
    render_diff_upto(diff, width, color, MAX_DIFF_LINES)
}

/// [`render_diff`] showing at most `max` changed lines.
pub fn render_diff_upto(diff: &str, width: usize, color: bool, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let (mut old, mut new) = (0usize, 0usize);
    let mut hunks = 0;
    let text_w = width.saturating_sub(9).max(20);
    let mut shown = 0;
    let mut hidden = 0;
    for line in diff.lines() {
        if line.starts_with("---") || line.starts_with("+++") || line.starts_with('\\') {
            continue;
        }
        if line.starts_with("@@") {
            if let Some((o, n)) = hunk_start(line) {
                (old, new) = (o, n);
            }
            if hunks > 0 && shown < max {
                out.push(if color { "\x1b[2m      ⋮\x1b[0m".into() } else { "      ⋮".into() });
            }
            hunks += 1;
            continue;
        }
        let (sign, body) = line.split_at(line.chars().next().map_or(0, char::len_utf8));
        let (num, style) = match sign {
            "-" => {
                old += 1;
                (old - 1, Some(&REMOVED))
            }
            "+" => {
                new += 1;
                (new - 1, Some(&ADDED))
            }
            _ => {
                old += 1;
                new += 1;
                (new - 1, None)
            }
        };
        if shown >= max {
            hidden += 1;
            continue;
        }
        shown += 1;
        let text = clip(body, text_w);
        let sign = if sign.is_empty() { " " } else { sign };
        let row = format!("{num:>5} {sign} {text}");
        out.push(match (color, style) {
            (false, _) => row,
            (true, None) => format!("\x1b[2m{row}\x1b[0m"),
            (true, Some(s)) => {
                let pad = width.saturating_sub(row.chars().count());
                let (n, b, f) = (s.num, s.bg, s.fg);
                format!(
                    "\x1b[48;2;{};{};{}m\x1b[38;2;{};{};{}m{num:>5} {sign}\x1b[38;2;{};{};{}m {text}{}\x1b[0m",
                    b.0, b.1, b.2, n.0, n.1, n.2, f.0, f.1, f.2, " ".repeat(pad)
                )
            }
        });
    }
    if hidden > 0 {
        let more = format!("      … {hidden} more diff lines");
        out.push(if color { format!("\x1b[2m{more}\x1b[0m") } else { more });
    }
    out
}

/// `+12 -3` for a diff.
pub fn diff_stat(diff: &str) -> (usize, usize) {
    let lines = diff.lines().filter(|l| !l.starts_with("+++") && !l.starts_with("---"));
    lines.fold((0, 0), |(a, r), l| match l.as_bytes().first() {
        Some(b'+') => (a + 1, r),
        Some(b'-') => (a, r + 1),
        _ => (a, r),
    })
}

/// Compact rate limits from response headers, e.g. `requests 49/50 · tokens 78.0k/80.0k`.
/// Pairs each `…remaining…` header with its `…limit…` twin.
pub fn limits_summary(headers: &[(String, String)]) -> Option<String> {
    let get = |name: &str| headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
    let mut parts: Vec<(String, String)> = headers
        .iter()
        .filter(|(k, _)| k.contains("remaining"))
        .filter_map(|(k, v)| {
            let limit = get(&k.replace("remaining", "limit"))?;
            let label = k
                .replace("anthropic-ratelimit-", "")
                .replace("x-ratelimit-", "")
                .replace("remaining", "")
                .trim_matches('-')
                .replace("--", "-");
            let label = match label.as_str() {
                "requests" => "requests".to_string(),
                "input-tokens" => "input tokens".to_string(),
                "output-tokens" => "output tokens".to_string(),
                _ => label,
            };
            let num = |s: &str| s.parse::<u64>().map(human).unwrap_or_else(|_| s.to_string());
            Some((label, format!("{}/{}", num(v), num(limit))))
        })
        .collect();
    if parts.is_empty() {
        return None;
    }
    parts.sort_by_key(|(l, _)| (l.len(), l.clone()));
    parts.truncate(3);
    Some(parts.into_iter().map(|(l, v)| format!("{l} {v}")).collect::<Vec<_>>().join(" · "))
}

/// One line per model response: `↑in ⟳cache ↓out · tok/s · ctx % · $ · limits`.
pub fn turn_line(turn: &Usage, secs: f64, window: u32, cost: Option<f64>, limits: Option<&str>) -> String {
    let mut s = format!(
        "↑{} ⟳{} ↓{}",
        human(turn.input + turn.cache_write),
        human(turn.cache_read),
        human(turn.output)
    );
    if secs > 0.05 && turn.output > 0 {
        s.push_str(&format!(" · {:.0} tok/s", turn.output as f64 / secs));
    }
    if window > 0 {
        let pct = turn.prompt_total() as f64 * 100.0 / window as f64;
        s.push_str(&format!(" · context {pct:.0}% of {}", human(window as u64)));
    }
    if let Some(c) = cost {
        s.push_str(&format!(" · ${c:.4}"));
    }
    if let Some(l) = limits {
        s.push_str(&format!(" · limits {l}"));
    }
    s
}

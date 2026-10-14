//! Terminal output: the token/cost status line and a streaming observer.

use coturbo_agent::Observer;
use coturbo_providers::Usage;
use coturbo_tools::ToolOutput;
use serde_json::Value;
use std::io::Write;

/// `950`, `12.3k`, `1.20M`.
pub fn human(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.2}M", n as f64 / 1e6),
    }
}

/// `tokens ↑in ⟳cache ↓out · $cost`. `↑` counts prompt tokens billed at
/// full or write price; `⟳` counts cache reads. No price, no dollar figure.
pub fn status_line(u: &Usage, cost: Option<f64>) -> String {
    let mut s = format!(
        "tokens ↑{} ⟳{} ↓{}",
        human(u.input + u.cache_write),
        human(u.cache_read),
        human(u.output)
    );
    if let Some(c) = cost {
        s.push_str(&format!(" · ${c:.4}"));
    }
    s
}

/// One-line, length-capped view of a tool call for the progress log.
pub fn tool_line(name: &str, input: &Value, out: &ToolOutput) -> String {
    const MAX: usize = 80;
    let arg = ["path", "symbol", "pattern", "command"]
        .iter()
        .find_map(|k| input[*k].as_str())
        .unwrap_or("");
    let first = out.content.lines().next().unwrap_or("");
    let clip = |s: &str| -> String {
        if s.chars().count() <= MAX {
            s.to_string()
        } else {
            let mut c: String = s.chars().take(MAX).collect();
            c.push('…');
            c
        }
    };
    let mark = if out.is_error { "✗" } else { "✓" };
    let mut line = format!("  {mark} {name} {}", clip(arg));
    if out.is_error {
        line.push_str(&format!(" — {}", clip(first)));
    }
    line
}

/// Streams model text to stdout and tool calls to stderr.
pub struct TermObserver {
    pub verbose: bool,
}

impl Observer for TermObserver {
    fn on_text(&mut self, delta: &str) {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(delta.as_bytes());
        let _ = out.flush();
    }

    fn on_tool(&mut self, name: &str, input: &Value, output: &ToolOutput) {
        eprintln!("\n{}", tool_line(name, input, output));
    }

    fn on_turn(&mut self, _turn: &Usage, task: &Usage) {
        if self.verbose {
            eprintln!("  · {}", status_line(task, None));
        }
    }
}

//! The full-screen WiFi sensing view: what the signal says about the room,
//! live, until the user presses `x`.

use coturbo_net::sense::{Presence, Reading};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Gauge, Paragraph, Sparkline};

const BLUE: Color = Color::Rgb(47, 123, 255);

#[derive(Debug, Clone, Default)]
pub struct SenseView {
    pub reading: Option<Reading>,
    /// Still measuring (false once the run ended).
    pub running: bool,
}

impl SenseView {
    pub fn new() -> Self {
        Self { reading: None, running: true }
    }
}

fn presence_color(p: Presence) -> Color {
    match p {
        Presence::Moving => Color::Rgb(80, 220, 120),
        Presence::Still => BLUE,
        Presence::Empty => Color::Rgb(140, 152, 175),
        Presence::Learning => Color::Rgb(240, 200, 80),
    }
}

/// The last `n` values, so a sparkline shows the newest at the right edge.
fn tail(v: &[u64], n: usize) -> Vec<u64> {
    v[v.len().saturating_sub(n)..].to_vec()
}

pub fn draw(f: &mut Frame, area: Rect, view: &SenseView, ms: u128) {
    let dim = Style::new().fg(Color::Rgb(140, 152, 175));
    let outer = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(BLUE))
        .title(Line::from(vec![Span::styled(" WiFi sensing ", Style::new().fg(BLUE).add_modifier(Modifier::BOLD))]))
        .title(Line::from(vec![Span::styled(" x ", Style::new().fg(Color::Black).bg(BLUE).add_modifier(Modifier::BOLD)), Span::styled(" close ", dim)]).right_aligned());
    let inner = outer.inner(area);
    f.render_widget(outer, area);
    if inner.height < 6 || inner.width < 20 {
        f.render_widget(Paragraph::new("x close"), inner);
        return;
    }

    let Some(r) = &view.reading else {
        let dots = ".".repeat((ms / 400 % 4) as usize);
        f.render_widget(Paragraph::new(format!("\n  Listening to the WiFi{dots}")).style(dim), inner);
        return;
    };

    let rows = Layout::vertical([
        Constraint::Length(2), // presence
        Constraint::Length(1), // motion gauge
        Constraint::Length(1),
        Constraint::Min(3),    // movement graph
        Constraint::Min(3),    // signal graph
        Constraint::Length(2), // breathing
        Constraint::Length(2), // source and footer
    ])
    .margin(1)
    .split(inner);

    // The verdict, big and coloured, with a pulse while measuring.
    let color = presence_color(r.presence);
    let pulse = if view.running && (ms / 500).is_multiple_of(2) { "●" } else { "○" };
    let mut head = vec![
        Span::styled(format!("{pulse} "), Style::new().fg(color)),
        Span::styled(r.presence.label().to_uppercase(), Style::new().fg(color).add_modifier(Modifier::BOLD)),
    ];
    if !view.running {
        head.push(Span::styled("   measurement finished", dim));
    }
    f.render_widget(Paragraph::new(Line::from(head)), rows[0]);

    let pct = (r.motion * 100.0).round() as u16;
    f.render_widget(
        Gauge::default().gauge_style(Style::new().fg(color).bg(Color::Rgb(30, 34, 44))).percent(pct.min(100)).label(format!("motion {pct}%")),
        rows[1],
    );

    let width = rows[3].width.saturating_sub(2) as usize;
    // One step above zero, so a still room is a visible baseline, not an empty box.
    let motion: Vec<u64> = r.motion_history.iter().map(|m| (m * 100.0) as u64 + 2).collect();
    f.render_widget(
        Sparkline::default()
            .block(Block::new().borders(Borders::TOP).border_style(dim).title(Span::styled(" movement, last minute ", dim)))
            .data(tail(&motion, width))
            .max(102)
            .style(Style::new().fg(color)),
        rows[3],
    );

    let lo = r.signal_history.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = r.signal_history.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    // Around the middle, on a span of at least 6 dB (or 2 amplitude units),
    // so a steady signal reads as a flat line and not as full bars.
    let span = (hi - lo).max(if r.csi { 2.0 } else { 6.0 });
    let mid = (hi + lo) / 2.0;
    let signal: Vec<u64> = r.signal_history.iter().map(|v| (((v - mid) / span + 0.5) * 100.0).clamp(1.0, 100.0) as u64).collect();
    let unit = match r.level {
        Some(l) => format!(" signal, last minute: {l:.0} dBm (range {:.0} dB) ", (hi - lo).max(0.0)),
        None if r.csi => " signal, last minute (CSI amplitude) ".to_string(),
        None => " signal, last minute: no samples yet ".to_string(),
    };
    f.render_widget(
        Sparkline::default()
            .block(Block::new().borders(Borders::TOP).border_style(dim).title(Span::styled(unit, dim)))
            .data(tail(&signal, width))
            .max(100)
            .style(Style::new().fg(BLUE)),
        rows[4],
    );

    let breath = match r.breathing {
        Some((bpm, c)) => Line::from(vec![
            Span::styled("breathing ", dim),
            Span::styled(format!("{bpm:.0}/min"), Style::new().fg(BLUE).add_modifier(Modifier::BOLD)),
            Span::styled(format!("  ({:.0}% confidence)", c * 100.0), dim),
        ]),
        None if r.csi => Line::from(Span::styled("breathing: shows when the person is still", dim)),
        None => Line::from(Span::styled("breathing: needs an ESP32 CSI node (UDP 5005); this WiFi shows movement only", dim)),
    };
    f.render_widget(Paragraph::new(breath), rows[5]);

    let foot = vec![
        Line::from(Span::styled(format!("{}  ·  {:.1} samples/s  ·  {:.0} s", r.source, r.rate_hz, r.seconds), dim)),
        Line::from(Span::styled("Estimates from WiFi: coarse, not a camera, not a medical measurement.  x to close", dim)),
    ];
    f.render_widget(Paragraph::new(foot), rows[6]);
}

//! O painel de trabalho: as etapas da tarefa como barras de progresso
//! (WRITE, CHECK, CLIPPY, TEST, FIX...) e o checklist que o modelo mantém.
//! A barra anda sozinha enquanto a etapa roda, com um brilho que corre por ela;
//! fica verde ao terminar e vermelha, pulsando, se falhar.

use super::{StageState, StageUpdate, TaskItem, TaskState};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::time::{Duration, Instant};

const BLUE: Color = Color::Rgb(47, 123, 255);
const SOFT: Color = Color::Rgb(225, 232, 245);
const GREEN: Color = Color::Rgb(86, 204, 128);
const DIM: Color = Color::Rgb(110, 125, 150);
const EMPTY: Color = Color::Rgb(40, 52, 76);
const SPIN: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// How long the panel stays after the work ends.
const LINGER: Duration = Duration::from_secs(30);
const MAX_TASKS: usize = 7;

#[derive(Debug, Clone)]
pub struct StageRow {
    pub name: String,
    pub state: StageState,
    pub note: String,
    started: Instant,
    pub took: Option<Duration>,
    expected: Option<f32>,
}

impl StageRow {
    /// How far along the stage looks, 0 to 1. A running stage moves with the
    /// time it took last time, or, never having run, ever more slowly towards
    /// the end without reaching it.
    pub fn fraction(&self, now: Instant) -> f32 {
        match self.state {
            StageState::Pending => 0.0,
            StageState::Done | StageState::Failed => 1.0,
            StageState::Running => {
                let t = now.saturating_duration_since(self.started).as_secs_f32();
                match self.expected {
                    Some(e) if e > 0.2 => (t / e).min(0.96),
                    _ => 0.96 * (1.0 - (-t / 8.0).exp()),
                }
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct Work {
    pub stages: Vec<StageRow>,
    pub tasks: Vec<TaskItem>,
    touched: Option<Instant>,
}

impl Work {
    pub fn clear(&mut self) {
        self.stages.clear();
        self.tasks.clear();
        self.touched = None;
    }

    pub fn set_tasks(&mut self, tasks: Vec<TaskItem>, now: Instant) {
        self.tasks = tasks;
        self.touched = Some(now);
    }

    pub fn apply(&mut self, u: StageUpdate, now: Instant) {
        self.touched = Some(now);
        let Some(row) = self.stages.iter_mut().find(|r| r.name == u.name) else {
            self.stages.push(StageRow {
                name: u.name,
                state: u.state,
                note: u.note,
                started: now,
                took: matches!(u.state, StageState::Done | StageState::Failed).then_some(Duration::ZERO),
                expected: u.expected,
            });
            return;
        };
        let was = row.state;
        row.state = u.state;
        if !u.note.is_empty() || u.state != was {
            row.note = u.note;
        }
        if u.expected.is_some() {
            row.expected = u.expected;
        }
        match (was, u.state) {
            (s, StageState::Running) if s != StageState::Running => {
                row.started = now;
                row.took = None;
            }
            (StageState::Running, StageState::Done | StageState::Failed) => row.took = Some(now.saturating_duration_since(row.started)),
            _ => {}
        }
    }

    /// Whether the panel shows: while work runs, and a while after it ends.
    pub fn visible(&self, busy: bool, now: Instant) -> bool {
        if self.stages.is_empty() && self.tasks.is_empty() {
            return false;
        }
        busy || self.touched.is_some_and(|t| now.saturating_duration_since(t) < LINGER)
    }

    /// The panel's lines for a screen `width` wide at time `now`.
    pub fn lines(&self, width: usize, now: Instant, ms: u128) -> Vec<Line<'static>> {
        let mut out = Vec::new();
        let bar_w = (width / 4).clamp(8, 28);
        for row in &self.stages {
            out.push(stage_line(row, width, bar_w, now, ms));
        }
        if !self.tasks.is_empty() {
            if !out.is_empty() {
                out.push(Line::default());
            }
            out.extend(task_lines(&self.tasks, width, ms));
        }
        out
    }

    /// Rows the panel takes at `width`.
    pub fn height(&self, width: usize, now: Instant) -> usize {
        self.lines(width, now, 0).len()
    }
}

fn lerp(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> Color {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t.clamp(0.0, 1.0)).round() as u8;
    Color::Rgb(m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

fn stage_line(row: &StageRow, width: usize, bar_w: usize, now: Instant, ms: u128) -> Line<'static> {
    let frac = row.fraction(now);
    let pulse = ((ms as f32 / 260.0).sin() + 1.0) / 2.0;
    let base = match row.state {
        StageState::Pending => EMPTY,
        StageState::Running => BLUE,
        StageState::Done => GREEN,
        StageState::Failed => lerp((255, 92, 92), (165, 38, 44), pulse),
    };
    let glyph = match row.state {
        StageState::Pending => "○".to_string(),
        StageState::Running => SPIN[(ms / 90) as usize % SPIN.len()].to_string(),
        StageState::Done => "✓".to_string(),
        StageState::Failed => "✗".to_string(),
    };
    let filled = (frac * bar_w as f32).round() as usize;
    // A glint runs along the filled part of a running bar.
    let glint = (ms / 55) as usize % (bar_w + 8);
    let mut spans = vec![
        Span::raw("  "),
        Span::styled(glyph, Style::new().fg(base).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(format!("{:<7}", row.name), Style::new().fg(if row.state == StageState::Pending { DIM } else { SOFT }).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
    ];
    for i in 0..bar_w {
        if i < filled {
            let hot = row.state == StageState::Running && i.abs_diff(glint.saturating_sub(4)) <= 1;
            spans.push(Span::styled("█", Style::new().fg(if hot { SOFT } else { base })));
        } else {
            spans.push(Span::styled("░", Style::new().fg(EMPTY)));
        }
    }
    let pct = match row.state {
        StageState::Failed => " FALHA".to_string(),
        _ => format!(" {:>3}%", (frac * 100.0).round() as u32),
    };
    spans.push(Span::styled(pct, Style::new().fg(base)));
    let used = 2 + 1 + 1 + 7 + 1 + bar_w + 6;
    if !row.note.is_empty() && width > used + 4 {
        let room = width - used - 2;
        let note: String = if row.note.chars().count() > room {
            format!("{}…", row.note.chars().take(room.saturating_sub(1)).collect::<String>())
        } else {
            row.note.clone()
        };
        spans.push(Span::styled(format!("  {note}"), Style::new().fg(DIM)));
    }
    Line::from(spans)
}

fn task_lines(tasks: &[TaskItem], width: usize, ms: u128) -> Vec<Line<'static>> {
    let done = tasks.iter().filter(|t| t.state == TaskState::Done).count();
    let mut out = vec![Line::from(vec![
        Span::raw("  "),
        Span::styled("TAREFAS", Style::new().fg(SOFT).add_modifier(Modifier::BOLD)),
        Span::styled(format!("  {done}/{}", tasks.len()), Style::new().fg(DIM)),
    ])];
    // With many steps, the window follows the one in progress.
    let doing = tasks.iter().position(|t| t.state == TaskState::Doing).or_else(|| tasks.iter().position(|t| t.state == TaskState::Pending)).unwrap_or(0);
    let start = if tasks.len() <= MAX_TASKS { 0 } else { doing.saturating_sub(2).min(tasks.len() - MAX_TASKS) };
    for t in tasks.iter().skip(start).take(MAX_TASKS) {
        let (glyph, color, bold) = match t.state {
            TaskState::Done => ("✓".to_string(), GREEN, false),
            TaskState::Doing => (SPIN[(ms / 90) as usize % SPIN.len()].to_string(), BLUE, true),
            TaskState::Pending => ("○".to_string(), DIM, false),
        };
        let room = width.saturating_sub(8);
        let title: String = if t.title.chars().count() > room {
            format!("{}…", t.title.chars().take(room.saturating_sub(1)).collect::<String>())
        } else {
            t.title.clone()
        };
        let mut style = Style::new().fg(if t.state == TaskState::Pending { DIM } else { SOFT });
        if t.state == TaskState::Done {
            style = style.fg(DIM);
        }
        if bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        out.push(Line::from(vec![Span::raw("    "), Span::styled(glyph, Style::new().fg(color)), Span::raw(" "), Span::styled(title, style)]));
    }
    let hidden = tasks.len().saturating_sub(MAX_TASKS);
    if hidden > 0 {
        out.push(Line::styled(format!("      … e mais {hidden}"), Style::new().fg(DIM)));
    }
    out
}

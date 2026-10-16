//! The screen: what it shows, how it draws, what the keys do. It runs on
//! its own thread and redraws every animation frame.

use super::commands::{self, Command, Parsed};
use super::input::Input;
use super::browser::BrowserView;
use super::text;
use super::work::Work;
use super::{Choice, In, Out, Question, Status, StreamKind};
use crate::splash::{self, Mood};
use coturbo_agent::Decision;
use coturbo_agent::plan::PlanChoice;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

const fn rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::Rgb(r, g, b)
}

const BLUE: Color = rgb(splash::BLUE);
const SOFT: Color = rgb(splash::SOFT);
const BORDER: Color = Color::Rgb(36, 66, 130);
const YELLOW: Color = Color::Rgb(240, 190, 80);
const PANEL: Color = Color::Rgb(16, 22, 36);
/// The text box grows with its content up to this many rows.
/// How long an alert or a refusal keeps the snail yellow or red.
const FLASH: Duration = Duration::from_millis(4500);
/// How long the snail stays green after a task, idle, before it goes white again.
pub const GREEN_FOR: Duration = Duration::from_secs(10 * 60);

const MAX_INPUT_ROWS: usize = 8;
/// Entries of a long list shown at once.
const LIST_ROWS: usize = 8;

fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

/// One entry of the conversation.
#[derive(Debug, Clone)]
enum Entry {
    Line(Line<'static>),
    /// Streamed model text, rendered from markdown when drawn.
    Markdown {
        text: String,
        /// Inside a fenced code block.
        code: bool,
        prefix: &'static str,
        style: Style,
    },
    /// Code shown shortened until Ctrl+B.
    Fold { preview: Vec<Line<'static>>, full: Vec<Line<'static>>, open: bool },
}

impl Entry {
    fn rows(&self) -> Vec<Line<'static>> {
        match self {
            Entry::Line(l) => vec![l.clone()],
            Entry::Markdown { text, code, prefix, style } => {
                let mut line = Line::default();
                text::push(&mut line, prefix, *style);
                let body = if text::is_fence(text) { Line::default() } else { text::markdown_line(text, *code, *style) };
                line.spans.extend(body.spans);
                vec![line]
            }
            Entry::Fold { preview, full, open } => if *open { full.clone() } else { preview.clone() },
        }
    }
}

/// The conversation: finished lines plus the one being streamed.
#[derive(Debug, Default)]
pub struct Transcript {
    entries: Vec<Entry>,
    stream: Option<StreamKind>,
}

impl Transcript {
    /// Every row as currently shown, before wrapping.
    pub fn lines(&self) -> Vec<Line<'static>> {
        self.entries.iter().flat_map(Entry::rows).collect()
    }

    pub fn push(&mut self, line: Line<'static>) {
        self.stream = None;
        self.entries.push(Entry::Line(line));
    }

    pub fn extend(&mut self, lines: Vec<Line<'static>>) {
        self.stream = None;
        self.entries.extend(lines.into_iter().map(Entry::Line));
    }

    /// Code that shows `preview` until expanded.
    pub fn fold(&mut self, preview: Vec<Line<'static>>, full: Vec<Line<'static>>) {
        self.stream = None;
        self.entries.push(Entry::Fold { preview, full, open: false });
    }

    /// Shows folded code in full, or folds it again. Returns the new state.
    pub fn toggle_folds(&mut self) -> bool {
        let open = !self.entries.iter().any(|e| matches!(e, Entry::Fold { open: true, .. }));
        for e in &mut self.entries {
            if let Entry::Fold { open: o, .. } = e {
                *o = open;
            }
        }
        open
    }

    /// Opens or closes one block of code (a click on it). `false` if the
    /// entry is not folded code.
    pub fn toggle_fold(&mut self, entry: usize) -> bool {
        match self.entries.get_mut(entry) {
            Some(Entry::Fold { open, .. }) => {
                *open = !*open;
                true
            }
            _ => false,
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.stream = None;
    }

    /// Appends streamed model output: reasoning dimmed in italics after
    /// `∴`, the answer as text; both rendered from markdown.
    pub fn stream(&mut self, kind: StreamKind, delta: &str) {
        let (style, first, indent) = match kind {
            StreamKind::Thinking => (Style::new().add_modifier(Modifier::DIM | Modifier::ITALIC), "∴ ", "  "),
            StreamKind::Text => (Style::new(), "", ""),
        };
        if self.stream != Some(kind) {
            let blank = match self.entries.last() {
                Some(Entry::Line(l)) => text::width(l) > 0,
                Some(_) => true,
                None => false,
            };
            if blank {
                self.entries.push(Entry::Line(Line::default()));
            }
            self.entries.push(Entry::Markdown { text: String::new(), code: false, prefix: first, style });
            self.stream = Some(kind);
        }
        for (n, part) in delta.split('\n').enumerate() {
            if n > 0 {
                let code = match self.entries.last() {
                    Some(Entry::Markdown { text, code, .. }) => *code != text::is_fence(text),
                    _ => false,
                };
                self.entries.push(Entry::Markdown { text: String::new(), code, prefix: indent, style });
            }
            if let Some(Entry::Markdown { text, .. }) = self.entries.last_mut() {
                text.push_str(part);
            }
        }
    }

    /// At most `height` rows wrapped to `width`, ending `scroll` rows above
    /// the bottom; the scroll actually used (clamped at the top); and the
    /// entry each row belongs to, for clicks.
    pub fn view(&self, width: usize, height: usize, scroll: usize) -> (Vec<Line<'static>>, usize, Vec<usize>) {
        let need = height + scroll;
        let mut rows = Vec::new();
        'entries: for (i, entry) in self.entries.iter().enumerate().rev() {
            for line in entry.rows().iter().rev() {
                if rows.len() >= need {
                    break 'entries;
                }
                let mut wrapped = text::wrap(line, width);
                wrapped.reverse();
                rows.extend(wrapped.into_iter().map(|l| (l, i)));
            }
        }
        let scroll = scroll.min(rows.len().saturating_sub(height));
        let mut visible: Vec<_> = rows.into_iter().skip(scroll).take(height).collect();
        visible.reverse();
        let owners = visible.iter().map(|(_, i)| *i).collect();
        (visible.into_iter().map(|(l, _)| l).collect(), scroll, owners)
    }
}

/// How a sent task appears in the conversation.
pub fn task_lines(task: &str) -> Vec<Line<'static>> {
    let prompt = Style::new().fg(BLUE).add_modifier(Modifier::BOLD);
    let mut lines = vec![Line::default()];
    for (i, l) in task.lines().enumerate() {
        let mut line = Line::default();
        text::push(&mut line, if i == 0 { "› " } else { "  " }, prompt);
        text::push(&mut line, l, Style::new().fg(SOFT).add_modifier(Modifier::BOLD));
        lines.push(line);
    }
    lines
}

/// Whether a question is offering its options or taking typed text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Choose,
    /// Writing a deny reason or what to change in a plan.
    Write,
}

struct Asking {
    q: Question,
    sel: usize,
    stage: Stage,
}

const APPROVE: [&str; 3] = ["Sim, desta vez", "Sim, durante esta sessão", "Não"];
const PLAN: [&str; 5] = [
    "Aprovar e executar (contexto limpo, só o plano)",
    "Aprovar e compactar o contexto",
    "Aprovar mantendo o contexto",
    "Refinar o plano",
    "Salvar e sair",
];
const CONFIRM: [&str; 2] = ["Sim", "Não"];

/// Options whose value or label contains `filter`, ignoring case.
pub fn filtered<'a>(options: &'a [Choice], filter: &str) -> Vec<&'a Choice> {
    let f = filter.trim().to_lowercase();
    options
        .iter()
        .filter(|c| f.is_empty() || c.value.to_lowercase().contains(&f) || c.label.to_lowercase().contains(&f))
        .collect()
}

pub struct App {
    transcript: Transcript,
    input: Input,
    /// What was in the text box when a question took it over.
    stash: Option<String>,
    history: Vec<String>,
    hist: Option<usize>,
    /// The text box before browsing the history.
    draft: String,
    /// Rows scrolled up from the bottom of the conversation.
    scroll: usize,
    /// Rows a PageUp moves: the conversation height at the last draw.
    page: usize,
    busy: Option<(String, Instant)>,
    /// The snail's working mood, a short flash over it (alert, denied), how
    /// far its animation has run and when it was last drawn: the pace follows
    /// the mood, so the frame is a running total, not the clock.
    mood: Mood,
    /// When the work ended and the snail turned green.
    done_at: Option<Instant>,
    /// The bars and the checklist of the job.
    work: Work,
    /// The mini browser: the page the agent is using, with its pointer.
    browser: BrowserView,
    flash: Option<(Mood, Instant)>,
    phase: f32,
    last_draw: Instant,
    status: Status,
    asking: Option<Asking>,
    suggest: usize,
    started: Instant,
    /// A plan file to open in the editor before the next frame.
    edit: Option<PathBuf>,
    /// Where the conversation was drawn, and the entry behind each row.
    body: Rect,
    owners: Vec<usize>,
    /// Set when the user quits; the agent side checks it too.
    closing: Arc<AtomicBool>,
    tx: UnboundedSender<In>,
    cancel: UnboundedSender<()>,
}

impl App {
    pub fn new(status: Status, tx: UnboundedSender<In>, cancel: UnboundedSender<()>, closing: Arc<AtomicBool>) -> Self {
        Self {
            transcript: Transcript::default(),
            input: Input::default(),
            stash: None,
            history: Vec::new(),
            hist: None,
            draft: String::new(),
            scroll: 0,
            page: 10,
            busy: None,
            mood: Mood::Calm,
            done_at: None,
            work: Work::default(),
            browser: BrowserView::default(),
            flash: None,
            phase: 0.0,
            last_draw: Instant::now(),
            status,
            asking: None,
            suggest: 0,
            started: Instant::now(),
            edit: None,
            body: Rect::default(),
            owners: Vec::new(),
            closing,
            tx,
            cancel,
        }
    }

    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    pub fn input(&self) -> &Input {
        &self.input
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn work(&self) -> &Work {
        &self.work
    }

    pub fn is_asking(&self) -> bool {
        self.asking.is_some()
    }

    /// The mood the snail shows now: yellow while a question waits for the
    /// user, a flash (alert, denied) for a few seconds, else the working mood.
    pub fn mood(&self) -> Mood {
        self.mood_at(Instant::now())
    }

    /// [`App::mood`] as of `now`. Green lasts [`GREEN_FOR`] once the work is
    /// done; left alone that long, the snail goes back to white.
    pub fn mood_at(&self, now: Instant) -> Mood {
        if self.asking.is_some() {
            return Mood::Alert;
        }
        match self.flash {
            Some((m, at)) if now.saturating_duration_since(at) < FLASH => m,
            _ if self.mood == Mood::Done && self.done_at.is_some_and(|t| now.saturating_duration_since(t) >= GREEN_FOR) => Mood::Calm,
            _ => self.mood,
        }
    }

    pub fn closing(&self) -> bool {
        self.closing.load(Ordering::SeqCst)
    }

    pub fn take_edit(&mut self) -> Option<PathBuf> {
        self.edit.take()
    }

    pub fn apply(&mut self, m: Out) {
        match m {
            Out::Lines(lines) => self.transcript.extend(lines),
            Out::Fold { preview, full } => self.transcript.fold(preview, full),
            Out::Stream(kind, delta) => self.transcript.stream(kind, &delta),
            Out::Busy(label) => {
                let since = self.busy.as_ref().map_or_else(Instant::now, |(_, t)| *t);
                match (&label, self.mood) {
                    // The work ended: green once there was work, white if there never was.
                    (None, Mood::Calm) => {}
                    (None, Mood::Done) => {}
                    (None, _) => {
                        self.mood = Mood::Done;
                        self.done_at = Some(Instant::now());
                    }
                    // A new task begins: blue, walking.
                    (Some(_), Mood::Calm | Mood::Done) => self.mood = Mood::Start,
                    _ => {}
                }
                self.busy = label.map(|l| (l, since));
            }
            Out::Stage(u) => self.work.apply(u, Instant::now()),
            Out::Tasks(t) => self.work.set_tasks(t, Instant::now()),
            Out::ClearWork => {
                self.work.clear();
                self.browser.clear();
            }
            Out::Browser(f) => self.browser.apply(f, Instant::now()),
            Out::Mood(m) if m.is_flash() => self.flash = Some((m, Instant::now())),
            Out::Mood(m) => self.mood = m,
            Out::Status(s) => self.status = s,
            Out::Usage(u) => self.status.usage = u,
            Out::Context(c) => self.status.context = Some(c),
            Out::Ask(q) => {
                if self.asking.is_none() {
                    self.stash = Some(self.input.take());
                }
                self.asking = Some(Asking { q, sel: 0, stage: Stage::Choose });
            }
            Out::CloseQuestion => {
                if self.asking.take().is_some() {
                    self.restore_input();
                }
            }
            Out::Export(path) => {
                let text: String = self
                    .transcript
                    .lines()
                    .iter()
                    .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>() + "\n")
                    .collect();
                let note = match path.parent().map(std::fs::create_dir_all).transpose().and_then(|_| std::fs::write(&path, text)) {
                    Ok(()) => format!("  → conversa salva em {}", path.display()),
                    Err(e) => format!("  ! não foi possível salvar: {e}"),
                };
                self.transcript.push(Line::styled(note, dim()));
            }
            Out::Clear => {
                self.transcript.clear();
                self.scroll = 0;
            }
        }
    }

    fn restore_input(&mut self) {
        let stash = self.stash.take().unwrap_or_default();
        self.input.set(stash);
    }

    fn quit(&mut self) {
        if self.busy.is_some() {
            let _ = self.cancel.send(());
        }
        self.closing.store(true, Ordering::SeqCst);
        let _ = self.tx.send(In::Quit);
    }

    fn note(&mut self, text: &str) {
        self.transcript.push(Line::styled(format!("  → {text}"), dim()));
    }

    pub fn on_event(&mut self, ev: Event, paste_follows: bool) {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k, paste_follows),
            Event::Paste(s) => self.on_paste(&s),
            Event::Mouse(m) => self.on_mouse(m),
            _ => {}
        }
    }

    /// The wheel scrolls the conversation; a click on code opens or closes it.
    pub fn on_mouse(&mut self, m: ratatui::crossterm::event::MouseEvent) {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
        match m.kind {
            MouseEventKind::ScrollUp => self.scroll += 3,
            MouseEventKind::ScrollDown => self.scroll = self.scroll.saturating_sub(3),
            MouseEventKind::Down(MouseButton::Left) => {
                let b = self.body;
                if (b.x..b.x + b.width).contains(&m.column) && (b.y..b.y + b.height).contains(&m.row)
                    && let Some(&entry) = self.owners.get((m.row - b.y) as usize)
                {
                    self.transcript.toggle_fold(entry);
                }
            }
            _ => {}
        }
    }

    fn on_paste(&mut self, s: &str) {
        match self.asking.as_ref().map(|a| (&a.q, a.stage)) {
            None | Some((_, Stage::Write)) => self.input.insert_str(s),
            Some((Question::Select { .. } | Question::Secret { .. }, _)) => self.input.insert_str(s.trim()),
            Some(_) => {}
        }
    }

    /// Editing keys shared by the text box and typed answers. `false` if
    /// the key is not one of them.
    fn edit_input(&mut self, k: KeyEvent) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Left if ctrl => self.input.word_left(),
            KeyCode::Right if ctrl => self.input.word_right(),
            KeyCode::Left => self.input.left(),
            KeyCode::Right => self.input.right(),
            KeyCode::Home => self.input.home(),
            KeyCode::End => self.input.end(),
            KeyCode::Backspace if ctrl || alt => self.input.delete_word(),
            KeyCode::Char('w') if ctrl && !alt => self.input.delete_word(),
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            // AltGr arrives as Ctrl+Alt on Windows: `@`, `{`, `€` are text.
            KeyCode::Char(c) if !ctrl || alt => self.input.insert(c),
            _ => return false,
        }
        true
    }

    pub fn on_key(&mut self, k: KeyEvent, paste_follows: bool) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        match k.code {
            KeyCode::Char('c') if ctrl && !alt => {
                if self.busy.is_some() {
                    if self.asking.take().is_some() {
                        self.restore_input();
                    }
                    let _ = self.cancel.send(());
                } else if self.asking.is_some() {
                    self.cancel_question();
                } else if !self.input.is_empty() {
                    self.input.take();
                } else {
                    self.quit();
                }
                return;
            }
            KeyCode::Char('d') if ctrl && !alt && self.input.is_empty() && self.asking.is_none() => {
                self.quit();
                return;
            }
            KeyCode::Char('b') if ctrl && !alt => {
                self.transcript.toggle_folds();
                return;
            }
            KeyCode::PageUp => {
                self.scroll += self.page;
                return;
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_sub(self.page);
                return;
            }
            _ => {}
        }
        if self.asking.is_some() {
            self.on_question_key(k, paste_follows);
            return;
        }
        let sugg = commands::suggestions(self.input.text());
        let pick = sugg.get(self.suggest.min(sugg.len().saturating_sub(1))).map(|c| c.name);
        match k.code {
            KeyCode::Enter if shift || alt || paste_follows => self.input.insert('\n'),
            KeyCode::Char('j') if ctrl && !alt => self.input.insert('\n'),
            KeyCode::Enter => {
                // A partly typed command runs the highlighted suggestion.
                if let Some(name) = pick
                    && matches!(commands::parse(self.input.text()), Parsed::Unknown(_))
                {
                    self.input.set(name);
                }
                self.submit();
            }
            KeyCode::Tab => {
                if let Some(name) = pick {
                    self.input.set(format!("{name} "));
                }
            }
            KeyCode::Up if ctrl => self.history_prev(),
            KeyCode::Down if ctrl => self.history_next(),
            KeyCode::Up if !sugg.is_empty() => self.suggest = self.suggest.saturating_sub(1),
            KeyCode::Down if !sugg.is_empty() => self.suggest = (self.suggest + 1).min(sugg.len() - 1),
            KeyCode::Up => {
                if !self.input.up() {
                    self.scroll += 1;
                }
            }
            KeyCode::Down => {
                if !self.input.down() {
                    self.scroll = self.scroll.saturating_sub(1);
                }
            }
            KeyCode::Esc => {
                if self.busy.is_some() {
                    let _ = self.cancel.send(());
                } else if self.browser.visible(Instant::now()) {
                    // With nothing running, Esc closes the mini browser first.
                    self.browser.dismiss(Instant::now());
                } else {
                    self.scroll = 0;
                }
            }
            _ => {
                if self.edit_input(k) {
                    self.hist = None;
                    self.suggest = 0;
                }
            }
        }
    }

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let i = match self.hist {
            None => {
                self.draft = self.input.text().to_string();
                self.history.len() - 1
            }
            Some(i) => i.saturating_sub(1),
        };
        self.hist = Some(i);
        self.input.set(self.history[i].clone());
    }

    fn history_next(&mut self) {
        match self.hist {
            None => {}
            Some(i) if i + 1 < self.history.len() => {
                self.hist = Some(i + 1);
                self.input.set(self.history[i + 1].clone());
            }
            Some(_) => {
                self.hist = None;
                let draft = std::mem::take(&mut self.draft);
                self.input.set(draft);
            }
        }
    }

    /// Sends the text box to the agent side, echoing it in the conversation.
    fn submit(&mut self) {
        let raw = self.input.take();
        let task = raw.trim();
        if task.is_empty() {
            return;
        }
        if self.history.last().map(String::as_str) != Some(task) {
            self.history.push(task.to_string());
        }
        self.hist = None;
        self.suggest = 0;
        self.scroll = 0;
        if matches!(commands::parse(task), Parsed::Command(Command::Exit, _)) {
            self.quit();
            return;
        }
        self.transcript.extend(task_lines(task));
        if self.busy.is_some() {
            self.transcript.push(Line::styled("  na fila: começa quando a tarefa atual terminar", dim()));
        }
        let _ = self.tx.send(In::Submit(task.to_string()));
    }

    /// Options of the open question, each with whether it can be picked.
    fn entries(&self, a: &Asking) -> Vec<(String, bool)> {
        let all = |items: &[&str]| items.iter().map(|s| (s.to_string(), true)).collect();
        match &a.q {
            Question::Approve { .. } => all(&APPROVE),
            Question::Plan { keep_allowed, .. } => {
                PLAN.iter().enumerate().map(|(i, s)| (s.to_string(), i != 2 || *keep_allowed)).collect()
            }
            Question::Confirm { .. } => all(&CONFIRM),
            Question::Select { options, .. } => {
                filtered(options, self.input.text()).into_iter().map(|c| (c.label.clone(), true)).collect()
            }
            Question::Secret { .. } => Vec::new(),
        }
    }

    fn typing(a: &Asking) -> bool {
        a.stage == Stage::Write || matches!(a.q, Question::Select { .. } | Question::Secret { .. })
    }

    fn move_sel(&mut self, down: bool) {
        let Some(a) = self.asking.as_ref() else { return };
        let entries = self.entries(a);
        let mut sel = a.sel.min(entries.len().saturating_sub(1));
        loop {
            let next = if down { sel + 1 } else { sel.wrapping_sub(1) };
            match entries.get(next) {
                Some((_, true)) => {
                    sel = next;
                    break;
                }
                Some((_, false)) => sel = next,
                None => break,
            }
        }
        if entries.get(sel).is_some_and(|(_, ok)| *ok)
            && let Some(a) = self.asking.as_mut()
        {
            a.sel = sel;
        }
    }

    fn on_question_key(&mut self, k: KeyEvent, paste_follows: bool) {
        let Some(a) = self.asking.as_ref() else { return };
        let typing = Self::typing(a);
        let count = self.entries(a).len();
        let is_plan = matches!(a.q, Question::Plan { .. });
        let is_confirm = matches!(a.q, Question::Confirm { .. });
        match k.code {
            KeyCode::Esc => self.back_or_cancel(),
            KeyCode::Enter if paste_follows && typing => {
                if a.stage == Stage::Write {
                    self.input.insert('\n');
                }
            }
            KeyCode::Enter => self.confirm(),
            KeyCode::Up => self.move_sel(false),
            KeyCode::Down | KeyCode::Tab => self.move_sel(true),
            KeyCode::Char(c @ '1'..='9') if !typing => {
                let i = c as usize - '1' as usize;
                if i < count && self.asking.as_ref().is_some_and(|a| self.entries(a)[i].1) {
                    if let Some(a) = self.asking.as_mut() {
                        a.sel = i;
                    }
                    self.confirm();
                }
            }
            KeyCode::Char('e') if !typing && is_plan => {
                if let Some(Asking { q: Question::Plan { path, .. }, .. }) = &self.asking {
                    self.edit = Some(path.clone());
                }
            }
            KeyCode::Char(c @ ('s' | 'n')) if !typing && is_confirm => {
                if let Some(a) = self.asking.as_mut() {
                    a.sel = usize::from(c == 'n');
                }
                self.confirm();
            }
            _ if typing => {
                if self.edit_input(k)
                    && let Some(a) = self.asking.as_mut()
                {
                    a.sel = 0;
                }
            }
            _ => {}
        }
    }

    fn back_or_cancel(&mut self) {
        if let Some(a) = self.asking.as_mut()
            && a.stage == Stage::Write
        {
            a.stage = Stage::Choose;
            self.input.take();
            return;
        }
        self.cancel_question();
    }

    /// Answers the open question with its "no" (Esc).
    fn cancel_question(&mut self) {
        let Some(a) = self.asking.take() else { return };
        match a.q {
            Question::Approve { reply, .. } => {
                let _ = reply.send(Decision::Deny(None));
                self.note("negado");
            }
            Question::Plan { reply, .. } => {
                let _ = reply.send(PlanChoice::SaveQuit);
                self.note("plano salvo, nada foi executado");
            }
            Question::Confirm { reply, .. } => {
                let _ = reply.send(false);
                self.note("não");
            }
            Question::Select { reply, .. } | Question::Secret { reply, .. } => {
                let _ = reply.send(None);
                self.note("cancelado");
            }
        }
        self.restore_input();
    }

    /// Answers the open question with the highlighted option or the typed text.
    fn confirm(&mut self) {
        let Some(a) = self.asking.take() else { return };
        let typed = self.input.text().trim().to_string();
        let sel = a.sel;
        match (a.q, a.stage) {
            (Question::Approve { reply, .. }, Stage::Write) => {
                self.note(if typed.is_empty() { "negado" } else { "negado, com o motivo enviado ao modelo" });
                let _ = reply.send(Decision::Deny((!typed.is_empty()).then_some(typed)));
            }
            (Question::Plan { reply, .. }, Stage::Write) => {
                self.note("pedido de ajuste enviado");
                let _ = reply.send(PlanChoice::Refine(typed));
            }
            (Question::Approve { title, reply }, Stage::Choose) => match sel {
                0 => {
                    self.note("permitido desta vez");
                    let _ = reply.send(Decision::Once);
                }
                1 => {
                    self.note("permitido durante esta sessão");
                    let _ = reply.send(Decision::Session);
                }
                _ => {
                    self.input.take();
                    self.asking = Some(Asking { q: Question::Approve { title, reply }, sel, stage: Stage::Write });
                    return;
                }
            },
            (Question::Plan { title, path, keep_allowed, reply }, Stage::Choose) => {
                let choice = match sel {
                    0 => PlanChoice::Execute,
                    1 => PlanChoice::Compact,
                    2 if keep_allowed => PlanChoice::KeepContext,
                    3 => {
                        self.input.take();
                        let q = Question::Plan { title, path, keep_allowed, reply };
                        self.asking = Some(Asking { q, sel, stage: Stage::Write });
                        return;
                    }
                    _ => PlanChoice::SaveQuit,
                };
                self.note(PLAN[sel.min(PLAN.len() - 1)]);
                let _ = reply.send(choice);
            }
            (Question::Confirm { reply, .. }, _) => {
                self.note(CONFIRM[sel.min(1)]);
                let _ = reply.send(sel == 0);
            }
            (Question::Select { title, options, custom, reply }, _) => {
                let picked = filtered(&options, &typed).get(sel).map(|c| c.value.clone());
                let value = picked.or_else(|| (custom && !typed.is_empty()).then_some(typed));
                match value {
                    Some(v) => {
                        let _ = reply.send(Some(v));
                    }
                    None => {
                        // Nothing to pick yet: keep the list open.
                        self.asking = Some(Asking { q: Question::Select { title, options, custom, reply }, sel, stage: Stage::Choose });
                        return;
                    }
                }
            }
            (Question::Secret { title, reply }, _) => {
                if typed.is_empty() {
                    self.asking = Some(Asking { q: Question::Secret { title, reply }, sel, stage: Stage::Choose });
                    return;
                }
                let _ = reply.send(Some(typed));
            }
        }
        self.restore_input();
    }

    /// Shows the plan again after it was edited outside.
    pub fn after_edit(&mut self, path: &Path, error: Option<String>) {
        match error {
            Some(e) => self.transcript.push(Line::styled(format!("  ! não foi possível abrir o editor: {e}"), Style::new().fg(YELLOW))),
            None => {
                let content = std::fs::read_to_string(path).unwrap_or_default();
                self.transcript.push(Line::default());
                self.transcript.push(Line::styled("── plano editado ──", Style::new().fg(BLUE).add_modifier(Modifier::BOLD)));
                for l in content.lines() {
                    self.transcript.push(Line::raw(l.to_string()));
                }
            }
        }
        self.scroll = 0;
    }

    /// Title and rows of the question panel.
    fn question_view(&self) -> Option<(String, Vec<Line<'static>>)> {
        let a = self.asking.as_ref()?;
        let title = match &a.q {
            Question::Approve { title, .. }
            | Question::Plan { title, .. }
            | Question::Confirm { title, .. }
            | Question::Select { title, .. }
            | Question::Secret { title, .. } => title.clone(),
        };
        let mut rows = Vec::new();
        if a.stage == Stage::Write {
            let what = if matches!(a.q, Question::Plan { .. }) {
                "Escreva o que deve mudar no plano e pressione Enter."
            } else {
                "Escreva o motivo (opcional) e pressione Enter. O modelo recebe o motivo."
            };
            rows.push(Line::raw(what));
            rows.push(Line::styled("Esc volta às opções", dim()));
            return Some((title, rows));
        }
        if let Question::Secret { .. } = a.q {
            rows.push(Line::raw("Cole a chave no campo abaixo e pressione Enter."));
            rows.push(Line::styled("Ela fica salva somente neste computador. Esc cancela.", dim()));
            return Some((title, rows));
        }
        let entries = self.entries(a);
        let sel = a.sel.min(entries.len().saturating_sub(1));
        let first = sel.saturating_sub(LIST_ROWS - 1).min(entries.len().saturating_sub(LIST_ROWS));
        for (i, (label, ok)) in entries.iter().enumerate().skip(first).take(LIST_ROWS) {
            let numbered = !matches!(a.q, Question::Select { .. });
            let n = if numbered { format!("{}. ", i + 1) } else { String::new() };
            let (mark, style) = match (i == sel, ok) {
                (_, false) => ("  ", dim()),
                (true, true) => ("› ", Style::new().fg(BLUE).add_modifier(Modifier::BOLD)),
                (false, true) => ("  ", Style::new()),
            };
            let tail = if *ok { "" } else { " (indisponível: contexto acima de 95%)" };
            rows.push(Line::styled(format!("{mark}{n}{label}{tail}"), style));
        }
        let mut help = String::from("↑↓ escolher · Enter confirmar · Esc cancelar");
        match &a.q {
            Question::Select { options, custom, .. } => {
                if entries.is_empty() {
                    rows.push(Line::styled(
                        if *custom { "Nenhum item corresponde; Enter usa o texto digitado." } else { "Nenhum item corresponde." },
                        dim(),
                    ));
                }
                help = format!("{} de {} · digite para filtrar · {help}", entries.len(), options.len());
            }
            Question::Plan { .. } => help.push_str(" · e editar o plano"),
            _ => {}
        }
        rows.push(Line::styled(help, dim()));
        Some((title, rows))
    }

    fn input_title(&self) -> Option<&'static str> {
        match self.asking.as_ref().map(|a| (&a.q, a.stage)) {
            Some((Question::Plan { .. }, Stage::Write)) => Some("o que mudar"),
            Some((_, Stage::Write)) => Some("motivo"),
            Some((Question::Select { .. }, _)) => Some("filtrar"),
            Some((Question::Secret { .. }, _)) => Some("API key"),
            _ if self.status.plan => Some("modo plano: somente leitura"),
            _ => None,
        }
    }

    pub fn set_graphics(&mut self, picker: ratatui_image::picker::Picker) {
        self.browser.set_picker(picker);
    }

    pub fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        let tick = self.started.elapsed();
        // The name types itself in on the clock; the snail's frame is a
        // running total so a change of mood changes the pace, never jumps.
        let typed = (tick.as_millis() / splash::FRAME.as_millis()) as usize;
        let mood = self.mood();
        let dt = self.last_draw.elapsed().min(Duration::from_millis(250));
        self.last_draw = Instant::now();
        self.phase += dt.as_secs_f32() / splash::FRAME.as_secs_f32() * mood.speed();
        let frame = self.phase as usize;
        let logo = area.width as usize >= splash::logo_width() + 2 && area.height as usize >= splash::ROWS + 12;
        let header_h = if logo { splash::ROWS as u16 + 1 } else { 2 };

        let asking = self.asking.as_ref();
        let mask = matches!(asking.map(|a| &a.q), Some(Question::Secret { .. }));
        let answering = asking.is_some_and(Self::typing);
        let box_w = (area.width as usize).saturating_sub(4).max(1);
        let (rows, (crow, ccol)) = self.input.layout(box_w, mask);
        let shown = rows.len().min(MAX_INPUT_ROWS);
        let first = (crow + 1).saturating_sub(shown);
        let question = self.question_view();
        let q_h = question.as_ref().map_or(0, |(_, l)| (l.len() as u16 + 2).min(area.height / 2));

        let now = Instant::now();
        self.browser.set_idle(self.busy.is_none());
        let work_lines = if area.height >= 22 && self.work.visible(self.busy.is_some(), now) {
            self.work.lines(area.width as usize, now, tick.as_millis())
        } else {
            Vec::new()
        };
        let work_h = (work_lines.len() as u16).min(area.height / 3);
        let [header, body, work, panel, hint, input, status] = Layout::vertical([
            Constraint::Length(header_h),
            Constraint::Min(1),
            Constraint::Length(work_h),
            Constraint::Length(q_h),
            Constraint::Length(1),
            Constraint::Length(shown as u16 + 2),
            Constraint::Length(1),
        ])
        .areas(area);

        if logo {
            draw_logo(f.buffer_mut(), header, frame, typed, mood);
        } else {
            let name = Line::from(vec![
                Span::styled(" Code", Style::new().fg(SOFT).add_modifier(Modifier::BOLD)),
                Span::styled("Turbo", Style::new().fg(rgb(mood.body())).add_modifier(Modifier::BOLD)),
            ]);
            f.render_widget(Paragraph::new(name), header);
        }

        let mut body = Rect { x: body.x + 1, width: body.width.saturating_sub(2), ..body };
        // The mini browser takes the right of the screen while the agent uses a page.
        let mut browser_rect = None;
        if self.browser.visible(now) && area.width >= 80 && area.height >= 20 {
            let pw = (body.width / 2).clamp(40, 80);
            if body.width > pw + 30 {
                let ph = self.browser.rows_for(pw).min(body.height);
                browser_rect = Some(Rect { x: body.right() - pw, y: body.y, width: pw, height: ph });
                // The conversation gives way as the window opens, and takes the room back as it closes.
                let given = (pw as f32 * self.browser.progress(now)).round() as u16;
                if given > 0 {
                    body.width -= given + 1;
                }
            }
        }
        self.page = (body.height as usize).saturating_sub(2).max(1);
        let (lines, scroll, owners) = self.transcript.view(body.width.max(1) as usize, body.height as usize, self.scroll);
        self.scroll = scroll;
        self.body = body;
        self.owners = owners;
        f.render_widget(Paragraph::new(lines), body);
        if let Some(r) = browser_rect {
            self.browser.draw(f.buffer_mut(), r, now);
            self.browser.draw_native(f, r, now);
        }
        if work_h > 0 {
            f.render_widget(Paragraph::new(work_lines), work);
        }

        if let Some((title, lines)) = question {
            let block = Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(YELLOW))
                .title(Span::styled(format!(" {title} "), Style::new().fg(YELLOW).add_modifier(Modifier::BOLD)));
            let rect = Rect { x: panel.x + 1, width: panel.width.saturating_sub(2), ..panel };
            f.render_widget(Clear, rect);
            f.render_widget(Paragraph::new(lines).block(block), rect);
        }

        let hint_line = match &self.busy {
            Some((label, since)) => {
                let mut l = text::ansi_line(&splash::spinner_line((tick.as_millis() / 80) as usize, label));
                text::push(&mut l, &format!(" · {}s · Esc interrompe", since.elapsed().as_secs()), dim());
                l
            }
            None if self.asking.is_some() => Line::default(),
            None if self.scroll > 0 => Line::styled("  há mais mensagens abaixo · Esc volta ao fim", dim()),
            None => Line::styled(
                "  Enter envia · Shift+Enter quebra a linha · clique ou Ctrl+B abre o código · Shift+arrastar seleciona · /ajuda",
                dim(),
            ),
        };
        f.render_widget(Paragraph::new(hint_line), hint);

        let mut block = Block::new().borders(Borders::TOP | Borders::BOTTOM).border_style(Style::new().fg(BORDER));
        if let Some(t) = self.input_title() {
            block = block.title(Span::styled(format!(" {t} "), Style::new().fg(BLUE)));
        }
        let inner = block.inner(input);
        f.render_widget(block, input);
        let prompt = Style::new().fg(BLUE).add_modifier(Modifier::BOLD);
        let mut lines: Vec<Line> = rows
            .iter()
            .enumerate()
            .skip(first)
            .take(shown)
            .map(|(i, r)| Line::from(vec![Span::styled(if i == 0 { "› " } else { "  " }, prompt), Span::raw(r.clone())]))
            .collect();
        if self.input.is_empty() && self.asking.is_none() {
            lines = vec![Line::from(vec![
                Span::styled("› ", prompt),
                Span::styled("Descreva a tarefa ou digite / para ver os comandos", dim()),
            ])];
        }
        let text_area = Rect { x: inner.x + 1, width: inner.width.saturating_sub(1), ..inner };
        f.render_widget(Paragraph::new(lines), text_area);
        if self.asking.is_none() || answering {
            f.set_cursor_position((text_area.x + 2 + ccol as u16, text_area.y + (crow - first) as u16));
        }

        let s = &self.status;
        let mut left = format!(" {} · {} · {}", s.provider, s.model, s.approval);
        if s.plan {
            left.push_str(" · modo plano");
        }
        let mut right = s.usage.clone();
        if let Some(c) = s.context {
            right.push_str(&format!(" · contexto {c}%"));
        }
        right.push(' ');
        // The folder sits next to the model; what does not fit is cut down to
        // the last three folders, then fewer.
        // The " · " before the folder and a gap before the usage come off the room.
        let room = (status.width as usize).saturating_sub(left.chars().count() + right.chars().count() + 3 + 3);
        if !s.folder.is_empty() && room >= 6 {
            left.push_str(" · ");
            left.push_str(&super::path::short_path(&s.folder, room));
        }
        f.render_widget(Paragraph::new(Line::styled(left, dim())), status);
        f.render_widget(Paragraph::new(Line::styled(right, dim())).alignment(Alignment::Right), status);

        if self.asking.is_none() {
            let sugg = commands::suggestions(self.input.text());
            if !sugg.is_empty() {
                let h = sugg.len().min(10) as u16;
                let rect = Rect {
                    x: input.x + 1,
                    y: input.y.saturating_sub(h),
                    width: input.width.saturating_sub(2).min(64),
                    height: h,
                };
                let sel = self.suggest.min(sugg.len() - 1);
                let lines: Vec<Line> = sugg
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        let style =
                            if i == sel { Style::new().fg(BLUE).add_modifier(Modifier::BOLD) } else { Style::new().fg(SOFT) };
                        Line::from(vec![
                            Span::styled(format!(" {:<12}", c.name), style),
                            Span::styled(c.about, dim()),
                        ])
                    })
                    .collect();
                f.render_widget(Clear, rect);
                f.render_widget(Paragraph::new(lines).style(Style::new().bg(PANEL)), rect);
            }
        }
    }
}

/// The logo with the snail at frame `i`, drawn with half blocks: the same
/// pixels as the terminal intro.
fn draw_logo(buf: &mut Buffer, area: Rect, snail: usize, typed: usize, mood: Mood) {
    let px = splash::pixels_mood(snail, typed, area.width as usize, mood);
    for (row, pair) in px.chunks(2).enumerate() {
        let y = area.y + row as u16;
        if y >= area.bottom() || pair.len() < 2 {
            break;
        }
        for (col, (top, bottom)) in pair[0].iter().zip(&pair[1]).enumerate() {
            let x = area.x + 1 + col as u16;
            if x >= area.right() {
                break;
            }
            let (symbol, fg, bg) = match (*top, *bottom) {
                (None, None) => continue,
                (Some(t), None) => ("▀", t, None),
                (None, Some(b)) => ("▄", b, None),
                (Some(t), Some(b)) => ("▀", t, Some(b)),
            };
            let cell = &mut buf[(x, y)];
            cell.set_symbol(symbol).set_fg(rgb(fg));
            if let Some(b) = bg {
                cell.set_bg(rgb(b));
            }
        }
    }
}

/// The terminal's picture protocol, when it has one: Kitty, iTerm2 or Sixel.
/// `CODETURBO_GRAPHICS=off` keeps the page in blocks.
fn graphics() -> Option<ratatui_image::picker::Picker> {
    use ratatui_image::picker::ProtocolType;
    if matches!(std::env::var("CODETURBO_GRAPHICS").as_deref(), Ok("off" | "0" | "blocks")) {
        return None;
    }
    let found = ratatui_image::picker::Picker::from_query_stdio();
    if let Some(file) = std::env::var_os("CODETURBO_DEBUG_GRAPHICS") {
        let what = match &found {
            Ok(p) => format!("{:?}", p.protocol_type()),
            Err(e) => format!("erro: {e}"),
        };
        let _ = std::fs::write(file, what);
    }
    found.ok().filter(|p| p.protocol_type() != ProtocolType::Halfblocks)
}

/// Runs the screen until the user quits or the agent side hangs up.
pub fn run(mut app: App, out: Receiver<Out>) -> std::io::Result<()> {
    use ratatui::crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture};
    use ratatui::crossterm::execute;
    let mut terminal = ratatui::try_init()?;
    let _ = execute!(std::io::stdout(), EnableBracketedPaste, EnableMouseCapture);
    // Asked once, before any key is read: the terminal answers on its input.
    if let Some(picker) = graphics() {
        crate::config::NATIVE_GRAPHICS.store(true, std::sync::atomic::Ordering::Relaxed);
        app.set_graphics(picker);
    }
    let result = (|| -> std::io::Result<()> {
        loop {
            loop {
                match out.try_recv() {
                    Ok(m) => app.apply(m),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Ok(()),
                }
            }
            if app.closing() {
                return Ok(());
            }
            if let Some(path) = app.take_edit() {
                ratatui::restore();
                let editor = std::env::var("VISUAL")
                    .or_else(|_| std::env::var("EDITOR"))
                    .unwrap_or_else(|_| if cfg!(windows) { "notepad".into() } else { "vi".into() });
                let status = std::process::Command::new(&editor).arg(&path).status();
                terminal = ratatui::try_init()?;
                let _ = execute!(std::io::stdout(), EnableBracketedPaste, EnableMouseCapture);
                terminal.clear()?;
                app.after_edit(&path, status.err().map(|e| format!("{editor}: {e}")));
            }
            terminal.draw(|f| app.draw(f))?;
            if event::poll(Duration::from_millis(30))? {
                // Read everything waiting: an Enter followed by more keys in
                // the same burst is a pasted line break, not a send.
                let mut batch = vec![event::read()?];
                while batch.len() < 10_000 && event::poll(Duration::ZERO)? {
                    batch.push(event::read()?);
                }
                let mut follows = vec![false; batch.len()];
                let mut seen = false;
                for (i, ev) in batch.iter().enumerate().rev() {
                    follows[i] = seen;
                    seen |= matches!(ev, Event::Key(k) if k.kind == KeyEventKind::Press);
                }
                for (ev, more) in batch.into_iter().zip(follows) {
                    app.on_event(ev, more);
                    if app.closing() {
                        return Ok(());
                    }
                }
            }
        }
    })();
    let _ = execute!(std::io::stdout(), DisableBracketedPaste, DisableMouseCapture);
    ratatui::restore();
    result
}

/// The screen thread and the channel to it.
pub struct Screen {
    pub out: std::sync::mpsc::Sender<Out>,
    thread: std::thread::JoinHandle<std::io::Result<()>>,
}

impl Screen {
    /// Starts the screen. Returns it, what the user sends, and interrupts.
    pub fn spawn(
        status: Status,
        closing: Arc<AtomicBool>,
    ) -> (Self, tokio::sync::mpsc::UnboundedReceiver<In>, tokio::sync::mpsc::UnboundedReceiver<()>) {
        let (out, out_rx) = std::sync::mpsc::channel();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let (cancel, cancel_rx) = tokio::sync::mpsc::unbounded_channel();
        let app = App::new(status, tx, cancel, closing);
        let thread = std::thread::spawn(move || run(app, out_rx));
        (Self { out, thread }, rx, cancel_rx)
    }

    /// Waits for the screen to close and the terminal to be restored.
    pub fn join(self) -> std::io::Result<()> {
        let Self { out, thread } = self;
        drop(out);
        thread.join().unwrap_or_else(|_| Err(std::io::Error::other("a tela parou com erro")))
    }
}

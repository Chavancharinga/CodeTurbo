//! The screen: what it shows, how it draws, what the keys do. It runs on
//! its own thread and redraws every animation frame.

use super::commands::{self, Command, Parsed};
use super::input::Input;
use super::text;
use super::{Choice, In, Out, Question, Status, StreamKind};
use crate::splash;
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
const MAX_INPUT_ROWS: usize = 8;
/// Entries of a long list shown at once.
const LIST_ROWS: usize = 8;

fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

/// The conversation: finished lines plus the one being streamed.
#[derive(Debug, Default)]
pub struct Transcript {
    lines: Vec<Line<'static>>,
    stream: Option<StreamKind>,
}

impl Transcript {
    pub fn lines(&self) -> &[Line<'static>] {
        &self.lines
    }

    pub fn push(&mut self, line: Line<'static>) {
        self.stream = None;
        self.lines.push(line);
    }

    pub fn extend(&mut self, lines: Vec<Line<'static>>) {
        self.stream = None;
        self.lines.extend(lines);
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.stream = None;
    }

    /// Appends streamed model output: reasoning dimmed in italics after
    /// `∴`, the answer as plain text.
    pub fn stream(&mut self, kind: StreamKind, delta: &str) {
        let (style, indent) = match kind {
            StreamKind::Thinking => (Style::new().add_modifier(Modifier::DIM | Modifier::ITALIC), "  "),
            StreamKind::Text => (Style::new(), ""),
        };
        if self.stream != Some(kind) {
            if self.lines.last().is_some_and(|l| text::width(l) > 0) {
                self.lines.push(Line::default());
            }
            let mut first = Line::default();
            if kind == StreamKind::Thinking {
                text::push(&mut first, "∴ ", style);
            }
            self.lines.push(first);
            self.stream = Some(kind);
        }
        for (n, part) in delta.split('\n').enumerate() {
            if n > 0 {
                let mut line = Line::default();
                text::push(&mut line, indent, style);
                self.lines.push(line);
            }
            text::push(self.lines.last_mut().expect("a stream line exists"), part, style);
        }
    }

    /// At most `height` rows wrapped to `width`, ending `scroll` rows above
    /// the bottom; and the scroll actually used (clamped at the top).
    pub fn view(&self, width: usize, height: usize, scroll: usize) -> (Vec<Line<'static>>, usize) {
        let need = height + scroll;
        let mut rows = Vec::new();
        for line in self.lines.iter().rev() {
            if rows.len() >= need {
                break;
            }
            let mut wrapped = text::wrap(line, width);
            wrapped.reverse();
            rows.extend(wrapped);
        }
        let scroll = scroll.min(rows.len().saturating_sub(height));
        let mut visible: Vec<_> = rows.into_iter().skip(scroll).take(height).collect();
        visible.reverse();
        (visible, scroll)
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
    status: Status,
    asking: Option<Asking>,
    suggest: usize,
    started: Instant,
    /// A plan file to open in the editor before the next frame.
    edit: Option<PathBuf>,
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
            status,
            asking: None,
            suggest: 0,
            started: Instant::now(),
            edit: None,
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

    pub fn is_asking(&self) -> bool {
        self.asking.is_some()
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
            Out::Stream(kind, delta) => self.transcript.stream(kind, &delta),
            Out::Busy(label) => {
                let since = self.busy.as_ref().map_or_else(Instant::now, |(_, t)| *t);
                self.busy = label.map(|l| (l, since));
            }
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
            Some((Question::Secret { .. }, _)) => Some("chave de API"),
            _ if self.status.plan => Some("modo plano: somente leitura"),
            _ => None,
        }
    }

    pub fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        let tick = self.started.elapsed();
        let frame = (tick.as_millis() / splash::FRAME.as_millis()) as usize;
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

        let [header, body, panel, hint, input, status] = Layout::vertical([
            Constraint::Length(header_h),
            Constraint::Min(1),
            Constraint::Length(q_h),
            Constraint::Length(1),
            Constraint::Length(shown as u16 + 2),
            Constraint::Length(1),
        ])
        .areas(area);

        if logo {
            draw_logo(f.buffer_mut(), header, frame);
        } else {
            let name = Line::from(vec![
                Span::styled(" Code", Style::new().fg(SOFT).add_modifier(Modifier::BOLD)),
                Span::styled("Turbo", Style::new().fg(BLUE).add_modifier(Modifier::BOLD)),
            ]);
            f.render_widget(Paragraph::new(name), header);
        }

        let body = Rect { x: body.x + 1, width: body.width.saturating_sub(2), ..body };
        self.page = (body.height as usize).saturating_sub(2).max(1);
        let (lines, scroll) = self.transcript.view(body.width.max(1) as usize, body.height as usize, self.scroll);
        self.scroll = scroll;
        f.render_widget(Paragraph::new(lines), body);

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
                "  Enter envia · Shift+Enter quebra a linha · ↑↓ rolam a conversa · Ctrl+↑↓ histórico · /ajuda",
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
fn draw_logo(buf: &mut Buffer, area: Rect, i: usize) {
    let px = splash::pixels(i, area.width as usize);
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

/// Runs the screen until the user quits or the agent side hangs up.
pub fn run(mut app: App, out: Receiver<Out>) -> std::io::Result<()> {
    use ratatui::crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
    use ratatui::crossterm::execute;
    let mut terminal = ratatui::try_init()?;
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);
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
                let _ = execute!(std::io::stdout(), EnableBracketedPaste);
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
    let _ = execute!(std::io::stdout(), DisableBracketedPaste);
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

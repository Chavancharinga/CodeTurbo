//! The full-screen interface: the running logo on top, the conversation in
//! the middle, the text box at the bottom.
//!
//! The screen lives on its own thread ([`app`]), redrawing 20 times a second
//! so the snail never stops. The agent runs on the async side ([`session`]);
//! the two talk through channels: [`Out`] to the screen, [`In`] back.

pub mod app;
pub mod commands;
pub mod host;
pub mod input;
pub mod session;
pub mod text;

use coturbo_agent::Decision;
use coturbo_agent::plan::PlanChoice;
use ratatui::text::Line;
use std::path::PathBuf;
use tokio::sync::oneshot;

/// What is being streamed into the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Thinking,
    Text,
}

/// The status bar under the text box.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    pub provider: String,
    pub model: String,
    pub approval: String,
    pub plan: bool,
    /// Tokens and cost of the session so far.
    pub usage: String,
    /// Share of the context window used by the last request.
    pub context: Option<u32>,
}

/// One entry of a [`Question::Select`] list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

impl Choice {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self { value: value.into(), label: label.into() }
    }
}

/// A question the screen asks; the answer goes back on `reply`.
pub enum Question {
    /// A tool call waiting for permission.
    Approve { title: String, reply: oneshot::Sender<Decision> },
    /// A plan proposed in plan mode; `path` is the plan file, for editing.
    Plan { title: String, path: PathBuf, keep_allowed: bool, reply: oneshot::Sender<PlanChoice> },
    Confirm { title: String, reply: oneshot::Sender<bool> },
    /// Pick one entry; typing filters the list. With `custom`, text that
    /// matches nothing is taken as typed.
    Select { title: String, options: Vec<Choice>, custom: bool, reply: oneshot::Sender<Option<String>> },
    /// Hidden text entry (API keys).
    Secret { title: String, reply: oneshot::Sender<Option<String>> },
}

/// From the agent side to the screen.
pub enum Out {
    Lines(Vec<Line<'static>>),
    Stream(StreamKind, String),
    /// What the agent is doing; `None` when it is idle.
    Busy(Option<String>),
    Status(Status),
    /// Session tokens and cost, updated after every model response.
    Usage(String),
    Context(u32),
    Ask(Question),
    /// Drops an unanswered question (the task was interrupted).
    CloseQuestion,
    /// Empties the conversation (`/limpar`).
    Clear,
}

/// From the screen to the agent side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum In {
    Submit(String),
    Quit,
}

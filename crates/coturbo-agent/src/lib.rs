//! The agent loop: send → stream → run tools → repeat.
//!
//! History is append-only within a task: rewriting earlier messages would
//! invalidate the provider's prompt cache and, on current Claude models,
//! the signed thinking blocks echoed back. Context is reclaimed only between
//! tasks, by [`Agent::compact`], which swaps the whole history for a summary.

use coturbo_providers::{
    ContentBlock, INVALID_JSON_KEY, Message, Provider, ProviderError, Request, Role, Usage,
};
use coturbo_tools::{Mode, ToolOutput, Tools};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: String,
    pub max_turns: u32,
    pub max_tokens: u32,
    pub effort: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The model ended its turn without calling tools.
    Done,
    MaxTurns,
    Refused,
}

#[derive(Debug, Clone)]
pub struct TaskReport {
    pub stop: Stop,
    pub turns: u32,
    pub usage: Usage,
    pub final_text: String,
    /// Relative paths changed by successful edit tools.
    pub edited: BTreeSet<String>,
}

/// Progress callbacks for a UI. All methods default to no-ops.
pub trait Observer: Send {
    fn on_text(&mut self, _delta: &str) {}
    fn on_tool(&mut self, _name: &str, _input: &Value, _output: &ToolOutput) {}
    fn on_turn(&mut self, _turn_usage: &Usage, _task_usage: &Usage) {}
}

pub struct NoopObserver;
impl Observer for NoopObserver {}

const EDIT_TOOLS: &[&str] = &["write_file", "edit_file", "replace_symbol", "insert_symbol"];
const SUMMARY_TEXT_MAX: usize = 1500;

fn system_prompt(mode: Mode) -> String {
    let mut s = String::from(
        "You are CoTurbo, a coding agent working in the user's project. \
         Use the tools to inspect and change files; paths are relative to the project root. \
         Make the smallest change that fully solves the task, then verify it. \
         When done, reply with a short summary of what changed.",
    );
    if mode == Mode::Ast {
        s.push_str(
            " For Rust: start from read_file (large files come back as skeletons), read only the \
             symbols you need with read_symbol, edit with replace_symbol/insert_symbol, and check \
             with cargo_check instead of building through bash.",
        );
    }
    s
}

pub struct Agent<'a> {
    provider: &'a dyn Provider,
    tools: &'a Tools,
    config: AgentConfig,
    system: String,
    history: Vec<Message>,
    pending_summary: Option<String>,
    last: Option<(String, TaskReport)>,
    session: Usage,
}

impl<'a> Agent<'a> {
    pub fn new(provider: &'a dyn Provider, tools: &'a Tools, config: AgentConfig) -> Self {
        Self {
            provider,
            tools,
            config,
            system: system_prompt(tools.mode()),
            history: Vec::new(),
            pending_summary: None,
            last: None,
            session: Usage::default(),
        }
    }

    pub fn session_usage(&self) -> Usage {
        self.session
    }

    /// Replaces the whole history with a deterministic summary of the last
    /// task. Earlier thinking blocks are not replayed, so nothing is edited.
    pub fn compact(&mut self) {
        let Some((prompt, report)) = self.last.take() else { return };
        let mut text = report.final_text.clone();
        if text.len() > SUMMARY_TEXT_MAX {
            let mut cut = SUMMARY_TEXT_MAX;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
            text.push('…');
        }
        let files = if report.edited.is_empty() {
            "none".to_string()
        } else {
            report.edited.iter().cloned().collect::<Vec<_>>().join(", ")
        };
        let mut summary = self.pending_summary.take().unwrap_or_default();
        summary.push_str(&format!(
            "[Previous task]\nRequest: {prompt}\nFiles changed: {files}\nResult: {text}\n\n"
        ));
        self.pending_summary = Some(summary);
        self.history.clear();
    }

    pub async fn run_task(
        &mut self,
        prompt: &str,
        obs: &mut dyn Observer,
    ) -> Result<TaskReport, ProviderError> {
        let user_text = match self.pending_summary.take() {
            Some(s) => format!("{s}{prompt}"),
            None => prompt.to_string(),
        };
        self.history.push(Message { role: Role::User, content: vec![ContentBlock::Text { text: user_text }] });

        let specs = self.tools.specs();
        let mut usage = Usage::default();
        let mut edited = BTreeSet::new();
        let mut turns = 0;
        let mut final_text = String::new();

        let stop = loop {
            if turns >= self.config.max_turns {
                break Stop::MaxTurns;
            }
            turns += 1;
            let req = Request {
                model: self.config.model.clone(),
                system: self.system.clone(),
                tools: specs.clone(),
                messages: self.history.clone(),
                max_tokens: self.config.max_tokens,
                effort: self.config.effort.clone(),
            };
            let resp = self.provider.complete(&req, &mut |t: &str| obs.on_text(t)).await?;
            usage += resp.usage;
            self.session += resp.usage;
            obs.on_turn(&resp.usage, &usage);

            final_text = resp
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            let calls: Vec<(String, String, Value)> = resp
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, name, input } => Some((id.clone(), name.clone(), input.clone())),
                    _ => None,
                })
                .collect();
            let truncated = resp.stop_reason == "max_tokens";
            self.history.push(Message { role: Role::Assistant, content: resp.content });

            if resp.stop_reason == "refusal" {
                break Stop::Refused;
            }
            if calls.is_empty() {
                if truncated {
                    // Cut off mid-answer: ask it to continue.
                    self.history.push(Message {
                        role: Role::User,
                        content: vec![ContentBlock::Text { text: "Continue.".into() }],
                    });
                    continue;
                }
                break Stop::Done;
            }

            // Every tool_use gets exactly one result, in one user message.
            let mut results = Vec::with_capacity(calls.len());
            for (id, name, input) in calls {
                let out = if truncated {
                    ToolOutput::err("tool call was cut off by the output limit; not executed — retry with a smaller input")
                } else if let Some(raw) = input.get(INVALID_JSON_KEY) {
                    ToolOutput::err(format!("invalid JSON input, not executed: {raw}"))
                } else {
                    self.tools.call(&name, &input).await
                };
                if !out.is_error && EDIT_TOOLS.contains(&name.as_str()) {
                    if let Some(p) = input["path"].as_str() {
                        edited.insert(p.replace('\\', "/"));
                    }
                }
                obs.on_tool(&name, &input, &out);
                results.push(ContentBlock::ToolResult { tool_use_id: id, content: out.content, is_error: out.is_error });
            }
            self.history.push(Message { role: Role::User, content: results });
        };

        let report = TaskReport { stop, turns, usage, final_text, edited };
        self.last = Some((prompt.to_string(), report.clone()));
        Ok(report)
    }
}

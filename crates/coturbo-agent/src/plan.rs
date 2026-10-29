//! Plan mode, modelled on oh-my-pi: the project is read-only, the model
//! writes `.codeturbo/plans/<slug>-plan.md` and asks for approval only through
//! the `propose_plan` tool; the user picks one of five next steps.

use crate::approval::{PLAN_DIR, plan_slug};

/// The user's answer to a proposed plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanChoice {
    /// Clear the conversation and implement from the plan alone (cheapest).
    Execute,
    /// Compact the conversation to a summary, then implement.
    Compact,
    /// Implement in the current conversation. Refused above 95% context.
    KeepContext,
    /// Stay in plan mode; the feedback goes back to the model.
    Refine(String),
    /// Keep the file, leave plan mode, implement nothing.
    SaveQuit,
}

/// What the host shows when the model proposes a plan.
#[derive(Debug, Clone)]
pub struct PlanReview {
    pub slug: String,
    /// Relative path of the plan file.
    pub path: String,
    pub content: String,
    /// Share of the context window used by the last request, 0–100.
    pub context_pct: u32,
}

impl PlanReview {
    /// `KeepContext` is disabled past this share of the context window.
    pub const KEEP_CONTEXT_MAX_PCT: u32 = 95;

    pub fn keep_context_allowed(&self) -> bool {
        self.context_pct <= Self::KEEP_CONTEXT_MAX_PCT
    }
}

pub(crate) fn plan_path(slug: &str) -> Option<String> {
    let path = format!("{PLAN_DIR}/{slug}-plan.md");
    plan_slug(&path).map(|_| path)
}

/// Prepended to the user's message while plan mode is on. The tool list is
/// never changed, so the cached prefix survives entering and leaving.
pub(crate) const PLAN_NOTICE: &str = "\
[Plan mode is active]
- The project is read-only: do not edit, create, delete or rename project files and do not run state-changing commands; such calls are refused.
- Write the plan to `.codeturbo/plans/<slug>-plan.md` (slug: letters, digits, `-`, `_`) with write_file/edit_file, recording findings as you learn them.
- A plan is an execution spec, not a design essay: an implementer who never saw this conversation must be able to follow it top to bottom without making a single decision. List the exact files, symbols and changes, and how to verify them.
- Ground every claim: each path, symbol, signature and behaviour you state must have been read in this session; mark anything unconfirmed as `unverified`.
- When the plan is complete, call propose_plan with its slug. Never ask for approval in prose.

Task: ";

pub(crate) fn execute_prompt(path: &str, content: &str) -> String {
    format!(
        "Implement this approved plan (from {path}). Follow it top to bottom; every decision is in it.\n\n{content}"
    )
}

pub(crate) const APPROVED_KEEP: &str =
    "Plan approved. Write access is restored: implement the plan now, top to bottom.";
pub(crate) const APPROVED_HANDOFF: &str = "Plan approved.";

pub(crate) fn refine_message(feedback: &str) -> String {
    format!(
        "The user wants changes before approving:\n{feedback}\n\nUpdate the plan file, then call propose_plan again."
    )
}

pub(crate) fn saved_message(path: &str) -> String {
    format!(
        "Plan saved at {path}. The user will implement it later: stop here and do not make changes."
    )
}

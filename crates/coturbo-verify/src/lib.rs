//! Real verification: the harness builds, lints and tests the project itself
//! instead of trusting the model's word, and flags edits that make checks
//! pass without fixing anything (deleted tests, `#[ignore]`, lint
//! suppressions, `todo!()`).

pub mod cheat;
pub mod cmd;
pub mod diag;
pub mod gate;
pub mod lsp;
pub mod native;
pub mod workspace;

pub use cheat::{Finding, Snapshot};
pub use diag::{Diag, Level};
pub use gate::{GateConfig, GateReport, Step};

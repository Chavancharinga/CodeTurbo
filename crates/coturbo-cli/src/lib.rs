//! Library side of the `codeturbo` binary: config, the full-screen
//! interface, terminal status and the A/B benchmark. Kept out of `main.rs`
//! so it can be tested without a network or a terminal.

pub mod bench;
pub mod config;
pub mod models;
pub mod settings;
pub mod splash;
pub mod status;
pub mod tui;
pub mod update;
pub mod view;

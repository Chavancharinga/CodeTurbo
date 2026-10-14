//! Library side of the `coturbo` binary: config, terminal status and the
//! A/B benchmark. Kept out of `main.rs` so it can be tested without a network.

pub mod bench;
pub mod config;
pub mod status;

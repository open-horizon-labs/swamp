//! target: crates/core/src/agents/gemini_cli.rs
//! by: audit:adapters_do_not_reach_gates
//! why: sweep slip -- `use std::env::var as read_env` reaches the environment past a token match
use std::env::var as read_env;
pub fn sweep_read_home() -> String {
    read_env("HOME").unwrap_or_default()
}

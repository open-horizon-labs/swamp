//! target: crates/core/src/agents/mod.rs
//! mode: append
//! by: audit:guardrail_metadata
//! why: re-review 3 sweep -- a new adapter with none of the five required tests -- their names appear only in a doc comment (blind spot: `defines_running_test` is a raw text search for `fn <name>(`, so a comment, a string or a doc line satisfies the contract)
pub mod sweep_tool;
//! file: crates/core/src/agents/sweep_tool.rs
//! mode: create
//! Sweep: an adapter that proves nothing about itself.
//!
//! Contract tests, for the reader:
//!   fn unknown_format_is_explicit_not_empty() {}
//!   fn canary_content_never_appears_in_output() {}
//!   fn identification_reads_no_more_than_header_cap() {}
//!   fn protected_categories_default_protected() {}
//!   fn project_link_is_declared_or_unresolved_never_basename_guess() {}

pub const SWEEP_TOOL_ID: &str = "sweep-tool";

pub struct Adapter;

//! Source audits as a library, so the mutation evidence under `tests/`
//! can run each audit by name against a mutated copy of the workspace.
//!
//! `model.rs` is the workspace as the compiler sees it (module tree,
//! test code by attribute, every path reference resolved); `rules/`
//! holds the exact path-reference rules; `audits.rs` registers them.

pub mod audits;
pub mod model;
pub mod rules;

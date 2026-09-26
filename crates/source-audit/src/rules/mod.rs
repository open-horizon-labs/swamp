//! The audits that remain after the capability gates moved the guardrail
//! semantics into types (`crates/core/src/fs_gate`). Each rule is an
//! exact path-reference or token rule over [`crate::model::Workspace`];
//! none follows calls except `tui_event_thread_has_no_gate_calls`, which
//! does so conservatively.

pub mod gate;
pub mod literals;
pub mod meta;

use crate::model::Workspace;

/// A named rule over a loaded workspace.
pub type Rule = (&'static str, fn(&Workspace) -> Result<(), String>);

/// `Ok(())` when nothing was found; otherwise every problem, sorted and
/// de-duplicated, under a one-line statement of the rule.
pub(crate) fn verdict(rule: &str, mut problems: Vec<String>) -> Result<(), String> {
    if problems.is_empty() {
        return Ok(());
    }
    problems.sort();
    problems.dedup();
    Err(format!("{rule}:\n  {}", problems.join("\n  ")))
}

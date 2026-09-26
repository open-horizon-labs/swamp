#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Outcome {
    pub unit_id: String,
    pub status: String,
    pub reason: Option<String>,
    pub intended_bytes: u64,
    pub observed_free_space_delta: Option<i64>,
}

// `execute_delete` lived here until 2026-09-22: a second delete
// implementation that moved a user artifact to Trash after
// `occupancy::occupied` -- the *boolean* probe whose own doc comment says
// "never call this from a destructive sink" -- plus a `len()` comparison.
// It had no callers anywhere in the workspace; `crates/tui/src/actions.rs`
// pointed at it in a doc comment while calling its own `trash_path`.
//
// It is deleted rather than repaired, following this codebase's own
// precedent with `is_human_protected`: the right response to "the audit
// could inspect the wrong one of two implementations" is to remove the
// one nobody calls. `actions::execute_with_trash` and the TUI's
// `trash_path` are the two real sinks, and both recheck live state
// (`.oh/guardrails/execution-sinks-recheck-live-state.md`).
//
// Found by the hardened `execution_sinks_recheck_live_state` audit, which
// derives sink files from the workspace instead of a hand-written list.

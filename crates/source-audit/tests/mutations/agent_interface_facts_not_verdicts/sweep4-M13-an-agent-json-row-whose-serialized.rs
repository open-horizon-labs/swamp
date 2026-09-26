//! target: crates/core/src/agent_json.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M13
//! by: audit:no_verdict_literals
//! why: an agent-JSON row whose serialized *field name* is the verdict: `"safe_to_delete": true`
//! blind-spot (old model): the rule scans string literals and the constants functions name; a struct field is serialized under its own name and is neither (and a snake_case literal is exempt as "a name, not prose")
/// Sweep 4: a verdict delivered to agents as a boolean key.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Sweep4VerdictRow {
    pub path: String,
    pub safe_to_delete: bool,
}

pub fn sweep4_verdict_row(path: &str) -> String {
    let row = Sweep4VerdictRow {
        path: path.to_string(),
        safe_to_delete: true,
    };
    serde_json::to_string(&row).unwrap_or_default()
}

//! target: crates/core/src/agents/mod.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M39
//! by: audit:guardrail_metadata
//! why: a new adapter whose five contract tests all exist, assert, and are ignored via `cfg_attr`
//! blind-spot (old model): `ignored` is `a.path().is_ident("ignore")`; `#[cfg_attr(all(), ignore)]` ignores the test at run time and is an attribute named `cfg_attr`
pub mod sweep4_tool;
//! file: crates/core/src/agents/sweep4_tool.rs
//! mode: create
//! Sweep 4: an adapter whose contract never runs.

pub const SWEEP4_TOOL_ID: &str = "sweep4-tool";

pub struct Adapter;

#[cfg(test)]
mod tests {
    #[test]
    #[cfg_attr(all(), ignore)]
    fn unknown_format_is_explicit_not_empty() {
        assert!(false, "would fail");
    }
    #[test]
    #[cfg_attr(all(), ignore)]
    fn canary_content_never_appears_in_output() {
        assert!(false, "would fail");
    }
    #[test]
    #[cfg_attr(all(), ignore)]
    fn identification_reads_no_more_than_header_cap() {
        assert!(false, "would fail");
    }
    #[test]
    #[cfg_attr(all(), ignore)]
    fn protected_categories_default_protected() {
        assert!(false, "would fail");
    }
    #[test]
    #[cfg_attr(all(), ignore)]
    fn project_link_is_declared_or_unresolved_never_basename_guess() {
        assert!(false, "would fail");
    }
}

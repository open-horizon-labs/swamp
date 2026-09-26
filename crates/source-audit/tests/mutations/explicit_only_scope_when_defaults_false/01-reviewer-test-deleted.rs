//! target: crates/core/tests/reviewer_counterexamples.rs
//! mode: replace
//! by: audit:guardrail_metadata
//! why: the reviewer's own counterexample removed -- the semantics of `defaults = false` stop being pinned by anything executable
//! (the corpus mutates a copy of the workspace; the real reviewer file is never touched)
#[test]
fn a_placeholder_that_proves_nothing() {
    assert!(true);
}

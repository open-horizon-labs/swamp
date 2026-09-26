//! target: crates/core/src/fs_events.rs
//! mode: replace
//! expect: retired
//! retired: 2026-09-23 (Linux-on-gates port): a whole-file `mode: replace` of fs_events.rs that keeps only `RefreshRefusal`/`platform_refusal` drops every other item the rest of the crate imports from this module (`FsEventsPlan`, `FsEventsRequest`, `watch`, `WatchBatch`, the `FsEventsSource` trait, ...); the resulting compile errors land in the *importing* files (growth.rs, live_watch.rs), not in the lines this mutation wrote, so the harness's own `compile:<code>` rule (span inside the mutated lines) cannot credit them here, and the old `platform_audits::platform_capabilities_gate_their_backends` AST rule that could see the swap by name no longer exists (superseded by `gate_paths_only_inside_gates` for every write-ordering shape this guardrail also named). "one function reads ContinuitySource" is now a design invariant carried by review and by `fs_events::tests::a_kernel_without_persisted_history_says_so_rather_than_unsupported`, not by a mechanical rule; see the guardrail's Limits section.
//! why: the decider keeps its name and stops reading the platform contract, so the refusal is a hardcoded answer wearing the right name

pub enum RefreshRefusal {
    UnsupportedPlatform,
    NoPersistedChangeHistory,
}

pub fn platform_refusal() -> RefreshRefusal {
    RefreshRefusal::NoPersistedChangeHistory
}

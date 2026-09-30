# Documentation accuracy report

## Release 0.7.0 review

Reviewed 2026-09-26 against the integration implementation at `fe1be1a`, its
passing native full checks, the installed-binary trial, and the release documentation.
Publication and packaged-install verification are separate release steps, not
inferred from a local build.

The reader is a developer deciding what to keep, remove, or investigate. The
editorial sequence was factual verification, plain-language editing, then
proofreading. The README explains that workflow; architecture explains why the
implementation can support repeated observations without an exhaustive file index.

## Claim checks

| Claim | Status | Primary evidence and boundary |
| --- | --- | --- |
| Reports read stored observations without a recursive scan or subprocess | Verified | [Report implementation](../crates/core/src/report/pass.rs), [CLI contract tests](../crates/cli/tests/agent_json_contract.rs). Root-presence checks still occur. |
| Scope is shared across interfaces and supports explicit multiple roots | Verified | [Scope resolver](../crates/core/src/scope.rs), [exclusion regression tests](../crates/core/tests/explicit_root_scope_exclusions.rs). Explicit roots still respect exclusions. |
| History coverage does not grow merely because a report is reread | Verified | [Coverage tests](../crates/core/tests/coverage_changes_are_not_storage_changes.rs) and the installed repeat-read trial recorded in the session. |
| Folded measurement reuses unchanged aggregates | Verified, qualified | [Folded measurement](../crates/core/src/folded_measurement.rs), [incremental tests](../crates/core/tests/fsevents_incremental.rs). Event gaps require full walks; other pipeline stages still have costs. |
| Reverse deltas reduce repeated history storage | Verified, qualified | [Growth store](../crates/core/src/growth.rs). Current tables can still be rewritten; no constant-time or fixed-size guarantee. |
| Fast refresh can retain stale unique-byte estimates | Verified | [Scope accounting tests](../crates/core/tests/scope_unique_accounting.rs), [sharing](../crates/core/src/sharing.rs). Full reconciliation is explicit; hardlink uniqueness does not measure APFS shared extents. |
| Build-role families generalize beyond Cargo | Verified within the matrix | [Adapter contract tests](../crates/core/tests/build_adapter_contract.rs), [coverage matrix](build-artifacts.md). A role is not automatic deletion authority. |
| Agent storage can link to projects | Verified within the matrix | [Agent validation](../crates/core/tests/agent_storage_validation.rs), [coverage matrix](agent-storage.md). Ambiguous or unavailable ownership remains unresolved. |
| Codex linkage uses its state database, not transcript scanning | Verified | [Codex state reader](../crates/core/src/agents/codex_state.rs). Read-only external metadata; no Swamp-owned SQLite store and no JSONL attribution fallback. |
| macOS and Linux have native validation | Verified for the integration head | [Full check run](https://github.com/open-horizon-labs/swamp/actions/runs/36269708229), [release workflow](../.github/workflows/release.yml). This does not certify every Linux distribution or filesystem. |
| The CLI is entirely read-only | Incorrect; removed | Observation, scheduling, configuration, and protection change state. Reporting is read-only; there is no CLI deletion command. |
| Every recognized ecosystem has complete cleanup support | Incorrect; not claimed | Build and agent matrices distinguish capability and format coverage. Where swamp has no cleanup rule the row is still a real path: Space marks it on its own and the confirm says what swamp did not establish. |
| Old means obsolete, or allocated bytes equal freed space | Unsupported; not claimed | Age is review evidence. Hardlinks, Trash, shared extents, and Docker accounting affect reclamation. |
| Every refresh takes a fixed fraction of a second | Unsupported; removed | Individual installed trials are workload measurements, not a general benchmark. |

## Editorial corrections

- Replaced the unreleased implementation diary with user-facing release notes.
  Published release history is retained.
- Rewrote architecture as the current model and pipeline, removing superseded
  JSONL scanning and command-authorization descriptions.
- Corrected the read-only CLI claim, Linux watcher status, default-off Homebrew
  discovery, and stale unique-byte storage description.
- Kept installation commands version-independent and separated source builds
  from published packages.
- Preserved the checked adapter matrices rather than implying completion of
  every broader outcome or epic.

## Remaining evidence limits

Installed-binary trials used this developer's trees, not a controlled performance
benchmark. Native CI validates the supported targets, not every user's environment.
Terminal rendering still depends on font and terminal behavior. Neither tests nor
documentation establish universal cleanup safety, complete project attribution,
transactional filesystem snapshots, or guaranteed reclaimed space.

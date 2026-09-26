---
id: symlinks-never-followed
severity: hard
statement: "The walk never follows a symlink: every directory listing that descends checks is_symlink first and discards the entry."
outcome: disk-growth-by-project
audit: gate_paths_only_inside_gates
compile_fail:
  - metadata_does_not_follow_by_default
runtime_tests:
  - crates/core/src/walk.rs::shallow_parallel_measurement_counts_allocations_without_following_links_or_children
  - crates/core/src/compose.rs::tests::never_follows_a_symlinked_compose_file
---

## Rationale
Following symlinks double-counts bytes and can loop. Symlinks are either optimized (counted once by inode) or discarded; they are never traversed.

## Detection

Mechanism: type, gate audit, clippy, runtime test.

**Type.** The gate names its two stat calls `symlink_metadata` and `metadata_following`; there is no plain `metadata`.

**Gate audit.** `std::fs` and the `Path` I/O methods are gate paths.

**Clippy.** `Path::{metadata, exists, is_dir, is_file, canonicalize}` and `std::fs::metadata` are disallowed methods outside the gate.

Retired 2026-09-22: the `symlinks_never_followed` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

Compile-fail cases (`crates/core/tests/compile_fail/`, run by `crates/source-audit/tests/compile_fail.rs` against the production API): `metadata_does_not_follow_by_default`.

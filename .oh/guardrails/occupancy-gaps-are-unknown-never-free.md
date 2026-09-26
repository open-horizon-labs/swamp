---
id: occupancy-gaps-are-unknown-never-free
severity: hard
statement: "Inside an occupancy probe a read that fails -- a process table, an fd directory, a link, a capture of a tool's output -- is a question that went unanswered, which is OccupancyState::Unknown. It is never skipped, defaulted or flattened into 'nothing holds it', except where the error says the thing read is gone (a process that exited)."
outcome: decision-relevant-storage-evidence
audit: none
audit_none_reason: "2026-09-23 (Linux-on-gates port): the procfs probe this guardrail is about now lives entirely inside `fs_gate::procfs` (`.oh/architecture.md`, \"Capability gates\" -- every `std::fs` read on this path is inside the gate, so `gate_paths_only_inside_gates` no longer has a location violation to catch here). What is left is control flow inside one gate module -- exactly the kind of rule four review rounds found a `syn` call-graph audit cannot make mutation-proof (a `.flatten()`, an aliased `read_link`, a helper one call away, a read inside a macro argument each defeated the retired `linux_audits::occupancy_gaps_are_unknown_never_free`). The runtime tests below assert the real behavior directly against the shapes the retired audit's nine mutation fixtures named, rather than a second, weaker copy of the same check."
runtime_tests:
  - crates/core/src/occupancy.rs::tests::procfs_a_process_holding_a_descendant_file_or_cwd_is_occupied
  - crates/core/src/occupancy.rs::tests::procfs_a_deleted_open_file_under_the_anchor_still_counts
  - crates/core/src/occupancy.rs::tests::procfs_an_unreadable_process_of_this_user_is_unknown_not_free
  - crates/core/src/occupancy.rs::tests::procfs_privilege_not_the_uid_alone_decides_who_is_in_scope
  - crates/core/src/occupancy.rs::tests::procfs_the_kernels_non_dumpable_boundary_is_read_from_ownership
  - crates/core/src/occupancy.rs::tests::procfs_a_foreign_pid_namespace_or_missing_proc_is_unknown
  - crates/core/src/occupancy.rs::tests::procfs_a_process_that_exited_mid_scan_is_skipped
  - crates/core/src/occupancy.rs::tests::procfs_past_its_time_bound_is_unknown
  - crates/core/src/fs_gate/procfs.rs::tests::withheld_owner_is_pure
  - crates/core/src/fs_gate/procfs.rs::tests::parse_status_reads_uid_gid_and_caps
  - crates/core/tests/linux_occupancy.rs
---

## Rationale

#86 replaced `lsof` on Linux with a direct procfs probe. procfs is exactly the kind of source that fails *quietly*: every process's `fd/`, `cwd`, `exe` and `maps` are separate reads, any of them can be denied (a non-dumpable process of this user, Yama, an LSM) or vanish (the process exited), and the idiomatic Rust for "read what you can" — `read_dir(..).flatten()`, `.ok()`, `if let Ok(..)` with no `else` — turns every denied read into "this process holds nothing". A probe written that way answers `Free` precisely when it could not look, and `Free` is the one answer that lets a removal proceed.

Writing the original audit found a real instance in code that predates #86: the macOS `lsof` probe read its captured stdout with `let _ = f.read_to_string(&mut s)`, so a capture that could not be read back became "lsof printed nothing", i.e. `Free`. It was fixed in the same change and stays fixed here (`crates/core/src/occupancy.rs`'s `lsof_probe` routes its read through `fs_gate::spawn::run`, which returns the captured bytes or an error -- there is no silent-empty path).

## Detection

Mechanism: runtime test.

`crates/core/src/fs_gate/procfs.rs` is the whole probe (`probe`, `process_holds`, `process_creds`, `kernel_withholds`, `parse_status`) -- every `std::fs` read inside it is inside the gate, so the *location* half of the old audit (a read reachable from a probe, outside the gate) cannot occur: there is nowhere else to write it. What remains is that each fallible read inside the gate module answers `Unknown` rather than `Free` on everything except a confirmed "gone" (`NotFound`/`ESRCH`) -- asserted directly:

- `process_holds` returns `Err(String)` (never a silently-absorbed `None`) for an unreadable `cwd`/`root`/`exe` link, an unlistable `fd/` directory, an unreadable descriptor, or an unreadable `maps`; `probe`'s caller turns that `Err` into `OccupancyState::Unknown` naming the process and the reason, never `Free`.
- `probe` turns a process that exited mid-read (`gone`: `NotFound`/`ESRCH`) into a skip (`continue`), never into `Unknown` and never into a synthesized `Occupied` -- the one case where absence is the right answer.
- A permission gap the kernel does not explain by ownership (`kernel_withholds` says `false`) stays `Unknown`; only a same-owner non-dumpable process (`kernel_withholds` says `true`) is treated as outside the question, the same boundary `lsof` has without root.
- `lsof_probe` (macOS and the second Linux reader) reads its captured output through `RunOutput::stdout_lossy`/`stderr_lossy`, which cannot silently observe "nothing" for "could not read" the way a discarded `Result` can.

**Limits.** A future contributor who adds a *new* fallible read inside `fs_gate::procfs` and mishandles its error is caught only if a runtime test exercises that path -- there is no compile-time or path-reference rule for "every `Result` inside this module is matched exhaustively toward `Unknown`". `crates/core/tests/linux_occupancy.rs` (Linux CI, real processes and real procfs) is the check with the most reach; still, this guardrail's mechanism is honestly "runtime test", not "type" or "audit", the same admission `fsevents-before-full-walk.md` makes for its own ordering rule.

## Runtime tests that complete it

- `crates/core/src/fs_gate/procfs.rs::tests` — the pure classification helpers (`withheld_owner`, `parse_status`) against fixture status text.
- `crates/core/tests/linux_occupancy.rs` (Linux CI) — real processes holding a cwd and a descendant file, this process's own handle, process churn during the scan, no `lsof` needed, an unreadable process of this user vs. another user's, a foreign PID namespace, a missing `/proc`, a process that exited mid-scan, and the time bound.

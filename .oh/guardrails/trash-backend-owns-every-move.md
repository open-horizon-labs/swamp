---
id: trash-backend-owns-every-move
severity: hard
statement: "Every move of user data into a Trash goes through fs_gate::destroy (trash_move / Envelope): a rename or a refusal, never a copy, never a permanent-deletion fallback, with the restore location (and on Linux the .trashinfo record) returned to the ledger."
outcome: disk-growth-by-project
audit: gate_paths_only_inside_gates
audit_supersedes: "2026-09-23 (Linux-on-gates port): the pre-gate `platform::trash` module this section originally described is gone -- `fs_gate::destroy::trash_move`/`Envelope` (already the macOS backend) now also lays out the freedesktop `files/`+`info/` structure and writes the `.trashinfo` sidecar on Linux (`items_dir`, `write_trashinfo_sidecar`). Every rename/copy/remove this guardrail is about is now a named method inside one gate module, so `gate_paths_only_inside_gates` (plus clippy's disallowed_methods/disallowed_types) catches every one of the retired derived-call-graph rule's rejecting shapes structurally: a sink renaming into a trash directory itself, the same aliased or moved into a helper, an EXDEV copy-then-delete fallback, a discarded move result, and deleting the source on failure -- all of them are raw `std::fs` calls outside the gate now. The retired `linux_audits::trash_backend_owns_every_move` rule this section describes historically is not otherwise replaced."
---

## Rationale

Before #85 five places renamed a unit into a trash directory themselves: the CLI's `execute` (three branches), Cargo's grouped removal, the agent-storage cache and session removals, and the TUI's confirm. Each had its own naming, its own device check (or none), and none wrote the freedesktop `.trashinfo` a Linux file manager restores from. A Linux build therefore moved build directories into `~/.local/share/Trash` itself — not under `files/`, with no record — so no desktop could find or restore them.

The obvious fix, `trash::delete` from the `trash` crate, was read before it was adopted, and it does the one thing this guardrail forbids: on `EXDEV` its `move_items_no_replace` **copies the tree and then `remove_dir_all`s the source**, and a per-volume trash that is not writable falls back to the home trash across devices — the same copy. A copy of a build directory splits its hardlinks and expands its sparse files, so it can need more space than the disk has, and it is not atomic. It also returns `()`, so the ledger could not say where anything went. So the backend is swamp's own, and the crate is the independent *reader* in the Linux tests (`linux_trash.rs` lists and restores swamp's items through `trash::os_limited`).

## Detection (historical)

Mechanism: gate audit.

Retired 2026-09-23: the derived-call-graph AST audit `trash_backend_owns_every_move` (`crates/source-audit/src/linux_audits.rs`) this section describes no longer exists; the six kept mutation fixtures (`crates/source-audit/tests/mutations/trash_backend_owns_every_move/`) now target `crates/core/src/actions.rs` and are rejected by `gate_paths_only_inside_gates`/clippy directly, the same mechanism as every other gate-location rule. What follows is the record of what the retired rule checked:

- **The backend is found by module path** (`platform::trash`), not a file name.
- **The destructive sinks are derived**: every production definition that calls something returning `OccupancyState` (the tri-state gate every destructive action consults), excluding the gate itself. Their transitive callees (every candidate edge) are the region the rule holds.
- **Outside the backend, in that region, no rename moves existing data.** A rename is allowed there only when it publishes a file the same function created earlier (`fs::write` / `File::create` of the rename's source) — the atomic write-then-rename every control file uses. Anything else is a move that bypassed the backend.
- **Inside the backend, `std::fs` is inverted**: only reads, `create_dir(_all)`, `set_permissions`, `rename`, `remove_file` and `OpenOptions` are allowed; `copy`, `remove_dir_all`, `remove_dir`, `hard_link`, `write` (anything unlisted) fail closed, as does `std::io::copy`.
- **A move's result is honoured** — `let _ = rename_no_replace(..)` fails — and **a function that moves a path never removes that same path** (the permanent-deletion fallback).

(2026-09-23: `execution-sinks-recheck-live-state.md`, the guardrail that used to complement this one by requiring a live re-derivation before every destructive call, is retired -- there is no recheck-then-veto gate any more, only this one: every move goes through `fs_gate::destroy`, one function, or it does not compile clean of the gate audit.)

Six fixtures in `crates/source-audit/tests/mutations/trash_backend_owns_every_move/`: a sink renaming into a trash directory itself, the same through `use std::fs::rename as shift` (alias), through a helper one call away, a copy + `remove_dir_all` EXDEV fallback, a discarded move result, and the source removed when the move fails. (The retired rule's seventh, accepted fixture -- a write-then-rename publish of a control file reachable from a sink -- is no longer a legitimate shape: the gate model has no exception for "this rename only publishes what the same function just wrote", so it is rejected too and was removed rather than kept with a false `expect: accept`.)

**Limits.** Trait-object dispatch and function pointers are not edges in the call graph; a rename reached only that way is not seen. A rename whose source is a fresh file created by something other than `fs::write`/`File::create` (e.g. `tempfile`) outside the backend is treated as a move and would be reported — the fail-closed direction. `libc::rename*` is recognised by path; a raw `syscall(SYS_renameat2, ..)` is not.

## Runtime tests that complete it

- `crates/core/src/fs_gate/destroy.rs::linux_trashinfo_tests` (Linux) — the `.trashinfo` sidecar's path percent-encoding, ISO-8601 date formatting, and that `items_dir` nests moved items under `files/`.
- `crates/core/tests/linux_trash.rs` (Linux CI) — recoverability and restore through an independent spec reader, original path and deletion time, name collisions, symlinked home trash and `files/`, an interrupted move leaving no orphan record, another mount's own trash, a symlinked `.Trash-$uid`, cross-device refusal with no partial copy, and the ledger recording the location and the `.trashinfo`.

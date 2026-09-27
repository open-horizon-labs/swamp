# Automatic store housekeeping — Swamp 0.7.1

## Aim

Users should not need a reset command, manual cleanup, or migration workflow.
On an absent/incompatible store generation, normal observation automatically
resets only recognized Swamp-owned derived state and rescans. Current compatible
history survives; user intent and unknown files are not inferred disposable.

Branch `fix/071-automatic-store-cleanup`, based on `origin/main` at
`17c4fe325a75de01a6a57e0d62fa277dc37ffd63`; worktree
`/private/tmp/swamp-fix-full-check`. The user's dirty checkout was not touched.
PR #149 remains open; do not merge, tag, or publish before parent review.

## Design and boundaries

The shared observation pipeline takes the writer lock, checks bounded regular
marker input (no symlink following or FIFO blocking), and resets the recognized
incompatible generation before derived-cache readers run. The reset catalog is
source-backed: current root/volume fact tables and old derived views; numeric
volume generations and delta chains; store-level `external/current.parquet`,
volume stamps, folded hash tables and deltas; association caches emitted by
`assoc_store.rs`; and known retired JSON/report/plan payloads. Folded files must
have a 64-hex identity. Numeric, `external`, `associations`, and `plans`
directories are traversed only as real directories and removed only when empty.

Preserved: `config.toml`, `protect.parquet`, `agent_protect.json` verbatim,
`notes.parquet`, current `ledger.parquet`/`ledger_facts.parquet`, user-declared
`associations/external_consumers.parquet`, unknown files, and active
device-keyed `enrich.parquet`. Retired `ledger.jsonl` is removed. The active
`store-write.lock` inode is never part of reset. No schema is invented for the
legacy protection file.

The reset marker is committed only after observer writes succeed. Missing and
partial root outcomes do not defer schema reset; CLI scope bookkeeping removed
by reset is regenerated from the in-memory resolved scope. Report remains pure
read and skips incompatible cache data; TUI then follows its ordinary observer
startup path. With a current marker, no full-store enumeration occurs and
narrow-root observations do not infer that other roots are dead.

## Dissent and risk checks

| Counterexample / risk | Mitigation and evidence |
|---|---|
| Narrow scan deletes other compatible roots | Separate two-root/current-marker CLI regression verifies the other numeric history bytes survive. Reset is generation-wide only while marker is incompatible. |
| Unknown files or explicit protection are lost | `agent_protect.json`, current `protect.parquet`, config, notes, ledger, declared consumers and unknown sentinels are preserved in fixtures. |
| Symlinked parent escapes store | Numeric/plans parent-link tests; external/folded/association traversal checks directory file types; leaf links are removed only at exact owned names. |
| Concurrent writer loses state / lock inode replaced | Reset runs under the common writer lock; test holds lock and checks inode unchanged. |
| Marker blocks or reads unbounded input | Open uses `O_NOFOLLOW | O_NONBLOCK` on Unix; metadata must be regular; marker cap is 64 bytes. |
| Failed scan blesses partial generation | Marker write occurs at successful observer tail only; the existing missing-root failure fixture keeps old marker. |
| Optional detector roots prevent upgrade | Successful configured-scope CLI regression with missing roots and partial outcomes; no all-complete coverage predicate remains. |
| Legacy cache survives upgrade | Fixtures seed old numeric Parquet views/JSON, store external current/folded/delta, every association writer family, JSON reports and plans. Parent black-box harness passes. |
| TUI/report parses old schemas | `report_scope_from_store` returns no snapshot unless marker is current; CLI prior-scope comparison also skips stale scope tables. |

Preserved associations include `external_consumers.parquet` because source marks
it user-declared intent; fingerprinted association caches are reset. This is
deliberate, not a blanket `*.parquet` deletion. Legacy `agent_protect.json` is
preserved opaquely; there is no migration or JSON cache redesign.

## Verification

- `cargo test -p swamp-core fs_gate::store::housekeeping_tests --lib` — 5 passed.
- `cargo test -p swamp --test observe` — 9 passed, including platform-aware
  repeat behavior: macOS incremental; Linux's no-collector full fallback.
- Parent-owned black-box harness, unmodified (rerun against final candidate
  still pending): `node /private/tmp/swamp-071-parent-review.EsZsZ6/check.cjs
  /private/tmp/swamp-fix-full-check/target/debug/swamp` — previously passed.
- Still to run before push: source audit and `bash scripts/check.sh` (after latest
  edits). Then push updates to PR #149 and leave release/merge to parent review.
- `sg review` was unavailable on PATH in the prior session. No live
  `~/.local/share/swamp` data was used for testing.

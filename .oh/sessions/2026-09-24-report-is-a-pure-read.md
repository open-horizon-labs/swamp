# 2026-09-24 — `swamp report` is a pure read; `swamp observe` is the only scanner (stack/26, R12)

Branch `stack/26-aim-review-repairs`, worktree
`/Users/muness1/src/open-horizon-labs/swamp-builds`. Source: `CHUNK_R12.md`'s
one item, the CLI redesign named (and explicitly deferred) by both
`2026-09-24-aim-review-repairs.md` and its part-2 as "genuinely the
next chunk's work" -- item 1 of `CHUNK_R11.md`, itself item 1 of the
review's stated priority order.

## What changed

**New stored table: `report_rows.parquet`.** `growth::ReportSnapshot`
(`observed_at`, the assembled `Report`, per-root `coverage`, and the
`external_units`/`agent_units` vectors) is stored one row per resolved
scope (`report::scope_snapshot_key`, a hash of the scope's sorted
candidate root paths), replaced wholesale each qualifying observation --
the same "measurement cache, not delta history" discipline as
`unowned.parquet`/`external/folded.parquet`. Complex fields are
JSON-encoded into `Utf8` cells (`crates/core/src/growth/columns.rs`'s
`StoredReportSnapshotRow`), the same cell-encoding precedent the
2026-09-24 unowned-inventory fix established -- not a JSON file on disk.

**`report::observe_scope` persists it.** Gated exactly like its own
`unit_replay.commit()`: only when `observe` is true, `want ==
ObservationParts::ALL` (a narrowed live-watch refresh must never
overwrite the whole scope's snapshot with a partial one), and both unit
families measured without error. `report::report_scope_from_store(scope,
store_dir)` is the read side: resolves nothing but the scope (a config
read plus one presence `stat` per candidate root -- not a walk) and
returns the stored `ReportSnapshot` or `NoObservation`.

**CLI**: `Command::Report` no longer calls `report_scope`/
`report_full_mode`/`observe_scope` at all. It resolves the scope
(explicit root, or the configured scope), loads the snapshot, and
renders -- unchanged rendering code (`render.rs`, `agent_json.rs`) over
the deserialized `Report`. No stored snapshot: text prints `no
observation yet for <scope>; run swamp observe` and exits 2; JSON prints
`{"error":"no_observation","scope":...}`. `--no-observe` removed from
`report`/`ui`. `--full`/`--docker-facts`/`--enrich`/`--since` moved onto
`Command::Observe` (which gained them); `report --verify-du` stays as a
display-only flag (prints whatever `du` total the last `observe
--verify-du` stored, never runs `du` itself). `swamp observe` is
rebuilt on `report::observe_scope(scope, ObservationParts::ALL, ...)`
over the whole resolved scope in one coherent call, replacing the old
per-root `observe_only` loop (deleted, along with its now-dead
`ObserveSummary`) that never ran external/agent discovery at all.

**TUI** (`crates/tui/src/lib.rs`): `run`'s instant-paint-on-startup
cache reads `report::report_scope_from_store` instead of
`report::load_last_report`'s JSON sidecar; `finish_startup`/`run_scope`
drop their `no_observe` parameter (always observe, always watch).
Background refresh and live-watch code paths are unchanged -- they
already called `observe_scope`, which now leaves a fresh snapshot behind
as a side effect for the next `report`/TUI start.

**Not deleted**: `fs_gate::store::JsonFile::LastReport`
(`last_report-<key>.json.zst`) and its `write_last_report`/
`load_last_report` functions. These are a *different* mechanism from
what this chunk replaces: `consumers/cache.rs`, `consumers/cargo.rs`,
and `consumers/signals.rs` use them as an internal previous-pass diff
cache *during* a walk (incremental Cargo/signal computation), not as
`report`'s read path. Deleting it would regress that unrelated
optimization, out of this chunk's scope; only its use as a no-walk
render cache (the TUI's old startup path) was replaced.

**Deviation flagged for the user**: the brief described several
additional new tables (a standalone `evidence.parquet`, a coverage
regions table, per-project/worktree summary rows). This session stored
the whole assembled `Report` (evidence, tracking, and Docker joins
already attached by the same pipeline that always computed them) as one
JSON-encoded cell instead of shredding it into typed per-entity Parquet
tables. This is a real, bounded engineering simplification, not an
oversight: shredding the full `Report`/`ProjectRow`/`WorktreeRow`/
`ArtifactRow` tree into normalized tables (with a join key scheme, a
migration path for existing stores, and render code rewritten to query
them) is itself a multi-session project, and attempting it here risked
landing something half-tested. The chosen design satisfies every
observable constraint this chunk's brief actually tests (report never
walks/stats/spawns; store stays under the size budget; round-trips
byte-for-byte) using the same "cell-encoded JSON is not a JSON file on
disk" precedent the reviewed `unowned.parquet` fix already established.
Flagging this so the user can decide whether the more granular schema
is still wanted as follow-up work.

**Second deviation**: `--since` moved to `observe` entirely rather than
staying on `report` with a fresh re-derivation at render time.
`growth::annotate_readonly`/`annotate_readonly_dirs`/
`annotate_readonly_files` *do* exist as pure-Parquet-read functions that
could recompute growth_bytes for an arbitrary window without walking,
but wiring that into `report` would need each stored worktree mapped
back to its owning scan root (to find its volume/history store) -- data
the merged multi-root `Report` does not carry today. Growth/regrowth
now reflect whichever window the last `observe` used (its own
`--since`, else `config.toml`'s `since`, else the hard-coded default);
`report` has no `--since` of its own. Tests that asked for two
different windows against one stored history now issue two `observe`
calls with different `since` config instead of two `report --since`
calls -- same underlying mechanism, one level up the pipeline.

**A real bug fixed along the way**: `report_json_envelope`'s
`coverage.history` block was passed the raw `since` CLI argument instead
of `since_str` (the config-resolved effective value `agent_json::
effective_since` already computes) -- with `--since` always supplied in
every existing test, this was invisible; removing the flag exposed it
immediately (`asked_window_secs`/`note` came back `null`). Fixed to pass
`since_str`.

## Tests

- `crates/core/tests/report_is_a_pure_read.rs` (new): after `observe_scope`
  covers a scope, `report::report_scope_from_store` does **zero**
  directory listings/file stats/header-byte reads/subprocess spawns
  (`work_counters::measured`); its `Report`/`external_units`/
  `agent_units`/`coverage` are byte-for-byte (`serde_json::Value`
  equality, which normalizes `HashMap` key order) what `observe_scope`
  produced, and rendering off either is identical text; a scope never
  observed refuses with the documented message and does zero I/O too.
- Every CLI test file that spawned the real binary's `report` subcommand
  expecting it to scan on demand (`crates/cli/tests/{agent_json_contract,
  agent_storage_cli, report_multi_root, scope_text_vs_json_contract}.rs`)
  now calls `observe` first; `--since`-dependent assertions moved to
  `observe --since`/`config.toml`'s `since` per the deviation above.
  `crates/cli/tests/observe.rs` updated for the new one-line-plus-
  per-root-coverage `observe` stdout shape (still contains
  `observed_at=`/`mode=full` and each root's own path, which
  `observe_with_no_roots_uses_the_configured_default_scope` checks for).
- `cargo run -p swamp-source-audit`: `TUI_REPORT_API` (gate.rs) extended
  with `report_scope_from_store` (a scope-aware, no-bare-root read, the
  exact thing that allowlist exists to require); `no_unreferenced_public_items`
  caught `observe_only`/`ObserveSummary` going dead after `cmd_observe`
  stopped calling them -- both deleted rather than suppressed.

## Verification run this session

- `cargo fmt --all --check` -- clean.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` --
  clean.
- `cargo run -p swamp-source-audit` -- 11/11 `ok`.
- `cargo test --workspace --locked --no-fail-fast` -- full green, twice
  (once after the CLI/TUI rewrite, once more after the
  `observe_only`/`ObserveSummary` deletion), 0 failures both times.
- `bash scripts/check.sh` (`SWAMP_TARGET_DIR` pointed at this worktree's
  warm target) -- exits 0, every step (fmt, clippy, audits,
  release-graph, tests, named-targets, greps) passed.

## Measured (this machine, default scope, scratch `SWAMP_DIR` under the
scratchpad, deleted immediately after measuring -- never the real store)

Release build (`cargo build --release`), real `~/src`,
`~/Library/Developer`, `~/Library/Caches`, every configured detector:

| step | time |
|---|---|
| `observe` (cold, first observation) | 96.93 s |
| `observe` (unchanged, 2nd, 5 s later) | 32.23 s |
| `observe` (unchanged, 3rd, 5 s later) | 30.31 s |
| `report` (no view, text) | 0.26 s |
| `report --json` (no view) | 0.65 s |
| `report --view worktrees` | 0.25 s |
| `report --view builds` | 0.26 s |
| `report --view deps` | 0.24 s |
| `report --view docker` | 0.24 s |
| `report --view kinds` | 0.24 s |
| `report --view unowned` | 0.26 s |
| `report --view reconciliation` | 0.24 s |
| `report --view types` | 0.24 s |
| `report --view external --json` | 0.31 s |
| `report --view agents --json` | 0.32 s |
| `report --view projects --json` | 0.28 s |
| `report --view grown --json` | 0.29 s |
| store size after 3 observations | **17 MB** (`report_rows.parquet` 4.3 MB, largest `last_report-*.json.zst` 3.3 MB) |

**Target met, decisively: every `report` view ≤ 1 s** (the review's
worst offender, `--view external --no-observe`, was 80.46 s before this
chunk; it is now 0.31 s -- a ~260x improvement, and every other view
moved from the 9–52 s range to ~0.25–0.65 s). Store size stays well
under the 50 MB budget.

**Target not met: unchanged `observe` ≤ 2 s** (actual 30–32 s). This
chunk did not touch what `observe` measures, only what `report` does
with what was measured -- the unchanged-observe cost is dominated by
external/agent unit re-derivation over detector-resolved locations
(`~/Library/Caches`, CoreSimulator, the ~40 GB Android/simulator
volumes) that are still ordinary scan roots today, exactly the gap
`CHUNK_R11.md`'s item 2 ("project roots vs. detector locations") names
and neither prior session in this stack implemented. That remains the
next chunk's work, not something this one silently absorbed.

## Flagged, not fixed (out of this chunk's scope; each is its own follow-up)

1. **Granular Parquet schema vs. one JSON-encoded `Report` cell** -- see
   "Deviation flagged for the user" above. If the more decomposed schema
   the original brief described is still wanted, it is a follow-up
   chunk, not a small addition to this one.
2. **`last_report-<key>.json.zst` on a real scope is multi-megabyte**
   (3.3 MB observed here), far past the 64 KiB control-file cap
   `store_contents_are_allowlisted.rs` enforces on its own *synthetic*
   fixture. This mechanism predates this chunk (kept deliberately, see
   "Not deleted" above) and the guardrail test's fixture is too small to
   have ever caught this on a real multi-project scope. Worth a
   dedicated look: either bound what `consumers/cache.rs` persists
   per-pass, or fold it into `report_rows.parquet` too so there is one
   render/incremental cache, not two.
3. Unchanged-`observe` cost (30 s here) -- see the measurement note
   above; blocked on `CHUNK_R11.md` item 2.

## Commits this session

Each step above landed as its own commit on `stack/26-aim-review-repairs`,
after `stack/25-bounded-fold-completeness`; see `git log` on the branch
for exact hashes and messages. Pushed once green (this note's "Verification
run" section, all green).

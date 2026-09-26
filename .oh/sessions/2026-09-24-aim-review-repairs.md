# 2026-09-24 — Aim review repairs (stack/26): unowned-to-Parquet, not the whole brief

Worktree `/Users/muness1/src/open-horizon-labs/swamp-builds`, branch
`stack/26-aim-review-repairs` off `stack/25-bounded-fold-completeness`.
Source: `REVIEW-AIM-139.md`'s Blockers/Improvements against
`.oh/handoffs/2026-09-21-claude-full-scope.md`, via `CHUNK_R10.md`, whose
own priority order named the HARD RULE (no JSON in the store) and item 1
(unowned inventory out of JSON, scan speed) as the whole point of this
chunk, to be done "before anything else." That is what got done, plus
five mid-session scope additions relayed by the coordinator (below). Items
2, 4 (partially), 5, 6 of the original brief, and the CLI redesign, the
project-roots-vs-detector-locations split, the Homebrew default-off, and
the Codex agents-view reconciliation defect are **not done** -- see
"Not done" below. This session made one real, tested, measured fix and
stopped there rather than leave five half-finished ones.

## What changed

**Item 1 / HARD RULE: `unowned.json` (473 MB on the reviewer's real
scope) is gone.** Root cause: `walk.rs` pushed one `UnownedRow` per
unowned *file* (`push_unowned_file`, called from `record_file`) --
millions of rows on a real `~/Library/Caches`-shaped scope, each row a
JSON object. Fixed in two parts:

1. **Folding** (`crates/core/src/walk.rs`): `record_file`/
   `record_file_typed` now return a `FileTally` (`Owned(bytes)` /
   `Unowned(bytes)` / `Duplicate`) instead of pushing a row as a side
   effect. `process_walk` accumulates `dir_unowned_bytes` across a
   directory's direct files (mirroring the existing `dir_own_allocated`
   accumulator) and pushes exactly one folded `UnownedRow` per directory
   at the end of the call, via the new `push_unowned_dir` (reason
   decided by the directory's own name, matching the convention
   `finish_size_job`'s classified-directory folding already used). A
   walk root that is itself a single file is the one case with no
   directory to fold into and still gets its own row (at most one).
   Permission-denied directories are unchanged (already one row each).
2. **Storage** (`crates/core/src/growth.rs`,
   `crates/core/src/growth/columns.rs`): `<volume>/unowned.json`
   (`JsonFile::Unowned`, removed) is now `<volume>/unowned.parquet`
   (`StoredUnownedRow`, `read_unowned_rows`/`write_unowned_rows`),
   following the `external/folded.parquet` precedent exactly: a
   measurement cache replaced wholesale each full walk, not reverse-delta
   history (unowned rows carry no growth/regrowth semantics). Complex
   fields (`containers`, `shared_with`, `evidence`) are JSON-encoded into
   Parquet `Utf8` cells -- that is a cell encoding, not a JSON file on
   disk, so it does not reopen the hard rule.

**Allow-list test tightened, not just patched.** `unowned.json` is gone
from `store_contents_are_allowlisted.rs`'s `ALLOWED_NAMES`, and its
per-file size cap (previously `.json`/`.jsonl` names only, 64 KiB) now
applies to *every* non-`.parquet` file -- closing exactly the loophole
the hard-rule decision named (`last_report*.json.zst` was exempt before).
`.oh/guardrails/store-data-is-parquet-not-json-sidecars.md` records the
2026-09-24 finding and lists what is *not yet* migrated (below).

**Item 3 (partial): two real Debug-formatting leaks fixed, with a test
that would have caught them.** `render.rs`'s evidence line used
`format!("{:?}", e.source)` on `EvidenceSource` -- an enum with `String`
fields -- so a real evidence line rendered `FilesystemMetadata { detail:
"mtime" }` verbatim; new `evidence_source_label()` gives every variant a
proper label. The external view used `{:?}` on `StorageCategory`;
switched to the existing `external::category_str()` (made
`pub(crate)`), matching how `kind_label`/`evidence_subtype_label` already
handle their enums. The projects/builds table used `{:?}` on
`ArtifactKind`; switched to the existing (already-written, previously
unused here) `kind_label()`. New test
`evidence_lines_never_render_a_debug_struct_literal`
(`render_snapshot.rs`) exercises every `EvidenceSource` variant and
asserts no line contains a `{ ` or `: "` shape -- the exact signature a
`{:?}` derive leaves and a hand-written label never does. **Not
attempted:** the future-mtime clock-skew bug, "no declared consumers"
header placement, multi-root header root naming, the launchd status line
reading the global plist, and the `unowned by top-level dir: /` row --
all named in item 3 but not investigated this session.

**Toolchain (coordinator decision, mid-session): floats on `stable`.**
`rust-toolchain.toml`'s `1.98.1` pin and both workflow files' matching
`dtolnay/rust-toolchain@master` + `toolchain: "1.98.1"` are gone;
`dtolnay/rust-toolchain@stable`. Confirmed the exact drift this was meant
to guard against actually happens: 6 of 38 `compile_fail` cases mismatched
under current stable (newer `[const] Default`/`Destruct` trait-bound
notes on already-correct errors), regenerated with `TRYBUILD=overwrite`;
diffs are additive-only (checked each one), same error codes, same root
cause per case. The per-OS `.stderr.<target_os>` override this crate
already had is exactly the mechanism the coordinator asked to confirm
existed.

## Measured (this machine, default scope, scratch `SWAMP_DIR`, never the real store)

Three real-machine runs (`~/src`, `~/Library/Developer`, `~/Library/Caches`,
plus every configured detector), `SWAMP_DIR` under the scratchpad, deleted
after measuring:

| step | time |
|---|---|
| `report` (cold, first observation) | 109.74 s |
| `report` (unchanged, 2nd) | 17.71 s |
| `report` (unchanged, 3rd) | 16.21 s |
| `report --no-observe --view builds` | 9.07 s |
| `report --no-observe --view agents` | 13.33 s |
| `report --no-observe --view external` | 80.46 s |
| store size after 3 observations | **13 MB** (largest single `unowned.parquet`: 2.96 MB) |

Store size: **target met** (≤ 50 MB; was 536 MB / 473 MB `unowned.json`
before this fix, on the reviewer's own scope). Timing: **targets not
met** (`report` ≤ 2 s, each `--no-observe` view ≤ 1 s). The unowned-JSON
fix cut store size by roughly 40x and cut the earlier review's 7-52 s
`--no-observe` costs somewhat, but did not reach the 1 s target --
`--view external`'s 80 s is now the visible floor, and profiling (`ps`
showing 800%+ CPU during that call) shows it doing live, parallel
re-derivation work on every call, not a stored-row read. That re-derivation
is the CLI-redesign item ("report never scans, only reads Parquet
current rows") named by the coordinator mid-session and explicitly not
implemented here -- see "Not done."

Isolated fold-ratio checks (synthetic fixtures, deleted after measuring):
20,000 unowned files folded into ~200 directory rows wrote a 4.3 KB
`unowned.parquet`; 5,000 single-file unowned directories (no folding
headroom by construction) wrote 33.6 KB. Both fixtures' git checkouts and
artifacts round-tripped correctly through the unchanged `report_golden`/
`report_growth` assertions once those tests were updated (below).

## Test fixes required by the folding change (real regressions caught, not pre-existing)

`report_golden.rs` and `report_growth.rs` asserted an unowned row's
`path_or_object` equals a loose file's own path
(`fx.loose_file.display()...`) -- true before folding, false after (the
row is now keyed by the file's *containing directory*). Added
`Fixture::loose_dir` and updated both assertions. Caught by running the
full workspace suite, not assumed.

## Pre-existing failure, not introduced here, blocks a clean `scripts/check.sh`

`crates/core/tests/build_adapter_history.rs`: 3 of 8 tests fail
(`an_unchanged_node_checkout_replays_its_units_without_reading_a_manifest`,
`cost_report_real_pipeline_unchanged_and_one_group_change`,
`python_go_swift_and_android_units_reach_the_report_and_full_equals_incremental`),
all on the same assertion: `counted.containers_identified` is `5`, not
the `0` the non-Linux branch of the test expects. **Confirmed
pre-existing**: `git stash` back to the unmodified `stack/25` tip and
re-ran this test file in isolation -- identical failure, identical
counts. The test file's own comment already documents this exact
symptom as an open, undiagnosed gap from the Linux-on-gates port
("suspect: a container's adapter-claim flapping between passes... Root
cause not yet found"), previously believed Linux-only; it now reproduces
on macOS too. Not investigated further this session (out of scope,
already tracked, and diagnosing a flapping-identification bug properly
is its own chunk, not a five-minute patch). This is why
`scripts/check.sh` exits 101 here, not 0 -- confirmed by running it
directly (not through a pipe, which was masking the real exit code on a
first attempt).

## Verification run this session

- `cargo fmt --all --check` -- clean.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` --
  clean.
- `cargo run -p swamp-source-audit` -- all 11 audits `ok`.
- `cargo test --workspace --locked --no-fail-fast` -- 66 test binaries
  green; only `build_adapter_history` fails (pre-existing, above).
- `cargo test -p swamp-source-audit --test compile_fail -- --ignored` --
  38/38 after `TRYBUILD=overwrite` regeneration under stable.
- `bash scripts/check.sh` (no `SWAMP_TARGET_DIR`, using this worktree's
  own warm 35 GB `target/`) -- fails at the `tests` step on the
  pre-existing `build_adapter_history` failure; every step before it
  (fmt, clippy, audits, release-graph) passed.

## Not done (mid-session scope additions, relayed by the coordinator, none implemented)

In arrival order, each verbatim in spirit:

1. **CLI redesign: `report` never scans; `--no-observe` removed;
   `observe` is the only scanner and always persists.** Not implemented.
   This is the change that would actually fix the timing numbers above
   (a report that only reads Parquet current rows cannot cost 80 s) --
   it is the natural next chunk, not a follow-up.
2. **Project-roots vs. detector-locations split**: only `~/src`-style
   roots + `[scan] include` + explicit command roots get Git discovery/
   classification/unowned inventory; detector-resolved locations
   (Cargo home, rustup, DerivedData, CoreSimulator, …) get one folded
   external row each, never their own Git/unowned pass; `~/Library/
   Caches`, `~/Library/Developer`, `/opt/homebrew` stop being scan roots
   entirely. Not implemented -- `BuiltinDefaultsDetector` (`locations/
   builtin.rs`) still proposes `~/Library/Caches` and `~/Library/
   Developer` as macOS default scan roots exactly as before. This is
   almost certainly why `--view external` still re-derives live instead
   of reading stored rows (there is no "which locations are
   detector-only" boundary yet for it to key off).
3. **Homebrew (and other system-install-tree detectors) default-off**:
   opt-in via `[scan] enabled_detectors`, `swamp scope` shows `disabled
   (default off)`, a test on the default-scope/enabled-scope pair. Not
   implemented. `HomebrewDetector` still runs by default.
4. **Codex agents-view reconciliation defect** (category totals don't
   sum to the home's folded bytes; SQLite stores land in
   `unclassified` instead of a protected-databases category; `plugins/`
   and `archived_sessions/` don't get their own categories). Not
   investigated -- no code read, no fix attempted.
5. Original brief items 2 (Claude Code header scan across first N
   records, inferred fallback), 4 (bytes-first ranking + largest/oldest
   summary in `--view agents`/`--view external`; item 4's builds
   accounting-basis fix at render.rs:78-90/1308 also untouched), 5
   (coverage-noise collapse, `swamp scope --verbose`), 6 (already
   partially true per `allocation_note`, not verified either way): not
   attempted.

## Why this session stopped here

Five substantive scope expansions arrived mid-session (Homebrew
default-off + other system detectors, the CLI observe/report split, the
project-roots/detector-locations split, the toolchain switch, the Codex
reconciliation defect) on top of an already large original brief (HARD
RULE + 6 numbered items) whose own priority order named item 1 as "the
whole point of this chunk." Attempting all of it in one pass risked
landing several half-finished, uncompiling, or untested changes instead
of one real, measured, tested fix. This session did the HARD RULE +
item 1 (the named priority) to completion with real before/after
numbers, the toolchain switch (small, mechanical, verified), two of
item 3's concrete Debug-formatting bugs (found while reading the exact
code item 1 touched, with a regression test), and stopped -- rather than
narrow scope silently, this note and the final report name every
undone item and why.

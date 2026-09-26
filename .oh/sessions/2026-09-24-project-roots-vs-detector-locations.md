# 2026-09-24 — Project roots vs. detector locations (stack/26, R13 item B); measured, item C not met (root cause found)

Branch `stack/26-aim-review-repairs`, worktree
`/Users/muness1/src/open-horizon-labs/swamp-builds`. Source:
`CHUNK_R13.md`'s three items (A: no JSON in the store, for real; B:
project roots vs. detector locations; C: unchanged `observe` ≤ 2 s).
This session did item B completely, tested and measured; found and
fixed a second, real defect item B's own test exposed
(`external::discover_and_measure` never measured two of
`builtin-defaults`' own candidates at all); profiled item C precisely
and found the actual top cost, but concluded a correct fix is its own
feature, not a patch -- documented below rather than guessed at. Item A
(the Parquet-table decomposition) was **not attempted**: see "Item A —
not done" below for why, and what the next worker needs.

## Item B — project roots vs. detector locations (done)

**Design implemented, exactly as `CHUNK_R13.md`/`CHUNK_R11.md` item 2
specified.** Only `~/src`-style built-in roots, `[scan] include`
entries, and explicit command roots get ordinary Git/ecosystem
discovery and an unowned-remainder walk. Every detector-resolved
location is a *detector location*: measured only as an external unit,
never walked for projects.

- `crates/core/src/scope.rs`: `resolve_effective_scope`'s
  candidate-building loop now tags exactly one of `builtin-defaults`'
  candidates -- the literal `~/src` path -- with the (previously
  vestigial) `RootReason::BuiltinDefault`, matched by identity against
  `env.home.join("src")`, not by index. Its other candidates
  (`~/Library/Caches`, `~/Library/Developer`, the XDG cache root on
  Linux) keep `RootReason::Detector { detector_id: "builtin-defaults",
  .. }`, exactly like every other tool-home detector.
- New `ScopeRoot::is_project_root()`: true iff a root's reasons include
  `BuiltinDefault`/`Included`/`ExplicitCommand`. This is the single
  predicate everything else keys off.
- `crates/core/src/report.rs`'s `report_scope_with_parts_covered`
  (the per-root walk loop `observe_scope` calls into): a `Present` root
  that is not a project root gets no walk at all -- not even the
  presence/readability re-probe -- and a new coverage row,
  `RegionStatus::DetectorOnly` (`crates/core/src/coverage.rs`,
  `RootCoverage::detector_only`), labeled "detector location (measured
  as an external unit, not scanned for projects)".
- `swamp scope`'s text renderer (`crates/cli/src/main.rs`) groups roots
  under two headings ("project roots" / "detector locations"), hides
  `SkippedAsNested` rows by default, and gained `--verbose` to show
  everything; `--json` is unaffected (still the full, unfiltered
  `EffectiveScope`).

**A real gap this exposed, found and fixed in the same session (not
left for later):** `external::discover_and_measure`'s
`authorized_candidates` filtered out *every* root whose `detector_id`
was `"builtin-defaults"` -- correct before this change (that detector's
roots were all ordinary scan roots, so nothing should double-measure
them), silently wrong after it: `~/Library/Caches`/`~/Library/Developer`
are now detector locations, and this was the *only* place left that
measures them at all. Fixed by removing the filter (`~/src` still never
reaches this function: it carries `RootReason::BuiltinDefault`, so
`AuthorizedRoot::detector_id` is `None` for it, and the existing `?`
early-return already excludes it). The identical, now-equally-stale
exemption in `scope.rs`'s nested-folding `ExternalPruneNote` pass was
also removed. Caught by a new assertion in the acceptance test below,
not assumed correct after the fact -- writing a file under a fixture
`~/Library/Caches` and asserting it shows up in
`discover_and_measure`'s output failed before this fix (the unit was
simply missing, not mis-categorized).

`locations::builtin::is_builtin_defaults` (now unreferenced) was
deleted -- caught by `cargo run -p swamp-source-audit`'s
`no_unreferenced_public_items`, not left as dead code.

### Tests

- `crates/core/tests/project_roots_vs_detector_locations.rs` (new):
  - `a_git_repo_inside_a_fake_detector_home_is_not_a_project_and_yields_no_unowned_rows`
    -- the exact acceptance criterion: a Git repository (with a real
    commit) planted deep inside a fixture `CARGO_HOME` produces no
    project, no unowned rows, `walked_total == 0`, a `DetectorOnly`
    coverage row, and is still measured as a Cargo-home external unit.
  - `macos_builtin_caches_and_developer_are_detector_locations_not_project_roots`
    -- `~/src` is a project root; `~/Library/Caches`/`~/Library/Developer`
    are not; and (added after finding the `external.rs` gap) both are
    still measured as external units, not silently dropped.
- `crates/core/tests/external_double_measurement.rs`: two pre-existing
  tests assumed `~/Library/Caches` got an ordinary walk with the
  Homebrew subtree pruned out of it -- no longer true (Caches is not a
  scan root at all now) -- rewritten to assert the simpler, stronger
  fact directly (`walked_total == 0` for a detector location; the
  nested unit is still measured on its own, with real bytes).

### Verification

- `cargo fmt --all --check` -- clean.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` --
  clean.
- `cargo run -p swamp-source-audit` -- 11/11 `ok`.
- `cargo test --workspace --locked --no-fail-fast` -- full green (70
  test-result blocks, 0 failures), run twice (once after the item-B
  commit, once more after the `external.rs`/`load_units` fixes below).

Commits: `777f2c4` (the split itself), `2f45f10` (the
`builtin-defaults`-in-`external.rs` fix + doc cleanup).

## Item C — unchanged `observe` ≤ 2 s (not met; root cause found and documented)

Measured on this machine's real, live default scope (`~/src`,
`~/Library/Developer`, `~/Library/Caches`, every enabled detector),
scratch `SWAMP_DIR` under the scratchpad, deleted after measuring,
release build (`cargo build --release`):

| step | before item B (2026-09-24 R12 note) | after item B | after the `load_units` fix below |
|---|---|---|---|
| `observe` (cold) | 96.93 s | 40.48 s | 40.88 s |
| `observe` (unchanged, ≥4 s later) | 30–32 s | 27.69 s | 27.08–29.18 s |

Item B's own fix (detector locations no longer get a second, full
project walk on top of their external-unit measurement) is real and
measured -- cold `observe` improved ~2.4x -- but it was never expected
to move the *unchanged* number much, because that number was already
dominated by the external-unit measurement pass itself, not by the
now-removed duplicate project walk. **Target not met: unchanged
`observe` is ~27-29 s, not ≤ 2 s.**

### A second real defect found and fixed along the way (small win, not the dominant cost)

`crate::build_stores::load_units` discarded any container whose stored
`units` decoded to an empty `Vec` (`if !units.is_empty() { out.insert
(...) }`). But `save_units` (`external.rs`'s post-measurement step)
*does* write an entry for every container it measured, including one
with zero identified nested artifacts -- a real, verified answer ("this
store has nothing an adapter recognizes"), not a cache miss. The read
side silently threw exactly those entries away, so any store-shaped
detector location with no build-tool structure inside it (a raw cache
directory, or -- see below -- a mounted read-only volume) could never
replay its container-identification cache and was re-identified in full
every single pass, forever. Fixed by removing the filter; new test
`build_stores::tests::a_container_with_zero_identified_units_still_replays`
asserts the full round trip (key present, `units` empty, `observed_at`
preserved), not just "returns something". This is real, tested,
committed -- but the numbers above show it was not the dominant cost
here.

### Profiling: the actual dominant cost, found and reproduced

Two temporary-turned-permanent `SWAMP_TRACE`-gated diagnostics added
this session (both follow the existing `SWAMP_TRACE` convention already
used in `growth.rs`/`walk.rs`/`bus/registry.rs`, so a future worker gets
them for free):

- `crates/cli/src/schedule.rs`'s `cmd_observe`: resets
  `work_counters` before the observation and prints the full
  `WorkCounters` snapshot after, under `SWAMP_TRACE`.
- `crates/core/src/external.rs`'s per-candidate measurement loop: prints
  each unit's canonical path, whether its container cache allowed
  reuse, and the `dirs_listed`/`files_statted` delta *that one unit*
  cost, under `SWAMP_TRACE`.
- `crates/core/src/report.rs`: prints each unit root's own
  `UnitRootCoverage` (`event_covered`/`reason`) under `SWAMP_TRACE`.

Running `SWAMP_TRACE=1 swamp observe` on an *unchanged* pass and
summing the per-unit deltas: **`/Library/Developer/CoreSimulator/Volumes`
alone accounts for 482,259 of ~526,000 directories listed and
1,774,756 of ~2,000,000 files statted -- over 90% of the total "unchanged"
cost** -- every single time, reproduced on three separate passes.
Everything else (`.codex`, `~/Library/Caches`, `.claude`, `.cargo`,
`~/Library/Developer`) together accounts for the remaining ~44,000
dirs/144,000 files, itself still real cost but an order of magnitude
smaller.

**Root cause, confirmed, not guessed:**
`/Library/Developer/CoreSimulator/Volumes` is a directory on the boot
volume whose children are themselves separate, independently mounted
APFS volumes -- three simulator runtimes on this machine (`mount`
shows `disk5s1`/`disk7s1`/`disk9s1`, each `sealed`, `read-only`,
`nobrowse`), 40 GB total (`du`). The unit-root FSEvents cursor
(`growth::replay_unit_roots`) watches only the **top-level path's own
device** -- the boot volume -- and `UnitRootCoverage` correctly reports
`event_covered: true, reason: incremental` for it (nothing on the *boot*
volume changed under that path). But
`folded_measurement::reuse_folded_measurement`'s reuse gate
(`EventCoverage::unchanged_since`) asks a *different* question: has
*anything under the unit's own root path* changed -- and the unit's own
40 GB of real content lives on the three *mounted child devices*, which
no FSEvents stream in this pass is watching at all. There is structurally
no way for the current single-device-per-unit-root event model to ever
answer "unchanged" for a unit whose measured bytes cross a mount
boundary, so this unit is re-walked in full on every pass regardless of
whether the simulator runtimes (sealed, read-only while mounted) have
truly changed.

Confirmed this is not a walk-boundary oversight that a quick "stay on
one device" fix could safely close:
`walk.rs`'s *ordinary* project walk already refuses to cross a device
boundary (`walk.rs:312`, `meta.dev() != device` skips the subtree), but
the folded external-unit measurement path (`process_size`, used by
`resize_artifact_stamped`) has no such guard -- and *should not* gain
one here: `~/Library/Developer/CoreSimulator/Volumes` exists as a
tracked external unit specifically to show how much space mounted
simulator runtimes use. Making the walk stop at the mount boundary
would make this unit permanently report ~0 bytes instead of 40 GB --
correctness regression, not a performance fix.

### What this needs (not attempted this session; a real follow-up, not a guess)

Multi-device unit-root event coverage: a unit whose folded measurement
crosses one or more mount points needs its *own* mounted device(s)
watched too (or a documented, deliberate alternative -- e.g. treating a
`sealed read-only` mount's total as static once measured, re-checked
only by comparing the *set* of mounted volume names under the parent,
which changes on simulator install/uninstall but not from ordinary use).
Either is a real design decision and its own bounded chunk: extending
`crate::fs_events`/`crate::growth::replay_unit_roots`'s one-cursor-per-
unit-root model to "one cursor per device a unit's measurement touches"
touches the `UnitRootCursor` storage format, `EventCoverage`'s trust
model, and every call site that currently assumes one device per unit.
Guessing at a narrower patch (e.g. special-casing "sealed" APFS mounts)
risks being wrong for a mount that changes in place, or for a different
detector-location that also happens to cross a mount for a legitimate,
mutable reason.

### Verification this session (after every step, not just at the end)

- `cargo fmt --all --check` -- clean.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` --
  clean.
- `cargo run -p swamp-source-audit` -- 11/11 `ok`.
- `cargo test --workspace --locked --no-fail-fast` -- full green, run
  after the item-B commit and again after the `load_units`/trace
  changes.
- `cargo test -p swamp-core --lib build_stores` -- 4/4, including the
  new `a_container_with_zero_identified_units_still_replays`.
- Measurements above: release build, scratch `SWAMP_DIR` under the
  scratchpad, deleted immediately after measuring, never the real
  store. `ps aux` shows no process this session started still running.

## Item A — not done

`CHUNK_R13.md` item A (replace `report_rows.parquet`'s single
JSON-encoded `Report`/`coverage`/`external_units`/`agent_units`/
`store_interiors` cells with the named typed tables --
`projects.parquet`, `worktrees.parquet`, `artifacts.parquet`,
`external_units.parquet`, `agent_units.parquet`, `evidence.parquet`,
`coverage.parquet`, `summary.parquet`, `series.parquet` -- and delete
the remaining JSON control files) was **not attempted**, for the same
reason the previous two sessions gave and this session independently
re-confirmed by reading the actual types involved:

- `Report` is a deeply nested tree (`ProjectRow` → `WorktreeRow` →
  `ArtifactRow`, each carrying optional `GithubFacts`/`MergeComplete`,
  `HashMap<String, Vec<Option<u64>>>` growth series, opt-in
  `dirs_by_worktree`/`files_by_worktree` maps, `Reconciliation`,
  `Summary`). `ExternalUnit` and `AgentUnit` each carry their own nested
  `Vec<ExternalConsumer>`/`Vec<AgentMember>` plus `Vec<Evidence>`.
  Decomposing this faithfully into the named tables needs a row-key
  scheme that spans every entity kind (so `evidence.parquet` can key
  its rows back to whichever project/worktree/artifact/external-unit/
  agent-unit produced them), a rewrite of `report_scope_from_store`'s
  assembly logic, and updates through `render.rs` (2154 lines),
  `agent_json.rs`, and the TUI wherever it reads `Report` fields
  directly.
- The brief's own new audit rule ("reject `serde_json::to_*`/`to_vec`/
  `json!` flowing into any Parquet writer or `fs_gate` store write")
  would, if written literally, also flag `unowned.parquet`'s existing
  `containers_json`/`shared_with_json`/`evidence_json` cell columns
  (`growth.rs:1364-1367`) -- the exact same "cell-encoded JSON is not a
  JSON file on disk" pattern the 2026-09-24 HARD RULE session
  established for that table and this chunk explicitly re-litigates for
  `report_rows.parquet`. Writing that rule *and* satisfying it cleanly
  means also fixing `unowned.parquet`, which the brief doesn't name but
  the rule as specified would catch.

This is a genuine multi-session architectural project, not a chunk that
fits alongside items B/C without risking a half-finished, undertested
schema change landing on top of otherwise-solid, measured work. Per
this chunk's own instructions ("if you cannot finish, commit coherent
progress and state exactly what remains... do not narrow scope
silently"), this is stated plainly rather than attempted partially: **no
code toward item A was written this session.** The concrete facts a
future worker needs are above (the exact fields two consumers already
depend on -- `consumers/signals.rs` needs `WorktreeRow.{worktree_id,
signals}` and `Report.observed_at`; `consumers/cargo.rs` needs
`Report.{observed_at, nested_artifacts}` -- which is exactly the shape
`worktrees.parquet`/`artifacts.parquet` need to carry to replace
`last_report-*.json.zst`'s current internal use, per the brief's own
item A.2).

## Files remaining under the store (unchanged by this session; item A's job)

`config.toml`, `ledger.jsonl`, `last_run.json`, `fsevents.json` (per
volume), `topology.json` (per volume), `docker_facts.json`,
`ui_state.json`, `scope.json`, `agent_protect.json`, `restore.json`
(per Trash envelope), `last_report-<key>.json.zst` (per scope/root,
still used internally by `consumers/{cache,cargo,signals}.rs` as a
previous-pass diff cache, not only as `report`'s old read path), plus
every `*.parquet` table. Not narrowed this session.

## Not pushed

Per the worker brief, push only when the whole chunk (A, B, C) is
green; item A is not done and item C's target is not met, so this
branch is **not pushed**. Commits are on `stack/26-aim-review-repairs`,
ready for review or a follow-up worker.

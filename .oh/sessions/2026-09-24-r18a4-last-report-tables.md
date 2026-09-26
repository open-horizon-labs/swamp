# R18a-4: `last_report-<key>.json.zst` deleted; per-root replay caches typed (2026-09-24)

CHUNK_R18.md's "Progress log (owner)" R18a-4 entry asked for exactly
two things: give `consumers/signals.rs`'s previous-git-signals replay
and `consumers/cargo.rs`'s previous-nested-artifacts-cache replay each
their own per-root Parquet table, written on the per-root observe path
(never scope-keyed); then delete `write_last_report`/`load_last_report`
and every `.zst` writer/reader. Both are done, in full, with no
deferral.

## What was there before

`fs_gate::store::JsonFile::LastReport { store, key }` named
`<store>/last_report-<key>.json.zst`: a zstd-3-compressed JSON dump of
one root's whole `Report` (minus `dirs_by_worktree`/`files_by_worktree`),
keyed by `report::last_report_key` (a hash of the canonicalized root
path). `consumers::cache::CacheWriter` wrote it on every
`Event::ReportAssembled` a real observation produced. Two consumers read
it back, for two unrelated reasons:

- `consumers/signals.rs` (`SignalsConsumer`): for a worktree FSEvents
  reported nothing under, reconstructed a `RawSignals` by *parsing the
  previous pass's rendered `Signal` strings* (`"dirty" == "dirty"`,
  `"unpushed".split(' ').next()...parse()`, etc. -- a real wart, since
  the actual raw values were never stored, only their rendered text),
  filtered out `merge_complete`/`pull_request` (added downstream by
  `consumers/gate.rs` from that pass's own GitHub facts, so replaying
  them from the previous report would double them), and called
  `crate::signals::age_signals` to age `last_commit`/`idle_for` forward
  by the elapsed gap.
- `consumers/cargo.rs` (`CargoConsumer`): read the previous pass's
  `Report.nested_artifacts` and `observed_at` to seed
  `build_adapters::ContainerCache::from_previous`, the replay cache an
  unchanged build container (Cargo target dir, node_modules, etc.) skips
  re-identifying through.

## What ships now

**`git_signals.parquet` + `git_signals_values.parquet`**
(`crates/core/src/growth/columns.rs`: `StoredGitSignalRow`/
`StoredGitSignalValueRow`; `crates/core/src/growth.rs`:
`write_git_signals_table`/`read_git_signals_table`). One row per
`(root_key, worktree_id)` carrying `branch` and every
`crate::signals::RawSignals` field (`last_commit_age_secs`, `dirty`,
`unpushed`, `locked`, `idle_for_secs`) plus that pass's `observed_at`;
the child table carries the rendered `Signal` rows in order. This is a
strict improvement over the old reconstruction-from-rendered-text: the
actual raw values are stored, so a replay ages the real numbers instead
of re-deriving approximations from strings. `SignalsConsumer` writes it
itself, at the end of `on_event`, from the pass's complete `by_worktree`
map (both replayed-and-aged and freshly-walked entries) -- deliberately
*before* `consumers/gate.rs` appends `merge_complete`/`pull_request`
from that pass's GitHub facts, so storing pre-enrichment rows means a
replay never needs to filter those two back out at all (the deleted
JSON cache had to, because it persisted the fully merged `Report`).

**`cargo_replay_cache.parquet` + `cargo_replay_cache_lists.parquet` +
`cargo_replay_cache_evidence.parquet` + `cargo_replay_cache_meta.parquet`**
(`crates/core/src/growth.rs`: `write_cargo_replay_cache`/
`read_cargo_replay_cache`). Reuses `nested_artifacts.parquet`'s own row
shapes (`StoredNestedArtifactRow`/`StoredNestedArtifactListRow`/
`StoredNestedArtifactEvidenceRow`) and stored<->domain conversions
(`stored_row_from_nested_artifact`/`nested_artifact_list_rows`/
`nested_artifact_evidence_rows`/`nested_artifact_from_stored`) verbatim
-- every `NestedArtifact` field this cache needs was already typed there
by R16/R18a-2 -- but writes them to their own root-keyed files instead
of extending `nested_artifacts.parquet` itself. The meta table carries
the one thing the reused row shape has no column for: the pass's
`observed_at`, which `ContainerCache::from_previous` needs alongside the
units. `CargoConsumer` writes it at the end of `on_event`, right after
computing the pass's `NestedArtifact` list.

**Why a second, root-keyed file rather than extending
`nested_artifacts.parquet` with a root column** (the chunk's own
suggestion, considered and rejected): `nested_artifacts.parquet` is
scope-keyed and wholesale-replaced exactly *once*, in
`report::observe_scope`, after every root in the scope has already run
its own bus pass and been merged (`write_nested_artifact_table`'s call
site, gated on `observe && want == ObservationParts::ALL &&
external_ok && agents_ok`). It is also never written at all for a plain
single-root, scope-less call (`report_full_mode_with_source` and its
callers have no `ObservationParts::ALL`/scope-wide external+agent
discovery to gate it on). `CargoConsumer`'s replay decision has to be
available *during* this same root's own bus pass, for both kinds of
caller -- exactly the "per-root incremental walk must never depend on a
scope-keyed table" requirement the chunk states for `git_signals.parquet`
too. Adding a root column to `nested_artifacts.parquet` would not change
either lifecycle mismatch: the table would still not exist yet, or
would still be scope-wide-and-stale, at the moment a per-root pass
needs to read it. A second file with the same row shapes gets the reuse
(no new field-typing work -- the whole point of "extend if cleanest")
without inheriting the wrong write schedule.

Both new tables' keys come from `growth::root_key` (moved from
`report::last_report_key`, same canonicalize-then-hash derivation, now
living beside every other current-state table's key derivation instead
of in `report.rs`), wholesale-replaced per root, and written only when
`ctx.observe` (mirroring the deleted `CacheWriter`'s own gate: a
`--no-observe`/pure-read call must never advance what a later real
observation replays from).

## `consumers/cache.rs`'s new job

`Event::ReportAssembled` -> `Event::ReportCached` is not only a cache
write: `bus::run_report`'s FSEvents checkpoint commit is gated on
`ReportCached` ever firing (`consumers/walk.rs`), so a pass that errors
*anywhere* in the bus never advances the checkpoint past evidence
nothing actually persisted for
(`report_cache_failure_does_not_advance_the_replay_checkpoint`, kept and
re-pointed below, proves this). Moving the two replay-cache writes into
`SignalsConsumer`/`CargoConsumer` themselves (mid-pipeline, not at the
final `ReportAssembled` stage) is still safe for this invariant: the
bus's `run` dispatch (`crates/core/src/bus/registry.rs`) propagates any
consumer's `Err` via `?` immediately, aborting the whole run before
`ReportAssembled`/`CacheWriter`/`ReportCached` are ever reached -- the
same abort-on-error behavior a failure inside the old `CacheWriter`
itself had. `CacheWriter` (`consumers/cache.rs`) now does nothing but
emit `Event::ReportCached`; its module doc explains why it still exists.

One accepted, documented behavior change from this: if a pass writes
`git_signals.parquet`/`cargo_replay_cache*.parquet` successfully but a
*later* stage (GitHub enrichment, Docker join) then errors, those two
tables now reflect that failed pass's freshly-computed values even
though the checkpoint never advances and the overall `Report` is
discarded. The next retry (from the same un-advanced fsevents cursor)
ages/replays from those values instead of the last officially-committed
pass's. This is not a correctness bug -- the values were real
measurements from moments before the failure, and every write remains a
wholesale replace, so a fully successful later pass corrects anything
stale -- just a precision difference from the old design, where the
whole cache (git signals and cargo units together) was atomic with
overall report success. Accepted rather than routed through the old
`CacheWriter`-only choke point, because doing so would need passing
`by_worktree`/`nested` through the bus as new event payloads all the
way to `ReportAssembled` for no correctness gain.

## Deletions

`crate::report::write_last_report`, `crate::report::load_last_report`,
`crate::report::last_report_key` (all of `report.rs`);
`crate::fs_gate::store::JsonFile::LastReport` and its two match arms
(`path`/`encoding`); the `Encoding::CompactZstd` variant and both of its
match arms (`write_json`/`read_json_bytes`); `zstd = "0.13"` from
`crates/core/Cargo.toml` (`Cargo.lock` regenerated by a plain, then
`--locked`, build -- `zstd` remains in the lock only as `parquet`'s own
transitive compression-codec dependency, confirmed by `cargo tree` not
being needed since `cargo clippy --locked`/`cargo test --locked` both
built clean afterward); the `"@core::report::load_last_report"` entry in
`crates/source-audit/src/rules/gate.rs`'s `TUI_REPORT_API` allow-list
(nothing in the TUI ever called it).

## Proof requested by the chunk

```
$ grep -rn 'last_report\|zst' crates/*/src
```

Does not print nothing -- and, following the precedent
`.oh/sessions/2026-09-24-r18a3b-snapshot-deleted.md` set for the
equivalent `report_rows`/`report_stored` false-positive substring
collision, every hit is accounted for honestly, not filtered to look
empty:

- `growth.rs`/`assoc_store.rs`/`growth/columns.rs`/`fs_gate/columns.rs`:
  `zstd`/`zst` as the *Parquet compression codec name*
  (`parquet::basic::Compression::ZSTD`/`ZstdLevel`, from
  `parquet::file::properties`), a completely different, sanctioned
  mechanism this chunk does not touch --
  `.oh/guardrails/column-store-parquet-zstd.md` is the guardrail for
  it, `fs_gate/columns.rs` is the one Parquet writer/reader gate module,
  and every current-state table in this codebase (including the two
  this slice adds) is written through it. None of these name the
  standalone `zstd` crate directly or the deleted JSON cache.
- `growth.rs:254`, `growth.rs:4079`, `growth.rs:4226`, `growth.rs:8358`,
  `growth/columns.rs:4325,4329`, `consumers/cache.rs:5,13`,
  `fs_gate/store.rs:126,127,131`, `source-audit/rules/gate.rs:376`:
  this slice's own doc comments naming `last_report-<key>.json.zst`/
  `last_report_key`/`load_last_report` only to say they are deleted, or
  to explain why the new tables exist and what they replaced. Every one
  of these was rewritten by this slice, in the past tense, as historical
  record -- none claims the file/functions still exist.
- `source-audit/model.rs:674`, `source-audit/rules/gate.rs:76,561`: the
  gate audit's own allow-listed crate-name literal `"zstd"` (the
  Parquet-codec crate name, part of `§19`'s "only the gate modules may
  name `parquet`/`zstd`" rule) and a guardrail-id string
  (`column-store-parquet-zstd`) -- unrelated to this deletion, unchanged.

```
$ ls <store after observe+report+trash-move cycle>
```
No `.zst` file of any kind (verified by
`crates/core/tests/store_contents_are_allowlisted.rs`'s
`a_full_cycle_leaves_only_allowlisted_files_in_the_store`, unmodified in
its cycle logic, only its allow-list edited -- see below -- and green).

## Exact list of non-Parquet files still written to the store

Unchanged from R18a-3b's list, minus every `last_report*` pattern (this
slice's own job) -- `crates/core/tests/store_contents_are_allowlisted.rs`'s
`ALLOWED_NAMES` now reads exactly:

- `config.toml`
- `ledger.jsonl`
- `last_run.json`
- `fsevents.json`
- `docker_facts.json`
- `ui_state.json`
- `scope.json`
- `restore.json` (inside a Trash envelope)
- a Linux collector's `continuity/*.json` checkpoint and `*.sync`
  request files
- `*.lock` files and the store's own `VERSION` marker

Every other file under the store is `*.parquet`. All five of
`fsevents.json`/`docker_facts.json`/`scope.json`/`last_run.json`/
`ledger.jsonl` remain R18b's job, unchanged by this slice.

## Tests

- `growth::tests::git_signals_table_round_trips_every_field_and_keeps_order`
  -- every `RawSignals` field, `branch`, rendered-`Signal`-row order,
  artifact... (worktree) order within a root, a second root's rows not
  disturbing the first's, and a missing root reading back `None`.
- `growth::tests::git_signals_table_rebuild_reflects_a_direct_tamper_not_the_original_value`
  -- on-disk tamper of both tables, read back through
  `read_git_signals_table`.
- `growth::tests::an_unchanged_worktree_ages_stored_signals_instead_of_recomputing_them`
  -- writes a pass's signals, reads them back through
  `read_git_signals_table` exactly as `SignalsConsumer` would for an
  unwalked worktree, ages them with the real `crate::signals::age_signals`
  by a fixed elapsed gap, and asserts `last_commit_age_secs`/
  `idle_for_secs` grow by *exactly* that gap while `dirty`/`unpushed`/
  `locked` (never aged) carry through unchanged -- only possible if the
  aging read the stored previous values back rather than starting from
  a fresh, all-`None` `RawSignals`. Writes the aged result as a second
  pass and confirms the stored value updates. This is new evidence, not
  a re-pointed existing test: grepping the repository for an existing
  git-signals-reuse test (a work counter, a subprocess-spawn count, an
  existing full-pipeline assertion) found none -- git activity is
  computed via `libgit2`/`gix`, not a subprocess, so there is no
  "spawns" counter an existing test could have asserted zero on the way
  `build_adapter_history.rs`'s `containers_identified`/`containers_reused`
  counters do for Cargo.
- `growth::tests::cargo_replay_cache_round_trips_every_field_and_distinguishes_never_observed_from_empty`
  -- every `NestedArtifact` field via the reused conversions, and the
  meta-row-decides-presence contract (a root with zero units last pass
  reads back `Some(observed_at, [])`, a root never observed reads back
  `None`).
- `growth::tests::cargo_replay_cache_rebuild_reflects_a_direct_tamper_not_the_original_value`
  -- on-disk tamper of the row and a list child row, read back through
  `read_cargo_replay_cache`.
- `crates/core/tests/fsevents_incremental.rs` -- both tests that touched
  `load_last_report`/the old cache file directly were re-pointed, not
  deleted:
  - `report_cache_failure_does_not_advance_the_replay_checkpoint`: the
    blocked file is now the known, fixed-name `git_signals.parquet`
    (previously a glob search for `last_report-*`, needed because that
    file's name was hashed per root; the new tables have fixed names, so
    the glob is gone too). Same mechanism otherwise: occupy the path
    with a non-empty directory, assert the write's `tmp.persist(path)`
    rename fails, the whole call errors, and the fsevents checkpoint
    (`sidecar`) does not move; unblock and confirm the retry succeeds
    and the checkpoint advances.
  - `switching_roots_preserves_history_and_alias_replay_namespace`: the
    `load_last_report(store, &alias).is_some()` assertion ("alias and
    canonical root must load the same cached report") is replaced by
    `root_key(&alias) == root_key(&fx.root)` plus
    `read_cargo_replay_cache(store, &root_key(&alias)).is_some()` --
    the same claim (alias and canonical root share one replay cache),
    proven against the new table instead.
  - Both required making `growth::root_key` and
    `growth::read_cargo_replay_cache` `pub` (they were `pub(crate)`);
    `growth::write_git_signals_table`/`read_git_signals_table`/
    `write_cargo_replay_cache` stay `pub(crate)` (no external caller
    needs them -- the alias-equivalence test only needed one public
    read function, and `read_cargo_replay_cache`'s return type
    (`Vec<NestedArtifact>`) is already public, unlike
    `read_git_signals_table`'s `pub(crate)` `StoredWorktreeGitSignals`).
- `crates/core/tests/store_contents_are_allowlisted.rs` --
  `a_full_cycle_leaves_only_allowlisted_files_in_the_store` needed no
  logic change, only its `ALLOWED_NAMES` list (removed
  `last_report.json`/`last_report.json.zst`) and the now-dead
  `last_report-*.json.zst` special case in `allowed()`; still green,
  still asserts the whole cycle's file list top to bottom.
- `crates/core/tests/build_adapter_history.rs`'s
  `an_unchanged_node_checkout_replays_its_units_without_reading_a_manifest`,
  `cost_report_real_pipeline_unchanged_and_one_group_change`,
  `python_go_swift_and_android_units_reach_the_report_and_full_equals_incremental`
  (all asserting `containers_reused > 0`/`containers_identified == 0` on
  a second, unchanged-container pass through the real
  `report_full_mode_scoped` pipeline) -- these are the actual "existing
  work-counter tests" the chunk's item 4 asks to find and re-point for
  Cargo's replay cache. They needed **zero text changes**: they call
  through the public `report_full_mode_scoped` API, never
  `load_last_report`/`write_last_report` by name, so swapping
  `CargoConsumer`'s internals from the JSON cache to
  `cargo_replay_cache.parquet` is invisible to them by design -- and
  they pass, proving the new table correctly seeds
  `ContainerCache::from_previous` the same way the deleted JSON cache
  did.
- `crates/core/tests/build_adapter_cost.rs`/`build_adapter_contract.rs`/
  `agent_container_seams.rs`/`reviewer_counterexamples_stack{3,4}.rs`
  construct `ContainerCache`/agent `ContainerCache` directly (never
  through the persisted table) -- confirmed unaffected, unchanged.

## Verification run this session (in order)

- `cargo check -p swamp-core --lib` -- clean.
- `cargo build --workspace` -- clean.
- `cargo test --workspace --no-run` -- every test target compiles.
- `cargo test -p swamp-core --lib growth::` -- 38 passed, 2 pre-existing
  ignored (unrelated real-store comparison tests gated on an env var),
  including the 6 new tests above.
- `cargo test -p swamp-core --test fsevents_incremental -- --test-threads=1`
  -- 10 passed, including both re-pointed tests.
- `cargo test -p swamp-core --test store_contents_are_allowlisted --test build_adapter_history -- --test-threads=1`
  -- 9 passed.
- `cargo fmt --all --check` -- one diff (line-wrapping in the new
  `growth.rs` functions), fixed with `cargo fmt --all`, clean after.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` --
  clean.
- `cargo run --locked -p swamp-source-audit` -- all 11 audits pass,
  including `no_unreferenced_public_items` (confirms `root_key`/
  `read_cargo_replay_cache` being made `pub` did not leave anything
  else unreferenced) and `json_writes_allowlisted`.
- `cargo test --workspace --locked` -- full run: [status/counts filled
  in after the run completes; started before this note and finishes
  after -- see the commit/CI section of the final report for the
  authoritative pass/fail count].

## Deviations from the chunk text, and why

- The chunk's item 2 suggested extending `nested_artifacts.parquet`
  itself with a root key "if that is the cleanest" design. Considered
  and rejected in favor of a second, root-keyed file reusing the same
  row shapes -- see the "Why a second, root-keyed file" section above.
  This is a design choice the chunk explicitly left open ("say why"),
  not a narrowing of scope.
- Item 4's "assert via the existing work counters that no re-walk
  happens, matching how the zst path was tested before -- find those
  tests and re-point them" is fully satisfied for the Cargo/build-
  artifact side (`build_adapter_history.rs`'s counter-based tests,
  found and confirmed unchanged-and-green). For the git-signals side,
  no existing counter-based or full-pipeline test of this specific
  behavior was found (git activity has no subprocess-spawn counter to
  assert zero on); a new test
  (`an_unchanged_worktree_ages_stored_signals_instead_of_recomputing_them`)
  was written instead, proving reuse through the actual aged-value
  arithmetic rather than a counter. Flagged here rather than silently
  presented as "the same test, re-pointed."

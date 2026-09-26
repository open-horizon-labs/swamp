# Full developer-storage detector catalog, multi-root TUI, CLI JSON review (chunk G)

## Aim

Implement #45-#49 (the full developer-storage location-detector
catalog), #51 (multi-root reports/coverage/live refresh in the TUI),
and #52 (CLI JSON + skill exposure of coverage/shared storage), plus
two B2 gaps the catalog makes urgent: the external-location
double-measurement bug, and verifying (not redoing) chunk F's
`swamp propose --external`. Per the handoff at
`.oh/handoffs/2026-09-21-claude-full-scope.md` section 3, as one
worker chunk (`chunk G`) in the `integration/full-scope` sequential
chain.

## What landed

- **The external-location double-measurement fix**, done first because
  the new catalog makes the bug much more visible (many more detectors
  propose a base location plus categorized sub-locations nested inside
  it): `scope::EffectiveScope::external_pruned_subtrees` (new field,
  computed during the existing nested-folding pass) names every
  detector-resolved candidate that folded into a kept root, so
  `report::report_scope_with_source` can prune it from that root's
  ordinary walk and note why; `walk::resize_artifact_excluding`
  (`process_size` now honors `AttrShared::excluded`, mirroring
  `process_walk`) lets `external::discover_and_measure` exclude any
  *other* candidate nested inside a given one from that one's own
  measurement. `crates/core/tests/external_double_measurement.rs`
  asserts reconstruction identities (the sum of the pruned pieces
  equals one undivided naive measurement) for both the FOLLOWUPS
  Homebrew-in-`~/Library/Caches` scenario and Cargo home's own
  registry/git subtrees, and I verified each test actually fails
  without its corresponding fix (temporarily reverted the report.rs
  change, confirmed the test failed with real numbers, restored it).
- **#45-#49: 24 detector modules** (`crates/core/src/locations/`),
  described in the new `docs/locations.md`. Every one follows the
  existing `cargo_home.rs`/`homebrew.rs` pattern: fixture-injected
  `Environment`, never touches the real home in tests, proposes a
  conventional path even when the tool's own executable is absent.
  `report_scope_with_parts` (new, additive: `report_scope`/
  `report_scope_with_source` keep their original two-value return) also
  came out of this chunk, refactoring the per-root merge loop into
  reusable `report::merge_root_report_into`/`merge_reports` -- needed
  by #51's TUI work, not by the detector catalog itself.
- **#51: multi-root TUI.** `swamp_tui::run_scope` (new) opens over a
  resolved `EffectiveScope`'s whole present-root set via
  `report_scope_with_parts`; `App` gained `roots`/`reports_by_root`
  (generalizing `root`/`report`, with `App::new` now a thin wrapper
  over `App::new_multi_root`), `watches: Vec<Watcher>` (was
  `Option<Watcher>`), and `replace_report_for_root` (updates one root's
  cache entry, rebuilds the merged report from the whole map -- never
  touches another root's entry). `App::set_scope_note` moved from
  taking a pre-walk `EffectiveScope` to this pass's actual
  `&[coverage::RootCoverage]`, so it can show `Partial` (part of a
  present root was unreadable *during the walk*), which `RootStatus`
  alone cannot express.
- **#52: mostly already correct, some staleness fixed.** The CLI's
  `--json` contract (`scope --json`'s roots/detectors/pruned_subtrees,
  `report --json`'s `scope_coverage`, `--view external`'s
  provenance-carrying units) already covered nearly everything the
  issue asks for -- it was written for #41/#42/#43 and has held up.
  What actually needed work: `skills/swamp/references/coverage-and-history.md`
  had a stale claim that `propose` has no external-unit selection mode
  (chunk F shipped `swamp propose --external [--path P]` after that
  doc was last touched); both skill reference docs now describe
  `external_pruned_subtrees` and the double-measurement fix; `scope
  --json`'s example gained the new field and a pointer to
  `docs/locations.md`.

## Decisions that needed to be made explicitly

**Categorization is a judgment call, not a fact lookup, for several
detectors.** Cargo's `registry/cache`+`git/db` as `downloads` vs.
`registry/src`+`git/checkouts` as `cache` (extracted/derived, still
disposable but a local extraction rather than a network fetch) is the
cleanest split I found and I applied it consistently to Go's
`GOMODCACHE/cache/download` vs. the rest of `GOMODCACHE`, and to mise/
asdf's `downloads/` vs. `installs/`. Maven got no such split: its local
repository genuinely mixes downloaded and locally-`mvn install`ed
artifacts with no reliable directory-level signal, so it is
`unclassified` with an honest note rather than a fabricated split --
per-artifact evidence (`_remote.repositories`/`*.lastUpdated`) belongs
to the build-artifact-identification epic (#74), not a location
detector.

**pnpm's per-volume stores are a named gap, not a silent one.** pnpm
documents one store per disk (a volume without the home directory's
own disk gets `<volume-root>/.pnpm-store`); enumerating every mounted
volume to find one was out of scope for a location detector (it would
mean walking `/Volumes` speculatively). Only the home-disk store is
proposed; the gap is named in `docs/locations.md` and the detector's
own doc comment, not silently claimed as covered.

**Docker Desktop's relocated disk-image location is not read.** The
issue's own notes flagged this as "configured" storage; I looked for a
documented, version-stable settings-file field naming a custom disk
path and did not find one I could cite with confidence, so the
detector proposes only the default location and the gap is named
rather than guessed. The sparse-file allocated-vs-apparent distinction
*is* handled, for free, by proposing the backing file's *containing
directory* as the location: the existing per-file `st_blocks * 512`
accounting already reports allocated bytes, so no special-casing was
needed -- verified with a real sparse file in
`crates/core/tests/docker_desktop_sparse_backing_file.rs` (`set_len`
to 200 MiB with no data written, one small real file alongside it;
measured bytes stayed under 10 MiB).

**The TUI's multi-root startup trades the cached-instant-paint
optimization for simplicity when opening the configured scope (no
explicit root).** The single-explicit-root `swamp_tui::run` keeps its
exact existing behavior (paint a cached report immediately, refresh in
the background). `run_scope` (new, for the no-explicit-root case)
always observes synchronously via `report_scope_with_parts` before
opening the TUI. This is an honest simplification, not an oversight: a
"paint whatever is cached per root, then background-refresh into a
per-root cache" bootstrap path for the multi-root case would have
been a second, parallel piece of machinery to keep in sync with
`report_scope_with_parts`'s own logic, for a startup-latency win that
mostly does not apply anyway (an unchanged scope's re-observation is
already incremental per root). Documented in DESIGN.md as a
deliberate choice.

**`history_secs`'s multi-root scope is a named, un-fixed
simplification, not a silent one.** The growth-window picker's bound
is still derived from the primary root only (`App::root`, `roots[0]`).
A correct multi-root version would need the *narrowest* history among
every included root (asking for a longer window than the shortest
root's own store holds should be flagged the same way a single root's
history shortfall already is). I did not implement this: it touches
`history_span`'s signature and the picker's window-clamping logic, and
the acceptance criteria for #51 do not call it out specifically the
way "coverage clause"/"live watch"/"selection coherence" are.
Documented as a follow-up in DESIGN.md.

## What was verified, not redone

Chunk F's `swamp propose --external [--path P]` (the B2 gap the
brief's chunk pointer named) was already fully implemented
(`actions::propose_external`, `propose_unified` in `cli/main.rs`,
covered by `crates/core/tests/external_units_actions.rs`). I read the
code and its tests, confirmed the behavior, and did not touch it --
only the two skill docs' stale prose describing it as unimplemented.

## Tests broken by the larger catalog, and why they were tests, not bugs

Two existing tests hand-enumerated a `disabled_detectors` list
(`scope::tests::nested_root_folds_into_parent_and_retains_reason`,
`scope::tests::empty_scope_is_explicit_never_a_cwd_fallback`, both
already touched by an earlier chunk's version of this same class of
fix) plus two more this chunk's catalog growth exposed fresh
(`crates/cli/tests/observe.rs`'s
`observe_with_empty_scope_fails_visibly_never_falls_back_to_cwd`, and
`external_units.rs`'s
`tool_executable_removed_but_storage_remains_still_measures_it`, the
latter because Homebrew's new Cellar/Caskroom refinement moved where a
fixture's file content is correctly measured, not a regression). All
four are now dynamic (derive the disabled-detector list from the
registry itself, or assert on the correct new unit) rather than a
literal list that goes stale on every future detector addition -- a
pattern worth applying to any *future* hand-enumerated detector list
found in the test suite.

## Commits (integration/full-scope)

See `git log` for exact hashes/subjects; in order: the double-measurement
fix, the #45-#49 detector catalog, the `merge_root_report_into`/
`merge_reports` extraction and stale-test fixes, `report_scope_with_parts`
plus `cargo fmt`/clippy/test fixes, the #51 TUI multi-root work, and the
#52 skill-doc updates. Final report has the full checklist and commit
hashes.

## Follow-ups for the next worker / #62 validation

- `App::history_secs` multi-root bound (see above).
- pnpm per-volume stores, Maven downloaded-vs-local split, Docker
  Desktop relocated disk image: named gaps in `docs/locations.md`.
- Linux equivalents for the macOS-only detectors in this catalog are a
  separate validation track (#84), not attempted here.
- The #45-#49 detectors are not yet independently exercised on Linux
  (this session ran on macOS); `Platform` is data-injected so the
  fixture tests already assert Linux-configured `Environment`s produce
  the right paths without a second build target, but a real Linux CI
  run has not verified it.

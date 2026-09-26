# Multi-root coverage and external storage units

## Aim

Implement #42 (coherent multi-root observation, coverage-aware history)
and #43 (external/shared storage units) as one worker chunk in the
`integration/full-scope` sequential chain, per the handoff at
`.oh/handoffs/2026-09-21-claude-full-scope.md` section 3, following on
from #41/#44 (previous worker). Also pick up the four explicit
follow-ups that worker left for this chunk: wire
`EffectiveScope.pruned_subtrees` into the walker; make `report`/`ui`
observe the whole resolved scope instead of only the first present
root; make a scheduled run re-resolve the configured scope on every
fire instead of replaying roots frozen at install time; and revisit
`defaults = false`'s meaning if real usage disagreed with it (it did
not -- kept as-is).

## What landed

### #42: coherent multi-root observation

- `crates/core/src/coverage.rs`: `RegionStatus` (`Complete` /
  `Partial{reason}` / `Excluded` / `Missing` / `Inaccessible{reason}`)
  and `RootCoverage`, the five observation-region outcomes the issue's
  acceptance criteria name.
- `report::report_scope`/`report_scope_with_source`
  (`crates/core/src/report.rs`): the one coherent entry point over a
  resolved `EffectiveScope`. Iterates every candidate root, re-checks a
  `Present` root for read access immediately before walking (scope
  resolution and this call are never atomic), calls the *existing*
  single-root pipeline for each one, and merges the results into one
  `Report` plus a `Vec<RootCoverage>`. Physical per-root Parquet stores
  are untouched (still `growth::root_scoped_volume_id`-keyed): this is
  an orchestration layer, not a merged store -- see "Decision:
  per-root stores, not a merged store" below.
- `growth::compute_unconfirmed_worktrees` +
  `TrackedWalk::unconfirmed_worktree_ids`: distinguishes a worktree
  that is genuinely gone (`ENOENT`, real tombstone, real regrowth on
  return) from one that only lost read access (path exists,
  `read_dir` fails -> its rows are protected from this pass's
  tombstone sweep). Threaded through the bus
  (`Event::RootObserved.unconfirmed_worktree_ids` ->
  `Event::ProjectsGrouped.unconfirmed_worktree_ids` ->
  `Draft::protected_worktree_ids`) from `WalkConsumer` to
  `GrowthConsumer`, which passes it to
  `growth::observe_and_annotate`'s new `protected_worktree_ids`
  parameter.
- `walk::discover_one`/`process_walk` now prune
  `EffectiveScope::pruned_subtrees` (#41, recorded but not consumed
  until now): `discover_parallel_excluding`/
  `attribute_parallel_carrying`'s new `excluded: &[PathBuf]` parameter,
  threaded through `Ctx::pruned_subtrees` (new field) ->
  `growth::stage_tracked_with_source`'s new `excluded` parameter ->
  `full_walk`. The incremental path reaches the same result by
  filtering FSEvents' `changed_dirs` against the same list before
  `apply_incremental`/`discover_shallow`/`attribute_one_worktree` ever
  run, rather than teaching those smaller re-walk primitives their own
  exclusion list.
- CLI (`crates/cli/src/main.rs`): `report`/`ui` with no explicit root
  now call `report_scope` over the whole configured scope instead of
  `resolve_single_root`'s "first present root + stderr note".
  `report --json` gains `scope_coverage` (only when non-empty, i.e.
  some root is not `Complete`). `swamp schedule --every` with no
  explicit roots installs `observe` with **no** roots in the
  LaunchAgent's `ProgramArguments` (`schedule::install` no longer
  requires at least one root); `swamp observe`'s existing
  `roots.is_empty()` path already re-resolves the configured scope on
  every invocation, so a scheduled fire now gets that behavior too.
  `swamp schedule` (status) prints "(configured scope, resolved fresh
  on every run)" instead of a blank Roots line.

### #43: external/shared storage units

- `crates/core/src/external.rs`: `ExternalUnit` (identity
  `detector_id` + `category` + `device` + canonical `path`, reusing
  `locations::StorageCategory` directly rather than inventing a
  parallel taxonomy), `ExternalConsumer`, `discover_and_measure`.
  Every non-`builtin-defaults` detector's `Resolved` location is
  measured as one opaque unit via `walk::resize_artifact` (never
  walked for project/worktree structure). The same
  exists-but-unreadable-vs-gone distinction as #42's worktree
  protection applies per unit (`fs::read_dir` failing on an existing
  directory protects it from tombstoning and reports a coverage note
  instead of a fabricated zero).
- `growth::observe_and_annotate_external`/`annotate_readonly_external`:
  a new key family in the *existing* current+reverse-delta Parquet
  store, under `${SWAMP_DIR}/external/` (scope-wide, not per-volume,
  since an external unit's device need not match any scan root's).
  Same retention/compaction/tombstone contract as artifact rows.
- Consumer associations: `external_consumers.json`, a small sidecar
  deliberately decoupled from the growth store --
  `associate_consumer`/`dissociate_consumer` never touch
  bytes/growth/regrowth.
- `actions.rs`: `PlanUnit::external_category` (new, `#[serde(default)]`
  so it never breaks existing serialized plans),
  `unit_from_external`/`propose_external`. `execute_with_trash_opts`
  refuses any unit with `external_category` set unconditionally,
  before grant/budget checks, citing "no supported selective action
  for `<category>`".
- CLI: `report --view external` (text via
  `render::render_view_external`, and `--json`'s `{units, total_bytes}`
  result). Detector-resolved, so it works the same under an explicit
  root or the configured scope; resolved independently of
  `report_scope`'s walked roots (see "Decision: external units are not
  folded into reconciliation" below).

## Decisions that needed to be made explicitly

**Per-root physical stores, not a merged store.** The issue text's
"one observation per resolved scope, with per-volume event
cursors/topology keyed by device" reads at least as naturally as "merge
everything on one device into one store" as it does "orchestrate
existing per-root stores coherently". I chose the latter, for three
concrete reasons: (1) the guardrail this issue exists to satisfy
(`coverage-changes-are-not-storage-changes.md`) is specifically about a
root dropping out of scope, or losing access, *never* looking like a
mass deletion -- a single merged store's tombstone sweep would need a
materially more complex "which regions did this pass actually confirm"
contract to avoid exactly that hazard, where independent per-root
stores make the hazard structurally unreachable (two roots literally
cannot share a tombstone sweep if they do not share a store); (2) the
existing `root_scoped_volume_id` design (device + canonical root path)
already isolates sibling roots on one device correctly -- there was no
demonstrated bug in the physical layer, only in the orchestration
around it (see the demonstrated hazard below); (3) "no legacy migration
requirement" reads as license to change the physical layout freely, not
as an instruction to. If a later worker disagrees, the guardrail's own
validation gap note ("a particular persistence schema is not selected")
leaves room for it; this session's read is recorded here so it is an
explicit, revisitable decision rather than an implicit default.

**The demonstrated hazard this closes.** Before this chunk, a
permission-denied *root* did not error -- `walk::process_walk`'s
`fs::read_dir` failure already degrades to an `UnownedRow{reason:
PermissionDenied, bytes: 0}`, by design, so unreadable subtrees inside
an otherwise-good walk are visible rather than silently dropped. But at
the *root* level this meant a `chmod 000` root produced a **successful**
walk with (effectively) zero projects -- indistinguishable from a
genuinely empty root -- which would have fed straight into
`growth::observe_and_annotate`'s "present before, absent now ->
tombstone" sweep on every row that root had ever recorded. I traced
this from the pre-existing code (`walk::process_walk`'s `read_dir`
failure path and the tombstone sweep in `observe_and_annotate`) rather
than empirically reverting the fix to reproduce it; the adversarial
test that would catch a regression here is
`crates/core/tests/multi_root_coverage.rs`'s
`inaccessible_root_is_not_reported_as_empty`, which passes against the
code as shipped in this chunk. The fix is the pre-flight re-check in
`report_scope` (root level) plus
`compute_unconfirmed_worktrees` (worktree level, for a root that stays
readable overall while one project inside it loses access -- the more
common real case, and the one #42's acceptance criteria explicitly
name).

**External units are not folded into `reconciliation`.** #43 asks that
external-unit bytes "reconcile in `reconciliation` without double
counting". I read "without double counting" as the binding requirement
and "reconcile" as satisfied by keeping the two accounting domains
visibly separate rather than merged: external units are never summed
into `walked_total`/`attributed`/`unowned`, so there is nothing to
double-count by construction, and `--view external`'s total is
presented explicitly labelled as independent. What I did **not** do:
resolve the pre-existing #41 overlap where a detector-resolved external
location (e.g. `~/.cargo`) that happens to sit inside an ordinary scan
root (e.g. `~/src`) gets measured *twice* -- once as walked/unowned
bytes under its containing root, once as its own external unit. That
overlap already existed before this chunk (#41 already turned every
detector location into a scan-root candidate); #43 did not introduce
it and closing it fully would mean either excluding detector-sourced
roots from ordinary walking or teaching `report_scope` and
`external::discover_and_measure` to share a candidate list -- a real
design decision belonging to whoever owns tightening #41/#43's
boundary next (documented in `docs/architecture.md`'s "External and
shared storage units" section and "Limits of the current
implementation").

**No new TUI `ViewKind` in this chunk.** #42's own acceptance text says
"keep TUI rendering minimal here, #51 later expands it"; #43 separately
asks for "a minimal External storage view/rows". I judged the two
together: #42's CLI-facing work (multi-root `report`/`ui` observation,
`scope_coverage`) is done and tested; a genuinely new interactive
`ViewKind` is a real feature by this codebase's own pattern (a render
function, a key binding, a row/selection model, a snapshot frame test
-- every existing `ViewKind` has all four), not a rendering tweak, and
I chose to spend the remaining time on a complete, tested core model
and CLI/JSON contract for #43 rather than a shallow TUI addition. This
is the one acceptance-criterion gap in this chunk; recorded honestly
rather than silently narrowed. `DESIGN.md` records the intended
minimal shape (coverage-line clause, `ViewKind::External` as `'9'`) so
the next worker does not have to re-derive it.

**`swamp propose`'s CLI has no `--external` selection mode.** The
action-layer contract (`actions::unit_from_external`/`propose_external`,
`execute` refusing every external unit unconditionally) is implemented
and tested end to end at the Rust-API level
(`crates/core/tests/external_units_actions.rs`), which is what #43's
acceptance criterion asks for ("wire the unit type through actions.rs
so a plan can name one"). `Command::Propose` in `crates/cli/src/main.rs`
still requires a walked report `root: PathBuf` and has no code path
that reaches `propose_external` -- adding one means either making
`root` optional (a larger, riskier change to a well-tested existing
command under this chunk's time budget) or a separate subcommand. Left
as an explicit, named gap rather than silently implied by the docs;
the docs were corrected during this session after an initial pass
overstated it ("`swamp propose` support follows the same pattern" was
not true when written -- caught and fixed before commit, not left for
review to find).

## Adversarial tests written (not just happy path)

- `crates/core/tests/multi_root_coverage.rs` (11 tests): two disjoint
  roots on one volume stay independent and are counted once each;
  totals are root-order independent; a nested root folds into its
  parent and is not walked/measured twice; a root that becomes
  entirely unreadable is reported `Inaccessible`, not empty, and
  regains its history with zero fabricated regrowth on restore; a
  worktree that loses access while its root stays readable is
  protected from tombstoning (the #42-specific hazard, distinct from
  the whole-root case); removing a root from scope is a coverage
  change (no tombstone, no regrowth on re-adding it); an `exclude`
  entry inside a kept root is genuinely pruned (not walked, not
  reported unowned); a real deletion still tombstones and regrows
  correctly (the protection above must not blunt real accounting); a
  brand-new root's first observation has `growth_bytes: None`, never a
  synthetic zero baseline; a missing configured root is its own
  region, not zero bytes.
- `crates/core/src/schedule.rs` (+2 tests): `install` with no roots
  freezes nothing into the plist and `status` says so in words; an
  explicit root list still freezes exactly those roots (only the
  no-roots case changed behavior).
- `crates/cli/tests/report_multi_root.rs` (4 tests, end-to-end through
  the built binary): a configured multi-root scope is observed
  coherently; a missing configured root surfaces in `scope_coverage`
  (JSON) and the stderr note (text); an explicit root stays
  single-root with no `scope_coverage` key at all; `--view external
  --json` lists a detector-resolved unit.
- `crates/core/tests/external_units.rs` (6 tests): measured as one
  unit independent of any project; empty consumer set is `[]` not
  missing; two consumers on one unit are both visible and the unit is
  still counted once in `total_bytes`; association churn (3 adds, 3
  removes, 1 kept) never creates/drops a unit row or perturbs
  bytes/regrowth; a unit that loses read access is protected (coverage
  note, not tombstoned) and regains unaffected history on restore; a
  tool's storage is measured with a `NullCommandRunner` (no working
  `brew` query, i.e. "tool executable removed") standing in for the
  tool being gone entirely.
- `crates/core/tests/external_units_actions.rs` (2 tests): a plan can
  name an external unit and even be human-approved, but `execute`
  still refuses it with the category-specific reason and moves
  nothing; `propose_external` with a non-matching path is a visible
  error, never a silent empty plan.

## Measured cost

Single-sample, one developer's machine, illustrative rather than a
benchmark suite (same convention as `CHANGELOG.md`'s existing timing
notes) -- a synthetic fixture, 400 files x 8 KiB (~3.1 MiB) of build
output per project, one project per root, `--full` (skips FSEvents
entirely so the "unchanged" number reflects the walk, not replay
latency):

| scope | first observation | unchanged refresh | one-root-changed refresh |
|---|---|---|---|
| 1 root | 223 ms | 48 ms | 71 ms |
| 2 roots (same per-root data volume) | 124 ms | 94 ms | 112 ms |

The qualitative result that matters: an unchanged-tree refresh's cost
roughly doubles going from one root to two roots of the same size
(48 ms -> 94 ms) -- consistent with "scales with roots/changed
containers, not all files" (the handoff's hard constraint): each root's
own unchanged-write short-circuit (`current_changed` skip in
`growth::observe_and_annotate`) still applies per root, so
`report_scope` is not paying for a full re-walk of every root just
because one changed. The "first observation" numbers are noisy (warm
vs. cold OS filesystem cache between runs in the same process) and
should not be read as a real 1-root-vs-2-root crossover; they are
included for completeness, not as a claim.

## Follow-ups for later workers

- **#50**: revisit whether `Ui`'s *interactive* model should itself
  become multi-root (right now only the pre-flight observation is
  scope-coherent; the TUI's own rendered `App` still seeds from one
  primary root's report, same as before this chunk. `Command::Ui` in
  `crates/cli/src/main.rs` documents this explicitly).
- **#51/#60**: TUI presentation for both per-root coverage (a header
  clause) and external units (a new `ViewKind`) -- `DESIGN.md`'s new
  "Coverage line and external rows (not yet implemented)" section
  records the intended minimal shape.
- **#41/#43 boundary**: a detector-resolved external location that
  also sits inside an ordinary scan root is currently measured twice
  (once as external, once as walked/unowned) -- not new in this
  chunk, but now more visible since external units are first-class.
  Closing it means either excluding detector-sourced roots from
  ordinary walking, or sharing a candidate list between `report_scope`
  and `external::discover_and_measure`; both are real design
  decisions, not a quick fix.
- **#57**: real evidence-sourced consumer associations for external
  units (manifests, lockfiles, Docker joins). This chunk's
  `associate_consumer`/`dissociate_consumer` sidecar is a minimal,
  explicit, non-evidence-based mechanism that proves the required
  shape (zero/one/many, counted once, never duplicating/resetting
  history); it is not a competing discovery pipeline and should be
  read as the seam #57 writes real evidence into, not replaced by it.
- Cross-root hardlink dedup (a shared inode between two *different,
  non-nested* top-level roots) is not implemented; documented as a
  limit in `docs/architecture.md`.

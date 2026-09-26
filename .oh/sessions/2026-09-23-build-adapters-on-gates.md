# 2026-09-23 — Build adapters ported onto the capability gates (stack/23)

Ports the build track (stack/16 "build-adapters-node-jvm", stack/19
"build-adapters-python-go-apple-android-docker") onto
stack/22-token-binding-and-gate-hardening. Both were cut from stack/13,
before `fs_gate`, the tokens and the old section-18 call-graph audits
were replaced by the 11 exact-path gate audits. Worktree
`/Users/muness1/src/open-horizon-labs/swamp-builds`, branch
`port/build-on-gates` off `stack/22-token-binding-and-gate-hardening`,
fast-forwarded into `stack/23-build-adapters-on-gates` and pushed.

## 1. Method

`git cherry-pick` of all 17 commits from `stack/13..stack/19`, one at a
time, in original order; conflicts resolved by hand. Two commits were
skipped outright (`d8ac968`, `824046c`): each landed only §18
guardrail docs/mutation fixtures for an audit family the gate model
already retires, with zero production code. A third (`c9ff7bb`,
rewriting §18 into a derived-set/call-graph model) was cherry-picked
but its whole diff discarded except one `scripts/check.sh` line, for
the same reason. A fourth (`0ed6715`, a session-note-only commit
recording the pre-port verification) was skipped since this note
replaces it with the real, current numbers.

## 2. Conflicts and how they were resolved

- **`crates/source-audit/{audits.rs,build_audits.rs,repair_audits.rs}`**:
  every hunk took stack/22's side (the 11-rule model) and
  `build_audits.rs`/`repair_audits.rs` were deleted outright — see §3.
- **`crates/core/src/build_adapters/bounded_io.rs`**: kept as a thin
  wrapper, but rewritten to read through
  `fs_gate::read::bounded_read` instead of raw `fs::File::open`. Its
  256 KiB cap is now its own named constant,
  `fs_gate::read::BoundedCap::BUILD_MANIFEST` — deliberately **not**
  reused from the existing `BoundedCap::MANIFEST` (1 MiB): an early
  pass tried that simplification and
  `build_adapter_contract.rs::identification_reads_no_more_than_manifest_cap`
  caught it immediately (a 900 KB fixture manifest that must truncate
  at 256 KiB stopped truncating at 1 MiB and the test's origin-unknown
  assertion failed).
- **`crates/core/src/build_adapters/mod.rs`, `cargo.rs`**:
  `BuildCtx::stat`/`from_file_metadata` now name `crate::fs_gate::Metadata`
  and go through `crate::fs_gate::symlink_metadata`; `BuildCtx::list`
  gained a `list_checked` returning `crate::locations::Truncation`
  (stack/22 changed `shallow_list`'s return type to carry truncation,
  which the ported `list` had to be adapted to, mirroring
  `agents::IdentifyCtx::list_checked` exactly).
- **`crates/core/src/cargo_artifacts.rs`**: took stack/22's side
  throughout — `folded_units`/`folded_executable`/`folded_output` were
  already superseded by `build_adapters::cargo`'s own identification,
  and the `.cargo/config` read already used
  `fs_gate::read::bounded_string`.
- **`crates/core/src/external.rs`**: merged stack/22's
  `report::DiscoveryPass`-gated `discover_and_measure_in` with the
  ported commit's store-join return shape. Net: renamed to
  `observe_external(_pass: &DiscoveryPass, ...) -> Result<ExternalObservation>`
  (`ExternalObservation { units, interiors }`), the token argument
  first as every other gated discovery entry point has it.
  `report.rs`'s one call site, `build_store_join.rs`'s four, and
  `report/pass.rs`'s doc comment were updated to match.
- **`crates/core/src/docker.rs`**: the ported commit reintroduced a raw
  `Command::new("docker")` runner with its own
  `OBSERVATION_QUERIES`/`is_observation_query` allow-list — this
  fork's `run_docker_json` already went through
  `fs_gate::spawn::run(Program::Docker, ...)` from an earlier
  gate-hardening pass, so the redundant CLI-level allow-list was
  deleted (superseded) rather than kept side by side, and a new
  `run_docker_text` was added alongside `run_docker_json` for
  `buildx du --verbose`'s non-JSON output. The three shapes BuildKit
  detail needs — `version --format json`, `buildx ls --format json`,
  `buildx du --verbose --builder <name>` — were added to
  `fs_gate::spawn::shapes(Program::Docker)`, with accept/refuse cases
  in `fs_gate/spawn.rs`'s own tests.
- **`crates/tui/src/{app,lib}.rs`**: stack/22 replaced a plain
  `RefreshedObservation` struct literal at each of four call sites with
  `RefreshedObservation::merged_on_worker(...)` (review-4 C3: the
  worker merges per-root reports so the event thread never does I/O)
  and `install_refreshed` on the event-thread side. The ported commit
  added a `store_interiors` field to the same struct. Merged:
  `merged_on_worker` gained a `store_interiors` parameter (all four
  call sites updated), and `install_refreshed` gained the matching
  `set_store_interiors` branch.
- **`CHANGELOG.md`**, **`docs/build-artifacts.md`** (via a later
  commit), **`.oh/sessions/2026-09-21-build-adapters-node-jvm.md`**
  (repeated modify/delete): additive content concatenated or, for the
  stale session file, deleted (superseded by this one).

## 3. Audits dropped, kept, added

**Dropped wholesale** (stack/22 already decided this for the agent
side; the same reasoning applies verbatim to the build side): every
rule in `build_audits.rs` and `repair_audits.rs` — the old
`syn`-call-graph shape both files shared is exactly what stack/22's
session (`.oh/sessions/2026-09-22-capability-gates.md`) retired,
replacing it with types plus 11 exact-path rules. Their guardrail docs
(`.oh/guardrails/build-adapter-*.md`, `build-adapters-*.md`,
`build-units-*.md`, `build-stores-join-by-capability.md`) and their
mutation-corpus fixture directories under
`crates/source-audit/tests/mutations/build_*` were deleted with them —
the harness globs every directory there and would have tried to run
them against rules that no longer exist.

**Held automatically, with no new code**, because `build_adapters/`
routes through the exact same `fs_gate`/type surface `agents/` does:
`gate_paths_only_inside_gates`, `sinks_have_no_path_predicates`,
`json_writes_allowlisted`, `bus_static_registration`,
`tui_event_thread_has_no_gate_calls`, `no_unreferenced_public_items`,
`no_verdict_literals`, `byte_units_only_in_the_formatter`,
`guardrail_metadata`.

**One rule stack/22 lacked, extended in its own path-reference style**:
`adapters_do_not_reach_gates`'s sibling, `ids_only_in_their_module`
(`crates/source-audit/src/rules/literals.rs`), only recognized
`agents::*`/`locations::*` modules — `build_adapters::android` naming
the literal `"android"` (its own adapter id, also the Android
detector's id) had nowhere to be "own", since `own` requires the
literal's module to be *inside* the id's owning module
(`locations::android`). Fixed by adding, symmetrically to the existing
tool/detector twin relationship
(`every_adapter_id_is_also_a_detector_id`), a `build_twin`: a
`build_adapters::<x>` module may name the id constant/literal of its
same-named `locations::<x>` detector. Also added
`build_adapters::registry`/`build_adapters::matrix` to the rule's
registry allow-list (mirroring `agents::registry`/`agents::matrix`),
and extended `crate::growth::ObservationOwnership::new`'s call-site
allow-list (`rules/gate.rs`) with `build_stores`, which owns the
`KeyFamily::BuildStore` sweep exactly as `external`/`agents` own
theirs. `python.rs`'s `.get("uv")` (a `pyvenv.cfg` field name, not the
detector) got a narrow, commented `SHARED_VOCABULARY` entry rather than
a broader exemption. New mutation fixtures, in
`crates/source-audit/tests/mutations/build_adapters_are_pluggable/`
(directory named after the sibling agent-side guardrail this mirrors;
fixtures are matched to the audit that judges them by their own
`//! by:`/`expect:` header, not by directory name -- the existing
corpus already keeps fixtures for `ids_only_in_their_module` under
`detector_ids_only_in_registry/` and `agent_adapters_are_pluggable/`
for the same reason):
`01-build-adapter-names-another-familys-detector-id.rs` (reject: a
build adapter naming a different ecosystem's detector id literal, with
no twin relationship) and
`02-build-adapter-names-its-own-twin-detector-id.rs` (`expect: accept`:
the twin relationship this fix adds). Both verified directly this
session -- the reject shape spliced into `gradle.rs` and the accept
shape into `android.rs`, `cargo run -p swamp-source-audit` confirming
exactly one failure (`ids_only_in_their_module`, on the gradle.rs line)
before both were reverted from the real files -- rather than through a
full `crates/source-audit/tests/mutation_sweep.rs` run, which was not
executed this session (see §7).

One `no_verdict_literals` hit fixed by rewording, not by audit change:
`build_adapters::matrix.rs`'s BuildKit `operation_granularity` string
said "removes every unused record"; reworded to "removes every record
not in use that matches its filters" (same fact, no banned word).

## 4. The queued bug: an unreadable subdirectory inside an external unit

Confirmed and fixed. `folded_measurement::measure` (used by
`external.rs` for every machine-wide/tool-home unit) discarded the
per-directory rollups `walk::resize_artifact_stamped` returned
(`let (row, _dirs, stamps) = ...`), so a `chmod 000` subdirectory a few
levels inside a unit — not the unit's own root, which
`folded_measurement::access` already probes — made the fold sum only
what it could read and store that smaller number as a complete,
correctly-measured total. Recorded into growth history unconditionally,
that read as the unit shrinking, and restoring access on a later pass
read as regrowth: exactly the coverage-change-as-storage-change bug
`.oh/guardrails/coverage-changes-are-not-storage-changes.md` forbids,
just one level of indirection away from the case that guardrail's own
runtime tests already covered (the whole unit unreadable).

The per-worktree `DirRollup` completeness tracking `walk.rs` already
had (added for #65, the project-tree/folded-artifact case) does not
fire here: `folded_measurement`'s calls into
`resize_artifact_stamped` pass `worktree: None` (an external unit is
not a checkout), and `DirRollup`s are only ever inserted when
`group.worktree_root` is `Some`. So the fix is a new, independent
completeness signal: `AttrShared` gained an `incomplete: AtomicBool`,
set (unconditionally, not gated on having a worktree) in
`process_size`'s existing unreadable-directory branch;
`resize_artifact_stamped` now returns a fourth value, `complete: bool`,
read from it. `FoldedUnit` gained a `complete` field carrying this
through; `folded_measurement::measure`/`observe_unit_with_dirs` never
call `record_folded_measurement` when `!complete` (protects the stored
row: an incomplete pass can neither overwrite a good prior measurement
with a partial one nor anchor a future reuse on an undercount).
`external::discover_and_measure_in`'s `Unit(row)` match arm gained a
`if !row.complete` guard that treats it exactly like
`UnitObservation::Unreadable` — protected from tombstoning, no
growth/regrowth delta, last known value shown with the existing
"coverage incomplete this pass" note.

New test:
`crates/core/tests/coverage_changes_are_not_storage_changes.rs::an_unreadable_subdirectory_inside_a_unit_is_not_growth_or_regrowth`.
`chmod 000` a plain subdirectory of the fixture's CARGO_HOME unit
(distinct from its separately-measured `registry/cache` nested unit)
across three passes: baseline, the pass that sees the smaller
(incomplete) total, and a pass with access restored. Asserts zero
growth and zero regrowth throughout, and that pass 3's total is
`Some(baseline)` exactly — not a partial number carried forward. Ran
red against the pre-fix code (recorded `growth_bytes: Some(-8192)`,
the missing subdirectory's exact size) and green after.

Related but out of scope, flagged as a follow-up rather than fixed
here: `folded_measurement::folded_bytes_bounded_stamped` (the agent
family's bounded per-session fold, a different function from the one
this bug lived in) has the identical `let Ok(rd) = ... else { continue };`
shape on an unreadable non-root subdirectory, with no completeness
signal at all — only `truncated` (hitting `max_entries`) is tracked.
Whether that matters depends on whether anything downstream persists
its total as history the way `external.rs` does; a spawned follow-up
task names the exact file and line.

## 5. Verification

- `cargo run -q -p swamp-source-audit`: 11/11 ok.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: clean.
- `cargo fmt --all -- --check`: clean.
- `cargo test --workspace --locked -- --skip unchanged_observations_spaced_past_the_toosoon_floor`:
  70 test binaries, 0 failed (one intermediate run caught the
  `BUILD_MANIFEST` cap regression and the `docs_table_equals_the_capability_matrix`
  staleness below; both fixed before this final run).
- `scripts/check.sh` (SWAMP_TARGET_DIR pointed at the shared warm
  target dir): exit 0, wall 11:23:48–11:40:34 (~17 min) — fmt, clippy,
  the 11 audits, the release-graph check, the full workspace test run,
  named-targets, and the greps, all in one pass, zero failures.
  **Update, same session, after the coordinator's macOS-bash-3.2 fix
  (`15e2447`, `"${target[@]}"` → `${target[@]+"${target[@]}"}` so an
  empty `target` array does not trip `set -u`) landed on the branch:**
  the coordinator's own mutation sweep run on stack/23 found four
  broken fixtures. Two were this session's own
  (`build_adapters_are_pluggable/{01,02}`): a multi-line `//! why:`
  continuation broke the header parser (it stops recognizing header
  lines at the first line that is not `//! key: value`, silently
  splicing the rest into the fixture body as production code) --
  collapsed to one line each, matching every other fixture. Two
  predated this port (`discovery_owned_by_report_pipeline/04` and
  `/06`): `04` was ported from stack/19 against
  `external::observe_external`'s pre-token signature and never had a
  `//! by:` line; `06` targeted `external::discover_and_measure_in`,
  which this port renamed to `observe_external`, so its E0425
  (function not found) masked the field-privacy violation it meant to
  exercise. Both updated to the current signature/name and, for `04`,
  given a `by: compile:E0451` (minting the token as a struct literal
  from `crates/tui`, which the private field refuses) -- spot-checked
  by splicing each into a scratch copy of its target and running
  `cargo check` before the harness run, per the coordinator's
  instruction not to rely on manual splicing alone.
  `cargo test -p swamp-source-audit --locked --test mutation_sweep --
  --ignored`: 2 passed, 0 failed, 1308 s.
- `scripts/check-full.sh`, `SWAMP_TARGET_DIR` **unset** (the
  coordinator's own repro condition): exit 0, wall 12:54:25–13:23:02
  (~28:37) -- `check.sh`'s full fast tier, 58/58 compile-fail cases,
  the mutation sweep (2/2, 1307 s), and the single-threaded cost test
  (`spawn_oracle_covers_every_program_the_gate_can_run`,
  `unchanged_observations_spaced_past_the_toosoon_floor`), all green.
  73 `test result: ok` lines total, zero failures, zero `FAILED`/`error[`
  in the log.

Two real regressions were caught and fixed mid-session, not just
theorized: a manifest-cap simplification that silently changed a
tested truncation boundary (§2), and a stale "planned" row in
`docs/build-artifacts.md` for Android that the next commit in the
original stack/19 sequence corrected on its own once cherry-picked.

## 6. Commits (in order, `stack/13-event-gated-reuse..stack/23-build-adapters-on-gates`)

1. *(skipped: `d8ac968`, old §18 guardrails)*
2. Give build artifacts a trait, a registry and three new ecosystems
3. Show what is inside a build container, in the CLI, the TUI and the docs
4. Keep the verdict-vocabulary scan out of the source it audits
5. Prove a Node unit gets nested history on the existing key family
6. Count only supported candidates, reconcile residuals, read Maven origin
7. Present neutral-vocabulary build interiors as family groups
8. Prove #65 through the real pipeline for Node and Gradle, and fix two holes it found
9. Protect external-unit folds from an unreadable subdirectory's partial total (this port; §4)
10. Carry the identified interior in `--view builds/deps --json`, and benchmark the real pipeline
11. Rewrite the section 18 audits over derived sets and the whole-workspace call graph *(content discarded; one check.sh line kept)*
12. Document the build-adapter contract as it now behaves
13. Update `build_adapter_contract`'s `growth::observe_and_annotate` calls to the gated signature (this port)
14. *(skipped: `824046c`, old store-join guardrail)*
15. Identify Python, Go, Apple, Android and BuildKit storage, and join machine-wide stores into live reports
16. Route build-adapter I/O through the capability gate; extend the gate audits for the ported store-join family (this port; §2–§3)
17. Show store interiors and BuildKit records in the CLI, JSON, TUI and docs
18. Scan store and BuildKit units for verdict vocabulary
19. Pin the store anchors a custom DerivedData and a free-form GOMODCACHE rely on
20. *(skipped: `0ed6715`, superseded by this note)*
21. Record the build-adapters-on-gates session; add `ids_only_in_their_module` fixtures
22. *(coordinator, `15e2447`)* Fix check scripts on macOS bash 3.2
23. Fix four mutation fixtures the harness rejected as broken (§5 update)

## 7. Limits, stated

- The `folded_bytes_bounded_stamped` gap in §4 (the agent-side bounded
  fold has the same silent-undercount shape, no completeness signal)
  remains a follow-up, spawned as a separate task.
- The `build_twin`/registry allow-list extension in §3 is scoped to
  `ids_only_in_their_module`; it does not touch
  `adapters_do_not_reach_gates`; `build_adapters` was already fully
  covered there without change (that rule's `is_adapter` predicate
  matches on `agents::*` specifically, and `build_adapters` modules
  never name `fs_gate`/`locations`/`actions` directly in the first
  place — they reach I/O only through `BuildCtx`, the same discipline
  `IdentifyCtx` gives agent adapters).

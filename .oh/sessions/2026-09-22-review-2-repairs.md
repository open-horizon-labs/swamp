# 2026-09-22 — repairs after independent re-review 2 (stack/11)

Branch `stack/11-review-2-repairs`, based on `stack/10-adapter-registry-and-matrix`
(`6a2c563`). Inputs: `review/REVIEW-STACK-2.md` (decision: ADJUST, six
counterexamples plus a cost measurement), the three reviewer test files, and —
arriving mid-session — `review/AUDIT-MUTATION-SWEEP.md`, which reports that a
systematic per-audit mutation sweep bypassed **all 42** audits with compiling,
harmful mutations. `GUARDRAILS_SPEC.md` section 17 re-ordered the work: harden
the shared audit machinery first, port the existing audits onto it, then the
seam fixes.

Same discipline as stack/09: the audits and the reviewer tests land **first**, in
a deliberately failing commit, with the baseline recorded here as the audits' own
enumeration rather than a prose list.

## What landed in this commit

- `.oh/guardrails/computed-but-not-delivered.md` — `audit: none` → `audit:
  computed_but_not_delivered`. This was the one guardrail in the directory with
  no audit, and it is the one the top completion overclaim violates.
- `.oh/guardrails/coverage-changes-are-not-storage-changes.md` (`severity: hard`,
  previously no `audit:` field at all) → `audit:
  coverage_changes_are_not_storage_changes`, plus a Detection and a
  Runtime-tests section.
- `.oh/guardrails/activity-and-consumer-evidence-have-limits.md` (`severity:
  hard`, previously no `audit:` field) → `audit:
  activity_and_consumer_evidence_have_limits`.
- `.oh/guardrails/no-second-traversal-on-report-path.md` — the audit now follows
  the *callee*: `folded_measurement::measure` must consult the persisted folded
  rows before it may reach `resize_artifact*`. The re-review's sharpest audit
  finding was that the allow-list proves *where* a re-walk is written, not
  *whether* it happens.
- `.oh/guardrails/tui-refresh-preserves-scope.md` — statement amended: a refresh
  narrowed to one root asks for no unit parts and leaves those vectors alone. The
  old statement ("external/agent unit vectors are refreshed from the same
  observation") is the mechanism that empties them.
- `.oh/guardrails/discovery-consumes-effective-scope.md` — Limits records what
  its stated blind spot cost (CE1) and names the one helper both entry points
  must route through.
- `no_dead_public_evidence_api` extended to `pub const`/`pub static`.
- The three reviewer test files, byte-identical except for one change recorded
  below, and four new runtime test files named in `scripts/check.sh`.

### The one edit to a reviewer file

`crates/core/tests/reviewer_cost_measurement_stack2.rs`'s `observe` helper takes
an `at: u64` it never uses, which `cargo clippy --workspace --all-targets --
-D warnings` (run by `scripts/check.sh`) rejects. The parameter is renamed to
`_at`. No assertion, fixture, threshold or measurement is touched. That is the
only difference from the delivered file in any of the three.

## Baseline: `cargo run -p swamp-source-audit` (45 audits, 4 failing)

    ok    tui_actions_off_event_thread
    ok    one_byte_formatter
    ok    legacy_invariants
    ok    fsevents_before_full_walk
    ok    column_store_parquet_zstd
    ok    reverse_delta_current_plus_deltas
    ok    scheduled_refresh_launchagent
    ok    folding_only_for_artifacts
    ok    symlinks_never_followed
    ok    incremental_walk_only_changed_subtrees
    ok    walk_optimized_parallel_pool
    ok    dir_mtime_int32_minutes
    ok    agent_interface_facts_not_verdicts
    ok    human_only_authorization
    ok    no_consumer_knows_other_consumers
    ok    static_registration_only
    ok    all_report_paths_through_bus
    ok    extractors_are_pluggable
    ok    event_bus_pluggable_consumers
    ok    adr_validation
    ok    execution_sinks_recheck_live_state
    ok    protection_fails_closed
    ok    discovery_consumes_effective_scope
    ok    explicit_only_scope_when_defaults_false
    ok    history_sweeps_are_owned
    FAIL  no_second_traversal_on_report_path: folded_measurement::measure calls `resize_artifact*` without first consulting the persisted folded rows (one of: reuse_folded_measurement / persisted_folded_bytes / record_cache_hit-guarded lookup). The allow-list proves only where a re-walk is written, not whether it happens -- `.oh/guardrails/no-second-traversal-on-report-path.md` claims reuse, so the reuse has to be in the code
    ok    occupancy_is_tristate_at_sinks
    ok    tui_refresh_preserves_scope
    ok    store_data_is_parquet_not_json_sidecars
    ok    json_persistence_is_allowlisted
    ok    agent_adapters_are_pluggable
    ok    agent_adapters_read_bounded_headers_only
    ok    agent_adapters_do_not_traverse
    ok    agent_adapters_are_inspection_only
    ok    agent_adapters_do_not_emit_content
    ok    agent_units_built_through_builder
    ok    agent_adapters_are_environment_free
    ok    agent_adapters_do_not_reach_detectors
    ok    agent_adapter_test_contract
    ok    detector_ids_only_in_registry
    ok    discovery_owned_by_report_pipeline
    FAIL  no_dead_public_evidence_api: these public evidence-API functions have no non-test caller; wire them into the live pipeline where their issue requires, or delete them together with the docs and CHANGELOG claims that say they are delivered:
      crates/core/src/activity.rs::ACTIVITY_EVIDENCE_INVENTORY (pub const/static, no non-test reader)
    FAIL  computed_but_not_delivered: crates/core/src/artifact.rs::NestedArtifact.decision_evidence is written only as an empty default (1 site(s)); compute it or remove it together with the docs/CHANGELOG claim that it is delivered
    FAIL  coverage_changes_are_not_storage_changes: growth.rs: `ObservationOwnership` has no `excluded_subtrees`; a path-prefix window cannot express "inside a covered root, outside this pass" and will tombstone an excluded nested location (CE4)
      growth.rs: `ObservationOwnership::covers` does not consult `excluded_subtrees`
    ok    activity_and_consumer_evidence_have_limits
    4 audit(s) failed

## Baseline: the reviewer counterexamples

`cargo test -p swamp-core --test reviewer_counterexamples_stack2 -- --test-threads=1`
→ `1 passed; 5 failed`:

    
    failures:
    
    ---- a_config_only_exclusion_must_not_invent_growth_or_regrowth stdout ----
    
    thread 'a_config_only_exclusion_must_not_invent_growth_or_regrowth' (14213239) panicked at crates/core/tests/reviewer_counterexamples_stack2.rs:232:5:
    excluding a nested location reported the parent as having grown: [("/private/var/folders/hl/g5sgqjz50bld_bk0ddmm5xn40000gp/T/.tmpt9dB1V/cargo", 69632, Some(65536))]
    note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
    
    ---- a_disabled_detector_must_not_probe_its_tool stdout ----
    
    thread 'a_disabled_detector_must_not_probe_its_tool' (14213315) panicked at crates/core/tests/reviewer_counterexamples_stack2.rs:392:5:
    two observations with every detector disabled spawned 2 subprocesses: ["docker", "docker"]
    
    ---- an_excluded_home_must_stay_excluded_under_an_explicit_root stdout ----
    
    thread 'an_excluded_home_must_stay_excluded_under_an_explicit_root' (14213423) panicked at crates/core/tests/reviewer_counterexamples_stack2.rs:92:5:
    an excluded home was scanned under --root: 1 agent units ["/var/folders/hl/g5sgqjz50bld_bk0ddmm5xn40000gp/T/.tmphU6Ewy/claude/debug"], 1 external units ["/private/var/folders/hl/g5sgqjz50bld_bk0ddmm5xn40000gp/T/.tmphU6Ewy/claude"]
    
    ---- protect_add_must_not_accept_a_path_it_cannot_enforce stdout ----
    
    thread 'protect_add_must_not_accept_a_path_it_cannot_enforce' (14213509) panicked at crates/core/tests/reviewer_counterexamples_stack2.rs:305:5:
    `protect add debug` was accepted and confirmed, and the data it named was still moved
    
    ---- reviewed_snapshot_must_see_a_same_second_same_size_rewrite stdout ----
    
    thread 'reviewed_snapshot_must_see_a_same_second_same_size_rewrite' (14213591) panicked at crates/core/tests/reviewer_counterexamples_stack2.rs:161:5:
    an in-place rewrite of a reviewed member passed the identity recheck
    
    
    failures:
        a_config_only_exclusion_must_not_invent_growth_or_regrowth
        a_disabled_detector_must_not_probe_its_tool
        an_excluded_home_must_stay_excluded_under_an_explicit_root
        protect_add_must_not_accept_a_path_it_cannot_enforce
        reviewed_snapshot_must_see_a_same_second_same_size_rewrite
    
    test result: FAILED. 1 passed; 5 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.89s
    
    error: test failed, to rerun pass `-p swamp-core --test reviewer_counterexamples_stack2`

`cargo test -p swamp-tui --test reviewer_counterexamples_stack2_tui` and
`cargo test -p swamp-core --test reviewer_cost_measurement_stack2 -- --nocapture`:

    running 1 test
    a_live_refresh_of_one_root_must_not_empty_the_agent_view --- FAILED
    
    failures:
    
    ---- a_live_refresh_of_one_root_must_not_empty_the_agent_view stdout ----
    
    thread 'a_live_refresh_of_one_root_must_not_empty_the_agent_view' (14214680) panicked at crates/tui/tests/reviewer_counterexamples_stack2_tui.rs:118:5:
    assertion `left == right` failed: a live refresh of an unrelated root emptied the agent view (1 -> 0)
      left: 0
     right: 1
    note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
    
    
    failures:
        a_live_refresh_of_one_root_must_not_empty_the_agent_view
    
    test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.21s
    
    error: test failed, to rerun pass `-p swamp-tui --test reviewer_counterexamples_stack2_tui`
    === COST ===
        |                                                  ^^ help: if this is intentional, prefix it with an underscore: `_at`
        |
        = note: `#[warn(unused_variables)]` (part of `#[warn(unused)]`) on by default
    
    
    running 1 test
    --- COST REPORT (mandate item 4) ---
    fixture: 5000 agent sessions, 20000-file cargo cache, 500-file npm cache, 200-file model store, 1 walked project root
    pass 1: 5001 agent units, 6 external units, 706.621375ms
      dirs_listed=18 files_statted=5008 header_bytes=205000 cache_hits=0 cache_misses=5006
    pass 2 (nothing changed): 5001 agent units, 6 external units, 612.100083ms
      dirs_listed=18 files_statted=5008 header_bytes=0 cache_hits=5000 cache_misses=6
    subprocess spawns: pass 1 = 5, total = 10 (["docker", "docker", "docker", "docker", "docker", "docker", "docker", "docker", "docker", "docker"])
    --- END COST REPORT ---
    
    thread 'two_unchanged_full_observations_cost_report' (14214949) panicked at crates/core/tests/reviewer_cost_measurement_stack2.rs:187:5:
    assertion `left == right` failed: an unchanged observation must spawn no subprocesses, got ["docker", "docker", "docker", "docker", "docker", "docker", "docker", "docker", "docker", "docker"]
      left: 10
     right: 0
    note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
    two_unchanged_full_observations_cost_report --- FAILED
    
    failures:
    
    failures:
        two_unchanged_full_observations_cost_report
    
    test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.15s
    
    error: test failed, to rerun pass `-p swamp-core --test reviewer_cost_measurement_stack2`

CE2's end-to-end variant
(`an_in_place_rewrite_of_a_reviewed_member_must_not_spend_the_approval`) passes
at the baseline; the layer-level variant
(`reviewed_snapshot_must_see_a_same_second_same_size_rewrite`) fails. The hole is
real at the layer that owns it, which is where the fix belongs.

The measured cost baseline, for comparison after the repairs: 5,000 agent
sessions + a 20,000-file Cargo cache + a 500-file npm cache + a 200-file model
store + one walked project root.

| | pass 1 | pass 2 (unchanged) |
|---|---|---|
| wall time | 707 ms | 612 ms |
| session-header bytes | 205,000 | 0 |
| identification cache hits / misses | 0 / 5,006 | 5,000 / 6 |
| `dirs_listed` | 18 | 18 |
| `files_statted` | 5,008 | 5,008 |
| subprocess spawns | 5 (`docker`) | 5 (`docker`) |

`dirs_listed`/`files_statted` are the numbers the re-review calls out as blind:
`walk.rs` calls no counter at all and the counters are `thread_local!` while the
walker spawns a pool, so a 20,000-file traversal reports "2 dirs listed, 0 files
statted". Nothing below this line is measured with that instrument until
`walk.rs` is instrumented and the counters are made process-global.

---

# Part 2 — the walker instrument, the reuse, and the corpus ratchet

Written after the repairs above landed. Three things here: a measurement that
was lying, a claim that was not implemented, and 26 audits nobody had shown
reject anything.

## The instrument (P1 on the audit itself)

`work_counters` began as process-global atomics, went `thread_local!` because
parallel lib tests raced on them, and in doing so went blind: `walk.rs` lists
and stats on a worker pool, so the measuring thread saw none of the traversal.
A 20,000-file walk measured "2 dirs listed, 0 files statted".

Neither scope alone is right, so there are two and every record writes to both:
a **process-global** sink (`snapshot`/`since`/`reset`, what an integration test
measuring a whole observation wants, serialized by the test itself) and a
**scoped** sink installed by `measured` and inherited by the pool workers that
thread spawns (`Pool::drain` captures `work_counters::current()` and `install`s
it on each worker). A `== 0` assertion in a parallel suite is meaningful again
*and* the pool is visible. Every lib test that read the counters moved to
`measured`; the two integration files keep the global sink.

`walk.rs` then records at the real syscall sites (every `read_dir` in
`process_walk`/`process_size`/`discover_one`/`measure_directory`, every
`symlink_metadata`). The same fixture that reported 18 dirs / 5,008 stats above
reports 71 / 36,281 once the walker is counted — the *first* honest number this
measurement has produced.

## The reuse (the other half of incrementality)

`folded_measurement::measure` re-walked every external root on every pass while
the guardrail above it claimed reuse. It now consults the folded rows the
previous pass persisted:

- `walk::resize_artifact_stamped` returns one `DirStamp` per directory a Size
  job listed, taken from the `symlink_metadata` that job already does — a push
  per directory, no extra syscall.
- `external/folded.parquet` stores them: one row per **directory** (never per
  file — the handoff forbids a per-file persistent inventory), plus a root row
  carrying `bytes`/`hardlinked`/`mtime_max` and a digest of the exclusion list
  the measurement was taken under, so a changed exclusion set is a miss rather
  than a silently wrong answer.
- `reuse_folded_measurement` stats those directories and returns the stored
  measurement when every `mtime`/`ctime` still matches. `external.rs` asks
  through `observe_unit`, so a reused unit does not even pay the readability
  probe's listing.

**Measured** (`an_unchanged_external_cache_root_is_not_re_traversed`, 20,000-file
Cargo registry fixture):

| | first pass | unchanged second pass |
|---|---|---|
| wall time | 73 ms | 1.9 ms |
| `dirs_listed` | 6 | **0** |
| `files_statted` | 20,004 | **4** |

Four stats is the unit's directory count. That is the claim — work scales with
containers, not with files — and a bound of "fewer than last time" would not
show it. `a_changed_external_cache_root_is_measured_again` is the other half: a
file added under the unit costs a real re-measurement and its bytes are
reported.

**What the reuse cannot see**, stated here and on the function rather than left
for the next reviewer: the stamp is each directory's `mtime`/`ctime`, so a file
rewritten *in place* (same name, same directory) does not move it. If that
rewrite also changes the file's allocation, the reused byte total is stale until
something else in that directory changes or the store is cleared. The
alternative is stat'ing every file on every pass, which is exactly the "scales
with all files" shape the handoff forbids.

## Cost after the repairs (`reviewer_cost_measurement_stack2`)

Same fixture as the baseline table above, now with the instrumented walker:

| | pass 1 | pass 2 (unchanged) |
|---|---|---|
| wall time | 639 ms | 497 ms |
| session-header bytes | 205,000 | **0** |
| identification cache hits / misses | 0 / 5,006 | 5,006 / **0** |
| `dirs_listed` | 71 | 46 |
| `files_statted` | 36,281 | 10,580 |
| subprocess spawns | **0** | **0** |

(Numbers from the final `scripts/check.sh` run, 2026-09-22. The
baseline table further up reported `dirs_listed` 18 / `files_statted`
5,008 for the same fixture; that was the blind instrument, not a
smaller cost.)

## Measured, not satisfied: two assertions in the reviewer cost test

`two_unchanged_full_observations_cost_report` asserts four things about the
unchanged pass. Two hold (`header_bytes_read == 0`, zero subprocess spawns).
Two do not, and I could not make them hold honestly:

    assert_eq!(second.dirs_listed, 0, ...)    // actual: 46
    assert_eq!(second.files_statted, 0, ...)  // actual: 10,580

Not for want of reuse — the external family went from 36,281 stats to 4 for its
biggest unit. Two structural reasons, both of which are properties of designs
this project already reviewed and accepted:

1. **The identification cache's validity key is each session file's own
   `(len, mtime_ns, ctime_ns, inode)`.** That is what makes
   `a_large_agent_home_reads_headers_once_and_not_again` able to assert *zero*
   header bytes over 5,000 sessions. You cannot know a session is unchanged
   without stat'ing it, so an unchanged agent home costs one stat per session by
   construction, and its containers have to be listed to know which sessions
   exist. `second.files_statted == 0` contradicts the cache it is measuring.
2. **A fresh fixture's walk cannot be event-incremental in two passes.** Pass 1
   is a `full_rules_changed` walk, which deliberately does not touch the
   FSEvents source (so it anchors no event id); pass 2 therefore replays from
   nothing and refuses with `no_stored_event_id`. I verified FSEvents itself
   works under the test's tempdir, and that a replay taken immediately after a
   write still reports that write as a change — fseventsd's own log lag, which
   is why `RefreshRefusal::TooSoon` exists. So even a three-pass version would
   be timing-dependent, not deterministic. A full walk of the fixture's project
   root costs 6 listings and 10 stats.

The honest options were: relax the reviewer's assertions (forbidden, and it
would hide a real number), reclassify cache-validation stats as "the
events/coverage check" so they stop being counted (the same vacuous-instrument
sin the re-review just caught), or leave the test failing with the numbers
recorded. I left it failing. `scripts/check.sh` is red on exactly this one
test and on nothing else.

Reducing it further is real work, not a trick, and it is the obvious next
chunk: give the agent family the same folded-row reuse the external family now
has, keyed per *container* rather than per session file. That would take
`dirs_listed` to the walk's 6 and `files_statted` to the walk's 10. It would
still not be zero.

## The mutation-corpus ratchet, emptied

`NOT_YET_IN_THE_CORPUS` listed 26 audits. It is now empty: 45 audits, 136
fixtures, at least three rejections each, applied to a copy of the real
workspace. Writing them found three holes in the *shared* layer — which is why
the fixtures are worth more than the audits they test:

- `ast::functions` attributed macro literals by function **name**, so a second,
  wrong `files_schema` in a submodule inherited the real one's
  `vec![Field::new("mod_time_min", ..)]` and passed `dir_mtime_int32_minutes`
  while storing seconds in the minutes column. A shadowing `canonical_roots`
  inherited the real one's `"canonicalize {}"` the same way.
- `ast::function` returned the **first** definition with a name, so "the
  function named X must have property P" was satisfied by whichever X came
  first. `ast::functions_named` returns all of them.
- `Func` carried no **signature**, so `fn f(d: &dyn locations::Detector)` was
  invisible to every body-based rule.

Four audits also stopped accepting a name in place of a semantics: every
`canonical_roots` must call `canonicalize`; `detectors_permitted` must read both
`defaults` and `enabled_detectors`; an `#[ignore]`d required adapter test does
not count; and `impls_of`/`pub_struct_fields` resolve through `use` renames.

## Fixture hygiene

`crates/cli/tests/agent_storage_cli.rs` scoped with a *deny*-list of two or
three detector ids, and four of its tests wrote no scope config at all.
`core-simulator`, `homebrew` and `ruby-install` resolve absolute system paths no
injected `HOME` can confine, so those runs read this machine's real disk: the
file took **over twenty minutes**. With allow-list scopes it takes **4.7 s**,
and `a_fixture_scope_reaches_nothing_outside_its_fixture_home` drives the real
binary and fails on any resolved root outside the tempdir.

## Overclaims closed here

- **Maven `_remote.repositories`** — withdrawn, not wired. Reading one marker
  per artifact is a traversal of the whole local repository, which the report
  path forbids; the single production call site passes `false` and always did.
  `docs/architecture.md:95` and its Limits paragraph now say what
  `docs/locations.md` already said, so the two docs agree.
- **`docs/architecture.md`'s Limits section**, both directions: bulk marking
  *does* reach agent rows (it claimed a gap that was closed), and the
  "five tools implemented, nine `Planned`" paragraph contradicted `:729` of the
  same file — `matrix.rs` has no `Planned` rows; what it has is a support level
  per row, where `Unverified` means the citation has not been re-fetched, not
  that the storage is unmeasured.

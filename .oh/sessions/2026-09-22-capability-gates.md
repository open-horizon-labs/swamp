# 2026-09-22 — Capability gates (stack/21)

Integration-owner decision after re-review 4 (46/46, 12/12 and 7/7
mutations slipped the `syn` call-graph audits): stop extending the
model. Guardrail semantics move into the type system. The audits shrink
to exact path-reference rules, and every retired rule gets a trybuild
compile-fail case. See `docs/architecture.md` "Capability gates" and
GUARDRAILS_SPEC §19.

## 1. What changed

- **Gate.** `crates/core/src/fs_gate/{mod,read,store,columns,spawn,destroy,sys}.rs`
  plus the FSEvents FFI (`fs_events/macos.rs`) are the only code in
  core, cli and tui that names `std::fs`, `std::os::unix::fs`, unbounded
  `std::io`, `OpenOptions`, `std::process`, `libc`, `trash`,
  `tempfile`, `parquet` or `zstd`. Two things enforce this: clippy
  `disallowed_methods`/`disallowed_types` (per-crate `clippy.toml`,
  denied at each crate root outside `cfg(test)`) and
  `gate_paths_only_inside_gates`.
- **Tokens with private constructors:**
  - `RecheckProof` comes only from `recheck::run_all`. It is not `Clone`,
    it is `#[must_use]`, it is consumed by the call that uses it, it
    expires after 120 s, and it covers a fixed set of paths.
  - `Authorized` comes only from `authorize` or `authorize_confirmed`.
  - `HumanConfirmed` comes only from `cli_command` or `tui_dialog`, and
    the audit pins where those may be called.
  - `bus::Stage` comes only from `EventBus::run`. The walk, the
    incremental re-walk, the three history writers and every consumer
    take one.
  - `report::DiscoveryPass` lives in its own module (`report::pass`),
    and the audit pins its one mint site to `observe_scope`.
  - The rest: `attribution::Classified`, `PermittedDetectors`,
    `growth::columns::Owned`, and `evidence::Reason` (every `FactStatus`
    reason is one).
  - Opaque types: `ProtectList` answers only `conflict()` and has no
    `Default`. `OccupancyState` has no boolean view and may be named
    only in `occupancy` and `recheck`; proposal time gets a refusal
    string from `recheck::occupancy_refusal`.
- **`testing` feature.** It gates `fs_events::testing`,
  `Stage::for_tests`, `DiscoveryPass::for_tests`, string-actor
  `approve`/`add_standing_grant`, `agents::is_active` and
  `From<&str> for Reason`. Only dev-dependencies enable it. A release
  build with it hits `compile_error!`, and the audit rejects any
  `[dependencies]` entry that enables it.
- **Audits.** The 45 semantic rules are gone. Eleven exact rules remain,
  on the compiler's module tree: orphan files and `#[path]` are
  rejected, and `cfg` is read as attributes, so doc comments are not
  `cfg(test)`. Section 3 lists what the sweep added.
- **Review-4 code findings:**
  - C1: `shallow_list` on an unreadable directory returns `Unreadable`.
  - C2: an older rules version with no stored anchor forces a full walk.
    The reviewer's test covers it, and so does a new TUI live-path test,
    `scope_preserving_refresh.rs::a_live_refresh_over_rows_from_an_older_rules_version_walks_in_full`.
  - C3: the canned FSEvents source is `testing`-only. The TUI replays
    `LivePlanSource`.
  - The TUI merges per-root reports on its workers
    (`RefreshedObservation::merged_on_worker`), so the event thread
    installs results and does no I/O.
- **Spawn oracle.** `reviewer_cost_measurement_stack3.rs` now shims
  every `Program::ALL` binary plus `simctl` and `mdls`.
  `spawn_oracle_covers_every_program_the_gate_can_run` pins that. The
  Linux branch of the cost test still needs to run in CI: this branch
  has no `ci.yml`, and the Linux track's `ci.yml` should run
  `--test reviewer_cost_measurement_stack3 -- --test-threads=1` on
  Linux once rebased.

## 2. Rules retired → what holds them now

Each guardrail's `## Detection` starts with a `Mechanism:` line.
Compile-fail cases are in `crates/core/tests/compile_fail/` (40 of them)
and run from `crates/source-audit/tests/compile_fail.rs` against the
production API.

| retired audit | now held by |
|---|---|
| execution_sinks_recheck_live_state | type (`trash_move_needs_a_recheck_proof`, `recheck_proof_is_minted_only_by_run_all`, `recheck_proof_is_not_clone`, `recheck_proof_is_spent_once`); gate audit; clippy |
| human_only_authorization | type (`authorized_is_minted_only_by_authorize`, `human_confirmation_is_not_a_struct_literal`, `approval_without_a_confirmation_does_not_exist`); gate audit mint sites |
| protection_fails_closed | type (`protect_list_has_only_conflict`, `protect_list_has_no_default`); `sinks_have_no_path_predicates` |
| occupancy_is_tristate_at_sinks | type (`occupancy_has_no_bool`, `occupancy_does_not_convert_to_bool`); gate group |
| history_sweeps_are_owned, coverage_changes_are_not_storage_changes, reverse_delta_current_plus_deltas | type (`history_rows_are_private_to_the_store`); ownership-window gate group; runtime tests |
| dir_mtime_int32_minutes, column_store_parquet_zstd | type (`parquet_codec_is_not_selectable`); Arrow/Parquet gate paths; `every_column_chunk_written_is_zstd` |
| discovery_owned_by_report_pipeline | type (`discovery_pass_*`); mint site pinned |
| discovery_consumes_effective_scope, explicit_only_scope_when_defaults_false | type (`permitted_detectors_*`, `detector_candidates_*`) |
| all_report_paths_through_bus, event_bus_pluggable_consumers, static_registration_only, extractors_are_pluggable, no_consumer_knows_other_consumers | type (`bus_register_is_private`, `bus_has_no_empty_constructor`, `bus_stage_*`); `bus_static_registration`; `no_unreferenced_public_items` |
| folding_only_for_artifacts | type (`classified_is_minted_by_classify`) |
| no_second_traversal_on_report_path, walk_optimized_parallel_pool, agent_adapters_do_not_traverse | Stage/DiscoveryPass types; `read_dir` gate group; clippy |
| agent_adapters_read_bounded_headers_only | type (`content_reads_need_a_cap`, `caps_are_named_constants`, `no_unbounded_read_in_the_gate`) |
| agent_units_built_through_builder | type (`agent_units_are_built_by_the_builder`, `agent_units_protection_not_via_a_binding`, `unprotect_needs_a_reason`) |
| json_persistence_is_allowlisted, store_data_is_parquet_not_json_sidecars, scheduled_refresh_launchagent | type (`json_writes_name_a_store_file`, `atomic_write_is_private`, `launch_agent_is_a_named_text_file`); `json_writes_allowlisted` |
| every_spawn_is_counted | type (`spawn_takes_a_program_not_a_string`, `no_command_escapes_the_gate`); gate paths; clippy |
| symlinks_never_followed | type (`metadata_does_not_follow_by_default`); gate paths; clippy |
| activity_and_consumer_evidence_have_limits | type (`blank_reason_does_not_compile`, `reason_is_not_a_plain_string`, `fact_status_reason_is_not_a_plain_string`) |
| agent_adapters_* (environment, emit, detectors, inspection-only, pluggable) | `adapters_do_not_reach_gates`, `ids_only_in_their_module` |
| agent_interface_facts_not_verdicts, one_byte_formatter, detector_ids_only_in_registry | `no_verdict_literals`, `byte_units_only_in_the_formatter`, `ids_only_in_their_module` |
| tui_actions_off_event_thread, tui_refresh_preserves_scope | `tui_event_thread_has_no_gate_calls`; TUI report-API allow-list; runtime tests |
| fsevents_before_full_walk, incremental_walk_only_changed_subtrees | runtime tests (ordering and data flow; `audit: none` with a dated reason) |
| agent_adapter_test_contract, adr_validation, no_dead_public_evidence_api, computed_but_not_delivered | `guardrail_metadata`, `no_unreferenced_public_items` |

## 3. Mutation evidence

`crates/source-audit/tests/mutation_sweep.rs` supersedes
`mutation_corpus.rs`, `mutation_operators.rs` and
`reviewer_mutation_sweep_stack3.rs`; copies are in the scratchpad. It
applies each fixture to a copy of the workspace. It then records the
failing audits and every `cargo clippy --lib --bins` error whose primary
span falls inside the mutation's own lines. Compiles are batched, and a
fixture is recompiled on its own only when a batch cannot decide it. A
fixture counts only if a kind named on its `//! by:` line rejected it.
Parse errors never count. An accept fixture must pass every audit and
compile with no errors.

- **Re-review 4 sweep** (converted from
  `reviewer_counterexamples_stack4_sweep.rs`): M 46/46, W 12/12,
  U 7/7 rejected; A 3/3 accepted. A1–A3 were ported: A1 moved into
  `recheck`, A1 and A2 made private, A3 moved to `approve_confirmed`.
  A4 is `accept/01`.
- **Re-review 3 sweep:** 45/45 rejected.
- **Corpus:** 125/125 rejected as intended. 20 retired, each with a
  written reason:
  - most only define functions nothing calls;
  - 3 are whole-file replacements that break unrelated code;
  - 1 is a legitimate shape written against the pre-gate API (its gate
    form is `accept/02`).

  The harness refuses to retire any review-sweep fixture. Several
  fixtures were ported to current APIs, and new ones were added where
  the old shape no longer applied (`human_only_authorization/05,06`,
  `discovery_owned_by_report_pipeline/05,06`,
  `occupancy_is_tristate_at_sinks/05`).
- **Accept fixtures:** 20 in `accept/` + A1–A3 = 23, all accepted.
- **By mechanism** (first matching kind): gate_paths 100,
  adapters 18, E0616 14, guardrail_metadata 11,
  no_unreferenced_public_items 11, E0425 10, E0061 9, ids 8, bus 7,
  E0277 7, E0624 5, E0063 5, verdicts 5, byte_units 4, E0603 4,
  clippy::disallowed_methods 4, sinks 3, others ≤2.
- **Operators** (old eight: alias, pub_use, helper, child_module,
  macro_wrap, via_constant; plus review-4's fn_item_binding and
  doc_cfg_test): 1116/1116 variants held. Rejected seeds keep their
  seed's kind, where "a name that does not exist" is one class across
  E0425/E0432/E0433/E0412/E0531. Accepted seeds stay accepted. The two
  test functions take 9:06 together.
- **Holes the sweep found and fixed:**
  - `concat!` pieces are now joined;
  - a type's own `impl` no longer counts as using it;
  - `Type::f` now names `Type`;
  - `Path::read_dir` and friends named as paths are rejected;
  - an id constant used as a path counts like its literal;
  - a unit table outside `render.rs` counts as a second formatter;
  - `#[allow(dead_code)]` in production is rejected (the one use, a
    dead `WorktreeFacts::merged`, was removed);
  - home-path literals in adapters are rejected;
  - one tool's adapter naming another's is rejected;
  - a consumer naming `EventBus` is rejected;
  - the TUI may reach report code only through `observe_scope`,
    `load_last_report` and `merge_reports`;
  - `let go = worker::spawn; go(..)` is still a hand-off (the model's
    statement visitor had bypassed its own `let` handling, so it was not).
  - The CLI's own size parser was a second unit table and now uses
    `filter::parse_size`. Dead public items were deleted: about two dozen functions
    or types and three whole modules (`extractor`, `measurement`,
    `volume`).

## 4. Limits, stated

- The gate is the trusted base. Code inside `fs_gate` may do anything,
  so a diff there is what a reviewer must read.
- Inside `growth`, the dirs and files Parquet writers are `pub(super)`.
  Their current-plus-delta discipline rests on runtime tests.
- Orderings and data flow are runtime tests, not types: FSEvents before
  the walk, and which subtrees a replay re-walks.
- trybuild `.stderr` snapshots depend on the rustc version (local:
  1.92.0). A CI toolchain bump may need `TRYBUILD=overwrite`.
- The event-thread rule follows methods by name, so it is conservative.
  A call through a non-path callee on the event thread is rejected
  rather than followed.

## 5. Verification

- `SWAMP_TARGET_DIR=<shared target> scripts/check.sh` finished with exit 0
  in 35:02 wall on macOS. It ran fmt, `cargo test --workspace`, clippy
  `-D warnings` on all targets and on the production lib/bins, and the
  audit binary (11/11 ok). It then reran the named runtime tests, the
  cost test with `--test-threads=1` and the TUI refresh tests. The
  compile-fail runner and the mutation sweep (2 tests, 545 s) came next,
  and the grep checks last. 88 test binaries ok, 0 failed; the only
  ignored tests are the 2 pre-existing read-only real-store comparisons.
- `cargo test --workspace --locked` on its own took 21:59 (cold for the
  new crate graph).
- Commits are unsigned (`git -c commit.gpgsign=false`) and not pushed.

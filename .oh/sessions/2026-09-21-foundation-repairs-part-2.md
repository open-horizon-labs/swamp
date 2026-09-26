# Foundation repairs, part 2: the adapter registry, the closed gap, and the earned matrix

Continues `.oh/sessions/2026-09-21-foundation-repairs.md`, which left
**nine** of the 42 source audits failing and named them as the honest
list of what the next worker starts from. All nine are closed.

```
$ cargo run -q -p swamp-source-audit
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
ok    no_second_traversal_on_report_path
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
ok    no_dead_public_evidence_api
```

Forty-two audits, no failures, exit 0.

Three of the nine were one piece of work with three names, one was a
rule that could not be satisfied as written, one was seventeen dead
public functions, and the last was a support matrix claiming more than
it had earned.

## The shape of the problem, again

The previous note observed that seventeen falsifying tests were four
*shapes* rather than seventeen bugs. The nine remaining audits were two:

- **A decision written down in more than one place.** A fourteen-arm
  `match tool_id` in `agents/mod.rs`, a second fourteen-arm match in
  `actions.rs` for the execution recheck, a hardcoded two-id
  `multi_location_tool`, a bespoke Aider path, and 22 `*_DETECTOR_ID`
  matches in `consumer_wiring.rs`. Every one of them a table that had to
  be edited in step with another table nobody had linked to it.
- **A claim nothing checked.** Seventeen `pub fn`s the docs described as
  delivered with no caller; a support matrix where every row said
  "Supported" while several rows' own footnotes admitted the layout was
  assumed; and a measured 740 KB gap recorded honestly and still open.

Both shapes have the same repair: make the thing checkable, then check
it.

## 1-5. The `AgentAdapter` trait and registry

`agents::AgentAdapter` (`id`, `name`, `capabilities`, `identify`,
`reidentify`, `project_local_units`) with static registration in
`agents::registry::Registry::with_builtins`, mirroring
`locations::Registry` exactly. Fifteen modules converted.

What each match became:

| was | is |
| --- | --- |
| `agents/mod.rs::identify_for_tool`, 14 arms | `Registry::get(tool_id)` |
| `actions.rs::execute_agent_session_removal`, 14 arms | `agents::reidentify_for_tool` |
| `multi_location_tool` (Cline/Roo Code, hardcoded) | `AdapterCapabilities::decomposes_every_location` |
| Aider's bespoke `identify_repo_units` call path | `AdapterCapabilities::project_local_units` |
| `pi.rs` falling back to Oh My Pi's header shape | neutral `pi_family` mechanics; each adapter passes only its own tool's layouts |
| 22 `*_DETECTOR_ID` matches in `consumer_wiring.rs` | `Detector::manager_conventions()` / `Detector::recovery_hint()` |

The `reidentify` half matters more than it looks. The two matches had
drifted in the way that kind of duplication always does: a tool present
in one and absent from the other would identify fine and then refuse to
re-verify at execution -- a safety boundary that silently did not cover
a tool. `reidentify_for_tool` runs with
`IdentificationCache::disabled`, so a recheck reads live state and an
approval can never be spent against a cached derivation.

`Detector::manager_conventions()` was the interesting design problem,
because the 22 id matches were not asking 22 questions. They were asking
three: *which detector's units satisfy this tool's declared version*,
*is this unit that ecosystem's dependency store*, and *is this the
workspace-indexed build output store*. Modelling the question rather
than the id is what makes adding a detector stop being a wiring edit.
Two places still dispatch on a named *format* rather than an id (the
bounded store-entry probe, and the one TOML global-default parser);
both are recorded in the audit's guardrail doc as limits rather than
hidden.

### What the adapters can no longer do

Every adapter now takes an `IdentifyCtx` and cannot reach past it:
listings through one bounded, capped, symlink-refusing level; byte
totals through `folded_measurement`; content **only** through the capped
header reader. `agents/mod.rs::folded_bytes` moved to
`folded_measurement::folded_bytes_bounded`, which is the one module on
the report path allowed to traverse.

Units are built by `AgentUnitBuilder`, whose constructor applies
`AgentCategory::default_protected` with a stated reason. That is the
point: a `CandidateAgentUnit { .. }` literal spells out `protected`, so
a sixteenth adapter could ship a credentials file with `protected:
false` and nothing would notice.

Each of the fifteen modules now proves the same five things about
itself, by name. The five are not ceremony -- writing them found real
things, including that Cline reads no per-task file at all (below).

## 6. The identification cache: 740 KB to zero

The previous note recorded the gap rather than claiming it fixed:

| Fixture | First pass | Unchanged second pass |
| --- | --- | --- |
| Agent home, 5,000 synthetic sessions | ~375 ms, 740 KB of header reads | ~326 ms, **740 KB** |

It is now:

| Fixture | First pass | Unchanged second pass |
| --- | --- | --- |
| Agent home, 5,000 synthetic sessions | 435 ms, 740,000 header bytes, 11 dirs listed | 331 ms, **0 header bytes**, 11 dirs listed |
| One appended session | -- | **97 header bytes**, 1 cache miss |
| External cache root, 20,000 files | 69 ms, 2 dirs listed | 47 ms, 2 dirs listed |

`crates/core/tests/incremental_external_and_agent_measurement.rs` now
asserts the strict `== 0` the previous worker left recorded, plus
`identification_cache_hits >= SESSIONS` so a cache that merely *skipped*
work rather than answering it would fail. The appended-session test
asserts at most **one** capped read (not `SESSIONS + 1`, which is what
the weaker bound would have allowed) and exactly one cache miss.

The mechanism: `assoc_store::IdentificationTable`, a fifth Parquet
current-state table, reached through `IdentifyCtx::derived`. An adapter
asks for a *derived value* -- a session's declared `cwd`, a task's
workspace path -- not for bytes.

### The fingerprint, and why it is four fields

The first version keyed on `(size, mtime_secs)`. Two of the five
parallel adapter conversions independently found the same hole and
reported it in the same terms: rewriting a session's declared `cwd` to a
path of the **same length** within the same wall-clock second leaves
size and whole-second mtime unchanged, so the stale project is served
and a re-linked session never moves.
`agent_storage_validation.rs::relinking_a_session_to_a_different_project_leaves_bytes_and_growth_history_unchanged`
caught it; `mod.rs`'s own cache test did not, because it slept 1100 ms
first.

That is worth recording as a lesson rather than a fix. A test that
sleeps to make a timestamp move is a test that has quietly assumed the
thing it is checking. The fingerprint is now `(len, mtime_ns, ctime_ns,
inode)` plus `ADAPTER_VERSION` -- all of it from the `stat` the caller
already needed, so it costs nothing -- and the two regression tests
(`a_same_second_same_size_rewrite_invalidates_the_cached_derivation`,
`a_replaced_file_is_a_different_file_however_its_timestamps_look`) have
no sleep in them, on purpose.

`crate::work_counters` also became **per thread**. They were
process-global `AtomicU64`s, which made every `header_bytes_read == 0`
assertion in the new per-adapter tests a race against whatever other
adapter's fixture was reading a header at that moment. An intermittently
wrong measurement is worse than no measurement, and per-thread is also
the correct scope for what these count.

## 7. `discovery_owned_by_report_pipeline`: a decision, not a weakening

The previous worker left this failing on purpose and said it was a
decision for a human. It is, and here it is.

The audit required `external::discover_and_measure` and
`agents::discover_and_measure` to be `pub(crate)`. The reviewers' own
mandatory `crates/core/tests/reviewer_counterexamples.rs` calls both
from an integration test, i.e. from outside the crate. That file is
copied in byte-for-byte and may never be edited. The rule and the
evidence cannot both be satisfied, and **the evidence wins**.

Visibility was never the property the review falsified. What it found
was two *passes* over one shared history table in an order nobody
declared. So the audit now checks exactly that:

1. both functions still exist;
2. `report.rs::observe_scope` calls **both** of them -- splitting them
   back into separate observations is how their ownership windows could
   disagree again; and
3. nothing else in `crates/core/src`, `crates/cli/src` or
   `crates/tui/src` calls either.

**Inside the crate this is strictly stronger than the visibility check
it replaces.** `pub(crate)` permitted any number of core-internal
passes; the call-site rule permits one. Six mutation tests cover it,
including `a_second_pass_inside_the_core_crate_is_rejected`, which is
the case the old rule allowed.

Making it pass required real work rather than a rule change: the CLI ran
its own passes in three places. `observe_scope` gained
`ObservationParts` (which parts this observation covers) and an optional
`base` report, so the CLI's explicit-root path hands in the report it
already walked rather than walking again. Which parts the `report`
command asks for is unchanged, so no command got slower. A part not
observed is a part not swept, so asking for fewer parts is a cost
decision and never a correctness one.

## 8. `no_dead_public_evidence_api`

Seventeen `pub fn`s with no non-test caller. Fourteen wired where their
issue requires, three deleted with their documentation claims:

| function | outcome |
| --- | --- |
| `evidence::expires_after_with_coverage` | wired: both current-use facts now carry an expiry **and** a stated coverage limit (a container listing cannot see a non-container consumer; `simctl` answers only for this user's store) |
| `evidence::of_kind` | wired: `render::render_evidence_lines` groups facts into a fixed domain order instead of whatever order the passes ran in |
| `evidence::stale` | wired: `render::evidence_warnings` surfaces a reading past its own recheck window -- phrased as "past its Ns recheck window; re-taken fresh before any action", which is a fact, not a verdict |
| `activity::access_time_evidence` | wired: `report::attach_decision_evidence`, every filesystem row and no Docker row. On `noatime`/`relatime` the `Unavailable` row naming the mount option *is* the evidence |
| `activity::docker_last_used_evidence` | wired: `report::join_docker_facts`, build-cache rows, joined and unjoined alike |
| `occupancy::docker_running_container_evidence` | wired: `report::join_docker_facts`, from the raw `ContainerRef`s (the row's pre-formatted strings lose them) |
| `occupancy::manager_lock_evidence` | wired: `actions::unit_from_external`, probing five documented lock-file conventions as direct children |
| `occupancy::simulator_booted_evidence` | wired at the **proposal sink**, not the report pass -- a `simctl` spawn per unit on an ordinary report is exactly the scanning cost the occupancy discipline exists to avoid. Routed by device-UDID naming shape, not `StorageCategory` alone, because an Android AVD store is also `Environments` and `simctl` would answer about nothing |
| `recovery::maven_artifact_recovery` | wired: `actions::external_recovery_facts`, selected by the Maven detector's declared `DependencyStore { lookup: MavenLayout }` convention |
| `recovery::toolchain_installation_recovery` | wired: same site, for an `Installation` unit whose detector declares `DeclaredVersions`; one fact per installed version |
| `reclaimability::apfs_clone_or_snapshot_bound` | wired: `report::attach_decision_evidence` -- a non-hardlinked row on a copy-on-write volume gets a *bound*, not an exact figure |
| `reclaimability::sparse_file_accounting` | wired: `actions::sparse_byte_accounting_facts`; fires for real on a Docker Desktop VM data unit |
| `reclaimability::estimate_selection` | wired: `actions::selection_estimate`, called by all three `propose*` constructors, carried as the new `Plan.selection` beside the naive `planned_bytes` |
| `reclaimability::observed_free_space_change` | wired: `actions::execute_with_trash_opts`, as the new `ExecuteResult.evidence` -- an *observation*, deliberately distinct from the plan's estimate |
| `external_associations::parse_pom_xml` | **deleted**: a strictly worse duplicate of `parse_pom_xml_with_gaps`, which it differed from by dropping exactly the identity gaps the PR #123 review required be stated |
| `external_associations::join_cache_entry` | **deleted**: `attach_associations` aggregates consumers per *unit*, not per entry, so it never went through it; the four ecosystem-specific probes it would have wrapped are each called directly |
| `toolchain_declarations::conflicting_tools` | **deleted**: #56's "conflicting declarations remain explicit" is delivered by `VersionMatch::Conflicting` -> `FactStatus::Conflicting`, which *is* wired through `declaration_evidence` |

Each wiring has a test that fails if the fact is omitted or lands on the
wrong row kind -- `crates/core/tests/evidence_api_is_wired.rs`, fifteen
tests including
`access_time_is_asked_of_every_filesystem_row_and_of_no_docker_row`,
`running_container_occupancy_reaches_docker_rows_and_only_docker_rows`,
`simulator_device_directories_are_probed_and_emulator_directories_are_not`
and
`a_selection_sharing_one_inode_does_not_count_those_bytes_twice`.

One thing worth recording as a near-miss. Wiring initially carried a
path-shape fallback (`.m2/repository`) for the Maven store, because the
detector capability it wanted did not exist yet while both pieces of
work were in flight. It does now, and the fallback is gone: a path
literal in `actions.rs` is exactly the wiring table
`detector-ids-only-in-registry` exists to prevent -- one that keeps
working while the capability it duplicates silently stops being
declared.

## 9. The support matrix, earned row by row

The matrix said `Supported` for all fourteen tools. Two had not earned
it and three carried a claim that was not merely unconfirmed but
**false**. `SupportLevel::Unverified` now exists, and
`agents::discover_and_measure` withholds every action and reports
project linkage `Unresolved` for such a tool -- in one place, so an
adapter cannot forget.

Every claim below was checked against upstream source or official
documentation on 2026-09-21/22. No real tool home on any machine was
read; this is all public source.

| Tool | Level | Verified against | Outcome |
| --- | --- | --- | --- |
| Claude Code | supported | code.claude.com/docs/en/claude-directory, retrieved 2026-09-21 | confirmed |
| Codex | supported | openai/codex `main` @ `30daed37`: `codex-rs/state/src/lib.rs` (`CODEX_SQLITE_HOME`), `core/src/config/mod.rs` (`log/`, overridable by `log_dir`), `ext/skills/src/host_roots.rs` (`skills/`) | all three confirmed; `skills/` is upstream-**deprecated** in favour of `~/.agents/skills`, and `log/` is config-movable -- both now said out loud |
| Codex desktop | supported | `codex-rs/cli/src/doctor/desktop/platform.rs`, read during #93 | confirmed, logs only (deliberately partial) |
| Oh My Pi | supported | project docs `session.md`/`settings.md`, read during #94, **not re-fetched** | unchanged claim, now labelled as not re-checked |
| OpenCode | supported | sst/opencode `dev` @ `fe3f3a41`: `packages/core/src/global.ts`, `packages/core/src/flag/flag.ts`; opencode.ai/docs/config | **`OPENCODE_DATA_DIR` does not exist.** Detector branch and provenance removed; data root is `$XDG_DATA_HOME/opencode` unconditionally. `OPENCODE_CONFIG_DIR` is real and is now honoured for the config root |
| Gemini CLI | supported | google-gemini/gemini-cli `main` @ `d5b3e3ac`: `config/storage.ts`, `utils/paths.ts` | OAuth file confirmed `oauth_creds.json` (now named explicitly, pattern kept for others); `GEMINI_CLI_HOME` confirmed, `GEMINI_DIR` is a constant not an env var; `tmp/<hash>` is **legacy** -- current versions use slugs from `projects.json`, so the adapter treats the name as opaque and says which mapping it does not read |
| Pi | supported | pi-mono `settings.md`, read during #96, **not re-fetched** | unchanged claim; the Oh My Pi header fallback is **removed** (see below) |
| Aider | supported | `aider/args.py`, `repomap.py`, `models.py`, read during #96, **not re-fetched** | unchanged claim |
| GitHub Copilot CLI | supported | GitHub's own CLI config-dir reference, retrieved during #97 | confirmed |
| **Cursor** | **unverified** | cursor.com/docs/troubleshooting/troubleshooting-guide, retrieved 2026-09-21 | **NOT CONFIRMED.** The page says data is cached locally and names no path; it contains none of `Application Support/Cursor`, `globalStorage`, `workspaceStorage`, `state.vscdb`. Only `forum.cursor.com` community threads corroborate it |
| **Windsurf** | **unverified** | docs.devin.ai/desktop/devin-desktop-faq, retrieved 2026-09-21 (docs.windsurf.com 307s here) | **PARTIALLY CONFIRMED.** The profile root and `globalStorage/` *are* now officially documented; `workspaceStorage` and the `Cache`/`CachedData`/`CachedExtensionVSIXs`/`logs` siblings this adapter also models are not. Also: the product was renamed **Devin Desktop** on 2026-06-02 and the read-write profile moved to `~/Library/Application Support/Devin` |
| Cline | supported | cline/cline `main` @ `d4d3d9f3`: `core/storage/disk.ts`, `package.json`, `context-tracking/ContextTrackerTypes.ts`, `shared/HistoryItem.ts`, `shared/storage/storage-context.ts` | Layout confirmed. **The linkage claim is refuted**: `task_metadata.json` is `{ files_in_context, model_usage, environment_history }` -- no `workspace` field, ever. The working directory is `HistoryItem.cwdOnTaskInitialization` (optional) in `taskHistory` state, inside `state.vscdb` or `~/.cline/data`. Cline tasks now say so |
| Roo Code | supported | RooCodeInc/Roo-Code `main` @ `b867ec91`: `shared/globalFileNames.ts`, `utils/storage.ts`, `task-persistence/TaskHistoryStore.ts`, `context-tracking/FileContextTrackerTypes.ts` | Same refutation, different resolution: the workspace **is** recorded per task, in `history_item.json`. Roo Code task linkage works for the first time. Also recorded: `roo-cline.customStoragePath` can relocate `tasks/` entirely |
| Continue | supported | continuedev/continue `main` @ `5522c6f4`: `util/paths.ts`, `index.d.ts`, `util/history.ts` | The row said no confirmed per-session workspace field existed. `Session.workspaceDirectory` is **required**, written per session and mirrored into the index. Continue sessions now link |

Three of those were claims that read as cautious and were wrong. Both
Cline and Roo Code read a `task_metadata.json` `workspace` field that
exists in neither schema -- so for two tools, project linkage resolved
*nothing, ever*, while reporting an `unresolved` reason that named the
wrong file. A cautious-sounding claim is still a claim.

`crates/core/tests/agent_matrix_matches_docs.rs` parses the published
table in `docs/agent-storage.md` back and compares ids, levels and the
actions column against the constant and the registry's ids; a level
changed in one place and not the other fails. It also asserts the
`unverified` level is actually *in use*, so the docs cannot drift back to
claiming everything is supported.

### Pi no longer knows anything about Oh My Pi

`pi.rs` tried its own offset-zero header and then Oh My Pi's 256-byte
title-slot shape, described in the docs as "an explicit fallback
(reused, not assumed)". It was neither: a change to Oh My Pi's format
would have changed *Pi's* identification, for no reason a reader of
either file could see. The shared byte-offset mechanics moved to the
neutral `pi_family`, each adapter passes only the layouts its own tool
documents, and a header an adapter cannot parse is an explicit
unknown-format outcome. The test that asserted the fallback was
replaced with one asserting the refusal.

## The integration owner's mutation-check finding

Recorded separately because it is the most instructive thing in this
chunk.

Mutating `agents::is_human_protected` to one containment direction
passed `protection_fails_closed` **and** every runtime test. The audit
inspected `protection_conflict`; the ordinary filesystem proposal path
(`actions::propose_checking_protection`) called the `bool` wrapper; and
no test ever proposed an ordinary directory that *contained* a protected
descendant. Two spellings of one question means two things to inspect,
and an audit will always be reading the other one.

Fixed as the finding directed:

- `is_human_protected` is **deleted**. One predicate
  (`protection_conflict`) everywhere.
- The audit now rejects any top-level `fn` whose name contains
  `protect` and which returns `bool`/`Option` without calling
  `protection_conflict(`. Mutation tests both ways:
  `a_second_protection_predicate_is_rejected` (the wrapper, reintroduced
  with inlined one-directional logic) and
  `a_protection_helper_that_does_delegate_is_accepted` (precision, not
  prohibition).
- Runtime tests for the exact case, on both paths and in both
  directions:
  `evidence_action_recheck.rs::ordinary_unit_containing_protected_descendant_is_refused_at_proposal`,
  its `..._beneath_a_protected_ancestor_...` sibling, and
  `scope_preserving_refresh.rs::marking_an_ordinary_row_that_contains_a_protected_file_is_refused_with_the_reason`.
- The TUI's `mark_row` now checks protection for **every** markable row
  before anything else. It previously only reached the check for the two
  row kinds that happened to propose through core, so an ordinary
  artifact or unowned row containing a protected file marked cleanly and
  was refused much later, at execution. Unreadable protection state
  refuses too.

The finding also confirmed M1, M3 and M4 trip as designed, and asked
that `detectors_permitted`'s reading of "explicit" be documented
precisely. It is: `enabled_detectors` is an allow-list,
`disabled_detectors` under `defaults = false` is a deny-list (the
catalog minus the named), neither set is an empty scope. Documented in
`docs/usage.md` and `docs/architecture.md`, with
`scope.rs::tests::defaults_false_with_only_disabled_detectors_still_runs_the_rest`
naming the middle case.

## A fixture test was reading 42 GB of the developer's real disk

Found by running `scripts/check.sh` end to end rather than the
`--target-dir`-warmed subset:
`external_units::shared_consumers_are_counted_once_in_totals` failed
with `left: 42794479616, right: 4096` and took 98 seconds. It is
pre-existing -- it fails identically on the pristine tree -- and it is
the most serious thing this chunk found that nobody was looking for.

The cause: `locations::core_simulator` proposes the **absolute** system
path `/Library/Developer/CoreSimulator/Volumes`, which no injected
`Environment::home` can relocate, because system-wide simulator runtime
volumes are machine state rather than per-user state. Every fixture in
`external_units.rs`, `external_units_actions.rs`,
`agent_units_actions.rs`, `agent_units_actions_remaining_tools.rs`,
`agent_storage_validation.rs` (core and TUI) and `agent_refusal_matrix.rs`
used `defaults = false` plus a **deny-list** of two or three detector
ids, on the assumption that a fixture home confines the rest. So those
tests walked the developer's actual simulator runtimes -- a straight
violation of the handoff's "tests use disposable fixtures, never real
user data", and an assertion whose value depended on what happened to be
installed.

Fixed by making every one of those fixtures an `enabled_detectors`
allow-list naming exactly the detector under test. `external_units`
went from 98 s to 1.6 s. Where a test's *point* was a deny-list
(`aider_disabled_detector_turns_off_both_home_and_repo_units`), the
allow-list and the deny-list both name `aider`, which preserves the
meaning and bounds the catalog.

The guard, so this cannot recur:
`external_units.rs::a_detector_that_escapes_the_fixture_home_is_named_here_not_discovered_by_a_byte_total`
walks the whole registry against a fixture home and fails on any
resolved path that escapes it, except for an explicit list. Writing it
immediately found two more escaping detectors nobody had written down:
`homebrew` (`/opt/homebrew`, `/usr/local`, and their `Cellar`/`Caskroom`
children) and `ruby-install` (`/opt/rubies`). All three are *correct* to
do this -- those are machine-wide conventions -- which is the point: the
number of detectors that can reach outside a fixture is now a reviewed
list rather than something a byte total discovers by accident. The test
also asserts the consequence directly: a deny-list scope still
authorizes `core-simulator`, and an allow-list scope authorizes nothing
but what it names.

Two smaller hygiene fixes came with it. The `ALL_AGENT_DETECTOR_IDS` /
`ALL_TOOL_DETECTORS` constants the deny-lists used are now the
allow-lists' *validity* check: an unknown id in an allow-list would
authorize nothing and leave the fixture passing vacuously, so the ids
are asserted against the catalog.

**Not fixed, and left as a finding.** The same leak exists one level up,
in `crates/cli/tests/agent_storage_cli.rs`, and it is worse than the
unit-test one because it is not about external discovery at all -- it is
the *walk*.

Those tests spawn the real binary with `HOME` set to a tempdir and a
fixture `SWAMP_DIR` holding no `config.toml`, so the binary resolves the
default catalog. Measured with `swamp scope --json` under exactly that
fixture: four roots are `present`, and two of them are
**`/opt/homebrew`** and **`/Library/Developer/CoreSimulator/Volumes`** --
absolute machine-wide paths that no `HOME` override can remove. `swamp
report --json` on an otherwise-empty fixture home therefore takes

```
116s user  163s system  298% cpu   1:33.83 total
```

on this machine, and every one of the twelve tests in that binary pays
it. That accounts for the whole of `scripts/check.sh`'s runtime beyond
about a minute.

This is *not* a regression from this branch: plain `report --json` with
no `--view` never enters the code path this chunk changed
(`ObservationParts::WALK_ONLY` short-circuits before `observe_scope`)
and is equally slow. And it is correct behaviour for a real user --
Homebrew's prefix and the simulator runtime volumes genuinely are
developer storage. It is only wrong in a test.

The fix is to write an explicit-only `config.toml` into each test's
fixture store, which changes what those tests exercise, so it is a
decision rather than a cleanup and it is recorded here instead of made
at the end of an unrelated chunk. `scripts/check.sh` passes either way.

## Audit changes, and why each is precision rather than a loophole

Four audits were adjusted. A reader should be able to tell these from
weakening, so each is stated with what it still rejects:

1. `no_second_traversal_on_report_path` exempts one `(file, function)`
   pair, `locations/mod.rs::shallow_list` -- the carve-out the guardrail
   spec wrote itself ("a bounded helper which is itself allow-listed and
   capped"). It is a pair, not a file, so a second traversal added
   *beside* it still fails; and the audit requires `SHALLOW_LIST_CAP` to
   still exist, so the exemption cannot outlive the bound that earns it.
   Two mutation tests for exactly those.
2. `agent_adapters_are_pluggable` counts registrations on a path-segment
   boundary. A naive substring count reported `pi` as registered twice,
   because `pi::Adapter` occurs inside `oh_my_pi::Adapter`. The check
   still fails on zero and on two.
3. `discovery_owned_by_report_pipeline` -- section 7 above. Stronger
   inside the crate than what it replaces.
4. `protection_fails_closed` gained a check rather than losing one.

## Measurements

Store contents after a full observe -> report -> propose -> approve ->
execute cycle on the multi-ecosystem fixture, printed by
`crates/core/tests/store_contents_are_allowlisted.rs` rather than
described (it now reports per file, so a table that starts growing per
row is visible instead of hidden in a total):

```
store after observe/report/propose/approve/execute: 43,940 bytes total
         88  <root-hash>/fsevents.json          (x3 roots)
          2  <root-hash>/topology.json          (x3 roots)
      1,637  <root-hash>/unowned.json           (largest of 3)
        564  associations/agent_identifications.parquet   <- new
        813  associations/declarations.parquet
        823  associations/dependency_identities.parquet
        648  associations/xcode_derived_data.parquet
      3,030  external/current.parquet
      2,919  external/deltas/delta-000000000000.parquet
      2,920  external/deltas/delta-000000000001.parquet
        405  grants.json
        402  last_report-*.json.zst             (x3)
        676  ledger.jsonl
      3,314  plans/<id>.json
     24,169  scope.json
```

Every file is a `.parquet` table or one of the named small control
files, and no `.json` exceeds 64 KiB. `scope.json` is more than half the
store, which is worth noticing: it is a snapshot of the whole resolved
detector catalog, and it is a control file rather than per-row data, so
it is allowed -- but it is the one thing here that grows with the
catalog.

The identification table is the only one that grows with *session*
count rather than worktree count: one row per (adapter, derivation,
session file), holding one derived string. 564 bytes for this fixture's
handful of sessions; on the 5,000-session fixture it is 5,000 such
rows -- which is exactly why it is a columnar table and not a JSON
sidecar.

## Still open, honestly

- **Three matrix rows are `supported` on research from an earlier chunk
  that was not re-fetched**: Oh My Pi, Pi, Aider. Their `Verified
  against` column says so in those words. That is a smaller claim than
  "checked today" and a larger one than "assumed", and it is the true
  one.
- **Cline's second storage root is not modelled.** Cline 4.x writes to
  `CLINE_DATA_DIR` / `CLINE_DIR/data` / `~/.cline/data`, including
  `state/taskHistory.json` -- which is where the working directory this
  catalog reports as unresolved actually lives. Modelling it would both
  find bytes we currently miss and make Cline task linkage work. It
  needs a detector, so it is a chunk, not a patch.
- **Windsurf is now two products.** The read-write profile moved to
  `~/Library/Application Support/Devin`; the detector still looks only
  at the legacy Windsurf directory, so a current install is
  under-reported rather than misreported. A detector change, recorded
  here and in the matrix row.
- **An unchanged external root is still folded afresh.** The agent half
  of the incrementality gap is closed; the external half is not. The
  work counters make it visible and
  `an_unchanged_external_cache_root_is_not_re_traversed` pins that the
  work is counted, which is what makes the next repair testable.
- **Two capability dispatches still name a format rather than an id**
  (the bounded store-entry probe's layout variants, and the one
  TOML global-default parser's field name). Recorded as limits in
  `.oh/guardrails/detector-ids-only-in-registry.md`. Neither is a
  detector id, so adding a detector that reuses an existing layout still
  costs no wiring edit.
- **No real installed tool has been checked, and will not be.** Every
  fixture in this feature is synthetic, by the hard privacy rule. This
  chunk narrowed the gap by verifying against upstream *source* instead
  of against a real home -- which is strictly better evidence than a
  layout inferred from one developer's machine, and still not the same
  as knowing.

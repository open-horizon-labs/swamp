---
id: no-second-traversal-on-report-path
severity: hard
statement: "The ordinary report path traverses directories only in the folded walk. External units take their bytes from folded rows; agent adapters take directory structure from those rows or from the capped locations::shallow_list; nothing on the report path re-walks a tree the walk already measured."
outcome: disk-growth-by-project
audit: gate_paths_only_inside_gates, adapters_do_not_reach_gates
compile_fail:
  - bus_stage_is_minted_by_the_bus
  - bus_stage_has_no_production_test_constructor
  - discovery_pass_is_minted_by_observe_scope
runtime_tests:
  - crates/core/tests/incremental_external_and_agent_measurement.rs
  - crates/core/tests/agent_container_seams.rs
  - crates/core/tests/unit_root_event_cursors.rs
---

## Rationale

P1 in the 2026-09-21 review: `external.rs` recursively re-sized every
external root on *every* call, `agents/mod.rs` implemented a second
recursive traversal, and each adapter repeated it and re-read session
headers every time. The validation session explicitly deferred
incrementality and argued bounded header reads were enough. They are
not: the handoff's requirement is that unchanged work scales with roots
and changed containers, not with all files, and a 300-session warm
fixture does not test an unchanged large tool home.

## Detection

Mechanism: type, gate audit, clippy, runtime test.

**Type.** The walk takes a `bus::Stage` (minted only by `EventBus::run`) and discovery a `report::DiscoveryPass` (minted only by `observe_scope`), so no other entry point can run either.

**Gate audit.** A directory listing is `fs_gate::read_dir`, which only the walker modules may name; `walkdir`/`jwalk` are gate paths.

**Clippy.** `Path::read_dir`/`std::fs::read_dir` are disallowed methods outside the gate.

Retired 2026-09-22: the `no_second_traversal_on_report_path` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

Compile-fail cases (`crates/core/tests/compile_fail/`, run by `crates/source-audit/tests/compile_fail.rs` against the production API): `bus_stage_is_minted_by_the_bus`, `bus_stage_has_no_production_test_constructor`, `discovery_pass_is_minted_by_observe_scope`.

## The gate: trusted event coverage, not directory stamps (2026-09-22)

Both unit families originally decided "unchanged" from the recorded
directories' own `mtime`/`ctime`. A directory stamp moves when an entry
is created, deleted, renamed or replaced, and **not** when a file inside
it is appended to or rewritten in place.

For an external cache that is a corner case. For agent storage it is the
normal case: a tool **appends to an open session transcript in place**,
which moves the file's own size and mtime and not its parent's. A
stamp-keyed cache therefore reported a growing session at its old size
until the next create/delete/rename in its container. "What grew" is the
question this tool exists to answer, so the integration decision of
2026-09-22 removed stamp-only reuse as a sufficient condition for both
families, and replaced it with `fs_events::EventCoverage`.

A stored measurement or container may be replayed only when all three
hold:

1. some root this pass replayed successfully is the path or an ancestor
   of it — FSEvents was watching;
2. that replay reports no event at the path or under it — including
   writes into existing files, which is the part a stamp could not see;
3. the stored rows are no older than the observation the window opens
   from. A pass that skipped a unit family (`report::ObservationParts`)
   leaves a gap between when the rows were written and when the window
   opened, and no window can see into its own past.

With no window — a full walk, any `fs_events::RefreshRefusal`, an
overflow, a first observation, a store-less caller — there is no reuse.
The unit is re-measured or re-identified, which is slower and always
correct; the per-file identification cache still keeps header reads at
zero for files that did not move. Directory stamps are consulted by
neither family any more: they are strictly weaker evidence than the
window and cost one `stat` per recorded directory per pass, so keeping
them as a second opinion would buy nothing and charge for it. A replayed
container therefore costs **no syscall at all**.

**What this trades away.** The window comes from the walk, one per scan
root that went incremental. A tool home or external cache root outside
every scan root has no window and is re-measured on every pass, which is
the common shape of a default install (a scope of project directories
does not contain `~/.claude` or `~/.cargo`). Giving each unit root its
own FSEvents cursor is the obvious completion and is **not** implemented;
it is written up, with its ordering hazard, in
`.oh/sessions/2026-09-22-event-gated-reuse.md`. Until it lands, the
container/folded-row reuse is a real saving only where the scope already
covers the tool home, and the cost of that is measured below.

**What it never was.** None of this is an authorization boundary. Every
execution sink re-derives from the live filesystem with both caches
disabled (`agents::reidentify_for_tool`), and
`relinking_a_session_to_a_different_project_leaves_bytes_and_growth_history_unchanged`
asserts it directly — as well as asserting, since the gate landed, that
the *ordinary* pass sees a relinked session too, under event coverage
and under a full walk alike.

## Runtime tests that complete it

- `crates/core/tests/incremental_external_and_agent_measurement.rs` —
  work counters (`crate::work_counters`) over a synthetic 5,000-session
  agent home and a 20k-file external cache root.
  - First observation reads headers; a vouched-for second observation
    reads **zero** header bytes, replays all five containers, and
    re-identifies none.
  - `replaying_containers_costs_nothing_per_container` pins the
    per-container cost at exactly zero by holding the tool home's own
    structure constant and varying only the container count: nine
    containers cost the same listings and header bytes as one, and
    exactly eight more `stat`s — the eight entries `projects/` yields
    when the home is scanned, which is how the pass learns those
    containers exist. Twelve times as many sessions per container cost
    *nothing* extra.
  - `an_appended_session_is_seen_under_event_coverage_and_under_a_full_walk`
    is the decision itself: a 1 KiB append to an existing transcript is
    reported in the same pass under a window that names it, and in the
    same pass under no window at all. This test replaces
    `a_session_rewritten_in_place_is_not_seen_until_its_container_moves`,
    which asserted the old lag.
  - `appending_one_session_re_identifies_exactly_one_container` — one
    appended session re-identifies exactly one container and replays the
    other four, with exactly one derivation-cache miss.
  - `an_unchanged_external_cache_root_is_not_re_traversed` — a
    vouched-for external root costs **zero** listings and **zero**
    stats over 20,000 files, and `a_changed_external_cache_root_is_measured_again`
    is the other half.
- `crates/core/tests/agent_container_seams.rs` — the same two
  properties for every adapter whose session storage is a directory
  tree: Codex (`sessions/<yyyy>/<mm>/<dd>/`), OpenCode
  (`storage/session/<project-id>/`) and Pi (`sessions/<dir>/`). Plus
  `a_codex_day_container_does_not_depend_on_its_siblings`, which pins
  the property that made the conversion possible: each container owns
  its own entry budget, so its stored rows mean the same thing as a live
  identification of the same directory. A budget shared across
  containers — which Codex's session walk used to carry — did not.
  Oh My Pi joined the seam on 2026-09-22
  (`oh_my_pi_session_directories_are_containers`); the home-wide
  shared-blob reference count that kept it off is now a per-container
  partial stored with the rows
  (`a_partially_replayed_oh_my_pi_home_sums_the_same_blob_counts`,
  `a_replayed_container_without_its_partial_makes_the_count_unknown`).
- `crates/core/tests/unit_root_event_cursors.rs` — the per-unit-root
  cursors (2026-09-22, stack/14).
  `an_unchanged_pass_over_out_of_scope_detector_roots_costs_nothing`
  measures a 5,000-session home and a 20,000-file cache as detector
  roots beside the project root: zero header bytes, every container
  replayed, fewer than 200 stats over 25,000 unit files.
  `a_walk_only_pass_between_full_passes_keeps_the_unit_window_aligned`
  is the adversarial one, with its own control: the walk's window alone
  reuses nothing (it opened after the rows were written), the unit
  cursor's window reuses all three containers.
  `a_detector_home_is_a_present_scan_root` records the premise
  correction — a detector home *is* walked, so the cursors buy
  independence from the walk's anchor, not reach.
- `crates/core/tests/reviewer_cost_measurement_stack2.rs` — two
  unchanged full observations over a multi-ecosystem fixture, measured
  with an instrumented `walk.rs` rather than an instrument blind to it.
  Zero header bytes and zero subprocess spawns hold. Its `dirs_listed
  == 0` / `files_statted == 0` assertions **do not** hold, because the
  two passes are back to back and `RefreshRefusal::TooSoon` refuses a
  replay inside the FSEvents log-lag floor. Measured 2026-09-22:

  | unchanged second pass | `dirs_listed` | `files_statted` | header bytes | spawns |
  |---|---|---|---|---|
  | stack/12 (directory stamps) | 40 | 5,605 | 0 | 0 |
  | stack/13 (event gate) | 71 | 36,281 | 0 | 0 |
  | stack/14 (unit-root cursors) | 71 | 36,281 | 0 | 0 |

  The residual is unchanged from stack/13 and is entirely the `TooSoon`
  floor: the fixture observes twice about a second apart, and the floor
  is three seconds. It is not a claim that the unit work is still being
  redone in general. On the **same fixture**, with more than the floor
  between passes:

  | pass | stack/13: listings / stats | stack/14: listings / stats |
  |---|---|---|
  | 1 (cold) | 71 / 36,281 | 71 / 36,281 |
  | 2 | 71 / 36,281 | 40 / 5,561 |
  | 3 | 6 / 8 | 6 / 8 |
  | 4 | 31 / 10,724 | **6 / 8** |

  Zero header bytes and zero spawns from pass 2 onwards on both.
  stack/13's pass 4 is the alternating-pass behaviour the cursors
  remove: the walk's anchor and the unit rows drifted apart, so reuse
  held on some passes and not others. stack/14 is flat from pass 3.

  The reviewer assertion is therefore still failing, with the numbers
  above, and the attribution is now a named floor rather than "no window
  reaches these units". Removing it would mean either lowering the
  replay-lag floor (which trades a correctness property for a benchmark)
  or changing the fixture to observe more than three seconds apart
  (which is the reviewer's test to change, not this chunk's). Left
  failing for re-review 3 to adjudicate.

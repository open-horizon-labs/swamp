# 2026-09-22 — what "unchanged" is allowed to mean (stack/13)

Branch `stack/13-event-gated-reuse`, based on
`stack/12-agent-container-reuse-and-matrix` (`faa8737`). Input:
`CHUNK_R5.md` — the integration owner's ruling on the two things the
previous chunk's note asked someone else to decide, plus three
consequences of it.

The previous chunk built container-level reuse keyed on directory
stamps, measured it, and then said plainly that the key could not see an
append to an open session transcript — the normal way agent storage
grows — and asked for a decision. The decision came back: **that lag is
not acceptable for a growth tool.** This chunk implements it.

---

# 1. The gate

`fs_events::EventCoverage` is the whole of the new rule. A stored
measurement (external `folded.parquet`) or a stored container
(`agent_containers.parquet`) may be replayed only when **all three**
hold:

1. some root this pass replayed successfully is the path or an ancestor
   of it;
2. that window reports no event at the path or under it;
3. the stored rows are **no older than the observation the window opens
   from**.

Condition 3 is not in the brief and is not optional. Without it the
scheme is wrong in two ways that a test found immediately:

- A pass that skips a unit family (`report::ObservationParts::WALK_ONLY`,
  which the TUI and the cost tests both use) advances the event cursor
  without refreshing that family's rows. The next pass's window then
  starts *after* changes that were never measured, and vouches for rows
  that predate them. `EventCoverage::unchanged_since` takes the rows'
  own `observed_at` and refuses.
- Symmetrically, a *replayed* unit must have its `observed_at`
  advanced, or reuse would only ever work on alternate passes (the
  window always starts where the last pass ended). Containers are
  re-stamped in the in-memory map, which is rewritten every pass;
  folded rows go through a new `growth::touch_folded_rows`, one read and
  one write for the whole reused set. `assoc_store::CachedRows` grew an
  `observed_at` field for the same reason, stamped per entry rather than
  uniformly at save time — a uniform stamp would have answered "just
  now" for an entry that was merely loaded and carried forward
  unverified, which is precisely the entry whose age matters.

**Directory stamps are gone from both reuse decisions.** The brief
permitted keeping them "as a secondary consistency check"; I did not,
and the reason is that they could only ever disagree with the window
when the window is wrong, while costing one `stat` per recorded
directory per pass. `agents::container_fingerprint` (a stamp digest)
became `container_shape_key` (the encoding version plus the recorded
directory list, no syscall), which still makes a container whose watched
set changed shape, or whose rows an older binary wrote, a free miss.
That is what makes a replayed container cost literally nothing, which is
the brief's own third test.

One loose end from that: `external/folded.parquet` still stores one row
per directory the measurement listed, with its stamps, and nothing reads
those stamps any more — only the root row is consulted. They are kept
because they record which directories a stored measurement actually
covered, which is real provenance and is what
`record_folded_measurement` already refuses to store an incomplete
version of; dropping them is a schema change with no urgency, not an
oversight.

## Plumbing

The window is the walk's. `growth::TrackedWalk` gained `event_window`:
the replay's **unfiltered** `changed_dirs` plus `prev_state.last_observed_at`,
set only where the incremental branch was actually taken. Unfiltered on
purpose — `changed_paths` is narrowed to the subtrees the walk may
descend into, and every external location nested under a scan root is
pruned from that walk precisely so it can be measured once as its own
unit, so a narrowed list would hand out a window that cannot speak about
the paths it is asked about. `consumers::walk` leaves it in
`bus::Ctx::event_window`; `report::report_full_mode_scoped_tracked`
reads it back; `report_scope_with_parts_covered` collects one window per
root (canonicalized, because FSEvents answers in canonical paths) and
`observe_scope` hands the result to both families.

A caller that hands in a pre-built report (`base`) gets
`EventCoverage::untrusted()` and reuses nothing: it did not walk, so it
has no evidence.

One known way this fails *closed* rather than wrong, recorded so nobody
debugs it twice: the window's root is canonical (FSEvents answers in
canonical paths) and `external::discover_and_measure` canonicalizes
every candidate, but `agents::authorized_tool_homes` deliberately does
not. A tool home reached through a symlinked spelling therefore does not
`starts_with` the window's root, the window does not cover it, and the
container is re-identified. Slower, never wrong; canonicalizing the
agent side is a one-line change with a namespace question attached (the
agent key scheme is built on the uncanonicalized spelling), so it is
named here rather than slipped in.

---

# 2. The cost, measured

`incremental_external_and_agent_measurement.rs`, 5,000 synthetic Claude
Code sessions across 5 containers, and a 20,000-file Cargo registry
cache:

| | first pass | second pass, window vouches |
|---|---|---|
| agent home: wall time | 977 ms | 565 ms |
| agent home: header bytes | 740,000 | **0** |
| agent home: `dirs_listed` | 11 | **5** |
| agent home: `files_statted` | 5,006 | **6** |
| agent home: containers | 5 identified | **5 replayed** |
| cargo cache: wall time | 108 ms | **9.7 ms** |
| cargo cache: `dirs_listed` | 6 | **0** |
| cargo cache: `files_statted` | 20,004 | **0** |

One appended session: 1 container identified, 4 replayed, 97 header
bytes, exactly 1 derivation-cache miss.

The agent home's residual 5 listings / 6 stats is the tool home's own
structure scan (`~/.claude`, `projects/`, and the absent
`file-history/`, `image-cache/`, `uploads/`), not per-container work.
`replaying_containers_costs_nothing_per_container` pins that by holding
the home level constant and varying only the shape: nine containers cost
the same listings and header bytes as one and exactly eight more stats —
the eight entries `projects/` yields — and twelve times as many sessions
per container cost nothing at all.

## And the cost it added

`reviewer_cost_measurement_stack2::two_unchanged_full_observations_cost_report`,
which I may not edit, still fails two assertions, and after this change
it fails them *harder*:

| unchanged second pass | `dirs_listed` | `files_statted` | header bytes | spawns |
|---|---|---|---|---|
| stack/12 (directory stamps) | 40 | 5,605 | 0 | 0 |
| stack/13 (event gate) | 71 | 36,281 | 0 | 0 |

The stack/13 numbers are **identical to that fixture's first pass**, and
that identity is the whole attribution: nothing was reused, because
nothing could be. The fixture's one walked root is `src/`; its Claude
Code home, Cargo home, npm cache and model store are siblings of it,
under no scan root. Condition 1 fails for every unit. (A fresh two-pass
fixture also has no anchor for the walk itself — pass 1 takes the
`full_rules_changed` branch, which deliberately anchors no id, so pass 2
refuses with `no_stored_event_id` — and the `TooSoon` floor would refuse
a back-to-back second pass anyway. The brief anticipated exactly this:
"a fresh two-pass fixture without an anchor legitimately re-stats".)

---

# 3. The thing the integration owner should look at next

**In a default install the window never reaches the tool homes, so this
reuse does nothing.** Scan roots are project directories; `~/.claude`
and `~/.cargo` are not under them. The decision as written is therefore
"correct everywhere, cheap only where the scope happens to contain the
tool home", and on the reviewer's fixture it is a 6x regression in
unchanged-pass syscalls versus stack/12.

I implemented the requirement rather than narrowing it, and I am raising
the consequence rather than burying it. The completion that removes the
regression **without** giving back the correctness is to replay the unit
roots themselves:

- give each authorized tool home / external root its own cursor,
  persisted inside the existing `fsevents.json` (allow-listed control
  file; a new `serde(default)` map, preserved across the walk's own
  checkpoint write);
- replay them in `observe_scope` **before** the walk, so the cursor they
  read is the previous pass's rather than this pass's;
- write those cursors before measuring: if the measurement is then
  skipped or fails, the rows are not refreshed, condition 3 fails next
  pass, and the outcome is a re-measurement rather than a wrong reuse.
  The `observed_at` guard makes that safe by construction, which is the
  main reason I am confident the design is right.

It is not implemented here because it is a new persisted-cursor
lifecycle with its own ordering hazard, and it would not have changed
the reviewer test's numbers anyway (`TooSoon` refuses a back-to-back
second pass regardless), so it belongs in its own reviewable chunk
rather than at the end of this one.

---

# 4. The relinking test, re-asserted

`relinking_a_session_to_a_different_project_leaves_bytes_and_growth_history_unchanged`
was weakened in stack/12 to "a replayed container still reports the
declared path it was stored with", because a same-length in-place
rewrite of a session's `cwd` did not move the container's stamp. With
the gate, a rewrite is an event, so the weakening had no reason left.
The test now asserts:

- the next ordinary observation **under event coverage** reports the new
  project;
- a pass with no window re-identifies and reports the new project too —
  two branches, one answer, no pass that reports the stale project;
- a later quiet pass replays the container and carries the *new*
  attribution, because the pass that saw the rewrite stored it. A replay
  can only ever be as stale as the observation that wrote it;
- no pass, on any branch, fabricates a growth delta for a session whose
  bytes did not change.

---

# 5. The PATH shim, removed

`a_disabled_detector_must_not_probe_its_tool` put fake
`docker`/`lsof`/`plutil`/`xcrun`/`du` scripts at the front of the
process-wide `PATH` and counted lines in a shared log file. That is not
a test-local fixture: a sibling test in the same binary that
legitimately probes occupancy appended to the same log, so the test
failed under the default harness with `["lsof", "lsof"]`, reading like a
real regression, and `scripts/check.sh` carried `--test-threads=1` for
it.

Every `std::process::Command` in `swamp-core` now calls
`work_counters::record_spawn()`, and the test measures through
`work_counters::measured`, whose sink is scoped to the calling thread
and the pools it starts. No process-wide state, and it counts the spawn
rather than the shim it would have hit.

Two things keep that from rotting:
`the_spawn_counter_counts_a_real_spawn` (a real allow-listed
`brew --prefix` through `SystemCommandRunner` must be counted as 1), and
a `scripts/check.sh` grep that fails any `Command::new` in
`crates/core/src` not preceded by its counter.

`--test-threads=1` now appears once in `check.sh`, for
`reviewer_cost_measurement_stack2` alone, with the reason written beside
it: that test reads the **process-global** counters, which is what a
whole-observation cost report needs.

I chose the spawn counter over routing those five call sites through the
injected `CommandRunner`. The brief allowed either. The counter is a
smaller change and measures the thing the test is about (did a process
start?); the runner would additionally *prevent* the spawn, which the
counter does not — a regression would spawn a real `lsof` once before
failing the assertion. That is the one property the PATH shim had and
this does not, and it is stated here rather than glossed.

---

# 6. Container seams for the remaining session trees

The previous chunk named the blocker precisely: Codex's
`collect_jsonl_files` carried one entry budget shared across `sessions/`
and `archived_sessions/` (`already_seen + out.len()`), so a day
directory's contents depended on how many files the days before it had
produced — and rows stored under that rule could not be replayed into a
pass that reached the day differently.

Resolved by making the bound **per container** (`MAX_CONTAINER_ENTRIES`,
owned outright by one day directory) plus a pass-level cap on how many
containers are identified at all (`MAX_CONTAINERS`). Both are decided by
the tree alone; neither depends on what a sibling produced.
`a_codex_day_container_does_not_depend_on_its_siblings` asserts the
property directly, as an equality over live identifications with a
sibling of 0 and of 500 files.

Converted: Codex `sessions/<yyyy>/<mm>/<dd>/` (and the archived tree),
OpenCode `storage/session/<project-id>/`, Pi `sessions/<dir>/`. Each has
the same two tests in `agent_container_seams.rs`: replayed under a quiet
window with zero header bytes, and re-identified — reporting the new
bytes — under a window naming a 4 KiB in-place append.

Two things this needed along the way:

- Codex and Pi built their session units with `.project_link(...)`,
  which records a *resolved* link (`LinkBasis::Fixed`) and makes the
  whole container unstorable by design, so neither adapter was actually
  storing anything until they were switched to
  `.project_link_declared(...)`. The container then re-resolves the
  declared path live, which is the rule that stops a replay reporting a
  worktree deleted between two passes.
- OpenCode's units reach outside their container —
  `storage/message/<session-id>/` and
  `storage/session_diff/<session-id>.json` — through raw
  `symlink_metadata`, which records nothing. Those paths are now
  declared with `ctx.watch`, so a session acquiring its first message
  directory or diff re-identifies the project directory. Its `claimed`
  set is also derived from the returned units rather than mutated inside
  the closure, or a replayed container would have reported its own
  session diffs as orphans on every reused pass.

**Oh My Pi is deliberately not converted**, and
`oh_my_pi_declares_why_it_does_not_use_the_container_seam` says so with
the reason: its session bodies feed a home-wide shared-blob reference
count, and a pass that replayed some containers would count only the
sessions it identified, printing a number that is *wrong* rather than
unknown. The two honest ways to convert it are to carry each unit's blob
references through the container rows (a stored-shape change) or to
report every blob count as unknown on any reused pass (a user-visible
regression); neither is this chunk's call to make unilaterally.

---

# 7. One flake seen, not caused

`crates/tui/src/app.rs::tests::start_watch_opens_one_stream_per_root`
failed once during the final `cargo test --workspace` with "1 watcher
for 2 roots". It opens two **real** FSEvents streams and already skips
itself when it gets zero; it has no guard for getting one. Re-run three
times in isolation immediately afterwards: three passes. Nothing in this
chunk touches `start_watch`, and the run that failed was the tail of a
thirteen-minute suite. Recorded as an environment flake with a cheap
fix available (treat any count below `roots.len()` the way it already
treats zero), not as a regression and not as something I silenced.

# 8. Follow-ups, recorded not done

- **Per-unit-root FSEvents cursors** (section 3). The one that decides
  whether any of this reuse fires in a default install.
- **Oh My Pi's container seam** (section 6), pending a decision on how
  its blob-reference aggregate should survive a replay.
- **`mutation_corpus` costs about two minutes**
  (`every_mutation_fixture_is_rejected_by_its_audit`): it copies the
  whole workspace and re-parses it once per fixture, 136 times. Parsing
  once and applying each fixture in memory is the fix — build the
  `syn` item tree for the workspace a single time, then for each
  mutation fixture overlay just that file's parsed items over the shared
  tree and run only the audit that owns it. It changes the machinery the
  audits' own evidence rests on, so it deserves its own review rather
  than a quiet edit; it is untouched here, as in stack/12.

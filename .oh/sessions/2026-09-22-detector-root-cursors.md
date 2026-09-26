# 2026-09-22 — per-unit-root cursors, and a premise that was wrong (stack/14)

Branch `stack/14-detector-root-cursors`, based on
`stack/13-event-gated-reuse` (`9653cd7`). Input: `CHUNK_R6.md` —
implement §3 of the previous chunk's note (per-root FSEvents cursors),
convert Oh My Pi to the container seam, make the watcher test
deterministic, and speed up the mutation corpus if the first three land.

---

# 1. The premise §3 reasoned from is false, and the conclusion survives it

§3 of `2026-09-22-event-gated-reuse.md` said:

> In a default install the window never reaches the tool homes, so this
> reuse does nothing. Scan roots are project directories; `~/.claude`
> and `~/.cargo` are not under them.

They are not under them, but they **are scan roots in their own right**.
`scope::resolve_effective_scope` turns every detector-resolved home into
a `RootStatus::Present` root, and `report::report_scope_with_parts_covered`
walks every `Present` root, so `~/.claude` gets its own folded walk, its
own `fsevents.json` and — once it has an anchor — its own window.
`a_detector_home_is_a_present_scan_root` pins that, because a wrong
premise left standing is how the next chunk gets planned wrong.

I found it by writing the headline cost test from the brief, watching it
pass, and not believing it: the walk had written its anchor into the
agent home's volume dir. The measurement on the reviewer's own fixture
confirms it — on **stack/13**, with more than the replay-lag floor
between passes, pass 3 already cost 6 listings and 8 stats.

So the cursors do not buy reach. What they buy is this:

**The walk's anchor and the unit rows drift apart, and the freshness
guard then refuses reuse — correctly, and permanently.** A pass that
walks without measuring units (`ObservationParts::WALK_ONLY`, which is
what the TUI's background refresh uses) advances the walk's anchor. The
next full pass's window therefore opens *after* the stored unit rows
were written, `EventCoverage::unchanged_since`'s third condition fails,
and nothing is reused. Left alone, a TUI refreshing between full
observations prevents unit reuse for as long as it runs.

A cursor owned by the unit families does not move on a pass that
measured no units, so its window keeps opening where the rows were
written. `a_walk_only_pass_between_full_passes_keeps_the_unit_window_aligned`
asserts it with its own control in the same test: the walk's window
alone replays **0** of 3 containers, the unit cursor's window replays
**3**.

The same drift is visible on the reviewer's fixture without any TUI: on
stack/13, pass 4 cost 31 listings and 10,724 stats after pass 3 cost 6
and 8. Reuse held on some passes and not others. That is the alternating
behaviour §1 of the previous note predicted for containers and did not
notice for the walk's own anchor.

---

# 2. The cost, measured

Reviewer fixture (`reviewer_cost_measurement_stack2`'s own `fixture()`:
5,000 Claude Code sessions in 5 project directories, a 20,000-file Cargo
registry cache, a 500-file npm cache, a 200-file model store, one walked
project root). Four `observe_scope` passes with the **real** platform
FSEvents source and 5 s between passes, so the `TooSoon` floor does not
apply. Two runs of each; the numbers below reproduced exactly on both
except stack/13's pass 3, which was 6/8 once and 16/715 once.

| pass | stack/13 listings / stats | stack/14 listings / stats |
|---|---|---|
| 1 (cold) | 71 / 36,281 | 71 / 36,281 |
| 2 | 71 / 36,281 | 40 / 5,561 |
| 3 | 6 / 8 | 6 / 8 |
| 4 | 31 / 10,724 | **6 / 8** |

Header bytes: 205,000 on pass 1, **0** on every later pass, both
branches. Subprocess spawns: **0** throughout, both branches.

Wall time is not a useful column here: passes 3 and 4 take 9–12 s on
both branches, essentially all of it inside `FSEventStreamStart` (see
section 4), which the cold pass does not pay because it has no anchor to
replay from.

The **back-to-back** reviewer assertion is unchanged and still failing:

| unchanged second pass | `dirs_listed` | `files_statted` | header bytes | spawns |
|---|---|---|---|---|
| stack/12 (directory stamps) | 40 | 5,605 | 0 | 0 |
| stack/13 (event gate) | 71 | 36,281 | 0 | 0 |
| stack/14 (this chunk) | 71 | 36,281 | 0 | 0 |

The attribution has changed even though the number has not. On stack/13
it was "no window reaches these units". Now it is entirely
`RefreshRefusal::TooSoon`: that fixture observes twice about a second
apart and the floor is three seconds, so neither the walk nor the
cursors will vouch for anything. Preserving the floor was an explicit
requirement of this chunk, and it is a correctness property — FSEvents'
persisted log can lag a write by longer than the store's whole-second
granularity, so a replay that soon cannot distinguish "nothing changed"
from "not logged yet". I did not lower it to make a benchmark pass, and
I did not edit the reviewer test. Left failing for re-review 3.

The 5k/20k fixture as *detector roots beside* a project root
(`unit_root_event_cursors::an_unchanged_pass_over_out_of_scope_detector_roots_costs_nothing`):
pass 3 reads **0** header bytes, replays all 5 containers, and does
fewer than 200 stats across 25,000 unit files.

---

# 3. What the cursors are, precisely

- **Where.** `scope::EffectiveScope::authorized_unit_roots()` — the
  authorized detector-resolved roots, minus the builtin-defaults
  pseudo-detector, de-duplicated. Derived from the same authorized seam
  the two unit families derive their candidates from, so a disabled
  detector or an exclusion removes a root's cursor exactly as it removes
  its units.
- **Stored** in the `unit_root` half of that root's own volume dir
  `fsevents.json` (event id, device, observed-at). A path can be both a
  scan root and a unit root, so both writers do a read-modify-write of
  their own half; `the_walk_and_the_unit_cursor_share_one_file_without_erasing_each_other`
  asserts both directions.
- **Replayed before the walk**, in `report::observe_scope`, so each
  cursor is read at its previous pass's value.
- **Committed** only when the pass observed **both** unit families and
  persisted them (`observe && want.external && want.agents &&
  external_ok && agents_ok`) — the same `ReportCached` gating the walk's
  checkpoint has. Not advancing is always the safe direction: it makes
  the next window wider, never blinder. A partial or failed pass
  therefore leaves the anchor alone
  (`a_partial_pass_does_not_advance_the_cursors`,
  `a_failed_pass_does_not_advance_the_cursor`).
- **One stream per device.** `FsEventsSource::replay_roots` takes a
  batch; the macOS implementation groups by `st_dev`, opens one stream
  over all of that device's roots from the earliest anchor, and splits
  the result with `partition_changes` so no root ever sees another
  root's events. A member whose own anchor is later just sees events it
  already knew about, which can only cause a needless re-measurement.
- **Refusals, per root**, surfaced as `coverage::UnitRootCoverage`:
  `no_stored_event_id`, `too_soon`, `root_mismatch`,
  `unsupported_platform`, `helper_inconclusive`, `full_forced`,
  `no_store`. The device check is done in `growth::replay_unit_roots`
  rather than left to the platform source, so it holds for every source
  including the injected ones.
- **Linux** keeps "continuity unavailable → full re-measure":
  `UnsupportedPlatformSource` refuses, no root is covered, every unit is
  re-measured (`a_platform_without_fsevents_re_measures_and_says_so`).
  Unchanged from stack/09's platform contract, until #81/#82.

## Two things I changed that the brief did not ask for

**`EventCoverage` windows carry both spellings of their root.**
`external::discover_and_measure` canonicalizes every candidate;
`agents::authorized_tool_homes` deliberately does not. A single-spelling
window would either miss the agent family entirely or — worse — match an
alias-form path against a change list FSEvents always answers in
canonical form, find nothing under it, and report the unit *quiet*
because the spellings differ. `trust_alias` stores both and rewrites the
queried path into the canonical namespace before testing it. That closes
the fail-closed caveat §1 of the previous note recorded, without
touching the agent key scheme.

**Roots that are not on disk get no row at all.** A Cargo home
contributes `registry/index` and `git/db` whether or not they have ever
been populated. They are authorized roots with no units; giving them a
refusal row would be a reason for something that was never a candidate.
They are skipped, like a `SkippedAsNested` scan root.

---

# 4. Oh My Pi's container seam

Converted by the route the brief recommended. Each immediate
subdirectory of `sessions/` is a container, and each container stores
its **own** partial shared-blob reference count with its rows
(`IdentifyCtx::container_with_facts`, `ContainerFacts`). The home level
sums stored partials and fresh ones, so a pass that replayed one
container and re-identified another prints the same count a fully
identified pass prints —
`a_partially_replayed_oh_my_pi_home_sums_the_same_blob_counts` is
written against the tempting failure, which prints `1` for a blob two
sessions reference. A replayed container whose rows carry no partial at
all is `ContainerFacts::Unrecorded` and makes every count **unknown**,
never short: that number is what a future reference-based GC would act
on. `a_replayed_container_without_its_partial_makes_the_count_unknown`
reaches that state by stripping the fact rows out of a stored container.

The conversion needed one thing along the way. Oh My Pi built its
session units with `.project_link(...)`, a *resolved* link
(`LinkBasis::Fixed`), which makes a container unstorable by design — the
same trap Codex and Pi hit in stack/13. Switching to a declared link was
not enough on its own, because this adapter widens to
`ProjectLinkState::Shared` when `additionalDirectories` names a second
project, and that widening has to be redone on replay rather than
frozen. So `LinkBasis::Declared` grew an `additional` list and the
widening moved into `agents::resolve_declared_workspace`, where the
shared layer can redo it without re-running the adapter. Stored in the
existing declared column, separated by a C0 control; rows written before
this decode to an empty `additional`.

`oh_my_pi_declares_why_it_does_not_use_the_container_seam` is gone, and
the exception it recorded with it.

---

# 5. The watcher flake was a real bug

`start_watch_opens_one_stream_per_root` was recorded in stack/13 as an
environment flake. It is not.

`FSEventStreamStart` is a synchronous request to `fseventsd` that is
serialized per process and costs **seconds**. Measured on two fresh temp
directories on this machine: 1.4 s and 2.9 s on a quiet run, 4.1 s and
2.1 s on another, 4.7 s and 6.6 s on a loaded one. `fs_events::watch`
bounded readiness at five seconds, and `App::start_watch` waited for
each root's stream before spawning the next. So the later root regularly
lost the race — and in the 6.6 s case the abandoned stream had reported
`started=true`, 1.6 s after the caller gave up on it.

Fixed in two places: `watch` is split into `watch_pending` +
`PendingWatch::ready(budget)` so `start_watch` spawns every root's
thread before collecting any readiness (bounding the wait by the slowest
stream instead of their sum), and the budget is now 30 s, overridable
via `SWAMP_FSEVENTS_WATCH_START_TIMEOUT_SEC` — its job is to stop a
caller hanging on an `fseventsd` that never answers, not to second-guess
a call known to take seconds.

The test now drives an injected factory
(`fs_events::testing::inert_watch_factory`), so it asserts the loop —
one stream per root, none shared, none dropped — rather than racing
`fseventsd`. It went from 2.95 s and occasionally wrong to 0.01 s.
`a_root_whose_stream_fails_does_not_stop_the_others` covers the partial
case the doc comment promises. Real-stream coverage stays where it
belongs, in `fs_events::tests::live_watch_reports_a_write_under_the_root`.

One bug the existing test caught during the split, worth recording: the
first `PendingWatch::ready` implementation let `Drop` fire after handing
the join handle away, which set the stop flag the returned `Watcher`
shared and killed the stream on the line that took ownership of it.

---

# 6. Mutation corpus: 191 s → 18 s

`every_mutation_fixture_is_rejected_by_its_audit` was **191 s**. It is
now **18.4 s** measured alone; the whole `mutation_corpus` test file,
which also runs the unmutated-copy check and the new cache-equivalence
check, is 32 s wall (18.4 + 11.4 + 5.3 s measured individually, sharing
twelve cores).

Two changes, and the first one is not what the brief's phrasing
predicted:

1. **Parsing is memoised on file contents, and so is the expensive
   derivation over it.** `ast::parse_cached` keys `syn::parse_file` on
   `(rel, text)` and compares the text exactly — never a stamp, never a
   digest — so a cached parse can only be returned for input that is
   byte-for-byte the file being parsed. `ast::CachedAst` carries an id
   assigned once per distinct `(path, contents)` and never reused, and
   `ast::memoised` keys derivations on it. Both caches are thread-local
   and cleared together, so a derived entry can never outlive the parse
   it came from.

   Caching the *parse* alone took 191 s to 136 s, which was the surprise:
   parsing was not the dominant cost. Measuring each audit showed no hot
   spot — thirty rules at 1–7 s each, all of them re-deriving the same
   function bodies. `ast::functions` (fifty-one call sites, and it
   rewrites every path in every body through the resolver) is what
   actually mattered; memoising it took 54 s to 31 s. Memoising
   `production_calls`, `string_literals` and `call_paths` as well
   changed nothing measurable, and they are kept only because they cost
   nothing to keep.

2. **The corpus is sharded.** The audits are pure functions of a
   directory, so each worker gets its own workspace copy and its own
   thread-local caches; the unmutated-copy check gets one worker per
   audit (it is read-only on both trees). 513 s of CPU on 12 cores.

   Peak RSS for the whole test file is 1.6 GB — twelve parsed copies of
   the workspace — which is why the worker count is clamped rather than
   simply `available_parallelism`.

Neither change touches what a fixture asserts.
`the_parse_cache_never_changes_an_audit_verdict` is the guard the brief
asked for, in the form this implementation needs: three fixtures from
across the corpus, each run cold and then warm, where the *warm* run is
warmed by the unmutated file's own parse of the same path. A cache keyed
on anything weaker than the contents — a path, a length, a stamp — fails
there. It also asserts the restored file passes its audit again, which
catches a cache holding a mutated parse for unmutated text.

---

# 7. Not done

**The `TooSoon` residual on the reviewer's back-to-back fixture**
(section 2). Needs the integration owner's call: either the fixture
observes further apart, or the assertion is retired, or the floor
changes. I did none of the three.

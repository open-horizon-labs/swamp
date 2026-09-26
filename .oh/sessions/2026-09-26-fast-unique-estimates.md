# Fast refresh, explicit unique-byte reconciliation

## Execute — selected policy

The owner selected fast refresh with explicitly stale unique-byte estimates
until reconciliation (#131). Do not add an inode inventory, database, or
automatic same-device rescan. Keep existing folded rows and typed Parquet.

Success: changed-container refresh stays local; current allocation and last
reconciled unique bytes are distinguishable in live and cached output;
explicit full observation measures shared inodes once across the measured
scope. Charge reassignment must not become claimed storage growth.

Pre-flight: scope is measurement/evidence, not cleanup authorization, install,
release, or perfect auditing. Existing report-only CLI and TUI cleanup remain.
The trade-off is stale unique-byte estimates between reconciliations, explicitly
approved by the owner. Partial coverage must not be labeled a complete scope.

Risk checklist (before editing):
- Cross-root links: fixture must reject summing root-local dedup totals.
- Device keys/root order: reject inode-only keys and first-root charge policy.
- Fast refresh: counters must reject rescanning unchanged sibling containers.
- Persistence: cached report must retain stale state and reconciliation age.
- History: no growth derived from switching unique-byte accounting bases.
- Coverage: partial/reused measurements cannot clear stale evidence.
- Storage: no per-file identity persisted, no new storage engine.

Dissent: an ephemeral ledger alone cannot reconcile reused roots. Therefore
only an explicit complete full measurement can refresh the scope-wide unique
estimate; ordinary refresh retains its last value as stale. Row allocations
remain useful for cleanup, and are not promised reclaimable bytes. Stop if
correctness requires silently broadening normal refresh into a global walk.

## Implementation / trade-off

An explicit `observe --full` uses an additional pass through the existing
parallel folded walker with one device/inode set across measured roots and
units. This avoids coupling every adapter identification path to accounting
or persisting file membership. It increases explicit-full cost; normal refresh
does no census. The scope accounting overlay does not rewrite root-local
charges or history. Per-row shared-with attribution is still separate work;
do not close all of #131 on this implementation.

The existing run table stores nullable unique bytes, reconciliation timestamp
and needs-reconciliation flag. Invalidation precedes observation so partial
family updates cannot leave the prior value certified current. Cold reports
restore the same state. Unknown is not zero. Incomplete full coverage cannot
clear the flag. The source audit reserves “stale” for cleanup verdicts, so the
delivered field is `needs_reconciliation`, describing measurement evidence.

Unowned measurement boundaries distinguish direct/folded shared rows and
unreconciled estimates. Hardlinks no longer force unrelated-root traversal;
unknown old boundaries and ownership transitions retain full fallback.

Initial evidence: all 16 build-adapter/history tests pass, including shared
unowned refresh with at most one directory listed. Three reconciliation tests
pass: device keys, excluded files/symlinks/missing paths, 20,000 shared entries.
The 20k fixture has 1,000 distinct inodes and 1,792 set slots: 28,672 bytes of
key capacity (not whole-process RSS; excludes allocator/hash-control overhead).
There are no retained identities or new store files.

## Review / dissent

Independent Luna review found no concrete blocker after six focused scope
tests passed. Parent review strengthened the external fixture: a nested cache
alone would also pass if external roots were accidentally omitted, so a disjoint
Cargo-home/project hardlink case now tests that actual failure mode. Scripted
events canonicalize paths and filter by requested root, matching the live
source rather than spuriously forcing every sibling root to rescan.

Known missing roots are not an incomplete read of an existing path. They stay
in coverage and contribute no paths to the observed-set estimate; this makes
unused default tool homes compatible with reconciliation. An unmounted volume
may also be missing, so neither the docs nor this aggregate claim to cover it.
Partial/inaccessible roots and incomplete units still prevent certification.
This changes no history/tombstone behavior.

Risk retirement:
- Cross-root/root-order/device/exclusion: scope fixtures plus walker key and
  pruning tests reject per-root sums, inode-only keys, and excluded traversal.
- Fast refresh: no-event scope fixture asserts zero listings/stats/header
  reads/spawns; changed-container fixture rejects traversing a 96-file sibling;
  shared-unowned fixture allows at most one listed directory.
- Cold persistence/partial updates: run-row round trips and partial-family
  invalidation tests reject a transient-only warning or old certified total.
- History: overlay writes only run facts; existing history regressions remain
  unchanged. No new delta or charge-assignment code exists.
- UI: all 30 frame tests pass; new narrow/wide-header assertions preserve the
  warning without changing unrelated snapshots. Unowned text also warns.
- Old optional columns: direct typed-Parquet regression rejects making the
  existing store unreadable merely because the estimate was never recorded.
- Limits accepted: full reconciliation costs an additional walk; ordinary
  estimates may be stale; no atomic audit, extent dedup or per-row shared-with
  mapping is claimed. Native macOS/Linux full validation remains a CI gate.

Decision: continue with the selected policy, not a new storage engine or an
automatic full rescan. #131 stays open for its broader row-attribution work.

## Final validation

Routine workspace gate passed at 14:12:47 local:
`/tmp/swamp-fast-unique-reviewed-check.log` (format, strict Clippy, all source
audits, release feature graph, workspace tests, named targets and greps).
Final focused checks also cover the last unowned-view warning: 16 build/history
tests and six scope-accounting tests, plus the 30 TUI frame tests. Four direct
unit checks cover the ephemeral ledger and absent optional run columns.
No user filesystem data was removed, and no binary was installed or released.

## Solution Space — remaining shared-file attribution

Problem: explain sharing between meaningful artifact containers so users can
choose cleanup, without exact per-file auditing or slowing ordinary refresh.
Criteria: decision usefulness, unchanged refresh work, compact folded storage,
stable history, explicit scope/age. The original #131 exact incremental charge
criteria conflict with the subsequently selected stale-estimate policy; issue
acceptance needs explicit reconciliation before claiming completion.

Candidates: (A) retain aggregate-only warning; (B) retain a persistent inode
membership index; (C) aggregate shared-container relationships during explicit
reconciliation; (D) inspect only selected cleanup units on demand. Recommend
C plus the existing D selection estimator, subject to architectural dissent
before schema/API implementation. A remains a fallback, B is rejected by the
no-per-file-persistence constraint. No implementation is authorized by this
solution-space turn.

For C, associate observed hardlinked identities with disjoint folded accounting
units during the explicit reconciliation walk, then discard identities. Store
container-level sharing groups, bytes, observation time and coverage in typed
Parquet, not pairwise per-file edges or JSON. Three owners of one inode form one
sharing group, not three additive byte charges. Parent views derive membership
from those units; ancestors are not extra owners. Ordinary refresh marks this
evidence stale; neither row allocations nor growth history change. Incomplete
link membership is explicit, not a guessed peer path. Allocation, scope-unique
bytes and selection reclaimability remain distinct. File hardlinks do not
establish APFS extent ownership.

Risk-retirement plan (planned checks, not completed evidence):
- Three-way store/project sharing: reject pairwise summation and first-root
  ownership; reverse root order and assert identical groups and union totals.
- Nested/excluded/out-of-scope links: reject ancestor-as-peer and falsely
  complete membership; selected-set estimate must retain outside-link limits.
- Incremental removal/replacement: reject stale peers presented as current,
  synthetic growth from charge reassignment, and eager sibling rescans. Include
  cold report restoration and explicit-full refresh of relationships.
- High fan-out: measure 20k-file fixture and actual cache layout, in-memory peak,
  persisted bytes and refresh counters; reject per-file retained membership or
  quadratic pair edges. Pivot to bounded aggregate summaries if group diversity
  defeats compactness; disclose truncation, never claim exhaustive peers.
- Cleanup: reject treating sharing as a deletion prohibition; selecting one vs
  all links changes the estimate, not deletion eligibility by itself.
- Accepted: peer discovery only in observed scope, stale between reconciliation,
  extra explicit-full cost, no exact APFS free-space promise.

Lineage: W1b under W1 / G1 remains selected; this is a candidate implementation
of that existing step, not a new scope reduction. W1a is a sibling requirement,
not newly selected or claimed complete here. Broader issue closure requires
updating its conflicting acceptance criteria with owner approval.

## Execute — container sharing (owner authorized)

Selected C plus existing D. Success: dated shared-container evidence in TUI and
reconciliation text/JSON, no additional ordinary refresh traversal, compact typed
storage, no charge transfers or cleanup prohibitions. Stop if peer discovery
requires a persistent file inventory or global ordinary refresh. Owner execution
approval also authorizes aligning #131's acceptance criteria; updated the issue,
left it open. No release/installation or user-data cleanup is part of this work.

Architecture dissent: pairwise edges are a tempting but wrong local optimum:
three-way sharing becomes three charges and fan-out becomes quadratic. Use one
membership group instead. Deepest-container assignment describes a disjoint
measurement partition, not project ownership. Adoption risk: a user could mistake
observed sharing for reclaimability; every displayed line names observation time,
staleness and not-reclaimable semantics. Opportunity-cost risk: enumerating peers
could undo folding; bound aggregate payload and explicitly disclose omissions.
Verdict: adjust/proceed with groups, no per-file persistence, no automatic census.

Implementation: explicit reconciliation's existing parallel walker collects only
hardlinked identities into ephemeral container membership sets. Aggregate groups
and unknown-link evidence share the existing estimate's age/invalidation; typed
List/Struct columns in runs.parquet carry them through cold reports. Max 4,096
groups, 65,536 memberships and 1 MiB path text; omitted totals are explicit.
No filesystem traversal was added to rendering or normal refresh. Source-audit
capability checks pass after using the existing metadata gate exports.

Risk retirement evidence:
- Three-way/nested/excluded/outside membership and reversed order: walker fixture
  rejects pairwise edges, first-root charging, ancestor-as-peer and invented peers.
- Link replacement plus cold stale state: six scope integration tests reject
  refreshed-looking old relationships and scanning the unchanged 96-file sibling;
  unchanged counters remain zero. Full reconciliation removes obsolete relations.
- Persistence: typed roundtrip and absent optional-column regression reject JSON
  blobs, transient-only state and unreadable older rows. Compact fixture run table
  is 5,721 bytes including all fields, representing 20,000 entries / 1,000 identities
  collapsed into one 20-container group (not 20,000 persisted rows).
- Fan-out: 1,000 containers form one group with 1,000 memberships; 5,000 distinct
  groups explicitly omit 904 after the 4,096 cap. No pairwise expansion.
- Real 20,000-entry filesystem fixture and synthetic grouping checks passed in
  6.46s including fixture creation/removal, maximum RSS 15,810,560 bytes. This is
  process RSS, not an isolated collector-memory claim.
- Real uv cache: explicit full observation of 1,744,928,768 unique bytes in 1.62s,
  max RSS 69,795,840 bytes; run table 5.6 KiB. No cross-container groups found in
  that selected root, so this is cost evidence, not proof of real peer discovery.
- TUI: selected-worktree evidence includes peer path, timestamp and stale warning
  at 80x24 and 200x60. No selection eligibility code changes; existing plan-set
  reclaimability tests remain the cleanup accounting guard.
- History: only run metadata gains sharing columns; no artifact/delta writes or
  charge reassignment added. Existing history suite is the regression check.

Accepted limits: scan-time evidence is not an atomic audit; scope excludes unknown
peers; hardlinks do not identify filesystem clones/snapshots; extra cost only on
explicit full. Human verification remains readability of the new selected-row
facts in normal use, not accounting correctness.

Final review: aligned, no charge ownership or deletion policy changes. Largest
groups are retained/displayed first, with deterministic ties and explicit caps.
`scripts/check.sh` passed at 14:52:59 (format, Clippy, audits, release graph,
workspace tests, named targets and greps). After final refinements, focused
reruns passed: five sharing-related unit tests, two typed-persistence tests,
six scope integration tests, all 31 TUI frames, source audits and strict
workspace/all-target Clippy. Logs: `/tmp/swamp-sharing-routine.log`,
`/tmp/swamp-sharing-final-focused.log`, `/tmp/swamp-sharing-final-clippy.log`.
No remaining model-checkable risk from this handoff is intentionally deferred.
Native CI/release remain separate gates; no installation or release claimed.

## Review — integration and release promotion

Owner requested integration merge and release-readiness review. Fast-forwarded
`integration/full-scope-merge` from b27de3f to 54d8aae and pushed. This preserves
the entire stack without conflicts; main and published v0.6.3 were not changed.

Verdict: **Adjust / hold release promotion.** Targeted review against the Claude
packet found two freshness/performance issues; it is not an exhaustive new audit
of the entire historical stack.

1. P2: `tui/src/app.rs::prune_removed` updates rows and totals after successful
   cleanup but leaves `unique_estimate.needs_reconciliation` false. The last
   unique estimate and peer facts can still look current until a later observer
   finishes; if it fails they remain misleading. Reproduced by seeding a fresh
   UniqueEstimate in the existing prune regression, then asserting it was
   invalidated after successful removal. Test fails at that exact assertion.
   Temporary test edits were removed after reproduction; production unchanged.
   Evidence: `/tmp/swamp-release-review-cleanup-freshness.log`. Required repair:
   invalidate in-memory reconciliation evidence immediately on successful local
   mutation; test failed/cancelled/no-op results separately from successful ones.
2. P2: CLI `observe --full` says it re-anchors the event ID, but a fresh probe
   store followed by ordinary observe produced `no_stored_event_id` and another
   full walk. A subsequent immediate run was full/too_soon; after the lag floor
   elapsed, incremental/no-change completed in 411ms. The forced-full unit-root
   test explicitly expects no source call/cursor. Clarify/fix that contract
   without certifying a cursor that could miss concurrent mutations. Logs:
   `/tmp/swamp-release-review-refresh-{1,2,3}.log`, probe scope ~/.cache/uv only.

Green: native macOS/Linux routine CI and Linux archive/smoke jobs on 54d8aae,
the local routine gate, shared-group/persistence regressions and 31 TUI frames.
Full-tier jobs (compile-fail/mutations/cost) on that exact code commit remain
running: https://github.com/open-horizon-labs/swamp/actions/runs/36264182848.
Their earlier-commit passes are not evidence for this head.

Frame remains developer cleanup, not audit-perfect accounting. The new finding
is an omitted state transition, not a reason to redesign storage or prohibit
hardlinked cleanup. Issue #131 policy remains owner-approved. All-catalog/epic
completion is not inferred from this targeted pass. Human checkpoint remains
whether real selected-row guidance is clear; reproduced freshness defects must
be fixed before asking the owner to validate the UI. No release/tag/install or
production repair was performed in this review-only step.

## Execute — release-review P2 repairs and installed trial

Owner authorized both P2 fixes, local installation, a subagent usage trial and
native mutation CI. No release publication or real cleanup is authorized here.
Success: successful local removal immediately invalidates reconciliation facts;
failed-only/empty actions do not. macOS full observation leaves a pre-measurement
baseline without replaying history, only after persistence succeeds. Unsupported
platforms do not invent journal continuity. Install the optimized build, then
test the public interface independently and report release gates honestly.

Risk checklist: reject flagging only the next observer (immediate cleanup test),
clearing useful last-measured facts (value/time/group retention assertions),
invalidating failed-only actions (negative and mixed-result tests), capturing an
end-of-walk cursor (anchor hook writes a fixture that the walk must measure),
publishing on failure (dropped checkpoint), forcing expensive replay (source
panics if replay is called), or advancing the wrong cursor (unit/walk separation).
Existing unsupported-platform tests retain the no-persistent-history contract.

Repairs: TUI invalidates the in-memory estimate after finding any successful
outcome. FsEventsSource gains an optional cheap pre-full baseline hook; the
macOS source queries current ID/device, while other sources default to none.
Both walk and unit-family paths stage it before measurement. CLI help and
architecture explain publication and platform limits. No new persistent fields,
scanner, global refresh, cleanup policy or data model.

Focused checks passed: two new full-anchor lifecycle tests, twelve unit-root
cursor tests, and Sagan's 73 TUI library tests. Parent reviewed Sagan's one-file
change; it preserves bytes/time/sharing and handles success/failure/mixed results.
Routine gate, optimized installed usage and final native full CI follow; do not
treat these focused results as a release sign-off.

Routine `scripts/check.sh` passed at 15:27:25; log `/tmp/swamp-p2-routine.log`.
Optimized build installed at ~/.local/bin/swamp with SHA256
672e8cc18e770274b82a177071bcab300753cac12ba36c69d1ec3abb3a0475a8,
matching target/release/swamp. Previous executable is recoverable at
/tmp/swamp-pre-p2-install.LlgGUk/swamp. Version remains 0.6.3, a local development
build, not a new published release.

Installed real-source uv probe: full_forced 1,152ms followed, after the lag floor,
by incremental/no-change 453ms, both 1,744,928,768 measured bytes. Separate
temporary store /tmp/swamp-p2-anchor-probe.6KUaQO; no user data deletion.
Logs `/tmp/swamp-p2-live-{full,incremental}.log`. This directly rejects the prior
missing-anchor behavior; it is a single-machine observation, not a guarantee.
Bohr has a separate public-CLI-only, no-deletion trial of actual Swamp checkouts.
Final native full/mutation CI must use the repair commit; previous-head native
passes are not a substitute. Release remains conditional on that gate and the
installed-trial result; no tag or publication is authorized or performed.

## Execute — installed-trial repair loop

Aim: the public CLI must let a developer observe several checkouts, read
that same scope, and understand Cargo storage without private command guidance.
Fix explicit report roots symmetrically with observe; preserve exact-scope reads,
configured exclusions, read-only reporting, existing storage and folded scans.
Fix Cargo aggregation at its organizational-profile boundary, not by suppressing
unknown rows. Correct verified help/skill contradictions. No release or deletion.

Risk checks: multi-root roundtrip with reversed root order; reject silently
answering a constituent-root query from the combined snapshot; preserve combined
snapshot after individual observation; exclusions through a symlink alias;
Cargo classified descendants survive profile grouping without double charging
their nested files; unsupported units remain visible. Alias exclusion check
exposed a preexisting lexical-only root comparison, repaired using the existing
comparison namespace (no recursive traversal). Empty exclusions do no extra I/O.

Completion gates: focused regressions, routine gate, optimized installation,
independent public-interface usage trial, review, exact-head native mutation CI.
Native CI for prior head af42025 has passed macOS and is still running on Linux;
it cannot validate this repair. Human TUI judgment and release authorization
remain separate from automated CLI readiness.

### Second independent trial and repairs

McClintock observed both real Swamp checkouts in 3.03s; reported bytes matched
the stored du verification (24,562,663,424 bytes). No deletion occurred. It
confirmed combined reporting works and identified duplicate linked-worktree
discovery rows, wall-clock-derived history duration, and oversized nested JSON.

Repairs combine projects/worktrees by identity before persistence and while
reading previously duplicated tables; a real Git linked-worktree CLI fixture
asserts unique identities, measured artifacts, counts and reconciled bytes.
History uses the report's observation timestamp, with per-root spans and the
shortest common scope span. Unknown history does not become an invented window.
Build/dependency interiors default to 30 units with independent unit-limit/
unit-offset pagination and explicit total/truncation; family summaries remain
complete. The pagination regression tests distinct pages and unchanged totals.
Cargo dependency advice says rebuild rather than registry reinstall.

Sagan's review additionally caught canonical descendant exclusions bypassing
lexical prune construction. Prunes now compare canonical paths and translate
back to traversal spelling; a symlink-root fixture grows excluded data by MBs
and asserts observed bytes do not change. No-exclusion resolution skips this
extra canonicalization. Full prior-head native mutation CI passed both OSes.
Routine gate passed before these second-trial changes; rerun on final state.

## Review — repair-loop completion gate

Aim remains usable, fast developer-storage decisions rather than forensic
certainty. Review status: Adjust until frozen-build checks and installed trial
finish. Scope additions were concrete counterexamples from the independent
trial/review, not new adapters or storage architecture. Existing typed Parquet,
folded scan, event paths, and human cleanup boundary are preserved.

The second review found clone labels and duplicate stored metadata. Checkout
kind normalization follows path order and preserves Linked; old duplicate facts
are deduplicated, missing metadata filled, and conflicting scalar provenance
reported explicitly rather than guessed. Three focused merge tests passed.
The static audit rejected a test-only #[path] module; tests were moved to the
normal module directory without weakening the capability-gate rule.

Model-checkable risks have dedicated regressions for scope identity/exclusions,
profile masking, nested double counts, duplicate worktrees and clone order,
stored metadata conflicts, frozen multi-root history, and independent nested
JSON pages. No real cleanup was performed. Candidate allocations are not a
promise of reclaimed APFS space. Human TUI judgment and final release approval
remain external checkpoints; native mutation results must reference final HEAD.

Installed retrial evidence: /private/tmp/swamp-storage-retrial.m8v7D6.
McClintock found no remaining scoped CLI blocker: two unique projects/three
unique worktrees, root-order-independent JSON, frozen zero history across three
reads, correct Cargo recompilation advice, and independent interior pages.
Default Builds JSON fell from 3.97MB to 181,408 bytes (30/679 units), with family
totals unchanged. Observe with du took 5.18s under concurrent tests; reads took
0.04–0.07s. Walked/attributed/du all matched 25,364,549,632 bytes.
Recommendation was to retain the actively built target during this work, then
review it or the narrower incremental caches; no deletion was done.

Final routine caught a stored-fact contract regression: normalization relabelled
a lone persisted Clone as Main. Restrict read-time repair to multiple mains;
all four project_worktree_tables tests then passed without weakening assertions.
Rerun routine and rebuild after that correction. Remaining estimate uncertainty,
last-page truncation meaning, and current-use/rebuild availability are documented
limitations, not claims of guaranteed savings or obsolescence.

Final installed SHA256: 2cf5524e13faf07bfc16ec21e2b901435764d01aad171c537e400d1ef9037720
at ~/.local/bin/swamp, matching target/release/swamp (optimized, version 0.6.3).
McClintock rechecked this exact binary against its saved trial: projects,
worktrees, reconciliation, builds and offset-page JSON match the successful
trial byte-for-byte; frozen history and dependency advice still correct.
First read 0.27s, subsequent reads 0.04–0.06s. No remaining scoped CLI blocker.
Review route: Continue to exact-head native full/mutation CI, not publication.

Final routine scripts/check.sh passed at 16:29:12, including formatting,
Clippy, source audits, workspace tests, doc tests and named-target checks.
Log: /tmp/swamp-loop-verified-routine.log. Optimized build log:
/tmp/swamp-loop-verified-release.log. Native full CI is dispatched after push;
its pending status is not replaced by the previous head's green results.

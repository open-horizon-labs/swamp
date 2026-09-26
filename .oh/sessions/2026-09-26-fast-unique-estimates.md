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

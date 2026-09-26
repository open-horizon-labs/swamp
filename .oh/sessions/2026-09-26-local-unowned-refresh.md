# Local unowned refresh and shared-byte accounting

## Aim / selected approach

Continue #134/#131 without a per-file inventory or an alternate database.
The previous explicit full-root fallback is correct but unnecessarily walks
unrelated projects when a loose file changes.

Options considered: keep that fallback; refresh affected existing folded rows;
add directory rollups for every interior of every unowned container. Select
affected folded rows with an explicit measurement boundary in the existing
unowned Parquet table. Direct-directory rows can be relisted; classified
subtrees remain opaque containers and are remeasured only when implicated.
The broader interior-rollup extension is not required to stop whole-root walks.

Preserve source exclusions, no symlink traversal, root-local dedup, project
discovery, unchanged reuse, and report-only CLI. No real cleanup or release.
Unknown old cache shapes, ownership changes, permission gaps and shared-inode
ambiguity can still use the established full measurement path. Do not advertise
those fallbacks as incremental work or close #134 on a full-scan-only test.

## Dissent / risk checks before implementation

The tempting shortcut replaces an ancestor row without knowing whether it is
direct bytes or a recursive aggregate. An explicit boundary must survive a
store round trip. Another shortcut sums path allocation after a hardlink edit;
prior/new shared-inode evidence must refuse that shortcut and reconcile.

Checks: add/grow/shrink/delete/rename vs full; changed parent plus child;
new/removed subtrees; newly discovered repo; excluded paths; directory replaced
with symlink; hardlinks; unchanged reuse; changed loose directory beside many
unchanged files, asserting actual work counters rather than mode alone.
Stop if correctness requires a per-file persisted inventory or silently
weakened dedup/history. #131 needs its own feasibility review: an in-memory
global set alone cannot dedup a newly measured root against reused roots.

Pre-mortem: stale descendants (boundary tests); whole-root latency hidden under
an incremental label (cost counter tests); scope creep into a new storage
engine (use existing typed Parquet rows, no new files/database).

## Verification

Implemented in the existing unowned table: optional typed measurement boundary
(`Direct`, `Subtree`, `Hardlinked`). No per-file rows or new store files.
Unknown/missing baselines reconcile instead of dropping retained bytes.

Independent review found that the first implementation handled directory events
but fell back on production file-plus-parent events. The strengthened root-file
cost test failed with `mode=full` before normalization; after the fix root-file,
loose-file and folded-subtree events all remain incremental. Symlink ancestors
are rejected before reading cache classification tags.

Measured 2,000-file sibling fixture: reference full scan 8 listed directories /
2,014 instrumented file stats; root-file change 1/6; loose-file change 2/11;
folded-subtree change 3/9; unchanged refresh 0/0. Counts include inherited pool
work and are fixture observations, not end-to-end timing guarantees.

A final review caught removed cache tags retaining an obsolete folded boundary.
The repair reconciles when prior `Subtree` evidence no longer classifies, with
a regression comparing the boundary rows to a fresh walk. New classified
subtrees retain their folded shape. Event storms choose full measurement before
attempting local work; the cost assertion rejects a double traversal. Independent
re-review found no further actionable defect in these changes.

Passing focused tests cover mutation/full parity, file+parent events, new folded
boundaries, ownership transfer reported at `.git`, exclusions, shared-link
reconciliation, symlink replacement and missing unowned baseline. The final
routine gate passed at 11:53:40 local (`/tmp/swamp-local-unowned-reviewed-check.log`):
format, strict Clippy, source audits, workspace tests and repository checks.
The strengthened event-storm/locality test also passed separately after its
last assertion was added. No real user files were removed.
Previous commit 382bfae passed both native full checks (run 36250838820); that
does not validate this new implementation.

Review decision: proceed with the local-refresh repair and native checks.
It improves #134 but keeps explicit reconciliation fallbacks and does not
claim completion of #131. No architecture/frame drift into another store,
per-file audit or cleanup authorization system. Human decision needed only
for the #131 precision/latency policy below; native validation remains pending
for this commit.

## #131 feasibility / decision boundary

Independent review confirms local aggregate bytes/hardlinked flags cannot tell
whether a changed root shares an inode with an unchanged root whose identities
were discarded. A shared ephemeral ledger can reconcile a full observation,
but cannot fix incremental reuse by itself. Exact updates need same-device
remeasurement when sharing may change, or retained membership. The user has
been asked to choose automatic device reconciliation versus fast refresh with
unique-byte totals explicitly stale until reconciliation. No silent weakening
of #131 acceptance or persistent inode inventory has been implemented.

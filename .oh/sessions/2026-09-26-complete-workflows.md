# Complete remaining workflows

## Aim and execution basis

Implement remaining known shortcomings from the Sep 26 review and original
full-scope handoff, then review and repair again. Preserve later owner decisions:
Sep 23 report-only CLI, human TUI removal without grants or post-mark vetoes;
Homebrew default-off; Codex metadata from the existing state index, not transcripts.
No new inventory DB or per-file persistent cache. No real user-data cleanup.

Success is public behavior and exact usable TUI operations backed by fixtures,
not catalog flags. Stop/pivot if an operation cannot preserve its selected
boundary or would require guessing a tool's format. Record that specific gap;
do not replace the full scope with a first increment.

## Execute checklist

- [x] Review/integrate #145 CLI filtering and actual supported guidance.
- [x] Historical-data versus cache consequences in models, previews and docs;
  tests must fail the tempting blanket 'all Trash-able data regenerates' fix.
- [x] Observation-window timestamps; test writes during scan and after its end,
  defeating display-only tolerance or suppression of every future-time warning.
- [x] Closed-pipe behavior (Luna); real subprocess test, not panic string masking.
- [x] Cross-ecosystem precise build actions and purpose selection, preserving
  unsupported native-operation limits; fixture moves must retain unrelated data.
- [x] On-demand Cargo target/variant inspection (#107), bounded/cancellable and
  absent from ordinary refresh/storage work.
- [x] Copilot linkage/action gap identified by this review: verified sources, no guessed
  formats, no universal missing-project refusal; tests for exact membership.
- [ ] Reconcile remaining acceptance gaps and native validation against current
  owner decisions; don't restore deliberately removed authorization machinery.
- [ ] Review again, repair actionable findings, routine/full native checks.

## Work ownership

Parent: stack/30-complete-cleanup-workflows in /private/tmp/swamp-fix-full-check.
Luna: CLI closed-pipe and explicit Cargo inspection on #145, integrated through
c96fb34. Now independently reviewing the parent's changes without editing.

## Review repairs and measurements

- Family marking initially included hidden nested units; fixed to call the
  same outermost-family helper as display. A hidden-cache fixture checks this.
- Project filtering now chooses the deepest checkout, not the first containing
  checkout. The real CLI fixture includes an independent nested Node project.
- Cargo inspection initially treated every persistent or missing lock file as
  partial coverage. Removed that false signal: inspection is a non-atomic read,
  not authorization. The entry-limit fixture now actually exceeds its limit.
- Copilot's metadata extraction is limited to workspace.yaml top-level cwd,
  based on pinned SDK file evidence and upstream reported layouts. Tests prove
  unsupported adjacent JSON is not searched and persisted cache reuse reads
  zero metadata bytes. Confirmed session directories are removable without a
  known project; settings, shared DB and sibling sessions remain intact.
- Actual Swamp debug deps, explicit larger-budget run: 247,883 entries,
  2,717,616 metadata bytes, 3,885 ms; 39,775,735,808 allocated bytes,
  31,285,080,064 inode-unique within the selection. 353 target/variant groups;
  38,575,489,024 bytes matched and 1,200,246,784 residual. No guessed package
  IDs. This is a single local measurement, not a portable performance promise.
- Default inspection budget adjusted from a mostly-partial 8,192-entry sample
  to 262,144 entries / five seconds / 8 MiB metadata based on that experiment.
  No index writes and no call from normal observation. Hard time ceiling 30s.

## Broader outcome limits (not silently closed)

#131 cross-root hardlink dedup remains a separate accounting architecture gap.
#134: persisted empty topology is now recognized, so unchanged checkoutless
roots can reuse their measurement. Changed checkoutless roots explicitly
remeasure instead of preserving stale totals. This does NOT implement changed-
directory rollups for unowned bytes or prove full mixed-scope acceptance.
The initial add/grow/shrink/delete/rename parity test was vacuous because it
fell back to full scans; its replacement checks the actual mode and the
explicit fallback reason. Neither issue should be closed by this PR.

Project-local cleanup does not claim native selective pruning of shared
stores, SDK installations, simulator state or Docker build-cache graphs.
These limits are visible capabilities, not a claim of whole-epic completion.

## Review and dissent, after implementation

Aim: make the reported storage actionable for a human without false recovery
claims, context-heavy interfaces or another persistent per-file index.

Alignment: necessary and aligned fixes to the previous six findings; the
mechanism reuses existing folded facts, adapter roles and exact TUI Trash.
Sufficient for those concrete review findings, not for every open outcome
issue listed above. No authorization/grant system was restored. Completion
remains subject to routine/native validation, not a release claim.

Independent review: Luna/Sagan reviewed the parent diff read-only. It checked
exact category membership, removal consequences, event-thread responsiveness,
Copilot metadata evidence and selected-session boundaries. Its only finding
was a displayed inspection cap mismatch, already corrected during measured
budget tuning; it rechecked the final constant and withdrew that finding.
Focused independent TUI and agent tests passed. This is additional model
review, not human verification.

Dissent: the strongest case for this change is useful project-local cleanup
through existing mechanisms. Contrary evidence actually changed the patch:
hidden descendants cannot be selected behind a category row, lock-file
existence cannot imply active use, and a full-scan fallback cannot prove
incrementality. All three have targeted regression tests. The weakest
remaining assumption is ecosystem layout completeness; unrecognized paths
and unsupported shared/native operations stay explicit rather than acquiring
an invented cleanup contract.

Pre-mortems: (1) a group selects more than displayed — shared helper and
hidden-descendant fixture; (2) a user cannot see limits or consequences —
TUI/CLI surface partial coverage, recovery and allocation basis, but human
terminal review remains useful; (3) inspection becomes an expensive routine
inventory — no observe call/persistence, bounded explicit worker and live
3.9-second complete-profile measurement. Decision: proceed with the concrete
fixes and native checks; do not close broad epics or the accounting gaps.

Human verification: try the new profile `i` panel and non-Rust category
selection on intended projects. Fixture Trash moves establish boundary
behavior, not a promise that every real ecosystem/version regenerates
identical output. No real user's files were moved or deleted in this work.

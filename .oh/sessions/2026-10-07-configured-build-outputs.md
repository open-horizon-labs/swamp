# Cache coverage and configured build outputs

## Aim refinement: disk decisions

The user clarified the aim: help a developer understand whether they need the things consuming substantial disk space. Filesystem activity is one source of evidence alongside ownership, contents and recovery cost; exposing a timestamp is a mechanism, not the outcome. Success means a large cache row offers useful evidence for that decision while distinguishing observed metadata from actual build or run history.

## Execute: filesystem activity visibility

Selected approach: retain access timestamps from metadata already read before directory enumeration, select bounded evidence for displayed child directories during observation, persist it with child rows, and show modification age plus explicitly sourced directory access in CLI/TUI reports. Existing real-use probes remain distinct. Assumptions: the mount honors access timestamps, and a developer can interpret weak evidence when its limitations are visible. Directory enumeration, including prior Swamp observations, can refresh directory access time; a recent timestamp therefore cannot establish that a developer still needs the contents. Sampling before the current enumeration avoids manufacturing a new access signal from this scan.

Preserve read-only report behavior, physical accounting, configured project references and recovery evidence. No new recursive walk or script inspection. Each unit performs reliability checks for at most its existing top-N displayed child anchors; the existing walk supplies their access timestamps without extra metadata reads. Stop/pivot if report rendering reads the filesystem, weak access is relabeled last use, or synthetic remainder rows acquire invented activity.

| Risk | Tempting wrong patch | Required check |
| --- | --- | --- |
| Access mistaken for actual use | Populate LastUsed from directory atime, or sample after enumeration | Separate access evidence and enumeration caveat; old-atime fixture preserves the pre-scan timestamp |
| Historical report changes during listing | Stat children in reclaim projection | Persist observation evidence; read-only reconstruction preserves it |
| Extra traversal or invented activity | Walk every file or probe remainder name | Bounded top-N anchors; synthetic rows have no access evidence |
| Mount suppresses access updates | Display arbitrary atime as trustworthy | Existing noatime/relatime reliability refusal retained |
| Empty decision display | Retain only tracking unsupported | Modification and sourced access available independently of use tracking |

## Aim

Explain developer storage outside ~/src through its project references and cache conventions, so disk-pressure investigation finds real build output without scanning arbitrary scripts.

## Problem Space

macOS native ~/Library/Caches is already included. Cross-platform ~/.cache is missing from macOS defaults. Linked Git worktrees outside roots already work. Cargo adapters recognize configured paths but identification receives only worktree measurements. Script-exported overrides cannot be followed from static tool configuration.

## Solution Space

Selected: include the XDG cache root alongside native macOS caches; add generic adapter output declarations, measured through existing external folded observation with project consumer evidence. Start with Cargo, TypeScript and literal Maven POM declarations where supported. Preserve bounded reads, no code execution, event-based reuse, shared-path single accounting and exclusions. Configuration parsers explicitly document their supported subset.

## Plan

- [#225](https://github.com/open-horizon-labs/swamp/issues/225): macOS XDG cache coverage.
- [#226](https://github.com/open-horizon-labs/swamp/issues/226): configured outputs through shared adapter measurement.

## Execute

Execution authorized by user after oh-plan; Luna implementation workers, Sol review and dissent before delivery. Work lives in an isolated managed worktree on codex/cache-configured-builds.

Success: cache roots appear under configured scope; supported configured output references are measured and persisted with consumer evidence; shared output bytes counted once; no scripts read or executed. Existing worktrees, exclusions and read-only report behavior preserved.

Risk checks:

| Risk | Tempting wrong patch | Required evidence |
| --- | --- | --- |
| External paths recognized but unmeasured | Only add adapter containers | End-to-end allocated bytes and interiors outside source root |
| Duplicate accounting | Measure shared output once per project, or cache plus output | Shared consumers and parent cache disjoint totals |
| Exclusions bypassed through references | Follow canonical config paths unconditionally | Excluded output and alias exclusion counterexamples |
| Broad traversal via config | Accept root/self/ancestor output declarations | Reject dangerous references without widening root |
| Stale external builds | Reuse on directory mtime alone | Independent output event coverage, append/change and no-window checks |
| Configuration interpretation guesses | Execute JS/scripts, expand unknown variables | JSONC and inheritance tests; unresolved forms rejected/documented |
| Nested workspace projects missed | Only inspect Git root Cargo.toml | Nested manifest configuration discovered from observed structure |
| Stored report loses new data | Only patch in-memory display | Store round-trip and report read-only checks |

Stop/pivot: duplicate ownership, exclusion bypass, or second recursive traversal invalidates the implementation. Review/dissent route recoverable defects to adjustment; salvage only if repeated reversals show the selected architecture does not fit.

Interim evidence: source audits pass. Nested source declaration transport passes all three cases, including tsconfig-only projects and excluding dependency copies. Shared Cargo/TypeScript symlink consumers, canonical exclusion, nested-output single accounting and retarget tests pass. Sol's independent output-cursor/cost regression passes: 200 output directories are traversed cold and on forced full observation, while a trusted quiet stream costs at most ten directory/file operations with actual reuse. The initial cost fixture incorrectly modeled an immediate persisted log and triggered the existing TooSoon guard; changing the fixture to a drained live stream tests the intended trusted-window branch without relaxing production guards. Append tests now assert unchanged output-directory timestamps, filesystem allocation rather than payload length, trusted victim-only event coverage, forced full and untrusted-window measurements. Stored reference writes and internal-owner notes remain under implementation; the cache exclusion test exposed a parent-cache subtraction gap that must be fixed before completion.

## Review and Dissent (interim)

Sol: Adjust. The selected architecture still explains the evidence; no salvage warranted. Correct effective Cargo precedence, reject false Cargo consumers without a Cargo manifest, avoid Maven fallback through unresolved directory values, authorize configured roots before replay, preserve internal reference visibility without duplicate bytes, and complete persistence. Functional failure would be stale or doubled bytes; adoption failure would be undisclosed ownership gaps; opportunity cost would be extra parser breadth delaying the common measurement seam. Runtime correctness and bounded replay are model-checkable and remain completion gates. The usefulness of the resulting report for a person's actual disk-pressure decision remains human judgment at PR review.

Review counterexamples reproduced two additional common-pipeline gaps: a shared Cargo/TypeScript output performed 402 directory listings and 801 stats on a trusted quiet refresh because adapter identities shared one path-only cache key; an excluded descendant reached through a canonical alias contributed 36,864 bytes instead of the allowed 4,096. These are adjustments within the selected architecture. The completion gate requires a deterministic single interior interpreter with all reference evidence retained, canonical descendant subtraction in folds/history, and filtering excluded identified units. Bounded metadata probes during adapter identification are not a promise of zero access to excluded names; excluded storage must not contribute displayed units, totals or ownership history.

## Final risk evidence and review

The common pipeline now passes the descendant-alias exclusion test live, after an excluded-file change and through a stored report; the Maven exact-file adversary also verifies shallow identification cannot reintroduce an excluded jar. Both Cargo/Maven custom external layouts preserve allocated bytes, interiors and evidence through typed storage. Nested declarations and a linked worktree outside the source root pass all six transport/authorization cases. Cache-parent, canonical aliases, nested outputs, retargeting, independent output events, forced-full/no-window append refresh and stored read-only references pass their edge fixtures.

The shared-output regression passes with one registry-selected interpreter and a stable `configured-build:<adapter>` physical identity. Both project references remain attached, quiet replay avoids member traversal, forced-full pays traversal, and repeated observations refresh derived evidence without accumulating prior facts. Decision evidence identifies the declaring source; multiple references leave universal recovery unknown and caveat the adapter's repair command. Current delivered external interior IDs supersede early project-adapter placeholders before typed persistence, avoiding duplicate stored evidence. The configuration reader is included in adapter source revision hashing so changes invalidate the interior cache.

Sol's frozen-source review and dissent: Continue, conditional on the final check tier. No remaining actionable P1/P2 findings and no salvage warranted: the selected architecture still fits. Named model-checkable risks are retired by the adversarial checks above; report usefulness for a person's actual disk-pressure decision remains a human PR-review checkpoint. Parser breadth remains deliberately bounded and unsupported forms are documented. Formatting, all-target clippy, source audits and release-graph validation pass; final workspace tests and full-tier CI remain pending at this record's current point.

Restart handoff: the first final workspace run stopped at an intermittent shared-output replay cost failure after 1,175 core unit tests passed. Serialized counters ruled out fixture interference. The configured cursor was captured after the project walk, while external caches used the earlier source observation epoch, so a clock-boundary crossing correctly made the cache older than its window. The fix captures one external observation epoch immediately before configured replay and uses it for external measurements and caches; the source report epoch stays intact. This preserves the stored-row/window freshness floor and TooSoon refusal. The cost fixture now forces a two-second source replay delay, asserts the output cursor starts later than the source report, checks two successive quiet refreshes, and retains forced-full traversal. Sol independently reviewed the timestamp consumers and confirmed typed evidence does not require matching the source epoch. The focused replay tests pass; the final check tier is rerunning.

Final local result: `scripts/check.sh` passed on the epoch-fixed tree, including formatting, all-target clippy, source audits, release graph, workspace unit/integration/doc tests, named targets, gate-script tests and forbidden-pattern checks. The 1,175 core unit tests pass, and the forced-boundary replay regression passes both quiet refreshes inside the workspace run. Sol's final epoch review and dissent: Continue / Proceed, with no remaining actionable P1/P2 findings and unchanged freshness/refusal guards. All declared implementation characteristics and model-checkable risk gates are met locally. Full-tier CI will be requested on the PR; human review remains the checkpoint for practical report usefulness and merging.

CI follow-up: both macOS tiers passed. Linux exposed a TypeScript fixture that still compared allocated bytes with its 8 KiB payload length; the measurement contract is physical allocation, which can be smaller. The fixture now syncs its generated file, captures `st_blocks * 512`, and asserts exact allocation. The focused fixture passes and Sol approved the test-only correction; production behavior is unchanged. The Linux archive build/smoke/validation succeeded, but GitHub's artifact service timed out through five upload attempts. New-head CI will cover the corrected fixture and retry that upload independently.

## Execute: cache layout and usage follow-up

Aim: explain real Cargo outputs found inside generic cache roots even when the build's environment override was transient; distinguish unsupported usage tracking from a supported probe with no record. Selected approach extends the adapter/shared measurement seam using observed layout metadata, retaining the cache's physical accounting and no project attribution without evidence. No scripts, project command execution, arbitrary recursive discovery or guessed last-used dates. Existing linked worktrees and configured declarations remain intact. User authorized Luna implementation, Sol review/dissent, local installation and updated observation.

Success: arbitrary-named genuine Cargo cache outputs expose stored build interiors; misleading directory names do not; generic cache children explicitly report unsupported use tracking while supported empty probes retain no-record semantics; changes are installed and verified on the actual cache.

| Risk / stop trigger | Tempting wrong patch | Required adversarial check |
| --- | --- | --- |
| False Cargo classification or ownership | Treat a target suffix as Cargo or source identity | Genuine arbitrary name recognized, misleading name rejected, owner/recovery stays unknown |
| Duplicate physical bytes | Add targets as new measured cache children plus parent totals | Parent and nested interiors reconcile without changing physical totals |
| Exclusions or symlink widening | Inspect or surface excluded targets through aliases | Excluded root/descendant and alias do not contribute units |
| Expensive quiet refresh | Run a second recursive recognizer every observe | Trusted quiet replay bounded independently of interior size |
| Stored report loses interpretation | Only decorate live UI | Read-only typed round-trip preserves interiors and tracking distinction |
| Misleading usage | Map every missing date to unsupported or use mtime | Supported empty probe differs from no applicable source; no invented date |

Stop/pivot if recognition requires scripts, source guesses, another recursive traversal, exclusion bypass, or loss of measurement/replay correctness. Unsupported project association remains accepted: a historical environment setting is unavailable and a name is insufficient ownership evidence.

### Follow-up review and risk evidence

Luna implementation and Sol review/dissent retired the named gates with seven generic-cache integration cases, fourteen build-adapter unit checks, the last-use store round-trip and focused usage/DerivedData tests. Recognition requires a regular Cargo rustc marker plus measured profile/category structure; hyphenated arbitrary target names do not win over metadata. Existing project and configured Cargo identification keeps its earlier semantics. Positive generic roots carry a measured mixed-cache presentation anchor so CLI families are visible, with unsupported whole-root interpretation and no asserted whole-cache rebuild consequence; negative results remain genuinely empty. Generic cache roots retain child drilldown at all sizes.

The stronger cached-negative check found a latent seam defect: typed storage retained empty results, but the cache constructor discarded their keys. The new constructor retains keyed generic empty results and the generic scope is namespaced, preventing a prior empty project adapter result at the same path from being reused. Ordinary marker-selected stores retain their previous nonempty adapter identity guard. Both probe and actual replay use the same keys. Two trusted quiet refreshes remain bounded while a forced full pass traverses more than 200 directories; reported changes refresh negative results, and a pre-feature root without adapter state is re-folded before interpretation.

Configured child overlap, canonical alias exclusion, genuine arbitrary layout versus fake target suffix, unknown source recovery/consumer evidence, CLI visibility and typed stored replay all pass focused checks. Fresh no-source probes persist `unsupported`; supported empty probes and legacy `none` remain `no record`, including Xcode/adapter child cases. Modification time is not converted to use. Formatting, focused Clippy and source audits pass. Sol's final verdict: Continue / Proceed, conditional on the final complete check tier and actual-machine install/observation. No salvage: the selected shared-fold/replay architecture still fits, and fixes address verified seam defects without scripts or another traversal. The Cargo marker/layout heuristic is accepted with rationale as a bounded supported subset; changing Cargo layouts must produce explicit unrecognized contents rather than guessed project ownership.

### Follow-up completion and local delivery

The final `scripts/check.sh` completed successfully on the frozen follow-up source: formatting, all-target Clippy, source audits, release graph, workspace unit/integration/doc tests, named checks and gate scripts. The seven generic-cache and last-use round-trip fixtures passed inside that complete run. Sol review/dissent: Continue / Proceed, no remaining actionable P1/P2 findings.

Installed the release build as `~/.local/bin/swamp` (the `/opt/homebrew/bin/swamp` symlink resolves there), verified matching SHA-256 `054b9990fa13bd85f0bb511357bd5ed44856db6201082232a147691a7b7bbcf0`. Cleaned only task-created Cargo targets: 66,515 files / 26.8 GiB reported for the validation target and 2,374 files / 678.2 MiB for the release target. The user’s caches and projects were not removed.

Actual-machine observation completed in 57 seconds: 50 projects, 85 external units and 4,251 agent units. Stored external JSON contains Cargo dependency interiors for both `~/.cache/tdongle-tailnet-target/debug/deps` and `~/.cache/tdongle-rc-target/debug/deps`, with no declared consumers. Both child rows and the generic cache root persist last-use source `unsupported`; the cache physical total remains 10,705,567,744 bytes. External text exposes identified Cargo families and mixed-cache limits, so positive interpretation reaches the user-facing report. Native cache coverage reports the existing stalled `~/Library/Caches/com.apple.Music` subtree as unmeasured; that gap is explicit and outside this follow-up’s target classification. Local delivery and model-checkable risk gates are complete; practical usefulness of the new wording remains the user’s judgment.

## Review and dissent: filesystem activity

Sol reviewed the source and found no remaining actionable P1/P2 issues, conditional on final validation and the actual-machine observation. Dissent caught and retired the tempting post-scan sampling approach: the existing pre-enumeration metadata now supplies access time. Reliability checks were moved after top-N selection, and access observations retain their original date during quiet and partial replay. CLI/TUI use absolute access dates with source and observation details; the TUI no longer calls access evidence “changed.” Directory atime remains weak evidence that can result from earlier enumeration, so practical need remains a developer judgment alongside ownership, modification, use records and recovery cost.

## Filesystem activity completion and local delivery

The final `scripts/check.sh` passed on the final source at 17:35:27 local time: formatting, all-target Clippy, source audits, release graph, workspace unit/integration/doc tests, named targets, gate scripts and forbidden-pattern checks. The first complete run caught two TUI snapshots where unknown modification labels crowded child names; compact unknown labels were removed, their details remain explicit, and the original golden frames pass without changes. The final run includes the Reclaim CLI text route and its access-versus-use test, future-time refusal, pre-enumeration capture, typed child storage, actual read-only report projection and partial-replay sample-time preservation.

| Named risk | Retirement evidence |
| --- | --- |
| Access conflated with use or created by the current scan | Old-atime walk fixture preserves the pre-listing value; CLI/TUI tests distinguish Accessed, Modified and actual use; real codex-runtimes retains September 8 access rather than today |
| Read-only rendering changes metadata facts | Actual typed Parquet round-trip and report_scope_from_store equality preserve the same evidence; report projection adds no filesystem child probe |
| Unbounded extra traversal or synthetic activity | Existing walk counters remain one root plus one child; source review verifies reliability checks after top-N truncation; 20,000-child fixture produces 15 facts and no remainder fact; real report has 108 access facts and none on synthetic rows |
| Incorrect reliability or future recency | Existing mount noatime/relatime refusal remains; captured future timestamps never become recent-use facts |
| Replay rebrands an old sample as current | Partial-replay test retains both raw atime and original access_observed_at; CLI/TUI show absolute event and original observation dates |
| Practical need inferred from a weak clue | Accepted human decision boundary: prior directory enumeration can refresh access time, and every captured child fact states that limitation. Ownership, modification, use history and recovery remain independent |

Built and atomically installed as `~/.local/bin/swamp`, reached by `/opt/homebrew/bin/swamp`; version remains 0.8.4 and SHA256 is `3e7c819226151a4343a7c531a242615ea517c190de753391842bd08471e73d17`. Task build targets were cleaned before observation: 17.8 GiB debug and 679.7 MiB release, plus the worker's earlier duplicate 5.3 GiB target. No user caches or projects were removed.

The installed `swamp observe --full --verbose` completed successfully in 26 seconds: 50 projects, 81 external units, 4,260 agent units. The generic cache total remains exactly 10,705,567,744 bytes. Reclaim shows tdongle-tailnet-target at 5.2 GB, modified 8h ago, directory access October 7 at 12:40 UTC; codex-runtimes at 1.7 GB, modified 9d ago, directory access September 8 at 04:58 UTC. Details identify pre-enumeration filesystem metadata and the original October 7 21:36 UTC observation, with unsupported tool-use tracking kept distinct. Apple Music's previously stalled native-cache subtree remains explicitly unmeasured. Sol reviewed the installed output and final checks: Continue / Proceed, no remaining actionable P1/P2.

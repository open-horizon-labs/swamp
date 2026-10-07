# Cache coverage and configured build outputs

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

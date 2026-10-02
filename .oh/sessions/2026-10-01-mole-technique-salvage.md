# Mole technique salvage and execution

## Aim

Make swamp quicker and easier to navigate by extracting demonstrated techniques from Mole across scanning, storage, display, interaction and information architecture. The user explicitly requested salvage and execution. This is learning extraction and bounded implementation, not a restart of swamp or a wholesale port.

## Salvage

**Salvaged:** Mole upstream `c430bac637929ebede043df81d3bef319428309c` and the user's `muness/mole` integrate fork `6e8a748b34e55b993b554ba8ca9e231f5259491c`, compared with swamp `8297c6767a9e108cd10b0168797590253c62f325`.

**Reason:** Prior work salvaged scanning/history mechanics from the fork but left its interface behind. The user's recent real cleanup flow exposed both expensive preparation and a confirmation that could not fit. Those defects are fixed in PR #216; this pass examines what the adjacent product can still teach us without mistaking already adopted mechanisms for new work.

**Frame shift:** Similar tasks can benefit from shared interaction techniques even when their measurement and action policies differ. Extract mechanisms and failure cases; implement against swamp's current contracts and actual measurements. No upstream source code is copied.

Existing founding salvage, the September 19 upstream salvage, `manage-ecosystems-not-filesystem-audits`, and the relevant `.oh/guardrails` were read first. Repeated rules belong there; this session adds source-specific findings and verification, not parallel generic guardrails.

### Techniques and transfer decisions

| Area | Source-grounded technique | Decision for swamp |
|---|---|---|
| Scanning | Separate bounded pools for directory entries, `du`, and fallback walks ([upstream scanner](https://github.com/tw93/Mole/blob/c430bac637929ebede043df81d3bef319428309c/cmd/analyze/scanner.go#L112)); graft unchanged subtrees from stored rollups ([fork walker](https://github.com/muness/mole/blob/6e8a748b34e55b993b554ba8ca9e231f5259491c/internal/inventory/walk.go#L131)). | Already represented by swamp's bounded folded traversal and FSEvents-gated reuse (`walk.rs`, `growth.rs`). No concurrency or scan-policy change is justified by this comparison. Parent directory mtimes alone miss in-place file changes; keep event-coverage requirements. |
| Storage | Current state plus reverse deltas, with a fixed observation heartbeat for unchanged passes ([fork inventory contract](https://github.com/muness/mole/blob/6e8a748b34e55b993b554ba8ca9e231f5259491c/internal/inventory/types.go#L1), [heartbeat](https://github.com/muness/mole/blob/6e8a748b34e55b993b554ba8ca9e231f5259491c/internal/inventory/observation.go#L11)). | Already present in swamp's current and reverse-delta Parquet tables. Keep measurement freshness distinct from last change. Do not add the fork's JSON-header/compressed-gob store alongside Parquet. |
| Storage footprint | Admit expensive-to-recompute directory results and cap the cache: 100 files / 10 MiB thresholds, 5,000 entries / 50 MiB caps ([upstream constants](https://github.com/tw93/Mole/blob/c430bac637929ebede043df81d3bef319428309c/cmd/analyze/constants.go#L28)). Comments report an earlier 1.88-million-file, 7.82-GB cache; this is upstream's account, not our measurement. | Useful failure case: a tiny result per file can cost much more in filesystem blocks and inodes than its payload. Swamp already uses shared columnar tables and includes small-file bytes in directory rollups while omitting individual small-file rows. No per-directory cache is added. |
| Display | Format `[offset, offset + viewport)` instead of every row ([upstream view](https://github.com/tw93/Mole/blob/c430bac637929ebede043df81d3bef319428309c/cmd/analyze/view.go#L297)). | Implemented in `draw_body`: preserve the full row model and global scale, format only on-screen rows. Measured below against a copied real observation store. |
| UI and UX | Preserve selected path through sorting, and save cursor plus scroll offset in navigation history ([sorting](https://github.com/tw93/Mole/blob/c430bac637929ebede043df81d3bef319428309c/cmd/analyze/update.go#L250), [history](https://github.com/tw93/Mole/blob/c430bac637929ebede043df81d3bef319428309c/cmd/analyze/cache.go#L47)). | Implemented using swamp's unit, aggregate and project identities. Sort/reverse and view return keep the same item selected; missing items fall back to a clamped position. Left moves through the hierarchy instead of leaving a leaf directly for Projects. |
| Responsive UI | Recompute page height and drop footer hints by priority, preserving primary actions ([pagination](https://github.com/tw93/Mole/blob/c430bac637929ebede043df81d3bef319428309c/lib/ui/menu_paginated.sh#L505), [footer](https://github.com/tw93/Mole/blob/c430bac637929ebede043df81d3bef319428309c/lib/ui/menu_paginated.sh#L566)). | Already present in swamp. Preserve contextual footer priorities and resize behavior with terminal-frame tests; no new pagination abstraction. |
| Information architecture | Present a navigable hierarchy with a visible current location and a reliable way back; separate selection identity from display ordering. | Apply the hierarchy behavior to swamp's existing Projects / Tools / Disk sections. Retain its owner, regeneration-cost, and evidence views. A broad Mac-maintenance command catalog would change the product's scope rather than clarify those tasks. |

### Counterevidence and guardrails retained

The fork's legacy analyzer cache is schema 4 and lacks the upstream schema 6 partial/refresh fields ([fork cache](https://github.com/muness/mole/blob/6e8a748b34e55b993b554ba8ca9e231f5259491c/cmd/analyze/cache.go#L25), [upstream cache](https://github.com/tw93/Mole/blob/c430bac637929ebede043df81d3bef319428309c/cmd/analyze/cache.go#L25)). Its newer inventory does not repair that separate legacy path. Do not treat a fork's newer feature as evidence that all older paths are stronger.

Mole's live `du`, process checks and product-specific cleanup catalog answer different questions from swamp's persisted accounting. They do not establish that bytes are unused or that moving an allocation frees space. Swamp retains declared roots, typed unknown/partial measurements, its own recovery ledger, and one Trash backend. The fork's successful-removal delta is a useful stale-inventory test question, but this comparison established no missing swamp behavior that would justify a second persistence path.

Both source snapshots are GPL-3.0. This work extracts general mechanisms and failure cases and implements them against swamp's Rust model; no source code is copied. The snapshots have separate Git histories and were inspected as pinned trees, not compared as branches sharing a base.

### Context for the next pass

The initial interface review concentrated on Mole's Bash selector, which eagerly formats candidates; its Go analyzer contains the useful viewport implementation. Inspect the corresponding execution path before recommending a transfer. Future scan or storage work should start from a measured swamp bottleneck or accounting failure. Future display work should measure full row derivation separately from terminal formatting: the large expanded tree still costs about 90 ms after this change.

## Solution Space

Chosen: viewport-only row formatting and selection/navigation continuity. These are reversible changes inside the TUI and are supported by inspected upstream code and current swamp evidence. The real copied store has 42 project rows, an explicitly fully expanded 14,119-row swamp tree, 6,311 agent rows, and 89 Reclaim rows. Release TestBackend redraw medians at 80×24 over 25 iterations: Projects 266 µs, Tree 139,602 µs, Agents 19,301 µs, Reclaim 735 µs. Every measured render made zero directory listings, file stats and subprocesses. This is rendering cost, not terminal transport latency or observation time. Baseline: `/Users/Shared/swamp-mole-salvage/render-baseline.log`.

Mole's Go analyzer bounds formatting to `[offset, offset + viewport)` in `cmd/analyze/view.go:297-303`. Its `sortLiveEntriesForActiveMode` saves and restores the selected path (`cmd/analyze/update.go:250-279`), and navigation snapshots retain the selected row and offset (`cmd/analyze/cache.go:47-62`, `update.go:1087` onward). Swamp formats every row then slices the formatted result, saves view cursors as numeric positions, and leaves a tree leaf by jumping to Projects. The selected changes address those concrete gaps.

The existing folded scans, event-gated reuse, bounded workers, no-follow traversal, compact Parquet/current-plus-reverse-delta history, and stored-report purity remain in place. No schema change, new persistent cache, cleaner catalog, blanket deletion policy, scan-triggering navigation, or performance claim from upstream timings is part of this execution.

## Execute

Task: apply the measured display optimization and predictable navigation. Success: reduce the expensive real redraws, retain identical terminal frames for existing states, preserve row identity across sorting/view return, and make Left move to the actual parent before leaving a hierarchy. The scope is TUI computation and navigation plus source-grounded salvage documentation. Existing byte accounting, source facts, cleanup authorization and storage history are preservation constraints.

| Risk / assumption | Tempting wrong patch | Required check | Status |
|---|---|---|---|
| Off-screen work dominates formatting cost | Cap the underlying data to the first N rows | Before/after real-store release replay; `page_and_home_end_keys_move_the_list` reaches the last row at four terminal sizes | Retired by evidence |
| Only displayed formatting should change | Rescale growth bars to the visible slice, or lose headers/details on scroll | Existing golden frames remain byte-identical; `scrolled_growth_bars_keep_the_offscreen_maximum` and resize/End regression pass | Retired by evidence |
| Selection belongs to the same item | Keep only the numeric index after sort or background reorder, or treat repeated labels/project names as identity | App regressions cover sort/reverse, away/back reorder, removal, duplicate aggregate labels and same-name projects; ambiguous identities use the documented index fallback | Retired by evidence |
| Left means one level out | Jump from any leaf straight to Projects, or select a sibling in a different root | `left_from_tree_child_selects_parent_then_collapses_then_exits`, `left_in_second_worktree_stays_with_that_worktree`, and root-boundary regression pass | Retired by evidence |
| Browsing remains a stored read | Add on-navigation live scans or a second persisted display cache | Real-store draws and fixture navigation report zero directory listings, file stats and probes; source-audit gates pass. Existing asynchronous UI preference writes remain separate from observation reads | Retired by evidence |
| Existing removal behavior remains intact | Reuse cursor restoration to change marks or bypass confirmation | All 305 TUI tests pass, including marks, confirmation, modal key handling, recovery and tool-removal regressions | Retired by evidence |
| Upstream scan/storage changes improve current workload | Copy higher concurrency or stale-cache heuristics without matching semantics | Pinned source comparison found these mechanisms already present; no measured missing scan/storage technique was established, so no such production change was made | Retired by evidence |

Invalidated if reduced draw work changes row content/accounting or navigation changes the action target without preserving identity. Stop/pivot if a fast path needs persistent data duplication, weaker event coverage, altered destructive permissions, or an unmeasured scanner change. Human preference for the feel of navigation will remain a user judgment; mechanical navigation and rendering behavior are independently testable.

### Measured display result

The retained ignored test `crates/tui/tests/render_cost.rs` loads the copied real store before timing, warms each view, then measures 25 release-mode redraws through Ratatui's TestBackend at 80×24. It asserts zero directory listings, file stats and subprocess launches inside the draw loop. Exact microseconds are retained here for reproducibility; user-facing summaries round them to milliseconds.

| Stored view | Rows | Before median / p95 | After median / p95 |
|---|---:|---:|---:|
| Projects | 42 | 266 / 325 µs | 241 / 338 µs |
| Fully expanded swamp tree | 14,119 | 139,602 / 145,852 µs | 90,075 / 91,785 µs |
| Agents | 6,311 | 19,301 / 19,901 µs | 3,239 / 3,327 µs |
| Reclaim | 89 | 735 / 843 µs | 608 / 658 µs |

The large-list improvements are about 35% for the expanded tree and 83% for Agents. The small-list differences are too small to present as a general performance result. The tree case is deliberately fully expanded, not the initial collapsed screen. Full row derivation still runs each redraw; the change removes off-screen formatting and mark-decoration work. These are local rendering measurements, not scan timings, input latency, terminal-transport timings, or a speed guarantee.

Reproduce with `SWAMP_RENDER_READONLY_STORE=/path/to/copied/store cargo test -p swamp-tui --release --locked --test render_cost -- --ignored --nocapture --test-threads=1`. The measured copy was `/Users/Shared/swamp-perf-sprint/accounting-check-store`; raw logs are `/Users/Shared/swamp-mole-salvage/render-baseline.log` and `render-after.log`. No observation or deletion was run for this measurement.

### Review and ownership

The root agent selected scope, implemented viewport formatting, measured the real store, and owns integration. The scanning/storage agent compared source only. The navigation agent implemented selection and hierarchy behavior. The dissent agent independently reviewed upstream policy differences and both production diffs; no blocker remains. A proposed concern about depth-one worktree roots was tested against the reverse parent search and withdrawn; an explicit two-worktree regression documents the behavior. Parent-directory `sg` and `ba` commands are unavailable on this host, so the review used the source-grounded dissent agent and this checked-in session record.

Drift check: aligned. The implementation remains inside TUI display/navigation. Scanning, storage, accounting, action permissions, and the existing section architecture are unchanged. No stop/pivot condition was triggered. The remaining expanded-tree row-derivation cost is recorded as a separate future measurement target, not hidden behind the formatting improvement.

An identity review found that separate containers can contain equally named residual rows without an underlying unit or expansion key. Cursor keys therefore use unit paths, expansion keys, project IDs when the name maps unambiguously to one project, then labels only when unique among unkeyed rows. Ambiguous structural rows use the clamped index fallback. Synthesizing an identity from parent names and sibling ordinals was rejected: those ordinals would still represent position, and recursive matching could make large-list navigation quadratic. Final key construction is linear; duplicate labels and duplicate project names have regressions.


### Execution complete

**Aim achieved:** measured large-list formatting work was reduced, navigation preserves known row identities and scroll positions, and Left follows the existing hierarchy. Scanning/storage findings are retained with pinned evidence and explicit transfer decisions.

**Delivered characteristics:** viewport-only formatting, identity-aware sort/view return, one-level-out navigation, reproducible read-only rendering replay, and updated usage/design/changelog documentation. No new persisted display cache or schema was introduced.

**Verification:** the workspace check passed CLI, core, harvest and source-audit tests before reaching one new TUI fixture failure: its maximum had not actually been placed off screen. The fixture was corrected with an explicit size sort. After that correction and the final project-name guard, the entire TUI suite passed (305 tests; two intentionally ignored harnesses), all workspace documentation tests passed, and `scripts/check.sh` with `SWAMP_CHECK_SKIP=tests` passed formatting, workspace/all-target Clippy, source audits, release-graph checks, named-target checks, gate-script tests and grep gates. Unchanged non-TUI tests were not repeated. Logs: `/Users/Shared/swamp-mole-salvage/check.log`, `tui-final.log`, `doc-tests.log`, and `static-final.log`. The real-store ignored render harness was run separately for the measurements above.

**Needs human verification:** whether the navigation feels better in daily use. This is a preference judgment; mechanical behavior, layout preservation, deep scrolling and stored-read behavior are covered by tests. Ambiguous informational rows intentionally restore position rather than asserting an identity they do not have. No live deletion was needed to verify this TUI-only change.

# Clarify and simplify the terminal views

## Aim

Help a developer understand the current view, compare storage, and find the next useful action without reading repeated bookkeeping. The user requested Impeccable clarify/simplify for every view and the top panel, and Execute for all of it. Refine the existing terminal interface; preserve its measurement and action contracts.

## Problem statement and direction

Committed 80×24 frames show repeated totals and observation ages, navigation instructions repeated across three lines, full filters displayed on views that ignore them, and row names dominated by long paths and internal enum names. Reclaim spends narrow-screen width on empty change values while regeneration cost is clipped in details. The wide top panel adds two more lines of global breakdown and pointers even when the user is already in the destination view.

Each of the thirteen views gets a plain title, a short purpose, appropriate column headings, compact labels, and useful empty states. The global panel keeps its developer-storage total and coverage/measurement limitations; detailed allocation breakdown belongs in Disk. Selection details retain exact paths and decision evidence. Reclaim prioritizes removal cost; Disk views omit meaningless growth columns. Sections, key bindings, measurements, filtering behavior, actions, and stored-read behavior remain unchanged.

## Execute

Success means all thirteen views are clearer at 80×24 and 200×60, the top panel is quieter, no global filter is presented as active where it is ignored, and no measurement caveat or removal fact is weakened. Implementation is presentation-only. Stop if simplification would change accounting, conceal unknown coverage, bypass review, add live I/O, or require new scan/storage architecture.

| Risk | Tempting wrong patch | Check | Status |
|---|---|---|---|
| Simpler totals conceal coverage | Drop all warnings with secondary summary text | Missing/unreadable/future ledger, previous scope, explicit root, spot-audit discrepancy and unmeasured frame assertions | Retired: all-view headline state matrix and zero-used regression; coverage limitations remain named. |
| Filter line overstates scope | Print the saved filter on every view, or pretend all predicates apply to aggregates | View-specific filter assertions, including Kinds and Ecosystems predicate subsets | Retired: unsupported-view and aggregate predicate projection checks; ignored project/build predicates explicitly labelled. |
| Short labels lose identity | Replace paths with basenames and discard the source | Exact unit IDs unchanged; full-path selected details and action regressions | Retired: exact UnitId retained; action fixtures now select by identity, not display prose; full-path detail test. |
| Short details hide consequences | Put metadata before in-use, unique-data or shared-byte facts | Evidence-priority tests and narrow selected-row frames | Retired: four-line evidence priority checks, model identity frames and simulator dual-removal-path regression. |
| Cleaner columns distort numbers | Hide unknowns as zero, drop accounting legends or rescale bars | Existing coverage/model frames, allocated-size caveats and global growth-scale test | Retired: unreadable rows retain not-read; zero/unknown distinction, allocation legend and global growth-scale tests. |
| Layout shifts or blocks actions | Resize chrome by state; replace the confirmation contract | All-view/size matrix, modal tests, stable chrome and scrolling checks | Retired: fixed chrome, keymap/modal/scrolling matrix and context-sensitive footer tests. |
| Presentation starts scanning | Recompute facts while rendering labels/details | Scoped work counters and source audits | Retired: all-view scoped work counters; copied real-store replay; source audit has no event-thread gate calls. |
| Cosmetic loop grows without an endpoint | Repeated small style edits | One batched visual inspection, one correction batch and one confirmation inspection | Retired: one batched fixture/real-store inspection, one correction batch, one final confirmation; subsequent test fixes preserve existing contracts. |

Impeccable context was already loaded this session; its terminal surface brief, current DESIGN.md, clarify/distill/operate references and craft floor were read. The recorded web platform is a schema workaround: Ratatui frames are the visual authority. Existing context-sidecar drift is outside this task. Root owns layout and integration, code agents own row/detail wording and view metadata, and a separate reviewer checks fact preservation.

## Delivered views

| View | Clarification |
|---|---|
| Projects | Project heading, no repeated global totals, project-level filter limitations explicit. |
| Project folders | Selected project in the title; readable checkout and artifact labels; Esc route retained. |
| Build outputs | Build output heading; existing cleanup consequences retain space at narrow widths. |
| Dependencies | Clear folder-kind names and dependency heading; unknown recovery and consumer facts take priority. |
| Ecosystems | Ecosystem heading; only ecosystem predicates presented as applied. |
| Storage kinds | Human category names; only kind predicates presented as applied. |
| Unassigned | Plain reasons distinguish no association, outside a checkout, and uncertain evidence. |
| Reclaim | Regeneration cost beside size at 80 columns; removal route and sourced last-use retained; concise consumer coverage and report pointer. |
| Docker | Docker object heading and Object ID detail label; permanent removal review unchanged. |
| Tool storage | Detector-first labels, exact selected paths and source/consumer metadata retained. |
| Agent storage | Tool/category/project/item labels; inferred project relationships remain explicitly inferred. |
| Disk usage | Short allocation labels with explanations in details; no meaningless Change column; unreadable remains nonnumeric. |
| Coverage gaps | Short group labels, named unreadable paths and outside-developer-storage groups; no Change column. |

## Verification and boundaries

Local evidence lives under `/Users/Shared/swamp-tui-clarity`: full TUI run in `final-tests.log` plus the corrected disk-empty-state rerun in `final-g4b-tests.log` and row-reuse regression run in `row-reuse-tests.log`, source/Clippy/static gate results in `checks.log`, real-store replay in `real-render-final.log`, and compact/wide captures in `confirmation` and `real-confirmation`. The copied observation store contains 42 projects, 14,119 Project folders rows, 6,311 Agent storage rows and 89 Reclaim rows. Rendering is measured separately from opening the stored report.

`./scripts/check.sh` ran formatting, workspace Clippy, source audits, release-graph check, named targets and gate-script tests with `SWAMP_CHECK_SKIP=tests`; the changed TUI package has its own complete test run. Non-TUI implementation is unchanged. The parent commit's native macOS and Linux CI run 36960980722 passed; it is baseline evidence, not a claim that this branch's new CI passed.

The Impeccable manual detector returned `[]` for the Rust UI targets. That detector is not terminal-layout evidence; Ratatui buffers and interaction tests are the applicable checks. No current critique-storage snapshot existed. The existing Impeccable sidecar/build-path drift was left alone, as recorded at context load. No merge, release tag, public announcement, data removal, or scanner/store change is part of this refinement.

Drift check: aligned. A fixed four-row detail pane necessarily prioritizes facts; the complete report and action inventory retain supporting evidence and full action paths. This work preserves that interface instead of introducing another inspection mode. The review found compact consumer coverage clipping and the first test run caught hidden model identity/removal routes; all were corrected before acceptance.

The final full TUI run passed 312 tests and caught one obsolete disk-empty-state copy assertion. The missing read-only explanation was restored without repeating the measurement command; all 19 tests in that target then passed. Across the package, 313 tests pass; two opt-in tests remain ignored by default. Real-store rendering was run explicitly. The replay also caught a new footer calling `app.rows()` again; the body and footer now share the same rows, and the 95-test frame/keymap/headline subset passes without updating frames. This is a measured regression repair within the presentation scope.

Final copied-store replay after row reuse (debug build, 25 draws per view): Projects 1.3 ms, Project folders 155 ms for 14,119 rows, Agent storage 6.6 ms for 6,311 rows, Reclaim 1.7 ms. Every measured draw recorded zero directory listings, file stats and subprocesses. The redundant-row version measured 289 ms for Project folders and 12.5 ms for Agent storage immediately before the fix. These diagnose this change on the current machine; they are not release timing guarantees or a controlled comparison with the parent commit.

The final confirmation covered all thirteen compact/wide real-store views plus representative empty/ledger fixtures. There are no further visual revision rounds. The narrow consumer coverage pointer, model identity, simulator removal route, unreadable numerics and fixed navigation positions were confirmed. The package has 313 passing tests after the focused correction, with explicit real-store replay in addition.

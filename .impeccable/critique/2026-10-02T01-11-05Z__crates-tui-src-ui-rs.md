---
target: all CLI commands and TUI views
total_score: 32
max_score: 40
na_heuristics: 
p0_count: 0
p1_count: 2
target_identity: "file:/Users/muness1/src/open-horizon-labs/swamp-perf-sprint/crates/tui/src/ui.rs"
target_fingerprint: "sha256:3663ab79442fbafb42edcbaeb794e6bb5541c24af17acbfecee642947761da86"
target_path: /Users/muness1/src/open-horizon-labs/swamp-perf-sprint/crates/tui/src/ui.rs
timestamp: 2026-10-02T01-11-05Z
slug: crates-tui-src-ui-rs
closed: true
---
# Impeccable Assessment A — Swamp CLI and TUI

Scope: source and committed terminal-frame review of the CLI and ratatui TUI in `/Users/muness1/src/open-horizon-labs/swamp-perf-sprint`. This is Assessment A only; no detector/browser assessment or Assessment B results were consulted. No live GUI interaction or store mutation was performed. Evidence includes `DESIGN.md`, `.impeccable/surfaces/tui.md`, CLI/TUI source, and committed 80×24/200×60 text frames under `crates/tui/tests/frames/`.

## Heuristic scores

Scores are 0–4 each, where 4 is strongest. These are for the combined CLI/TUI product surface, with terminal accessibility considered from source and frames rather than a live screen-reader test.

| Heuristic | Score | Evidence and rationale |
|---|---:|---|
| Visibility of system status | 2/4 | Strong stored-observation age, scope/coverage, stale-estimate, action, and refusal facts. However, progress bytes come from process-global per-walk counters that reset per top-level root. `ui.rs` appends “N roots” to the current byte count and CLI progress has no per-root label, so multi-root `observe` can make “seen” totals jump backwards or appear cumulative when they are not. |
| Match between system and real world | 3/4 | Names like Projects, Reclaim, Disk, Trash, “frees when Trash is emptied,” and “for good, no Trash” map well to user outcomes. Some specialized terms remain (“unowned,” “coverage,” “regeneration,” “allocated,” “FSEvents”), but details and docs explain many of them. |
| User control and freedom | 4/4 | Escape/back, keep/unmark, filter clear, cancellation between units, and human confirmation are present. Filesystem deletions go to Trash; Docker’s irreversible path is called out. The design intentionally keeps deletion out of CLI. |
| Consistency and standards | 3/4 | Shared report model, coherent keyboard navigation, consistent row/detail language, predictable `report --view` names, and stable terminal layouts. CLI options are extensive and partly overlapping (`--kinds`, `--docker`, `--view`, `--dirs`, `--project`, `--worktree`), so users must learn a large option grammar. |
| Error prevention | 4/4 | Explicit confirmation, exact scope/size, distinct irreversible Docker warning, protected paths, refusal reasons, stale-plan rechecks, and refusal when required warning content cannot fit. The UI errs on the side of blocking confirmation rather than hiding plan facts. |
| Recognition rather than recall | 3/4 | Visible section names, current-view label, footer keys, contextual details, empty-filter recovery, and 109-line paginated help make features discoverable. The screen still depends on keyboard conventions, and a single help overlay is long enough to require paging. |
| Flexibility and efficiency | 4/4 | Keyboard-first navigation, filters/sorts, bulk marking, JSON pagination, explicit scopes, schedules, and machine-readable outputs serve both interactive and scripted users. |
| Aesthetic and minimalist design | 3/4 | Strong terminal-cell alignment, hierarchy, restrained color semantics, and compact row facts. At 80×24 the fixed headline plus navigation leaves little table space; confirmation sheets can spend several rows blank for short plans and become a dense text wall for long plans. |
| Help users recognize, diagnose, and recover from errors | 3/4 | Refusal frames explain why and show the relevant row; empty filters show exact expression and how to change/clear it; scope and coverage notes expose incomplete observation. Some recovery still requires discovering `?`, a particular key, or the CLI command. |
| Help and documentation | 3/4 | In-app help, usage docs, trust model, implementation limits, and detailed design docs exist. Help is comprehensive but 109 lines in a paginated modal; it is organized by controls rather than user task, and some technical concepts are easier to resolve in external docs. |
| **Total** | **32/40** | **Good foundation; fix the status truthfulness issue first and improve review-sheet scanning without weakening safety.** |

## What is working

- The product separates observation (`observe`) from stored-fact reading (`report`), and repeatedly tells the user which path walks or runs processes. That is a strong trust boundary for a storage tool.
- The TUI keeps project/worktree context through drilldown, aligns byte and growth columns, and explains accounting and action consequences near the selected row.
- Destructive-action safeguards are unusually explicit: Trash versus permanent Docker removal, exact path/size, warnings and recovery facts, and revalidation before commit are visible in source and confirmation fixtures.
- Empty-filter and refusal states preserve the user’s context and name a next step. Help documents bulk behavior and important exceptions.
- The CLI’s JSON and bounded pagination support automation without turning the interactive interface into a shell-only product.

## Priority findings

### P1 — Multi-root observe progress presents a per-root counter like a run total

Evidence: `crates/core/src/walk.rs` owns process-global progress counters; the outer walk lifecycle resets them between independent root walks. `crates/tui/src/ui.rs` reads `progress::snapshot()` and renders `N GB seen · N roots`. `crates/cli/src/main.rs::spawn_progress_line` also reads the same counters but does not label them as the current root. In a multi-root scan, the byte/directory count can drop at a root boundary even while the run is progressing. The TUI’s “N roots” suffix makes the number especially easy to read as an aggregate.

Fix: maintain explicit run-level completed bytes/directories plus current-root bytes/directories, or label the displayed number as “current root” and show a root index/name. Keep elapsed time run-wide. Test a two-root run where the second root is smaller and assert the run total is monotonic while current-root progress may reset. Cover both CLI progress and TUI header.

### P1 — The Trash plan has poor information density at 80×24

Evidence: `confirm_80x24.txt` shows a one-item, three-line plan in a fixed-height box with five blank rows, while the active tree remains visible underneath and the bottom adds another summary/footer. Conversely, `reclaim_lines` deliberately emits exact paths plus per-path facts and warnings; `confirm_fits` blocks Enter when all lines do not fit. This protects users from hidden warnings, but long plans become a wall of similarly styled lines and can force a resize or smaller selection. In `reclaim_80x24.txt`, selected-row details are visibly clipped (“last use…”), while the confirm path will need much more detail than this view.

Fix: make the sheet content-sized within a maximum height, reclaim unused blank rows, and visually group the decision-critical order: destination/reversibility, count+bytes, exact selected paths, then warnings/facts. For long plans, provide explicit paging/scrolling and keep Enter unavailable until every required warning has been visited or the full plan is otherwise reviewable. Do not silently collapse or omit a warning. Add narrow-terminal frames for a short plan, a multi-path warning-heavy Trash plan, and mixed Trash/Docker plan.

### P2 — Narrow default TUI spends most of its 24 rows on fixed chrome

Evidence: `projects_80x24.txt` reserves five headline rows, two navigation rows, column header, and footer; two visible projects leave a large blank table area. On real datasets the same fixed chrome reduces the number of visible data rows. The design intentionally stabilizes table position, but the tradeoff is severe at short terminal heights.

Fix: collapse secondary headline clauses by default below 30 rows and expose them through a detail/help affordance, while preserving the activity chip, scope, warning, and action safety facts. Make the visible table viewport the primary budget. Add 80×18/80×24 frames with a populated long list, a warning, and an active observation to ensure the layout remains legible.

### P2 — In-app help is comprehensive but too large to scan as a first-stop aid

Evidence: `help_80x24.txt` says “1-22 of 109” and the page spends much of its area on the detailed `Space`/`Backspace` behavior. Paging is available, but help is organized by keys and a single overlay rather than common tasks.

Fix: put a short “Find growth / understand a row / review cleanup / protect a path / refresh” task index first, then link each task to the existing detailed key reference. Keep the key table available for experts. A short first page should answer how to inspect, mark, cancel, and confirm without reading 109 lines.

### P2 — CLI’s command surface is broad and report options overlap

Evidence: `crates/cli/src/main.rs` exposes `inspect-cargo`, `ui`, `scan`, `report`, `observe`, `collect`, `schedule`, `config`, `scope`, and `protect`. `report` combines named views, legacy `--kinds`/`--docker` aliases, `--project`, `--worktree`, `--dirs`/`--depth`, filtering, sorting, JSON pagination, unit pagination, and verification output. The implementation has explanatory clap docs, but the large option set makes it hard to predict valid combinations from `--help` alone.

Fix: keep current flags backward-compatible, but reorganize help into task-oriented examples (“refresh facts,” “read last observation,” “inspect one project,” “script a page,” “schedule,” “explain scope,” “protect a path”). Validate contradictory combinations early and phrase errors with the closest valid invocation. Avoid adding another alias; direct users toward `--view`.

## CLI surface inventory and focused critique

| Command | Purpose / evidence | Assessment and targeted improvement |
|---|---|---|
| `ui [root]` (also default) | Opens stored report, scans only on first run or explicit refresh; `R` refreshes. | Good direct path into the product. Clarify on startup when the report is old and whether a background observation is already running; avoid implying “observed” means current. |
| `scan [root] [--store]` | Legacy/direct scan path. | Its position next to `observe` risks a first-timer choosing a lower-level or differently scoped path. Help should say when to use `scan` versus the normal multi-root/history-producing `observe`. |
| `observe [roots]` | Walks scope and writes history; flags `--full`, `--docker-facts`, `--verify-du`, `--since`, `--no-enrich`, `--enrich`, `--volume`. | Central workflow; see P1 progress label. `--verify-du` and `--volume` are clearly slower/explicit. Make root-scope and per-root progress distinction explicit. |
| `report [roots]` | Stored, no-walk report. Named views, drilldown, sorting/filtering, text/JSON pagination, unowned Docker. | Strong stored-read contract. See P2 option density. Put “does not refresh; run observe” prominently in short help, not only long description. |
| `inspect-cargo <profile>` | Bounded existing-profile inspection; does not run Cargo; JSON/max-entry/time bounds. | Good bounded promise. Help should state the profile path examples and what a time/entry bound means for partial results. |
| `collect [roots]` | Linux foreground inotify collection; `--status`/JSON. | Clear opt-in distinction and platform limits. Surface that this is a continuing foreground process and how to stop it in concise help. |
| `schedule` | Install/replace/remove per-user LaunchAgent or systemd timer; optional collector. | Good lifecycle coverage. Make effective roots and interval visible after installation and make `--off` precedence/invalid combinations explicit. |
| `config show` | Effective config and declared roots. | Useful overview. Separate defaults from user-set values visually so users can tell what they changed. |
| `config path` | Prints config location. | Simple and clear. |
| `config init` | Writes a fully commented config, never overwrites. | Good error prevention. State how to recover when file already exists (use `config show`, edit path, or change individual keys). |
| `config add-root <path>` | Add source root; `--allow-missing` supports absent mount. | Safe nested-root behavior is documented. Ensure output distinguishes “added,” “already covered,” and “missing but recorded.” |
| `config remove-root <path>` | Remove declared root. | Good narrow operation; show remaining effective coverage or point to `scope` immediately. |
| `config set <key> <value>` | Validates known key/value and preserves other TOML/comments atomically. | Powerful but key/value typing is opaque. Include accepted values inline in errors and link `config list`. |
| `config get <key>` | One effective value. | Clear; distinguish defaulted from explicitly configured if the source is not obvious. |
| `config list` | Every writable key, current effective value and meaning. | Strong discovery surface; ensure output stays compact and examples fit common terminals. |
| `scope [roots]` | Effective roots, status, reason, detector catalog; JSON and `--verbose`. | Strong explainability, but full detector catalog may overwhelm. Keep a summary first and explain why disabled/unresolved entries matter. |
| `protect add <path>` | Add human keep intent for agent-storage path. | Important safety feature. Confirm the normalized protected path in output and explain how it affects marking. |
| `protect remove <path>` | Remove human keep intent. | Make clear this changes only protection, not files or stored observation. |
| `protect list [--json]` | List keep intents. | Useful recognition and audit. Empty state should explain how to add one. |

`report --view` inventory (root view names): `worktrees`, `builds`, `deps`, `docker`, `kinds`, `unowned`, `reconciliation`, `types`, `rust`, `projects` (JSON-only), `grown` (JSON-only), `external`, `reclaim`, and `disk`. With `--project`, `worktrees` is the project tree drill. Targeted improvements: expose the same one-sentence view description in CLI `--help`; clearly tag JSON-only views before a user tries text mode; make read-only/removal capability visible in each view description. `--dirs` is a separate text-only directory-growth rendering, not a named view, and should be presented as such.

## TUI view inventory and focused critique

Sections are Projects, Tools, and Disk. `Tab`/`Shift-Tab`, `1`/`2`/`3`, and `v` provide predictable section/view movement. The thirteen views are all reachable; the UI model documents this same set. Each retains the common row/detail/action model.

| View | Assessment and targeted improvement |
|---|---|
| Projects | Strong overview of size and growth, but initial filter `growth > 100MB in 7d` can hide smaller/unchanged projects. Keep the visible empty-filter recovery; on first launch make the active filter’s effect unmistakable and offer a one-key clear. |
| Tree | Preserves project → worktree → artifact hierarchy and selected-row detail. Confirm overlay can cover active rows; make overlay selection context persistent in the plan header. |
| Builds | Purpose and consequence-led groups are well-designed. The many role groups can be hard to compare; keep advice first and provide a compact group summary before expanding. |
| Deps | Good project-local dependency context. Clarify whether a row is a local installation or shared store before marking, especially when cleanup support differs. |
| Docker | Strongly differentiates daemon-owned removal from Trash. Keep “for good” adjacent to each selected object in the confirmation, not only the summary. |
| Kinds | Useful cross-project rollup. “Kinds” is technical shorthand; consider “By folder type” in the visible title while preserving key/CLI compatibility. |
| Unowned | Important boundary for storage without project attribution. Make “unowned means not linked, not necessarily unused” visible in the first explanatory detail. |
| Types | Ecosystem rollup useful for broad comparison. Ensure it does not look like a distinct byte total; label as a grouping of existing storage. |
| External | Valuable shared-tool storage perspective. Keep consumer evidence age/scope close to the row and distinguish manager-aware refresh/removal from ordinary Trash. |
| Reclaim | Best decision-oriented details (cost, last use, consumers, removal path). The 80×24 `reclaim` frame’s detail pane clips “last use…” and leaves much unused body; let detail text wrap/scroll or use a dedicated detail page. |
| Summary (Disk) | Ledger distinguishes accounted, residual, system, and unmeasured. The headline `disk ledger: not measured yet; run swamp observe --volume · observed 749 d ago` shows why age and availability must remain separate; make the action to measure prominent when absent. |
| Not measured (DiskGaps) | Honest uncertainty is a strength. Use explicit status labels and avoid any visual treatment that makes unmeasured look like zero; preserve that distinction in narrow layouts. |
| Agents | Redaction-aware listing and protection model are strong. Names like sessions/checkpoints imply unique data; retain recovery and protection facts in detail and confirmation, even when bulk marking skips protected rows. |

Additional states inspected: `empty_80x24.txt` (filter recovery), `refusal_80x24.txt` (selected-row consequence), `confirm_80x24.txt` (Trash plan), `agents_confirm_session_removal_80x24.txt` (unique session warning), `docker_80x24.txt` (permanent removal), `scope_coverage_header_80x24.txt` (header), `help_80x24.txt`, and wide counterparts. Text fixtures document rendering rather than proving behavior on every terminal; emoji-width fixture notes make this limitation explicit.

## Focused persona checks

- **Impatient power user:** strong keyboard and batch paths; the central friction is the broad `report` option grammar and progress counter semantics. Keep shortcuts and JSON contracts stable while improving help examples.
- **First-time maintainer:** labels and action consequences are mostly plain. “Unowned,” “Kinds,” and “coverage” still need a one-line explanation at first encounter. Trash plan should lead with exact destination and selected scope.
- **Stress tester:** empty scope, refusal, long help, mixed Docker/Trash, and incomplete coverage have explicit paths. Ensure long confirmation content stays reviewable and does not become confirmable while warning facts are off-screen.

## Recommended order

1. Correctly distinguish current-root and whole-run progress in CLI and TUI (P1 correctness of status).
2. Make Trash review content-sized and scannable while preserving the existing no-hidden-warning invariant (P1).
3. Reduce fixed chrome at short heights and improve first-page task guidance (P2).
4. Reorganize CLI help and view discovery without adding aliases (P2).

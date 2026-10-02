# Terminal design

The UI presents disk growth as a table that opens into a project tree. Size, signed change, and activity facts stay close to the row they describe. Use [committed frames](crates/tui/tests/frames/) and the [renderer](crates/tui/src/ui.rs) to check the current behavior.

## Rows and hierarchy

Each row contains a name and measured size. Show signed change only where the view has recorded change values, notes only where rows have facts, and change bars only where at least one change is nonzero. A measured zero stays `0B`; an unknown measurement never becomes zero. Project rows group checkouts and linked worktrees; tree rows show artifacts and the remaining directories. Box-drawing rails preserve parent-child relationships. Names truncate in the middle; numbers align on the right.

Column headings name the thing being compared: Project, Folder, Build output, Dependencies, Ecosystem, Storage kind, Unassigned storage, Storage item, Docker object, Tool location, Agent storage, Disk allocation, or Location. Name truncation and padding use grapheme-aware terminal-cell widths. Badges have separating spaces. Build outputs, Dependencies and build drilldowns reserve Notes at 80 columns; Reclaim reserves If removed. These and Reclaim hide numeric change below 100 columns. Disk views omit change entirely and label facts Measurement. These views cap names at 64 cells only when notes need the remaining width. Other views hide bars below 140 columns and show facts only in selected-row details below 100 columns. Zero and unknown changes have no vertical bar. Keep the selected row visible when scrolling.

Build and dependency labels lead with the project and checkout-relative path; additional checkouts carry a discriminator. Unassigned rows lead with their path. Tool labels keep the detector and identifying path suffix. Exact paths remain in selected details and action review. Reclaim table costs come from the typed regeneration class; original wording and source remain in the details. Current use, unique-copy, sharing, sourced last use and unknown facts precede secondary metadata in the fixed-height detail pane. Last use keeps its source and is separate from modification age.

Collapsed build categories lead with a recommendation and removal consequence: start with compiler caches (slower next build), review tests/examples (rebuild before rerunning), and lower-priority build-script output (scripts rerun). Item counts and selected bytes follow where space permits. Modification age belongs in supporting details, explicitly labelled; it is not last use. Final outputs say Rebuild before running again. Routine cleanup-rule and adapter explanations stay out of primary advice.

Candidate directory descendants are not counted again. Nested allocated sizes have a `*` suffix and a persistent shared-file accounting legend. When space remains below the tree, preview a subset of the selected category with paths, allocated sizes, explicitly labelled modification times and rebuilding effects. This preview is read-only; expand the category to select individual members.

Opening a project shows Cargo profiles with purpose-based cleanup groups:
Compiler caches, Compiled tests & examples, and Build-script output. Groups
contain only present, nonempty supported members in that profile. Tests and
Examples are expandable subgroups. Space marks the exact members for review;
profile rows also select their supported descendants, never the whole profile
directory. Profile advice says Open it and pick items.
Marking a fully marked group clears its members. A failed member review rolls
back newly added marks, preserving earlier selections. No virtual group is a
directory deletion target. Age ordering applies within groups; individual members
can be selected instead of the whole group.

A build container whose interior an adapter identified in the neutral role vocabulary (a `node_modules`, a `dist`, a Gradle `build/`, a Maven `target/`) expands into one group per role family -- Build outputs, Test & coverage output, Caches & intermediates, Installed dependencies, Shared store entries, Tool metadata -- plus a Not identified group for unrecognised entries and bytes no unit claims. Groups start closed. Each group row leads with review guidance of at most 32 characters ("Start here: slower next build", "Review: reinstall from registry", "Shared: other projects may link"), then the count; the adapter's own consequence, the accounting basis and what its cleanup rule does not cover are the row's details. That order is the point: an 80-column advice column shows the guidance whole and gives up the numbers first. An opened group lists its members oldest first (unknown ages last), each leading with its consequence in that ecosystem's words. Supported project-local output units can be selected for Trash by family; shared stores and units without a cleanup rule are marked one at a time on their own row. Action support comes from the adapter contract, not the role label alone. Which presentation a container gets follows the roles its units carry, never a comparison with an adapter id; Cargo containers keep the purpose groups above.

Inspect directories is a structural navigation row without a duplicate size or accounting explanation. It opens the physical tree of the same storage. Expansion and collapse retain the selected item and its screen position, allowing blank rows at the bottom rather than refilling the viewport from above. Physical category rows are navigation, not selective cleanup units. The selected-row detail area shows recommendations and rebuilding consequences. Compiler caches are a suggested starting point, not a claim of obsolescence. No age-only or newest-hash-wins verdicts.

The change bar grows right for an increase and left for a decrease. Its length uses a logarithmic scale relative to all changes in the current view, including off-screen rows. Changes below 1 MB use a small tick and dimmed text. The signed number supplies the rounded change; the bar is not a linear scale of bytes.

Growth sorts descending by signed change. Other sorts cover size, name, ecosystem, and age. Tree traversal preserves hierarchy; Tree, Reclaim and Disk ignore sort/reverse keys and omit those hints. Sorting or reversing keeps the same item selected. Ecosystem glyphs follow project names; linked-worktree and build-output badges add context.

## Color

Growth is red, shrink is green, and secondary information is dim. The selected row is one full-width bar in reverse video (with bold), and no span inside it sets a color: reverse video follows the terminal's own foreground and background on any theme, survives `NO_COLOR`, and does not depend on telling two colors apart. Marked rows are bold with an `✗` prefix. Warnings and the confirmation headline are bold, not yellow, and results are plain, because yellow and cyan are close to unreadable on many light themes (about 1.7 and 2.0 to 1 against white). Refusals use red text and begin with the word `refused:`. Zero and unknown changes are dim, never dark gray. Signed values and bar direction carry information independently of color.

The renderer uses the terminal's own colors and attributes. Committed frames exercise 50, 80, 120 and 200 column layouts; the light-theme contrast figures are computed, not observed on a particular terminal.

## Navigation and filters

`→` opens or expands. `←` collapses an expanded row, selects the parent of a leaf or collapsed child, or returns to projects when there is no parent. Returning to a view restores the selected item and scroll position; if the item disappeared or an informational row has no unique identity, selection falls back to its previous position, clamped to the remaining rows. Enter opens a project or confirms an action. Esc cancels the active interaction or returns to projects. `/` opens the filter form; `:` edits the expression; `0` clears it. Invalid expressions stay open for correction, with the error and recovery instruction in the status rows. The previous valid filter, marks and position remain active; Esc restores the accepted expression. Invalid drafts are never saved.

When no operation or overlay owns the status area, its existing two rows keep pending marks visible across navigation, filtering and view changes. The summary names the count, selected stored size and Trash/permanent Docker split. Blocked items retain their count and `b` recovery route. Row position counts the full current list, with a contextual action hint when space permits. Narrow screens drop optional size, position and hints before permanent-removal or blocked facts. Manager hints follow the actual routing: a manager list opens only with no marks; other marks require selecting that folder for Trash or returning to the marked rows. These cues never appear as commands while editing a filter, and they add no scanning or extra row construction.

The initial filter is `growth > 100MB in 7d`. Saved filter and sort choices take precedence on later runs. The views are nested in three sections (Projects, Tools, Disk): `Tab`/`Shift-Tab` move between sections, `1` `2` `3` jump to one, and `v` cycles the views inside the current section. The [usage guide](docs/usage.md#terminal-controls) holds the full key table.

## Header, progress, and history

The header shows the root, observation status, and available history as space permits. It drops trailing clauses on narrow terminals, but the activity chip (`⠋ observing 12s`, or `⠋ another observation running (pid N, 1m 12s)`) owns the left edge at every width. The UI opens on the stored report at any age and never scans when one exists; with none, the first scan runs in the background and its progress shows in the header. `R` refreshes on demand, and says so, rather than starting a second walk, when another process already holds the observation lock. That cache is the store's own typed Parquet tables (`swamp_core::growth::ReportSnapshot` assembles them into the one value both the TUI and `swamp report` read) -- not a JSON sidecar, and not a second data path from the one `swamp observe` writes.

Observation progress keeps a whole-run elapsed clock. The CLI names the active work; the TUI names the scope being updated. Per-walk byte counters are not displayed as run totals because they reset between roots. There is no percentage, because the total is not known. The TUI opens no filesystem watch: nothing scans on a file event, so a stored report is exactly as old as the header says. A lock poll only notices when another process observes, shows it, and reloads the stored report when that run ends. The right side of the header is the history sparkline with the net change it covers and the window it is over (`-41.4GB in 1w`); body rows use change bars.

Human output shares decimal byte units, grouped counts, whole-second durations, and coarse ages. Ages use minutes, hours, days, months, or years; timestamps show UTC through the minute. Rounded rows need not add up visually: totals are calculated from exact stored bytes. JSON, persisted records, and explicit diagnostic output retain exact numeric values.

### The developer-storage headline block and the view strip

Under the header, every view has a fixed two-row block: developer-storage total and coverage. It uses one row on terminals from 12 to 15 rows tall and none below that. Detailed allocation belongs in Disk usage. Missing or unreadable ledgers, previous scope, explicit-root scope, unmeasured folders, ledger age, and audit discrepancies remain visible in priority order. The spot-audit warning takes precedence over the total when only one row is available. State changes never move the table.

The section strip names `1 Projects  2 Tools  3 Disk`, with the current section in reverse video so `NO_COLOR` and light themes keep it. Until Tools or Disk is opened once, spare width holds `Tab switches sections`; that choice is remembered in `ui_state.json`. The following line names the current view and shows its applicable filter, sort, and short purpose as width permits. Ignored predicates are identified, and views that ignore filtering show no filter clause. Only Project folders includes the selected project in its title.

`Tab`/`Shift-Tab` change sections, `v` cycles views inside a section, and `1` `2` `3` open each section’s default view. Enter or → on a project opens Project folders; Esc returns to Projects. The legend shows `Tab section  v view`, without a key per view. Modals own their keys: Tab completes a raw filter instead of changing sections. Keymap tests check duplicate bindings and reachability.

The Disk section's Disk usage view is the stored volume ledger as rows: accounted, everything else
with its folders, system volumes, not measured (never a size), the protected
folders estimate, the bookkeeping line and the walk's spot audit. It is read
only and built from the stored ledger, so opening it lists nothing.

### Tool storage, Agent storage and scope coverage

- **Tool storage.** `ViewKind::External`, third in Tools, lists `ExternalUnit`s with detector/category labels, sizes and recorded changes. Selected details retain exact paths, consumers and source notes. Enter or → expands stored folder rows or identified interior groups. Real path rows can be marked for Trash; structural and remainder rows are informational. Depth-2 folder rows name unmeasured allocations and corrections instead of treating them as measured sizes. Last use stays in details with its source. On a manager row, Backspace opens the manager’s list only when the row is unmarked and no marks are pending; Space selects the folder for Trash instead.
- **Agent storage.** `ViewKind::Agents`, fourth in Tools, lists `AgentUnit`s with tool, category, project and item labels. Inferred associations stay labelled; details retain exact paths and link evidence. `actions::propose_agents_for_human` supplies the warnings for an individual mark, including kept-by-default, unknown-category and database-like rows. The human’s own protect entries block conflicting selections. `A` reviews eligible rows across the complete current list, skipping `individual_only` rows and reporting skips; these rows remain available for individual review with Space. Removal uses the ordinary background worker.
- **Linked storage in Project folders.** `ViewKind::Tree` includes a collapsed Agent storage (linked) summary per contributing tool, built from the same project tree as the CLI drill. These rows are informational (`unit: None`); individual selection happens in Agent storage. No linked units means no summary row, rather than a zero-byte placeholder.
- **Scope-coverage header clause, now driven by real observation
  outcome (#51).** `App::set_scope_note` adds one short header clause
  when there is more than one region or the one region is not simply
  `Complete` (e.g. `2 roots (1 missing)`, `3 roots (1 inaccessible:
  permission denied)`, `2 roots (1 partial: 2 path(s) unreadable
  during this walk)`), dropped last by the existing "fit clauses to
  width" rule like every other trailing clause -- never silently
  hidden, just lower priority than size/growth facts on a narrow
  terminal. It now takes `&[coverage::RootCoverage]` -- this pass's
  *actual* per-root walk outcome (`RegionStatus`), the same shape
  `report_scope`/`report --json`'s `scope_coverage` field shows --
  instead of the pre-walk `scope::RootStatus` snapshot an earlier
  chunk shipped, which could not distinguish "part of a present root
  was unreadable during the walk itself" (`Partial`) from a clean
  `Complete` region. The common case (one `Complete` region) shows no
  clause at all, matching ordinary single-project usage.

### Multi-root reports, coverage inspection, and refresh (#51)

`swamp ui` with no explicit root opens the TUI over the *whole*
configured scope, not just its first present root: `swamp_tui::run_scope`
calls `report::report_scope_with_parts` (the same coherent multi-root
entry point `report`/`observe` use) once at startup, giving the `App`
every present root's own report (`App::reports_by_root`) plus one merged
`Report` (`App::report`) built by folding them together
(`report::merge_reports`). This is what makes project/shared/external/
agent-tool storage from *any* included root show up together, including
a root with no Git checkout in it at all (previously invisible: the CLI
used to resolve the whole scope only to throw it away and hand the TUI
exactly one present root, which is all `swamp_tui::run` -- kept as-is
for the `swamp ui <explicit-root>` case -- has ever rendered).

Refresh preserves this per-root separation instead of ever
replacing the whole merged report at once. The TUI opens no filesystem
watch: a refresh is the first scan (only when there is no index), `R`,
or the observe after a delete.

- `App::observe_in_background` (the first scan, `R` and the post-delete
  refresh) re-observes every root in `App::roots`, sequentially, in one
  worker thread.
- It reports its result(s) as `(root, Report)` pairs; `App::
  replace_report_for_root` updates exactly that root's entry in
  `reports_by_root` and rebuilds `report` from the *whole* map
  (`report::merge_reports`) -- a refresh of one root can never erase,
  stale-mark, or duplicate another root's rows, because that root's own
  cached entry is never touched.

Selection/filters/sort keep working unchanged: they operate on the one
merged `Report` exactly as a single-root report always has. A unit
shared across projects (an external unit's `consumers`, or agent
storage linked from more than one project's tree row) was already
counted once by `external.rs`/`agents.rs`'s own identity model (#43/
#91); multi-root scope does not change that -- merging per-root reports
only concatenates each root's own `projects`/`unowned` rows, it never
re-derives external/agent-unit identity.

Known, named simplification: `App::history_secs` (the growth-window
picker's bound) is still derived from the *primary* root (`App::root`,
`roots[0]`) only, not the narrowest history among every included root
-- a multi-root picker can currently offer a window longer than a
non-primary root's own store actually has. Revisit alongside #62's cost
validation.

## Actions

Space marks a row. Backspace opens the confirmation for the current row or marked set. Confirmation is a content-sized review overlay with destination totals, selection context, shared warnings shown once, and item-specific exceptions. The primary summary must be displayed before Enter authorizes its action. Press `l` for the complete path, size, and member inventory; this detail view scrolls, Enter is disabled there, and Esc returns to the summary. Inspecting every inventory row is optional. Esc from the summary cancels.

Human keep/protect intent (`swamp protect`) is checked before **any**
row is marked, in both directions: a row beneath a protected path, and a
row that *contains* one. Protecting a single file inside a build
directory therefore refuses the directory, in the status rows, at the moment
you press Space -- not silently at execution. Protection state that
cannot be read is *unknown*: the confirm says the keep marks were not checked. This used to be reached
only for the two row kinds that happened to propose through core, which
is how a one-directional protection bug survived every test; see
`.oh/guardrails/protection-fails-closed.md`.

Project rows expand to actionable artifacts. If none exist, a direct project action may offer the checkout. Bulk review with `A` skips that fallback and opens the plan before acting. Worktree and source-directory selections carry their own warnings; the `ignored` and `untracked` summary buckets are not individual paths to delete.

Docker images and volumes must be named in the confirmation because their removal has no Trash recovery. Tool-managed removal (mise versions, simulator runtimes) has its own sheet over the screen, opened by Backspace on an unmarked mise installs or simulator runtimes row when no marks are pending: the manager's own list, then a confirm whose rows keep a fixed order (what is removed, "No Trash recovery: this cannot be undone", the command heading, the command, never cut, then program, size, reinstall cost, open files, the manager's quoted reasons, warnings, what the dry run removes, and the dry run verbatim, bounded with a "+k more lines" row), and whose last inner row is always its keys (`Y remove (cannot be undone) · Esc cancel`: Enter only opens the review, and `Y` counts only after the confirm has been drawn and 1 s has passed). A refusal shows "Reason:" and "Next:" and that nothing ran; a result says what the manager's list showed afterwards. `Y` runs nothing on a terminal too small to show the whole command block. Successful removals leave the displayed report, totals are adjusted, and the UI observes again. Refusals show in the status rows with their reason; what a check or a delete could not include is listed with `b` (or `d` on the plan), each with its whole reason and a next step, and `r` there checks again.

The selected row's own decision evidence (#53/#60) renders below the
table, in a fixed four-row detail pane (`crates/tui/src/detail.rs`): what
the row is and what rebuilding costs, then one plain sentence per fact that
changes a decision (in use now, may be the only copy, used by, last
changed), then one line naming what could not be established, so a missing
fact never reads as nothing to worry about. The sources, freshness and
coverage of every fact stay in `swamp report` and `--json`
(`render::render_evidence_lines`). The space-freed caveat (files shared
with other copies count once) shows only where the gap is at least 1MB.
The pane never changes height, so a keypress never moves the table. The confirmation row's
warnings line adds `render::evidence_warnings(&row.evidence)` --
a declared consumer, current use, or an uncertain recovery/
reclaimability fact, stated selectively rather than every fact restated
as a warning -- next to the pre-existing `PlanUnit::warnings`
(dirty/unpushed/untracked/no-remote facts). Both read `model::Row`'s
own `evidence` field, populated from the same `ArtifactRow`/
`ExternalUnit`/`AgentUnit` every other row field already comes from, so
there is no second, presentation-only evidence path to keep in sync.

## Layout, review and plan

The chrome is the same rows in every state: header, view and filter line, the
body, two status rows, and the key legend. Nothing resizes the body. A blocked list is a fixed 10-row sheet drawn over the bottom of the list. A plan uses a content-sized overlay, up to the terminal height minus its persistent action footer; long warning summaries scroll, while the complete path inventory is a separate optional detail view. A result stays in the status rows until the next key. A detail pane of fixed
height sits under the list, and the list scrolls only when the selection leaves
its window, so one Down moves the selection one row.

The review summary shows restore cost, sourced last use and consequential exceptions. Known adapter, accounting, coverage and selection bookkeeping stays in optional `l` details with original wording and every path and size. Unrecognized warnings remain visible; no warning count or length cap hides them. Enter is disabled in details. Successful removals prune stored interior rows and invalidate derived caches immediately; model manifest removal retains shared payload allocation until observation remeasures it. Failed moves stay listed.

Only rows inside that window are formatted into terminal cells. The complete row model still determines order, selection, totals, and growth-bar scale; scrolling does not change the scale or discard off-screen rows.

Marking builds a plan and changes nothing. Check, ready and blocked are the
words (never successful and refused), with `Nothing has been changed` while a
check runs and the plan sheet after it. Review and deletion run on background
workers. During an operation the status rows show a moving glyph and the elapsed
time first, at any width, then `Checked 41 of 342` (or the count alone when the
total is unknown; never a fabricated percentage) and the item by plain name.
Never imply byte reclamation progress. Esc, Ctrl-C and q request cancellation
between items, with a visible Stopping state; no second action starts while
busy. Completed outcomes are retained and blocked or unattempted marks remain for
explicit retry. Idle Ctrl-C exits. Observation results are held while busy and
pre-deletion results discarded so an out-of-date report cannot resurrect removed
rows. A result says what moved to Trash and what was removed for good, and that
space is freed when Trash is emptied; it carries no measured free-space figure,
because a move to Trash on the same volume frees nothing yet.

Keys move by row, by page (PgUp, PgDn) and to the ends (Home, End) in the main list. Help, blocked reasons and Cargo inspection scroll by displayed lines and clamp after resize so wrapped text remains reachable. The picker moves between fields with arrows; its project field accepts literal letters, digits and spaces. Esc cancels everywhere; q also closes the picker outside its project field. `v` names the view and its place; each view remembers its cursor, and an empty list says why and what to press. `k` says which way it flipped and what that means. Footer hints describe the current action: `⌫ review`, a manager list, or `A review all`; fixed-order views omit sorting. Hints drop whole at narrow widths and preserve help and quit.

The UI paints only when something changed: a key, a resize, a worker's result,
anything busy (every 200 ms, for the glyph and the clock), or a clock-driven part
of the screen reading differently (the age of the index, a refusal that ran
out). Idle it writes nothing. Sort, filter and `k` are written to `ui_state.json`
on a worker, newest wins, and flushed on exit.

The terminal is put back however the program ends: on return, on a panic on the
UI thread (the message prints on the normal screen), and on SIGTERM, SIGHUP or
SIGINT (`fs_gate::terminal`, chained to the child-kill handlers).

Keep the footer visible. Use overlays for help, the filter form, and action review. The help overlay ends with the activity-evidence inventory (which domains this pass can establish a real activity fact for, and which it reports as unknown), the same table `docs/usage.md` carries. Check empty results, narrow layouts, long paths, mixed filesystem/Docker selections, and missing history. The frame tests cover rendered text and layout; they do not establish readability on every font or color theme.

## Human-readable values

Human output shares decimal byte units across CLI and TUI, with whole KB and one decimal for larger units, carrying rounded boundaries into the next unit. Counts use digit grouping where needed. Elapsed durations use whole seconds, subsecond work reads `<1s`, and stored timestamps show dates or coarse relative ages with a named timezone where relevant. Machine-readable JSON, logs, and ledger records retain exact values.

Unsupported Ctrl, Alt, Super, Hyper and Meta chords do not invoke plain-key commands. Ctrl-C retains its explicit cancel/quit behavior; Shift commands and BackTab remain available. Only key press events dispatch actions. Manager confirmation names Y as the execution key; Enter only reviews its list selection.

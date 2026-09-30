# Terminal design

The UI presents disk growth as a table that opens into a project tree. Size, signed change, and activity facts stay close to the row they describe. Use [committed frames](crates/tui/tests/frames/) and the [renderer](crates/tui/src/ui.rs) to check the current behavior.

## Rows and hierarchy

Each row contains a name, bytes, signed growth, a change bar, and any visible facts. Project rows group checkouts and linked worktrees; tree rows show artifacts and the remaining directories. Box-drawing rails preserve parent-child relationships. Names truncate in the middle; numbers align on the right.

Column headings identify name, size, change, and cleanup/facts. Name truncation
and padding use grapheme-aware terminal-cell widths. Badges have separating
spaces. Build drilldowns reserve a compact candidates/oldest-modified column even
at 80 columns, hide the change bar, and hide numeric change below 100 columns.
Their name column caps at 64 cells. Other views hide bars below 140 columns and
show facts only in selected-row details below 100 columns. Zero and
unknown changes have no vertical bar. Keep the selected row visible when scrolling.

Collapsed build categories lead with a recommendation and removal consequence:
start with compiler caches (slower next build), review tests/examples (rebuild
before rerunning), and lower-priority build-script output (scripts rerun).
Counts, allocated candidate size, and oldest known modification age follow only
when space permits; narrow views drop these statistics before clipping advice.
Unknown age is `?`.
Candidate directory descendants are not counted again. Final outputs say
`Inspect only: removes built output`, without suggesting a supported selective action.
Other unsupported rows say `Selective cleanup unsupported`, not a safety verdict.
Nested allocated sizes have a `*` suffix and a persistent accounting legend.
When space remains below the tree, preview the selected category's oldest
candidates with paths, allocated sizes, ages, and rebuilding effects. This preview
is read-only: expand the category to select exact groups.

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

A build container whose interior an adapter identified in the neutral role
vocabulary (a `node_modules`, a `dist`, a Gradle `build/`, a Maven `target/`)
expands into one group per role family -- Build outputs, Test & coverage output,
Caches & intermediates, Installed dependencies, Shared store entries, Tool
metadata -- plus a Not identified group for unrecognised entries and bytes no
unit claims. Groups start closed. Each group row leads with review guidance of at
most 32 characters ("Start here: slower next build", "Review: reinstall from
registry", "Shared: other projects may link"), then the count and oldest known
modification; the adapter's own consequence, the accounting basis and
"inspection only" are the row's details. That order is the point: an 80-column
advice column shows the guidance whole and gives up the numbers first. An opened
group lists its members oldest first (unknown ages last), each leading with its
consequence in that ecosystem's words. Supported project-local output units can
be selected for Trash; shared stores and unsupported units remain inspection-only.
Action support comes from the adapter contract, not the role label alone. Which
presentation a container gets follows the roles its units carry,
never a comparison with an adapter id; Cargo containers keep the purpose groups
above.

The physical tree remains under a collapsed Inspect directories row; it is a
second view of the same bytes, not additional storage. Physical category rows
are navigation, not selective cleanup units. The selected-row detail area shows recommendations
and rebuilding consequences. Compiler caches are a suggested starting point,
not a claim of obsolescence. No age-only or newest-hash-wins verdicts.

The change bar grows right for an increase and left for a decrease. Its length uses a logarithmic scale relative to visible changes. Changes below 1 MB use a small tick and dimmed text. The signed number supplies the actual value; the bar is not a linear scale of bytes.

Growth sorts descending by signed change. Other sorts cover size, name, ecosystem, and age. Tree traversal preserves hierarchy. Ecosystem glyphs follow project names; linked-worktree and build-output badges add context.

## Color

Growth is red, shrink is green, and secondary information is dim. The selected row is one full-width bar in reverse video (with bold), and no span inside it sets a color: reverse video follows the terminal's own foreground and background on any theme, survives `NO_COLOR`, and does not depend on telling two colors apart. Marked rows are bold with an `✗` prefix. Warnings and the confirmation headline are bold, not yellow, and results are plain, because yellow and cyan are close to unreadable on many light themes (about 1.7 and 2.0 to 1 against white). Refusals use red text and begin with the word `refused:`. Zero and unknown changes are dim, never dark gray. Signed values and bar direction carry information independently of color.

The renderer uses the terminal's own colors and attributes. Committed frames exercise 50, 80, 120 and 200 column layouts; the light-theme contrast figures are computed, not observed on a particular terminal.

## Navigation and filters

`→` opens or expands; `←` collapses or returns to projects. Enter opens a project or confirms an action. Esc cancels the active interaction or returns to projects. `/` opens the filter form; `:` edits the expression; `0` clears it. Parse errors retain the previous valid filter.

The initial filter is `growth > 100MB in 7d`. Saved filter and sort choices take precedence on later runs. Ten views are available through `v` and `1`–`9` (External is `9`; Agents has no dedicated digit -- `0` is "clear filter" -- and is reached only by cycling with `v`). The [usage guide](docs/usage.md#terminal-controls) holds the full key table.

## Header, progress, and history

The header shows the root, observation status, available history, and totals as space permits. It drops trailing clauses on narrow terminals, but the activity chip (`⠋ observing 12s`, or `⠋ another observation running (pid N, 1m 12s)`) owns the left edge at every width. The UI opens on the stored report at any age and never scans when one exists; with none, the first scan runs in the background and its progress shows in the header. `R` refreshes on demand, and says so, rather than starting a second walk, when another process already holds the observation lock. That cache is the store's own typed Parquet tables (`swamp_core::growth::ReportSnapshot` assembles them into the one value both the TUI and `swamp report` read) -- not a JSON sidecar, and not a second data path from the one `swamp observe` writes.

Observation progress shows the elapsed time and the bytes seen; there is no percentage, because the total is not known. The TUI opens no filesystem watch: nothing scans on a file event, so a stored report is exactly as old as the header says. A lock poll only notices when another process observes, shows it, and reloads the stored report when that run ends. The right side of the header is the history sparkline with the net change it covers and the window it is over (`-41.4GB in 1w`); body rows use change bars.

### External and Agents rows, and a scope-coverage header clause

`report_scope`/`external.rs` (#42/#43) and `agents.rs` (#91/#92) gave
the TUI three facts recorded here as unimplemented intent by an earlier
chunk; all three now ship:

- **External rows.** `ViewKind::External` (`'9'`) lists `ExternalUnit`s
  the same shape as `ViewKind::Unowned` lists unowned rows: path,
  category, size, growth, consumer count. The row is never markable
  (`Row.unit: None`): an external unit is shared, detector-resolved
  storage (a package manager's cache, a toolchain install), shown for
  review, and there is no delete affordance to offer -- act on it with
  the manager's own tools, not swamp. A unit that is a
  machine-wide build store (a Maven repository, Go's module cache,
  DerivedData, the Android SDK, ...) is expandable: `Enter`/`→` opens it
  onto the **same** family groups a project container shows (closed
  until opened, guidance first, every row `blocked`, no `UnitId`), from
  `ScopeObservation::store_interiors` of the same pass. The Docker view
  gains one row per BuildKit builder that opens the same way; its member
  rows say "created (daemon)" rather than "modified", because the time
  is the daemon's record, not a file's. A unit that declares a last-use
  source, and an unclassified root of 1 GiB or more, also opens onto its
  depth-2 rows first (its top 15 child folders largest first, then one
  remainder row that makes the rows sum to the unit's total; a folder that
  could not be read draws `unmeasured` in the Size cell, a signed correction draws `-50MB adj`, and neither ever draws a size; a unit that also has an identified interior holds it under one closed "identified interior" header so no byte is listed twice). Every such
  row is `blocked`, no `UnitId`, and the table layout is unchanged: the
  last-used fact (`Last run or opened: Jul 8 (file access time)`) is the
  first line under the signals in the selected-row detail pane, not a
  column, so no width rule moves. An unowned row for a standalone Cargo
  target directory is markable like any unowned row and its confirm line
  says what it is and that `cargo build` remakes it.
- **Agents rows.** `ViewKind::Agents` (no dedicated digit -- `0` is
  "clear filter"; reached by cycling with `v`) lists `AgentUnit`s the
  same way: tool/category/relative-path/project-link facts. Every row
  carries `Row.unit: Some(...)` (protected/unmarkable ones included):
  `Space`/`Backspace` mark the selected unit through
  `actions::propose_agents` and open the confirm banner with its
  current facts (session-removal loss warnings, the linked project);
  `Enter` moves it to the Trash through the ordinary background-worker
  path (`actions::execute_plan_progress`) every other markable view
  already uses -- never a new blocking call on the event/render thread,
  and with no re-check between marking and moving. A protected row, or
  one whose category has no Trash move at all, cannot be marked:
  `propose_agents`'s own refusal (protected category, no Trash move for
  this category, database-like file) becomes the status text, never a
  generic "nothing to delete." Bulk marking (`Shift+A`,
  `mark_all_in_view`) reaches agent rows too: since `model::agent_rows`
  sets `Row.unit` but never `Row.kind` (there is no `ArtifactKind` for
  an agent-storage unit), `mark_all_in_view` has a third branch
  alongside its `row.kind`/`ArtifactKind` and projects-view
  `row.project` ones -- when a row has neither but does carry `unit`, it
  reuses `mark_row`'s own per-row refusal rather than duplicating that
  logic, and counts a skip instead of a hard stop. The status rows name how
  many agent rows were skipped and why whenever at least one row *was*
  marked, never silently proceeding as if the skipped rows were not on
  screen.
- **Project tree's collapsed "Agent storage (linked)" row (#100
  completion).** `ViewKind::Tree`'s own drill (`model::tree_rows_with_agents`,
  built from the same `crate::tree::build_project_tree` the CLI's
  `--project` text drill uses) now appends one row per tool
  contributing linked agent storage to the selected project, after the
  project's own worktrees. It is informational only (`unit: None`):
  the row exists so "does this project have any linked agent storage,
  from which tools, how much" is visible from the project drill itself
  without also opening the separate Agents view -- acting on a specific
  unit still happens there, where per-unit protections/occupancy are
  checked. Absence of any linked unit means no row at all, never a
  zero-byte placeholder.
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

Space marks a row. Backspace opens the confirmation for the current row or marked set. Confirmation is a single inline row with selected paths, sizes, warnings, and destinations. Enter authorizes the action; Esc cancels it.

Human keep/protect intent (`swamp protect`) is checked before **any**
row is marked, in both directions: a row beneath a protected path, and a
row that *contains* one. Protecting a single file inside a build
directory therefore refuses the directory, in the status rows, at the moment
you press Space -- not silently at execution. Protection state that
cannot be read is *unknown*, so it refuses too. This used to be reached
only for the two row kinds that happened to propose through core, which
is how a one-directional protection bug survived every test; see
`.oh/guardrails/protection-fails-closed.md`.

Project rows expand to actionable artifacts. If none exist, a direct project action may offer the checkout. Bulk marking with `A` skips that fallback. Worktree and source-directory selections carry their own warnings; the `ignored` and `untracked` summary buckets are not individual paths to delete.

Docker images and volumes must be named in the confirmation because their removal has no Trash recovery. Successful removals leave the displayed report, totals are adjusted, and the UI observes again. Refusals show in the status rows with their reason; what a check or a delete could not include is listed with `b` (or `d` on the plan), each with its whole reason and a next step, and `r` there checks again.

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
body, two status rows, and the key legend. Nothing resizes the body. A plan or
blocked list is a fixed 10-row sheet drawn over the bottom of the list, and a
result stays in the status rows until the next key. A detail pane of fixed
height sits under the list, and the list scrolls only when the selection leaves
its window, so one Down moves the selection one row.

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

Keys move by row, by page (PgUp, PgDn) and to the ends (Home, End) in the list,
the help, the blocked list, the cargo popup and the picker. `v` names the view
and its place (`builds of mole (3 of 10 · v next · Esc: projects)`), each view
remembers its cursor, and an empty list says why and what to press. `k` says
which way it flipped and what that means, since it is remembered. The key legend
keeps `/ filter  v view  R refresh  ⌫ delete  ? help  q quit` at 80 columns and
drops movement keys first.

The UI paints only when something changed: a key, a resize, a worker's result,
anything busy (every 200 ms, for the glyph and the clock), or a clock-driven part
of the screen reading differently (the age of the index, a refusal that ran
out). Idle it writes nothing. Sort, filter and `k` are written to `ui_state.json`
on a worker, newest wins, and flushed on exit.

The terminal is put back however the program ends: on return, on a panic on the
UI thread (the message prints on the normal screen), and on SIGTERM, SIGHUP or
SIGINT (`fs_gate::terminal`, chained to the child-kill handlers).

Keep the footer visible. Use overlays for help and the filter form, with inline action confirmation. The help overlay ends with the activity-evidence inventory (which domains this pass can establish a real activity fact for, and which it reports as unknown), the same table `docs/usage.md` carries. Check empty results, narrow layouts, long paths, mixed filesystem/Docker selections, and missing history. The frame tests cover rendered text and layout; they do not establish readability on every font or color theme.

//! Application state and key handling. Pure aside from the action layer
//! calls the confirm step makes; the rest is unit-testable without a
//! terminal.

use crate::actions::{self, MarkedUnit};
use crate::filter::{self, Filter};
use crate::model::{self, Row, Sort};
use crate::names;
use crate::units::UnitId;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use swamp_core::report::Report;

/// What pressing a mark key on a projects-view row did.
#[derive(Debug, PartialEq, Eq)]
enum ProjectMark {
    /// The project has nothing that can be marked.
    Nothing,
    /// Everything markable was already marked; the press unmarked it.
    Cleared,
    /// This many units were newly marked.
    Marked(usize),
}

/// Every distinct refusal with how many rows it covered, so a bulk mark
/// that skipped rows says how many and why, not only the first reason.
fn summarize_refusals(reasons: &[String]) -> Option<String> {
    if reasons.is_empty() {
        return None;
    }
    let mut distinct: Vec<(&str, usize)> = Vec::new();
    for r in reasons {
        match distinct.iter_mut().find(|(t, _)| *t == r.as_str()) {
            Some((_, n)) => *n += 1,
            None => distinct.push((r.as_str(), 1)),
        }
    }
    if distinct.len() == 1 && distinct[0].1 == 1 {
        return Some(distinct[0].0.to_string());
    }
    let shown: Vec<String> = distinct
        .iter()
        .take(3)
        .map(|(t, n)| format!("{t} (x{n})"))
        .collect();
    let extra = distinct.len().saturating_sub(3);
    let extra = if extra > 0 {
        format!("; +{extra} more reasons")
    } else {
        String::new()
    };
    Some(format!(
        "{} rows skipped: {}{extra}",
        reasons.len(),
        shown.join("; ")
    ))
}

pub const REFUSAL_DISPLAY: Duration = Duration::from_secs(4);
/// An index older than this is called stale in the header (a warning,
/// not an error): the schedule is meant to keep it younger.
pub const STALE_AFTER_SECS: u64 = 15 * 60;

/// The three sections the views are nested in. `Tab` / `Shift-Tab` move
/// between them, `1` / `2` / `3` jump to one, and `v` cycles the views
/// inside the current one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Projects,
    Tools,
    Disk,
}

impl Section {
    pub const ALL: [Section; 3] = [Section::Projects, Section::Tools, Section::Disk];

    pub fn title(self) -> &'static str {
        match self {
            Section::Projects => "Projects",
            Section::Tools => "Tools",
            Section::Disk => "Disk",
        }
    }

    /// The digit that jumps here.
    pub fn key(self) -> char {
        match self {
            Section::Projects => '1',
            Section::Tools => '2',
            Section::Disk => '3',
        }
    }

    pub fn from_key(k: char) -> Option<Section> {
        Self::ALL.iter().copied().find(|s| s.key() == k)
    }

    /// The views inside, in the order `v` walks them; the first is the
    /// one a jump lands on.
    pub fn views(self) -> &'static [ViewKind] {
        match self {
            Section::Projects => &[
                ViewKind::Projects,
                ViewKind::Tree,
                ViewKind::Builds,
                ViewKind::Deps,
                ViewKind::Types,
                ViewKind::Kinds,
                ViewKind::Unowned,
            ],
            Section::Tools => &[
                ViewKind::Reclaim,
                ViewKind::Docker,
                ViewKind::External,
                ViewKind::Agents,
            ],
            Section::Disk => &[ViewKind::Disk, ViewKind::DiskGaps],
        }
    }

    pub fn default_view(self) -> ViewKind {
        self.views()[0]
    }

    pub fn next(self) -> Section {
        let at = Self::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Self::ALL[(at + 1) % 3]
    }

    pub fn prev(self) -> Section {
        let at = Self::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Self::ALL[(at + 2) % 3]
    }

    /// One line on what the section is for, for `?` help.
    pub fn describe(self) -> &'static str {
        match self {
            Section::Projects => "your projects and what they hold",
            Section::Tools => "storage outside any project: toolchains, caches, AI tools",
            Section::Disk => "where the whole disk went, from the stored volume ledger",
        }
    }
}

/// Same view set the CLI's `--view` exposes at root (`worktrees` there
/// is `Projects` here: one row per project, same aggregation), plus
/// `Tree`, the per-project drill-down `--project` renders (#33). `v`
/// cycles this exact order on both surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewKind {
    Projects,
    Tree,
    Builds,
    Deps,
    Docker,
    Kinds,
    Unowned,
    /// Per-ecosystem rollup (`Report.summary`).
    Types,
    /// External/shared storage units (#43), the minimal shape DESIGN.md
    /// recorded: read-only, one row per detector-resolved unit.
    External,
    /// The Reclaim view (#175): one row per unit of developer storage,
    /// largest first, with what getting it back costs, when it was last
    /// used, who is known to need it and which removal path exists.
    /// Read-only and built from stored facts: opening it scans nothing.
    Reclaim,
    /// Where the whole disk went (#169, #170): the stored volume ledger's
    /// parts (accounted, everything else, system volumes, what could not
    /// be read). Read-only and built from the stored ledger: opening it
    /// scans nothing.
    Disk,
    /// The Disk section's second view: what was not measured (unreadable
    /// and not-yet-measured folders) and the largest measured folders
    /// outside developer storage. Read-only, from the stored ledger.
    DiskGaps,
    /// Agent-tool storage (#91/#92/#100): read-only, one row per
    /// `AgentUnit`. See `model::agent_rows`'s doc comment for why
    /// marking is not wired up in this chunk.
    Agents,
}

impl ViewKind {
    /// The section this view lives in.
    pub fn section(self) -> Section {
        match self {
            ViewKind::Projects
            | ViewKind::Tree
            | ViewKind::Builds
            | ViewKind::Deps
            | ViewKind::Types
            | ViewKind::Kinds
            | ViewKind::Unowned => Section::Projects,
            ViewKind::Reclaim | ViewKind::Docker | ViewKind::External | ViewKind::Agents => {
                Section::Tools
            }
            ViewKind::Disk | ViewKind::DiskGaps => Section::Disk,
        }
    }

    /// The name on the view strip and in `?` help.
    pub fn title(self) -> &'static str {
        match self {
            ViewKind::Projects => "Projects",
            ViewKind::Tree => "Tree",
            ViewKind::Builds => "Builds",
            ViewKind::Deps => "Deps",
            ViewKind::Docker => "Docker",
            ViewKind::Kinds => "Kinds",
            ViewKind::Unowned => "Unowned",
            ViewKind::Types => "Types",
            ViewKind::External => "External",
            ViewKind::Reclaim => "Reclaim",
            ViewKind::Disk => "Summary",
            ViewKind::DiskGaps => "Not measured",
            ViewKind::Agents => "Agents",
        }
    }

    /// One line on what the view shows, for `?` help.
    pub fn describe(self) -> &'static str {
        match self {
            ViewKind::Projects => "one row per project: size, growth, what can be rebuilt",
            ViewKind::Tree => "a project's worktrees and their folders",
            ViewKind::Builds => "build output across projects, by kind",
            ViewKind::Deps => "dependency folders across projects",
            ViewKind::Docker => "Docker images, volumes and build cache",
            ViewKind::Kinds => "storage grouped by kind of folder",
            ViewKind::Unowned => "storage that belongs to no project",
            ViewKind::Types => "storage grouped by ecosystem",
            ViewKind::External => "toolchains, caches and stores outside any project",
            ViewKind::Reclaim => {
                "regenerable developer storage by unit: what getting it back costs, last used"
            }
            ViewKind::Disk => {
                "where the whole disk went: accounted, everything else, system volumes, not measured"
            }
            ViewKind::Agents => "AI coding tools' sessions, caches and logs",
            ViewKind::DiskGaps => {
                "what could not be read or is not measured yet, and the largest measured folders outside developer storage"
            }
        }
    }

    /// Every view, section by section, in the order `v` walks them within
    /// each.
    pub const ALL: [ViewKind; 13] = [
        ViewKind::Projects,
        ViewKind::Tree,
        ViewKind::Builds,
        ViewKind::Deps,
        ViewKind::Types,
        ViewKind::Kinds,
        ViewKind::Unowned,
        ViewKind::Reclaim,
        ViewKind::Docker,
        ViewKind::External,
        ViewKind::Agents,
        ViewKind::Disk,
        ViewKind::DiskGaps,
    ];

    /// 1-based place within the view's own section, for "2 of 3".
    pub fn position(self) -> usize {
        self.section()
            .views()
            .iter()
            .position(|v| *v == self)
            .unwrap_or(0)
            + 1
    }

    /// The next view within the same section, wrapping around (`v`).
    pub fn next(self) -> Self {
        let views = self.section().views();
        let at = views.iter().position(|v| *v == self).unwrap_or(0);
        views[(at + 1) % views.len()]
    }
    pub fn label(self) -> &'static str {
        match self {
            ViewKind::Projects => "projects",
            ViewKind::Tree => "tree",
            ViewKind::Builds => "builds",
            ViewKind::Deps => "deps",
            ViewKind::Kinds => "kinds",
            ViewKind::Docker => "docker",
            ViewKind::Unowned => "unowned",
            ViewKind::Types => "types",
            ViewKind::External => "external",
            ViewKind::Reclaim => "reclaim",
            ViewKind::Disk => "disk",
            ViewKind::DiskGaps => "not-measured",
            ViewKind::Agents => "agents",
        }
    }
}

/// One background observation's outcome: every `(root, report)` pair it
/// managed to produce (#51 -- a live/cached refresh can cover more than
/// one root per worker thread; see `App::pending`'s doc comment).
/// One background/live observation's whole result. External and agent
/// units travel with the per-root reports because they come from the
/// *same* pass (`report::observe_scope`): the review found the TUI's
/// agent view could go arbitrarily stale while the header said the
/// report was live, because only startup ever refreshed those vectors.
pub struct RefreshedObservation {
    pub per_root: Vec<(PathBuf, Report)>,
    /// Every root's latest report (the cache the worker was handed, with
    /// `per_root` folded in) and their merge, computed **on the worker**:
    /// the merge re-attaches consumer associations from the store's
    /// declaration caches, which is I/O the event thread must not do
    /// (`tui_event_thread_has_no_gate_calls`). `None` when nothing was
    /// re-observed.
    pub merged: Option<MergedReports>,
    /// `None` when this refresh did not re-derive units (nothing should
    /// clear them); `Some` replaces them wholesale.
    pub external_units: Option<Vec<swamp_core::external::ExternalUnit>>,
    pub agent_units: Option<Vec<swamp_core::agents::AgentUnit>>,
    /// The machine-wide build stores' interiors from the same pass;
    /// `None` exactly when `external_units` is.
    pub store_interiors: Option<Vec<swamp_core::artifact::NestedArtifact>>,
}

/// What the lock poller reports back to the event loop.
enum LockPollMsg {
    /// Who holds the observation lock right now (not this process).
    Holder(Option<swamp_core::schedule::LockHolder>),
    /// The holder finished: the store's newest observation, read off the
    /// event thread.
    Reloaded(
        Box<swamp_core::report::ReportSnapshot>,
        Box<swamp_core::volume_ledger::LedgerReading>,
    ),
}

/// A reload waiting for the confirm or check to end.
enum HeldReload {
    Fresh(Box<RefreshedObservation>),
    Snapshot(
        Box<swamp_core::report::ReportSnapshot>,
        Box<swamp_core::volume_ledger::LedgerReading>,
    ),
}

const REFRESH_WAITS: &str = "A check is running; press R after it finishes";

/// The smallest terminal that shows the permanent-removal line of a plan.
pub const CONFIRM_MIN_ROWS: u16 = 9;
pub const CONFIRM_MIN_COLS: u16 = 40;

type PendingObservation = anyhow::Result<RefreshedObservation>;

/// A merged multi-root report and the per-root cache it came from.
pub struct MergedReports {
    pub by_root: std::collections::HashMap<PathBuf, Report>,
    pub report: Report,
}

impl RefreshedObservation {
    /// Builds the result a worker sends: folds `per_root` into `cache`
    /// (the app's per-root cache when the worker started -- only one
    /// observation is ever pending, so nothing else changed it) and
    /// merges. Called on worker threads only.
    pub fn merged_on_worker(
        roots: &[PathBuf],
        mut cache: std::collections::HashMap<PathBuf, Report>,
        per_root: Vec<(PathBuf, Report)>,
        external_units: Option<Vec<swamp_core::external::ExternalUnit>>,
        agent_units: Option<Vec<swamp_core::agents::AgentUnit>>,
        store_interiors: Option<Vec<swamp_core::artifact::NestedArtifact>>,
    ) -> Self {
        let merged = (!per_root.is_empty()).then(|| {
            for (root, r) in &per_root {
                cache.insert(root.clone(), r.clone());
            }
            let report = swamp_core::report::merge_reports(roots, &cache);
            MergedReports {
                by_root: cache,
                report,
            }
        });
        RefreshedObservation {
            per_root,
            merged,
            external_units,
            agent_units,
            store_interiors,
        }
    }
}

/// What the cached headline was built from: the report's time, the scope
/// it covers, and the clock minute (a ledger dated ahead of now is judged
/// against it).
type HeadlineKey = (u64, Option<usize>, bool, u64);

pub struct App {
    /// Ephemeral on-demand details; never persisted in the observation store.
    pub cargo_inspection: Option<Vec<String>>,
    pub cargo_inspection_scroll: u16,
    /// The tool-managed removal sheet (#177), when open.
    pub tool_sheet: Option<crate::tool_sheet::ToolSheet>,
    /// The one tool worker in flight (listing, review or removal): one at
    /// a time, never on this thread.
    tool_rx: Option<std::sync::mpsc::Receiver<crate::tool_sheet::ToolEvent>>,
    /// A confirm just appeared: queued input is dropped before any key.
    confirm_drain: bool,
    /// How tool removal reaches the machine: the real managers, or a
    /// test's sandbox of fakes.
    pub tool_host: swamp_core::tool_removal::Host,
    pub operation: Option<Operation>,
    operation_rx: Option<std::sync::mpsc::Receiver<OperationEvent>>,
    review_progress: Option<std::sync::mpsc::Sender<OperationEvent>>,
    review_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Worker side: what this check could not include.
    blocked_log: Vec<BlockedItem>,
    /// Worker side: the row being marked, for naming a refusal.
    refusal_ctx: Option<String>,
    /// What the last check could not include (`b` lists it).
    pub blocked: Vec<BlockedItem>,
    /// What the last check looked at: every row in the view, or one row.
    /// `r` in the blocked list runs it again.
    last_check: Option<(bool, Option<Row>)>,
    /// The marks that check added, so a re-check replaces exactly those and
    /// keeps marks made any other way.
    last_check_marks: Vec<String>,
    pub blocked_open: bool,
    /// First blocked item shown in the blocked list.
    pub blocked_scroll: usize,
    /// First body row shown; moves only when the selection leaves the
    /// window, so one keypress moves the selection one row.
    pub scroll_offset: std::cell::Cell<usize>,
    /// Redraws so far; drives the busy glyph.
    pub frame: u64,
    /// The one writer of `ui_state.json`, started on first use: a slow or
    /// full disk stalls it, never a keypress.
    ui_state_tx: Option<std::sync::mpsc::Sender<UiStateMsg>>,
    /// Rows a page key moves in whatever is on top (the list, the blocked
    /// sheet, help, the cargo popup); each drawer sets it to its own
    /// viewport so PgUp/PgDn always mean "one screenful".
    pub page: std::cell::Cell<usize>,
    /// First help line shown. The drawer clamps it to the real end, so
    /// `End` may set it as far as it likes.
    pub help_scroll: std::cell::Cell<usize>,
    /// Where the cursor was in each view when it was last left, so coming
    /// back (Esc, `v`) lands on the same row instead of the top.
    view_cursor: std::collections::HashMap<ViewKind, usize>,
    /// Per-project mark counts for the projects view, kept until the marks
    /// or the report change (computing one is a tree build per project).
    mark_cache: std::sync::Mutex<Option<(u64, std::sync::Arc<MarkStates>)>>,
    pub report: Report,
    /// Primary root: the first entry of `roots`, kept for every call
    /// site that only ever needed one representative path (a "resize
    /// this one thing" worker sub-`App`, a device lookup for an FSEvents
    /// plan). Never the sole scan target once `roots.len() > 1` --
    /// `report` and the live-refresh machinery below always operate
    /// over the whole `roots` list.
    pub root: PathBuf,
    /// Every root this report covers (#51): a single-root `swamp ui
    /// <path>` invocation gets exactly one entry; the configured-scope
    /// invocation (`swamp ui` with no explicit root) gets every present
    /// root the resolved `EffectiveScope` walked, so project/shared/
    /// external units from any of them are all in `report` at once --
    /// including a root with no Git checkout in it at all (only
    /// external/agent-tool storage), which used to be invisible because
    /// the CLI picked exactly one present root before the TUI even
    /// started.
    pub roots: Vec<PathBuf>,
    /// Each root's own last-observed single-root report, keyed by its
    /// walked path -- the input to `report::merge_reports`, which
    /// rebuilds `report` from this map. A live refresh or cached-startup
    /// re-observation of one root replaces exactly that root's entry
    /// and re-merges, so it can never erase or stale-mark any other
    /// root's rows (#51's "updating one root does not erase/stale-mark
    /// unrelated measured roots"). Empty for a fixture `App` built
    /// directly from a `Report` (tests): `replace_report_for_root` still
    /// works in that case, it just starts from one entry.
    pub reports_by_root: std::collections::HashMap<PathBuf, Report>,
    pub view: ViewKind,
    pub filter_text: String,
    pub filter: Filter,
    pub filter_error: Option<String>,
    pub editing_filter: bool,
    /// Filter text as it was when editing began; restored on Esc.
    pub filter_before_edit: String,
    /// Background observation result, when one is in flight: one or more
    /// `(root, report)` pairs (a live refresh touches whichever one root
    /// owned the changed paths; a cached-startup refresh re-observes
    /// every root in one worker thread), each applied via
    /// `replace_report_for_root` so it updates exactly that root's entry
    /// in `reports_by_root` regardless of how many roots this `App`
    /// covers. `Err` is scope-wide (the worker thread itself failed
    /// before producing any per-root result, e.g. a channel/panic
    /// issue) rather than naming one root, since a single-root failure
    /// is instead represented as that root simply being absent from an
    /// `Ok` vec (its previous `reports_by_root` entry is left as-is,
    /// same "coverage change is not a storage change" contract as
    /// `report_scope`'s own per-root `Inaccessible` handling).
    pub pending: Option<std::sync::mpsc::Receiver<PendingObservation>>,
    /// One-line status shown in the footer slot (errors, notices).
    pub status: Option<String>,
    pub selected: usize,
    pub selected_project: Option<String>,
    pub collapsed: HashSet<String>,
    /// Marked units, keyed by path string for stable identity.
    pub marked: BTreeMap<String, MarkedUnit>,
    pub confirm_open: bool,
    /// Unit ids the press that opened the current confirm marked (Backspace
    /// or `A` on an unmarked selection). Esc on that confirm unmarks exactly
    /// these, so cancelling never leaves marks the screen did not show
    /// before; marks made earlier with Space stay, and stay visible.
    /// The marks that existed when the open confirm was first opened; Esc
    /// undoes whatever was marked beyond them, however many rechecks or
    /// repeated presses happened since.
    confirm_base: Option<std::collections::BTreeSet<String>>,
    /// A newer index that arrived while a check or a confirm was open. It
    /// is installed when they end, so what Enter would move never changes
    /// under the human's eyes.
    held_reload: Option<HeldReload>,
    pub help_open: bool,
    /// The filter picker form, when open.
    pub picker: Option<crate::picker::Picker>,
    /// Tab-completion candidates shown under the raw filter line.
    pub completions: Vec<String>,
    pub refusal: Option<(String, Instant)>,
    pub observing: Option<(u32, u32)>,
    /// When this UI's own observation started, for the header's elapsed
    /// time.
    pub observing_started: Option<Instant>,
    /// Another process (the scheduled `swamp observe`) currently holds
    /// the observation lock; kept fresh by `start_lock_poll`.
    pub external_observer: Option<swamp_core::schedule::LockHolder>,
    lock_poll_rx: Option<std::sync::mpsc::Receiver<LockPollMsg>>,
    /// Last operation outcome or refusal worth one header clause.
    pub last_result: Option<String>,
    pub observed_label: String,
    /// False only while the very first observation has not landed.
    pub has_index: bool,
    /// The store on disk was written by an older swamp, so its derived
    /// tables are being rebuilt by the background observation. Only ever
    /// set together with `has_index == false`; it changes what the empty
    /// list says, never what is drawn or how fast.
    pub store_rebuild: bool,
    /// Header age and stale warning come from the report's own
    /// `observed_at` against the clock (real sessions); off, the header
    /// shows `observed_label` verbatim (fixtures with made-up times).
    pub live_age: bool,
    /// Set when the store's volume is nearly full: the refresh was
    /// skipped and this stored report is all there is. Shown first in
    /// the header.
    pub disk_banner: Option<String>,
    pub actor: String,
    pub sort: Sort,
    /// Flip the active sort's order (`r`). Persisted with the sort.
    pub reverse: bool,
    /// Copy compiled outputs to `<worktree>/bin/` before trashing a build
    /// directory (`k` on the confirm line). Persisted.
    pub keep_executables: bool,
    pub quit: bool,
    /// Terminal width at the last draw; the header fits its clauses to it.
    pub width: u16,
    /// Terminal height at the last draw; 0 until the first one.
    pub height: u16,
    /// Seconds of observation history the store holds; bounds the growth
    /// windows a human may pick (the store cannot answer beyond it).
    pub history_secs: Option<u64>,
    /// git tracking status per absolute path, filled when a project is
    /// opened (one exclude stack per worktree, reused for its rows).
    pub track: std::collections::HashMap<PathBuf, swamp_core::ignore::TrackState>,
    /// Store dir, when known: the applied filter is persisted there so it
    /// survives relaunch (`ui_filter.txt`).
    pub store_dir: Option<PathBuf>,
    /// External/shared storage units (#43), for `ViewKind::External`.
    /// Empty until `set_external_units` is called (once, at startup --
    /// detector resolution is disk I/O and never runs on this struct's
    /// own event/render path).
    pub external_units: Vec<swamp_core::external::ExternalUnit>,
    /// The identified interiors of the machine-wide build stores among
    /// `external_units` (`ScopeObservation::store_interiors`), set with
    /// them, from the same pass.
    pub store_interiors: Vec<swamp_core::artifact::NestedArtifact>,
    /// Agent-tool storage units (#91/#100), for `ViewKind::Agents`. Same
    /// startup-only population contract as `external_units`.
    pub agent_units: Vec<swamp_core::agents::AgentUnit>,
    /// A short header clause naming how many roots the *configured*
    /// scope resolves to and the worst non-`Present` status among them
    /// (e.g. `"3 roots (1 missing)"`), or `None` when the scope is a
    /// single present root -- the ordinary case, worth no clause at
    /// all. Derived from `scope::EffectiveScope::roots`
    /// (`scope::RootStatus`, resolved without walking anything), not
    /// from `report_scope`'s own per-root `coverage::RegionStatus`
    /// (that would require making the TUI's own rendered report
    /// multi-root, #50's still-open job -- see DESIGN.md). Populated
    /// once at startup, same contract as `external_units`/`agent_units`.
    pub scope_note: Option<String>,
    /// Set when the report shown is the last observation of an earlier,
    /// overlapping scope (the roots changed since): how many roots that
    /// observation covered. The header says so and points at `R`.
    pub previous_scope_roots: Option<usize>,
    /// The header clause for declared source roots (`2 declared roots`,
    /// `3 declared roots (1 missing)`), and the full lines the help screen
    /// prints. From the scope and the stored coverage; never a walk.
    pub declared_note: Option<String>,
    pub declared_lines: Vec<String>,
    /// The declared roots and their state as of the stored observation,
    /// for the Reclaim view's statement of what its consumer evidence was
    /// checked against. Set with `declared_lines`; never a walk.
    pub declared_roots: Vec<swamp_core::roots::DeclaredRoot>,
    /// What package managers reported in the last scheduled `observe`
    /// (`swamp_core::manager_facts`), read from the store with the rest of
    /// the snapshot. The TUI never asks a manager anything itself.
    pub manager_facts: swamp_core::manager_facts::ManagerFacts,
    /// What the stored disk ledger says (`swamp observe --volume` writes
    /// it), read with the stored snapshot: two small Parquet reads, never
    /// a directory read. The headline's percent and its "everything
    /// else", "system volumes" and "not measured" lines come from it.
    pub ledger: swamp_core::volume_ledger::LedgerReading,
    /// The person has already opened Reclaim or Disk (stored in
    /// `ui_state.json`), so the first-run pointer to them is not shown.
    /// `true` for a fixture app; `finish_startup` sets it from the store.
    pub views_seen: bool,
    /// The Reclaim view built from the stored facts above, kept until one
    /// of them changes: building it joins every unit to its interior.
    reclaim_cache: std::cell::RefCell<Option<std::sync::Arc<swamp_core::reclaim::ReclaimView>>>,
    /// The headline built from the facts above, kept until one of them
    /// (or the clock minute) changes: drawing reuses it instead of
    /// rebuilding it every frame.
    headline_cache:
        std::cell::RefCell<Option<(HeadlineKey, std::sync::Arc<swamp_core::headline::Headline>)>>,
    /// The authorized scope this TUI is showing. Every refresh --
    /// background, post-action re-observe -- goes through it,
    /// so exclusions and external pruning survive an update rather than
    /// applying only to the first render
    /// (`.oh/guardrails/tui-refresh-preserves-scope.md`).
    pub scope: Option<swamp_core::scope::EffectiveScope>,
}

/// One thing a check could not include, and what to do about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockedItem {
    pub name: String,
    pub reason: String,
    pub next: String,
}

/// The next step for a reason a check gave. Unknown reasons still get a
/// safe one: refresh and check again.
pub fn blocked_next_step(reason: &str) -> &'static str {
    let r = reason.to_lowercase();
    if r.contains("human-protected") {
        "remove the protection (swamp protect), then check again"
    } else if r.contains("protection state could not be read") {
        "fix the protect file in the swamp store, then check again"
    } else if r.contains("nothing reclaimable") {
        "open the project with Enter and mark what you want inside it"
    } else if r.contains("category total") {
        "open the category and mark one of its items"
    } else if r.contains("not a path swamp can move") {
        "mark a folder or file listed under it instead"
    } else if r.contains("agent-storage") || r.contains("active") {
        "close the agent session that uses it, then check again"
    } else {
        "press R to refresh, then check again"
    }
}

/// Marked and total units per project (report project name).
pub type MarkStates = std::collections::HashMap<String, (usize, usize)>;

pub struct Operation {
    pub label: &'static str,
    /// Items checked (review) or moved (delete) so far.
    pub completed: usize,
    pub total: usize,
    /// Review: ready. Delete: moved.
    pub succeeded: usize,
    /// Review: blocked. Delete: not moved.
    pub failed: usize,
    /// Plain name of the item being worked on.
    pub current: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub started: Instant,
    pub cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// A review's one open-file snapshot (`lsof`) is being taken;
    /// no group can finish until it returns. When it began.
    pub checking_open_files: Option<Instant>,
}

/// What the `ui_state.json` writer is told.
enum UiStateMsg {
    /// The state to keep; a newer one that has already arrived wins.
    Save(UiState),
    /// Everything before this has been written; tell the sender.
    Flush(std::sync::mpsc::Sender<()>),
}

enum OperationEvent {
    /// The review's open-file snapshot started (`true`) or finished.
    OpenFileCheck(bool),
    /// How many items this check will look at.
    ReviewTotal(usize),
    /// The check is looking at this path now.
    ReviewStep(PathBuf),
    /// One item is ready.
    ReviewReady,
    /// One item is blocked.
    ReviewBlocked,
    Inspected(Vec<String>),
    Progress {
        completed: usize,
        total: usize,
        path: PathBuf,
        outcome: Option<bool>,
    },
    Reviewed {
        marked: BTreeMap<String, MarkedUnit>,
        blocked: Vec<BlockedItem>,
        refusal: Option<String>,
        confirm: bool,
        cancelled: bool,
    },
    Deleted {
        results: Vec<actions::UnitResult>,
        total: usize,
    },
    Failed(String),
}

/// What the TUI remembers between sessions: the applied filter and the
/// sort. Both are choices a human made about how to look at their own
/// machine; asking again every launch is the tool forgetting on purpose.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct UiState {
    #[serde(default)]
    pub filter: String,
    #[serde(default)]
    pub sort: String,
    #[serde(default)]
    pub reverse: bool,
    #[serde(default)]
    pub keep_executables: bool,
    /// The person has opened Tools or Disk once: the "New: Tab opens Tools
    /// and Disk" line is not shown again. Additive: an older swamp ignores
    /// the key and keeps its own.
    #[serde(default)]
    pub views_seen: bool,
}

pub fn load_ui_state(store: &std::path::Path) -> UiState {
    let Ok(store) = swamp_core::fs_gate::StoreDir::at(store) else {
        return UiState::default();
    };
    swamp_core::fs_gate::store::read_json_bytes(swamp_core::fs_gate::store::JsonFile::UiState {
        store: &store,
    })
    .ok()
    .flatten()
    .and_then(|t| serde_json::from_slice(&t).ok())
    .unwrap_or_default()
}

pub fn sort_from_str(s: &str) -> Sort {
    match s {
        "growth" => Sort::Growth,
        "size" => Sort::Size,
        "name" => Sort::Name,
        "type" => Sort::Type,
        "age" => Sort::Age,
        _ => Sort::None,
    }
}

pub fn sort_to_str(s: Sort) -> &'static str {
    match s {
        Sort::Growth => "growth",
        Sort::Size => "size",
        Sort::Name => "name",
        Sort::Type => "type",
        Sort::Age => "age",
        Sort::None => "none",
    }
}

impl App {
    pub fn new(report: Report, root: PathBuf) -> Self {
        Self::new_multi_root(report, vec![root])
    }

    /// Same as [`App::new`], for a report covering more than one root
    /// (#51). `roots` must be non-empty; `roots[0]` becomes `self.root`
    /// (the "one representative path" a handful of call sites still
    /// need -- see `root`'s doc comment). `reports_by_root` starts
    /// empty: a caller that already has each root's own report (the
    /// ordinary startup path, via `report::report_scope_with_parts`)
    /// should populate it directly on the returned `App` before the
    /// first live refresh, so that refresh re-merges from real per-root
    /// data instead of a single placeholder entry.
    pub fn new_multi_root(report: Report, roots: Vec<PathBuf>) -> Self {
        let filter = filter::default_filter();
        let root = roots.first().cloned().unwrap_or_default();
        App {
            cargo_inspection: None,
            cargo_inspection_scroll: 0,
            tool_sheet: None,
            tool_rx: None,
            confirm_drain: false,
            tool_host: swamp_core::tool_removal::Host::system(),
            operation: None,
            operation_rx: None,
            review_progress: None,
            review_cancel: None,
            blocked_log: Vec::new(),
            refusal_ctx: None,
            blocked: Vec::new(),
            blocked_open: false,
            blocked_scroll: 0,
            scroll_offset: std::cell::Cell::new(0),
            frame: 0,
            last_check: None,
            last_check_marks: Vec::new(),
            ui_state_tx: None,
            page: std::cell::Cell::new(10),
            help_scroll: std::cell::Cell::new(0),
            view_cursor: std::collections::HashMap::new(),
            mark_cache: std::sync::Mutex::new(None),
            report,
            root,
            roots,
            reports_by_root: std::collections::HashMap::new(),
            view: ViewKind::Projects,
            filter_text: filter::default_filter_text().to_string(),
            filter,
            filter_error: None,
            editing_filter: false,
            filter_before_edit: String::new(),
            pending: None,
            status: None,
            selected: 0,
            selected_project: None,
            collapsed: HashSet::new(),
            marked: BTreeMap::new(),
            confirm_open: false,
            confirm_base: None,
            held_reload: None,
            help_open: false,
            picker: None,
            completions: Vec::new(),
            refusal: None,
            observing: None,
            observing_started: None,
            external_observer: None,
            lock_poll_rx: None,
            last_result: None,
            observed_label: "just now".to_string(),
            has_index: true,
            store_rebuild: false,
            live_age: false,
            disk_banner: None,
            actor: "human".to_string(),
            sort: Sort::None,
            reverse: false,
            keep_executables: false,
            quit: false,
            width: 0,
            height: 0,
            store_dir: None,
            track: std::collections::HashMap::new(),
            history_secs: None,
            external_units: Vec::new(),
            store_interiors: Vec::new(),
            agent_units: Vec::new(),
            scope_note: None,
            previous_scope_roots: None,
            declared_note: None,
            declared_lines: Vec::new(),
            declared_roots: Vec::new(),
            manager_facts: swamp_core::manager_facts::ManagerFacts::default(),
            ledger: swamp_core::volume_ledger::LedgerReading::NotMeasured,
            views_seen: true,
            reclaim_cache: std::cell::RefCell::new(None),
            headline_cache: std::cell::RefCell::new(None),
            scope: None,
        }
    }

    /// Sets the declared-roots header clause and help lines from the
    /// resolved scope and the stored per-root coverage.
    pub fn set_declared_roots(&mut self, roots: &[swamp_core::roots::DeclaredRoot]) {
        self.declared_note = swamp_core::roots::declared_summary(roots);
        self.declared_lines = swamp_core::roots::render_declared_roots(roots)
            .lines()
            .skip(1)
            .map(str::to_string)
            .collect();
        self.declared_roots = roots.to_vec();
        self.reclaim_cache.borrow_mut().take();
    }

    /// Sets what package managers reported in the last scheduled
    /// `observe`, from the same stored snapshot as the units.
    pub fn set_manager_facts(&mut self, facts: swamp_core::manager_facts::ManagerFacts) {
        self.manager_facts = facts;
        self.reclaim_cache.borrow_mut().take();
    }

    /// Sets what the stored disk ledger says.
    pub fn set_ledger(&mut self, ledger: swamp_core::volume_ledger::LedgerReading) {
        self.ledger = ledger;
        self.headline_cache.borrow_mut().take();
    }

    /// The developer-storage headline over the stored facts this app
    /// holds: a pure function of them (`swamp_core::headline::build`),
    /// so drawing it lists nothing, stats nothing and starts no process.
    pub fn headline(&self) -> std::sync::Arc<swamp_core::headline::Headline> {
        use swamp_core::headline::ScopeKind;
        let explicit = self.scope.as_ref().is_some_and(|s| s.explicit);
        let now = swamp_core::entities::now();
        let key: HeadlineKey = (
            self.report.observed_at,
            self.previous_scope_roots,
            explicit,
            now / 60,
        );
        if let Some((k, h)) = self.headline_cache.borrow().as_ref()
            && *k == key
        {
            return h.clone();
        }
        let built =
            std::sync::Arc::new(swamp_core::headline::build(&swamp_core::headline::Input {
                units: &self.external_units,
                report: &self.report,
                ledger: &self.ledger,
                scope: match (explicit, self.previous_scope_roots) {
                    (true, _) => ScopeKind::ExplicitRoot,
                    (false, Some(roots)) => ScopeKind::Previous { roots },
                    (false, None) => ScopeKind::Current,
                },
                observed_at: self.report.observed_at,
                now,
            }));
        *self.headline_cache.borrow_mut() = Some((key, built.clone()));
        built
    }

    /// The Reclaim view over the stored facts this app holds. A pure
    /// function of them (`swamp_core::reclaim::build`): opening the view
    /// lists nothing, stats nothing and starts no process.
    pub fn reclaim_view(&self) -> std::sync::Arc<swamp_core::reclaim::ReclaimView> {
        let mut cache = self.reclaim_cache.borrow_mut();
        if let Some(v) = cache.as_ref()
            && v.observed_at == self.report.observed_at
        {
            return v.clone();
        }
        let view = std::sync::Arc::new(swamp_core::reclaim::build(
            &swamp_core::reclaim::ReclaimInput {
                units: &self.external_units,
                interiors: &self.store_interiors,
                unowned: &self.report.unowned,
                manager_facts: &self.manager_facts,
                declared_roots: &self.declared_roots,
                explicit_scope: self.scope.as_ref().is_some_and(|s| s.explicit),
                projects: self.report.projects.len(),
                observed_at: self.report.observed_at,
            },
        ));
        *cache = Some(view.clone());
        view
    }

    /// Sets `external_units` for `ViewKind::External` (#43). Called once
    /// at startup, never from the event/render loop -- detector
    /// resolution and measurement are disk I/O.
    pub fn set_external_units(&mut self, units: Vec<swamp_core::external::ExternalUnit>) {
        self.external_units = units;
        self.reclaim_cache.borrow_mut().take();
        self.headline_cache.borrow_mut().take();
    }

    /// Sets the store interiors shown under `ViewKind::External`. Same
    /// contract as `set_external_units`, and always from the same pass.
    pub fn set_store_interiors(&mut self, units: Vec<swamp_core::artifact::NestedArtifact>) {
        self.store_interiors = units;
        self.reclaim_cache.borrow_mut().take();
    }

    /// Sets `agent_units` for `ViewKind::Agents` (#91/#100). Same
    /// startup-only contract as `set_external_units`.
    pub fn set_agent_units(&mut self, units: Vec<swamp_core::agents::AgentUnit>) {
        self.agent_units = units;
    }

    /// Sets `scope_note` from this pass's actual per-root observation
    /// outcome (#51 -- replaces the pre-walk, `scope::RootStatus`-only
    /// version #50's chunk shipped): `RegionStatus` reflects what
    /// `report_scope` actually managed to observe this time (e.g.
    /// `Partial` when part of a `Present` root could not be read during
    /// the walk itself), which a resolved `EffectiveScope` alone cannot
    /// -- that only knows what existed *before* walking. `None` when
    /// there is exactly one region and it is `Complete` (the ordinary
    /// case): every other case -- more than one region, or the one
    /// region not simply `Complete` -- gets one short clause, worst
    /// status first, e.g. `"3 roots (1 missing)"` or `"2 roots (1
    /// inaccessible: permission denied)"`. A `SkippedAsNested` root gets
    /// no `RootCoverage` row at all (`report_scope_with_source` folds it
    /// into its parent's own region), so it is naturally never counted
    /// here either.
    pub fn set_scope_note(&mut self, coverage: &[swamp_core::coverage::RootCoverage]) {
        use swamp_core::coverage::RegionStatus;
        if coverage.len() <= 1
            && coverage
                .iter()
                .all(|c| matches!(c.status, RegionStatus::Complete))
        {
            self.scope_note = None;
            return;
        }
        let total = coverage.len();
        let not_complete = coverage
            .iter()
            .filter(|c| !matches!(c.status, RegionStatus::Complete))
            .count();
        let worst = coverage
            .iter()
            .find_map(|c| match &c.status {
                RegionStatus::Inaccessible { reason } => Some(format!("inaccessible: {reason}")),
                _ => None,
            })
            .or_else(|| {
                coverage.iter().find_map(|c| match &c.status {
                    RegionStatus::Partial { reason } => Some(format!("partial: {reason}")),
                    _ => None,
                })
            })
            .or_else(|| {
                coverage
                    .iter()
                    .any(|c| matches!(c.status, RegionStatus::Excluded))
                    .then(|| "excluded".to_string())
            })
            .or_else(|| {
                coverage
                    .iter()
                    .any(|c| matches!(c.status, RegionStatus::Missing))
                    .then(|| "missing".to_string())
            });
        self.scope_note = Some(match worst {
            Some(w) if not_complete > 0 => format!("{total} roots ({not_complete} {w})"),
            _ => format!("{total} roots"),
        });
    }

    /// Annotates every row of one project with its git tracking status:
    /// one exclude stack per worktree, one lookup per displayed path.
    pub fn annotate_project(&mut self, project_name: &str) {
        let Some(p) = self.report.projects.iter().find(|p| p.name == project_name) else {
            return;
        };
        let mut found = std::collections::HashMap::new();
        for wt in &p.worktrees {
            let Some(lens) = swamp_core::ignore::IgnoreLens::open(&wt.path) else {
                continue;
            };
            for a in &wt.artifacts {
                let rel = a
                    .path
                    .strip_prefix(&wt.path)
                    .map(|r| r.display().to_string())
                    .unwrap_or_default();
                found.insert(a.path.clone(), lens.status(&rel, true));
            }
            if let Some(dirs) = self
                .report
                .dirs_by_worktree
                .as_ref()
                .and_then(|m| m.get(&wt.worktree_id))
            {
                for d in dirs.iter().filter(|d| !d.rel_path.contains('/')) {
                    found.insert(wt.path.join(&d.rel_path), lens.status(&d.rel_path, true));
                }
            }
        }
        self.track.extend(found);
    }

    /// Swap in a fresh report (background observation finished). Rows are
    /// derived from `report` on demand, so nothing else needs rebuilding;
    /// the selection is clamped by `rows()` consumers.
    pub fn replace_report(&mut self, report: Report) {
        // A background observation must not move the cursor: remember
        // which row it is on and put it back on the same row, wherever
        // the new report sorts it.
        let anchor = self.selected_row_key();
        self.report = report;
        self.headline_cache.borrow_mut().take();
        self.restore_selection(anchor);
    }

    /// Replaces exactly one root's contribution to `self.report` (#51):
    /// updates `reports_by_root[root]`, then rebuilds `self.report` from
    /// every root's latest cached report (`report::merge_reports`).
    /// Every other root's rows are re-folded unchanged from their own
    /// cached entry -- a refresh of one root can never erase, stale-mark,
    /// or duplicate another root's data, because that data is never
    /// touched, only re-read from `reports_by_root`.
    #[cfg(test)]
    pub fn replace_report_for_root(&mut self, root: PathBuf, report: Report) {
        let anchor = self.selected_row_key();
        self.reports_by_root.insert(root, report);
        self.report = swamp_core::report::merge_reports(&self.roots, &self.reports_by_root);
        self.restore_selection(anchor);
    }

    /// Installs a worker's result (what the event loop does with every
    /// finished observation): the merged report and per-root cache the
    /// worker already computed, and the unit vectors from the same pass.
    /// No I/O: everything was prepared off the event thread.
    pub fn install_refreshed(&mut self, fresh: RefreshedObservation) {
        self.has_index = true;
        if let Some(m) = fresh.merged {
            self.reports_by_root = m.by_root;
            self.replace_report(m.report);
        }
        // External and agent units come from the same pass, so the agent
        // view is never older than the header.
        if let Some(units) = fresh.external_units {
            self.set_external_units(units);
        }
        if let Some(units) = fresh.store_interiors {
            self.set_store_interiors(units);
        }
        if let Some(units) = fresh.agent_units {
            self.set_agent_units(units);
        }
    }

    /// True while a check or a confirm is open: a new index must wait.
    fn reload_must_wait(&self) -> bool {
        self.confirm_open || self.operation.is_some()
    }

    /// Whether a newer index is waiting for the confirm or check to end.
    pub fn new_data_waiting(&self) -> bool {
        self.held_reload.is_some()
    }

    /// Lands the result of our own observation: now, or after the confirm
    /// or check that is open ends.
    pub fn land_observation(&mut self, fresh: RefreshedObservation) {
        if self.reload_must_wait() {
            self.held_reload = Some(HeldReload::Fresh(Box::new(fresh)));
            return;
        }
        self.install_refreshed(fresh);
        self.observed_label = "just now".into();
        self.drop_marks_missing_from_report();
    }

    fn install_snapshot(
        &mut self,
        snap: swamp_core::report::ReportSnapshot,
        ledger: swamp_core::volume_ledger::LedgerReading,
    ) {
        self.has_index = true;
        self.set_ledger(ledger);
        self.replace_report(snap.report);
        self.set_external_units(snap.external_units);
        self.set_store_interiors(snap.store_interiors);
        self.set_agent_units(snap.agent_units);
        self.set_manager_facts(snap.manager_facts);
        self.observed_label = "just now".into();
        self.status = None;
        self.drop_marks_missing_from_report();
    }

    /// Installs a held reload once no confirm or check is open. True when
    /// the screen's inputs changed.
    pub fn apply_held_reload(&mut self) -> bool {
        if self.reload_must_wait() {
            return false;
        }
        match self.held_reload.take() {
            Some(HeldReload::Fresh(f)) => {
                self.install_refreshed(*f);
                self.observed_label = "just now".into();
                self.drop_marks_missing_from_report();
                true
            }
            Some(HeldReload::Snapshot(snap, ledger)) => {
                self.install_snapshot(*snap, *ledger);
                true
            }
            None => false,
        }
    }

    /// Every path the current report and unit lists can name.
    fn known_unit_keys(&self) -> std::collections::HashSet<String> {
        let mut keys = std::collections::HashSet::new();
        for wt in self.report.projects.iter().flat_map(|p| &p.worktrees) {
            keys.insert(wt.path.display().to_string());
            for a in &wt.artifacts {
                keys.insert(a.path.display().to_string());
            }
        }
        for u in &self.report.unowned {
            keys.insert(u.path_or_object.clone());
        }
        for n in self
            .report
            .nested_artifacts
            .iter()
            .chain(&self.store_interiors)
        {
            keys.insert(n.path.display().to_string());
        }
        for u in &self.external_units {
            keys.insert(u.path.display().to_string());
        }
        for u in &self.agent_units {
            keys.insert(u.path.display().to_string());
        }
        keys
    }

    /// After a new index lands: a mark for something the index no longer
    /// lists is dropped, and the result line says how many.
    fn drop_marks_missing_from_report(&mut self) {
        if self.marked.is_empty() {
            return;
        }
        let known = self.known_unit_keys();
        let gone: Vec<String> = self
            .marked
            .iter()
            .filter(|(k, u)| u.observed_at < self.report.observed_at && !known.contains(*k))
            .map(|(k, _)| k.clone())
            .collect();
        if gone.is_empty() {
            return;
        }
        for k in &gone {
            self.marked.remove(k);
        }
        let n = gone.len();
        self.set_result(format!(
            "The new scan no longer lists {n} marked item{}; unmarked. {} still marked.",
            if n == 1 { "" } else { "s" },
            self.marked.len()
        ));
    }

    /// Identity of the selected row: its unit path when it has one
    /// (stable across re-sorts), otherwise its label.
    fn selected_row_key(&self) -> Option<String> {
        self.rows()
            .into_iter()
            .nth(self.selected)
            .map(|r| r.unit.map(|u| u.0).unwrap_or(r.label))
    }

    fn restore_selection(&mut self, key: Option<String>) {
        let rows = self.rows();
        let found = key.and_then(|k| {
            rows.iter().position(|r| {
                r.unit
                    .as_ref()
                    .map(|u| u.0.clone())
                    .unwrap_or_else(|| r.label.clone())
                    == k
            })
        });
        self.selected = found.unwrap_or_else(|| self.selected.min(rows.len().saturating_sub(1)));
    }

    /// Rows for the current view, honoring the active filter and, for
    /// the tree view, the currently selected project (defaulting to the
    /// first one).
    pub fn rows(&self) -> Vec<Row> {
        // The tree view is a hierarchy (worktree -> artifact), not a flat
        // ranked list, so sort never reorders it -- reordering would break
        // the rail's parent/child adjacency.
        let mut rows = match self.view {
            ViewKind::Projects => model::projects_rows(&self.report, &self.filter),
            ViewKind::Tree => {
                let name = self
                    .selected_project
                    .clone()
                    .or_else(|| self.report.projects.first().map(|p| p.name.clone()));
                return match name {
                    Some(n) => model::tree_rows_with_agents(
                        &self.report,
                        &n,
                        &self.filter,
                        &self.collapsed,
                        &self.track,
                        &self.agent_units,
                    ),
                    None => Vec::new(),
                };
            }
            ViewKind::Builds => model::builds_rows(&self.report, &self.filter),
            ViewKind::Deps => model::deps_rows(&self.report, &self.filter),
            ViewKind::Kinds => model::kinds_rows(&self.report, &self.filter),
            ViewKind::Docker => model::docker_rows_with(&self.report, &self.collapsed),
            ViewKind::Unowned => model::unowned_rows(&self.report),
            ViewKind::Types => model::types_rows(&self.report, &self.filter),
            ViewKind::External => {
                let mut rows = model::external_rows_with(
                    &self.external_units,
                    &self.store_interiors,
                    &self.collapsed,
                    self.report.observed_at,
                );
                // Standalone Cargo targets are their own kind here too.
                rows.extend(model::standalone_target_rows(&self.report));
                rows
            }
            // A hierarchy (a unit, then its folders): sort never
            // reorders it, like the tree.
            ViewKind::Reclaim => {
                let mut rows = model::reclaim_rows(&self.reclaim_view(), &self.collapsed);
                // A unit its manager removes itself also opens that
                // manager's list on Backspace, as in the External view.
                for row in rows.iter_mut().filter(|r| r.depth == 0) {
                    let Some(id) = row.unit.as_ref() else {
                        continue;
                    };
                    row.tool = self
                        .external_units
                        .iter()
                        .find(|u| u.path.display().to_string() == id.0)
                        .and_then(|u| {
                            swamp_core::tool_removal::manager_for_unit(&u.detector_id, &u.path)
                        });
                }
                return rows;
            }
            ViewKind::Agents => model::agent_rows(&self.agent_units),
            // Parts of one disk, largest meaning first as the ledger
            // orders them: sort never reorders them.
            ViewKind::Disk => {
                return model::disk_rows(&self.ledger, self.headline().accounted_sentence());
            }
            ViewKind::DiskGaps => return model::disk_gaps_rows(&self.ledger),
        };
        model::apply_sort(&mut rows, self.sort, self.reverse);
        rows
    }

    pub fn set_sort(&mut self, sort: Sort) {
        self.sort = if self.sort == sort { Sort::None } else { sort };
        self.persist_ui_state();
    }

    /// `r`: flip the order of whatever sort is active.
    pub fn toggle_reverse(&mut self) {
        self.reverse = !self.reverse;
        self.persist_ui_state();
    }

    /// `k`: whether a delete first copies compiled outputs to `bin/`.
    /// The setting is remembered between sessions, so the result line
    /// says which way it went and what that means; a key that changes
    /// something lasting never answers in silence.
    pub fn toggle_keep_executables(&mut self) {
        self.keep_executables = !self.keep_executables;
        self.persist_ui_state();
        self.set_result(if self.keep_executables {
            "Keep executables is now on: release and debug programs are copied to bin/ before their folder goes to Trash. Remembered for next time. k turns it off.".to_string()
        } else {
            "Keep executables is now off: build folders go to Trash as they are. Remembered for next time. k turns it on.".to_string()
        });
    }

    /// Switches view. The cursor of the view being left is remembered and
    /// the one of the view being entered is restored (clamped to its
    /// rows), so `v` or Esc and back never sends the cursor to the top.
    pub fn set_view(&mut self, v: ViewKind) {
        if v == self.view {
            return;
        }
        self.view_cursor.insert(self.view, self.selected);
        self.view = v;
        // Opening either of the two views the first-run hint points at
        // ends the hint, for good.
        if v.section() != Section::Projects && !self.views_seen {
            self.views_seen = true;
            self.persist_ui_state();
        }
        let len = self.rows().len();
        self.selected = self
            .view_cursor
            .get(&v)
            .copied()
            .unwrap_or(0)
            .min(len.saturating_sub(1));
    }

    /// `Tab`, `Shift-Tab` and `1`-`3`: open a section on its first view.
    pub fn set_section(&mut self, s: Section) {
        if s != self.view.section() {
            self.set_view(s.default_view());
        }
    }

    pub fn move_selection(&mut self, delta: i32) {
        let len = self.rows().len();
        if len == 0 {
            self.selected = 0;
            return;
        }
        let cur = self.selected as i32 + delta;
        self.selected = cur.clamp(0, len as i32 - 1) as usize;
    }

    /// PgUp / PgDn: one screenful, keeping one row of overlap.
    pub fn page_selection(&mut self, direction: i32) {
        let step = self.page.get().max(1) as i32;
        self.move_selection(direction * step);
    }

    /// Home.
    pub fn select_first(&mut self) {
        self.selected = 0;
    }

    /// End.
    pub fn select_last(&mut self) {
        self.selected = self.rows().len().saturating_sub(1);
    }

    /// Enter filter editing with the current text kept, so the human edits
    /// what is there (Backspace to trim, type to extend) rather than
    /// starting from an empty line behind a `/`.
    pub fn start_filter_edit(&mut self) {
        self.filter_before_edit = self.filter_text.clone();
        if self.filter_text == "0" {
            self.filter_text.clear();
        }
        self.editing_filter = true;
        self.filter_error = None;
    }

    pub fn open_picker(&mut self) {
        self.picker = Some(crate::picker::Picker::from_report(
            &self.report,
            &self.filter_text,
            self.history_secs,
        ));
    }

    pub fn apply_picker(&mut self) {
        if let Some(p) = self.picker.take() {
            self.filter_text = p.compose();
            self.editing_filter = false;
            self.commit_filter();
        }
    }

    /// `e` in the picker: carry its composed text into the raw line.
    pub fn picker_to_raw_edit(&mut self) {
        if let Some(p) = self.picker.take() {
            self.filter_before_edit = self.filter_text.clone();
            self.filter_text = p.compose();
            if self.filter_text == "0" {
                self.filter_text.clear();
            }
            self.editing_filter = true;
            self.filter_error = None;
        }
    }

    pub fn filter_tab_complete(&mut self) {
        let projects: Vec<String> = self
            .report
            .projects
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let (text, cands) = crate::picker::apply_completion(&self.filter_text, &projects);
        self.filter_text = text;
        self.completions = if cands.len() > 1 { cands } else { Vec::new() };
    }

    pub fn cancel_filter_edit(&mut self) {
        self.completions.clear();
        self.filter_text = std::mem::take(&mut self.filter_before_edit);
        self.editing_filter = false;
    }

    pub fn filter_input(&mut self, c: char) {
        self.filter_text.push(c);
        self.completions.clear();
    }

    /// Rows the current filter would show — for the picker's live count.
    pub fn match_count_for(&self, text: &str) -> Option<usize> {
        let f = crate::filter::parse(text).ok()?;
        let mut probe = App::new(self.report.clone(), self.root.clone());
        probe.view = self.view;
        probe.selected_project = self.selected_project.clone();
        probe.filter = f;
        Some(probe.rows().len())
    }

    pub fn filter_backspace(&mut self) {
        self.filter_text.pop();
    }

    pub fn commit_filter(&mut self) {
        self.editing_filter = false;
        self.completions.clear();
        match filter::parse(&self.filter_text) {
            Ok(f) => {
                self.filter = f;
                self.filter_error = None;
                self.persist_filter();
            }
            Err(e) => {
                self.filter_error = Some(e);
                // previous filter stays applied
            }
        }
        self.selected = 0;
        self.view_cursor.clear();
    }

    pub fn clear_filter(&mut self) {
        self.filter_text = "0".to_string();
        self.filter = Filter::default();
        self.filter_error = None;
        self.selected = 0;
        self.view_cursor.clear();
        self.persist_filter();
    }

    fn persist_filter(&mut self) {
        self.persist_ui_state();
    }

    /// Hands the current filter, sort and `k` to the writer thread and
    /// returns at once. Writes never wait on the event thread; when several
    /// arrive together only the newest is written.
    fn persist_ui_state(&mut self) {
        let Some(store) = self.store_dir.clone() else {
            return;
        };
        let state = UiState {
            filter: self.filter_text.clone(),
            sort: sort_to_str(self.sort).to_string(),
            reverse: self.reverse,
            keep_executables: self.keep_executables,
            views_seen: self.views_seen,
        };
        if self.ui_state_tx.is_none() {
            let (tx, rx) = std::sync::mpsc::channel::<UiStateMsg>();
            crate::worker::spawn(move || {
                let write = |state: &UiState| {
                    if let Ok(store) = swamp_core::fs_gate::StoreDir::at(&store) {
                        let _ = swamp_core::fs_gate::store::write_json(
                            swamp_core::fs_gate::store::JsonFile::UiState { store: &store },
                            state,
                        );
                    }
                };
                let mut waiting: Vec<std::sync::mpsc::Sender<()>> = Vec::new();
                while let Ok(first) = rx.recv() {
                    let mut latest = None;
                    let mut msgs = vec![first];
                    msgs.extend(rx.try_iter());
                    for m in msgs {
                        match m {
                            UiStateMsg::Save(s) => latest = Some(s),
                            UiStateMsg::Flush(done) => waiting.push(done),
                        }
                    }
                    if let Some(s) = latest {
                        write(&s);
                    }
                    for done in waiting.drain(..) {
                        let _ = done.send(());
                    }
                }
            });
            self.ui_state_tx = Some(tx);
        }
        if let Some(tx) = &self.ui_state_tx {
            let _ = tx.send(UiStateMsg::Save(state));
        }
    }

    /// Waits (a moment at most) for the writer to finish what it was given,
    /// so the last choice survives quitting. Called once, on the way out.
    pub fn flush_ui_state(&self) {
        if let Some(tx) = &self.ui_state_tx {
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            if tx.send(UiStateMsg::Flush(done_tx)).is_ok() {
                let _ = done_rx.recv_timeout(Duration::from_secs(2));
            }
        }
    }

    fn selected_row(&self) -> Option<Row> {
        self.rows().into_iter().nth(self.selected)
    }

    /// Toggles a worktree's collapsed state (tree view only).
    /// `→`: go one level in. On an expandable row that means expanding
    /// it; in the projects view it opens the project; otherwise nothing,
    /// because there is nowhere further in.
    pub fn enter_row(&mut self) {
        if self.view == ViewKind::Projects {
            self.drill_into_selected();
            return;
        }
        if self.selected_row().is_some_and(|r| r.expandable) {
            self.toggle_expand();
        }
    }

    /// `←`: go one level out. Collapse an expanded row where that is what
    /// "out" means; otherwise leave the view, the same as `Esc`. At the
    /// projects view there is no level above, so it does nothing rather
    /// than quitting.
    pub fn leave_row(&mut self) {
        if self.view == ViewKind::Projects {
            return;
        }
        let expanded = self
            .selected_row()
            .is_some_and(|r| r.expandable && r.collapsed_children.is_none());
        if expanded {
            self.toggle_expand();
        } else {
            self.set_view(ViewKind::Projects);
        }
    }

    pub fn toggle_expand(&mut self) {
        // The tree, and the two views whose rows open onto an identified
        // interior: a machine-wide store (External) and a BuildKit
        // builder (Docker).
        if !matches!(
            self.view,
            ViewKind::Tree | ViewKind::External | ViewKind::Docker | ViewKind::Reclaim
        ) {
            return;
        }
        if let Some(key) = self.selected_row().and_then(|r| r.expansion_key) {
            if !self.collapsed.remove(&key) {
                self.collapsed.insert(key);
            }
            self.selected = self.selected.min(self.rows().len().saturating_sub(1));
        }
    }

    /// Enters a project from the projects view into its tree.
    pub fn drill_into_selected(&mut self) {
        if self.confirm_open {
            if self.height != 0 && !self.confirm_fits(self.width, self.height) {
                return;
            }
            self.confirm_delete();
            return;
        }
        // A Reclaim row opens onto the unit's folders; there is no
        // project to drill into from it.
        if self.view == ViewKind::Reclaim {
            self.enter_row();
            return;
        }
        if self.view == ViewKind::Projects
            && let Some(row) = self.selected_row()
        {
            // Strip the worktree count and the `[rs][js]` ecosystem tags.
            let shown = row.label.split("  · ").next().unwrap_or("").trim();
            let shown = shown
                .rsplit("] ")
                .next()
                .unwrap_or(shown)
                .trim()
                .to_string();
            // The row shows `owner/repo`; the report keys on the project's
            // own name, so map back rather than searching for the label.
            let name = self
                .report
                .projects
                .iter()
                .find(|p| model::project_display_name(p) == shown)
                .map(|p| p.name.clone())
                .unwrap_or(shown);
            // Source rows start collapsed; the human opens the one they
            // care about with →.
            if let Some(p) = self.report.projects.iter().find(|p| p.name == name) {
                for wt in &p.worktrees {
                    self.collapsed
                        .insert(format!("source:{}", wt.path.display()));
                }
            }
            self.annotate_project(&name);
            // Show profiles and categories immediately, with individual groups
            // available inside the same tree rather than a separate view.
            for unit in &self.report.nested_artifacts {
                if unit.role == swamp_core::artifact::ArtifactRole::Profile {
                    self.collapsed
                        .insert(format!("layout:{}", unit.path.display()));
                    for kind in ["cache", "runnable", "tests", "examples", "scripts"] {
                        self.collapsed
                            .insert(format!("cleanup:{kind}:{}", unit.path.display()));
                    }
                }
                if unit.is_dir
                    && !matches!(
                        unit.role,
                        swamp_core::artifact::ArtifactRole::Container
                            | swamp_core::artifact::ArtifactRole::Profile
                    )
                {
                    self.collapsed
                        .insert(format!("cargo:{}", unit.path.display()));
                }
            }
            self.selected_project = Some(name);
            // A different project's tree starts at its top.
            self.view_cursor.remove(&ViewKind::Tree);
            self.set_view(ViewKind::Tree);
        }
    }

    /// Backspace: mark the selected row for deletion, or refuse inline
    /// with the reason.
    /// `a`: mark every row in this view the tool knows how to act on,
    /// so "everything we know of here" is one gesture and still one
    /// confirm. Rows it cannot act on are left alone, and the refusal
    /// names how many and why.
    pub fn mark_all_in_view(&mut self) {
        let rows = self.rows();
        let mut refused: Vec<String> = Vec::new();
        let mut marked = 0usize;
        // Agents view (#91/#100/#101): `model::agent_rows` sets `unit`
        // on every row, protected/unsupported ones included, but never
        // sets `kind` (there is no `ArtifactKind` for an agent-storage
        // unit). Counted separately so the footer can say how many were
        // skipped and why, rather than folding it into the single
        // static per-kind refusal strings below.
        let mut agent_skipped = 0usize;
        let mut path_skipped = 0usize;
        let mut individual = 0usize;
        for row in rows {
            // A path no cleanup rule covers is marked one at a time.
            if row.individual_only {
                individual += 1;
                continue;
            }
            // Reclaim and External: each top-level row is a real path. Its
            // listed folders are inside it (marking both would overlap) and
            // stay for Space on the folder itself. A row already marked is
            // left marked: `A` adds, it does not toggle.
            if matches!(
                self.view,
                ViewKind::Reclaim | ViewKind::External | ViewKind::Disk | ViewKind::DiskGaps
            ) && row.kind.is_none()
                && row.project.is_none()
                && let Some(id) = row.unit.clone()
            {
                if row.depth > 0 || self.marked.contains_key(&id.0) {
                    continue;
                }
                self.mark_row(&row);
                if self.marked.contains_key(&id.0) {
                    marked += 1;
                } else {
                    path_skipped += 1;
                }
                continue;
            }
            let Some(kind) = row.kind.clone() else {
                // Projects view: each row stands for a whole project.
                if let Some(project) = row.project.clone() {
                    // Bulk marking never reaches for a checkout: `A` over
                    // a screen of projects would otherwise queue every
                    // checkout under the root behind one Enter.
                    match self.mark_project(&project, false) {
                        ProjectMark::Nothing => {
                            let why = "nothing reclaimable in this project";
                            let name = self.row_display_name(&row);
                            self.note_blocked(name, why);
                            refused.push(why.into())
                        }
                        ProjectMark::Cleared => {}
                        ProjectMark::Marked(n) => marked += n,
                    }
                } else if row.unit.is_some() {
                    // `mark_row` already knows how to refuse a
                    // protected/unsupported/active agent-storage row
                    // (via `actions::propose_agents`'s own refusal
                    // text) -- reused here instead of duplicating that
                    // logic, so Shift+A gives the same reason Backspace
                    // would on the same row, not a generic one.
                    let before = self.marked.len();
                    self.mark_row(&row);
                    if self.marked.len() > before {
                        marked += 1;
                    } else {
                        agent_skipped += 1;
                    }
                }
                continue;
            };
            match crate::units::markable(&kind) {
                Ok(()) => {
                    if row.unit.is_some() {
                        self.mark_row(&row);
                        marked += 1;
                    }
                }
                Err(why) => {
                    let name = self.row_display_name(&row);
                    self.note_blocked(name, why);
                    refused.push(why.into())
                }
            }
        }
        if individual > 0 && marked > 0 {
            refused.push(format!(
                "{individual} row{} no cleanup rule covers {} left out: Space marks one at a time",
                if individual == 1 { "" } else { "s" },
                if individual == 1 { "was" } else { "were" }
            ));
        }
        if path_skipped > 0 {
            refused.push(format!(
                "{path_skipped} row{} could not be marked (b lists each with its reason)",
                if path_skipped == 1 { "" } else { "s" }
            ));
        }
        if agent_skipped > 0 {
            refused.push(format!(
                "{agent_skipped} agent-storage row{} protected, unsupported, or active; skipped",
                if agent_skipped == 1 { " is" } else { "s are" }
            ));
        }
        if marked == 0 {
            let why = summarize_refusals(&refused)
                .unwrap_or_else(|| "nothing in this view can be acted on".into());
            self.set_refusal(&why);
            return;
        }
        // Some rows were left alone (agent-storage skip, or an empty
        // project) even though at least one row *was* marked: say so,
        // rather than silently proceeding to a confirm that looks like
        // it covers everything the human saw on screen.
        if let Some(msg) = summarize_refusals(&refused) {
            self.set_refusal(&msg);
        }
        self.confirm_open = true;
    }

    pub fn mark_selected(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        self.mark_row(&row);
    }

    /// Mark every reclaimable artifact in one project: dependency
    /// trees, build output, caches, and its Docker objects. The
    /// checkout, its `.git`, and its source tree are never included —
    /// removing those is a deliberate act one level in, on the row that
    /// names the worktree and carries its dirty/unpushed warnings.
    ///
    /// The project's own filter state is deliberately not applied: the
    /// projects row shows the project's whole size, so marking it acts
    /// on the whole project rather than on whatever the current filter
    /// happens to show. Returns how many units it newly marked; a second
    /// press on a fully marked project clears it and returns 0.
    fn mark_project(&mut self, project: &str, include_checkouts: bool) -> ProjectMark {
        let units = self.project_units(project, include_checkouts);
        if units.is_empty() {
            return ProjectMark::Nothing;
        }
        let marked_already = |app: &Self, r: &Row| {
            r.unit
                .as_ref()
                .is_some_and(|u| app.marked.contains_key(&u.0))
        };
        if units.iter().all(|r| marked_already(self, r)) {
            for r in &units {
                if let Some(u) = &r.unit {
                    self.marked.remove(&u.0);
                }
            }
            return ProjectMark::Cleared;
        }
        let mut newly = 0usize;
        for r in units {
            if marked_already(self, &r) {
                continue;
            }
            self.mark_row(&r);
            newly += 1;
        }
        ProjectMark::Marked(newly)
    }

    /// How many of a project's markable units are marked, and how many it
    /// has: what a projects-view row draws (`✗` all, `~n/m` some). The unit
    /// set is the one Space/Backspace on the row would mark, checkouts
    /// included when nothing rebuildable exists.
    pub fn project_mark_state(&self, project: &str) -> (usize, usize) {
        if self.marked.is_empty() {
            return (0, 0);
        }
        let mut units = self.project_units(project, false);
        if units.is_empty() {
            units = self.project_units(project, true);
        }
        let marked = units
            .iter()
            .filter(|r| {
                r.unit
                    .as_ref()
                    .is_some_and(|u| self.marked.contains_key(&u.0))
            })
            .count();
        (marked, units.len())
    }

    /// `project_mark_state` for every project that owns a marked unit,
    /// cached until the marks or the report change. A drawn frame asks for
    /// this, so it must cost nothing when nothing changed.
    pub fn project_mark_states(&self) -> std::sync::Arc<MarkStates> {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.marked.keys().for_each(|k| k.hash(&mut h));
        (self.report.observed_at, self.report.projects.len()).hash(&mut h);
        let sig = h.finish();
        if let Ok(cache) = self.mark_cache.lock()
            && let Some((s, states)) = cache.as_ref()
            && *s == sig
        {
            return states.clone();
        }
        let mut owners: Option<HashSet<String>> = Some(HashSet::new());
        for u in self.marked.values() {
            match names::project_key_of(&self.report, &u.path) {
                Some(p) => {
                    if let Some(o) = owners.as_mut() {
                        o.insert(p);
                    }
                }
                // Not under any checkout (docker, unowned): any project
                // may own it, so look at all of them.
                None => owners = None,
            }
        }
        let mut states = MarkStates::new();
        if !self.marked.is_empty() {
            for p in &self.report.projects {
                if owners.as_ref().is_none_or(|o| o.contains(&p.name)) {
                    states.insert(p.name.clone(), self.project_mark_state(&p.name));
                }
            }
        }
        let states = std::sync::Arc::new(states);
        if let Ok(mut cache) = self.mark_cache.lock() {
            *cache = Some((sig, states.clone()));
        }
        states
    }

    /// The rows a mark on this project row acts on.
    fn project_units(&self, project: &str, include_checkouts: bool) -> Vec<Row> {
        let rows = model::tree_rows(
            &self.report,
            project,
            &Filter::default(),
            &std::collections::HashSet::new(),
            &self.track,
        );
        let mut units: Vec<Row> = rows
            .iter()
            .filter(|r| {
                r.unit.is_some()
                    && !r.individual_only
                    && r.kind
                        .as_ref()
                        .is_some_and(|k| crate::units::markable(k).is_ok())
            })
            .cloned()
            .collect();
        if units.is_empty() && include_checkouts {
            // A project with nothing rebuildable in it -- a checkout and
            // its source, and that is all. Refusing here was the whole
            // complaint: the row is not actionable and the human has to
            // open it to reach the only thing there is. So the row
            // offers the checkouts themselves, each carrying its own
            // dirty / unpushed / untracked-content warnings onto the
            // confirm line.
            units = rows
                .iter()
                .filter(|r| r.worktree.is_some() && r.unit.is_some())
                .cloned()
                .collect();
        }
        units
    }

    /// The mark decision for one row (testable without a selection).
    ///
    /// Anything with a path can be marked: an artifact, a Source
    /// directory, a linked worktree, a whole checkout. There is no bar —
    /// the human decides. What the tool owes them is the facts, so each
    /// unit carries its warnings and the confirm line states them before
    /// Enter. Docker objects are the one exception: there is no
    /// implementation to remove them yet, so marking one would be a lie.
    pub fn mark_row(&mut self, row: &Row) {
        let name = self.row_display_name(row);
        let outer = self.refusal_ctx.replace(name);
        self.mark_row_inner(row);
        self.refusal_ctx = outer;
    }

    /// A row's plain name: a unit by what it is, anything else by its label.
    fn row_display_name(&self, row: &Row) -> String {
        match &row.unit {
            Some(u) => names::friendly_unit_name(&self.report, Path::new(&u.0)),
            None => row.label.trim().to_string(),
        }
    }

    /// A refusal that also counts as blocked in the check in progress.
    fn refuse(&mut self, msg: &str) {
        let name = self.refusal_ctx.clone().unwrap_or_default();
        self.note_blocked(name, msg);
        self.set_refusal(msg);
    }

    fn note_blocked(&mut self, name: String, reason: &str) {
        self.blocked_log.push(BlockedItem {
            name,
            reason: reason.to_string(),
            next: blocked_next_step(reason).to_string(),
        });
        if let Some(tx) = &self.review_progress {
            let _ = tx.send(OperationEvent::ReviewBlocked);
        }
    }

    fn mark_row_inner(&mut self, row: &Row) {
        if self
            .review_cancel
            .as_ref()
            .is_some_and(|c| c.load(std::sync::atomic::Ordering::SeqCst))
        {
            return;
        }
        if let Some(key) = row
            .expansion_key
            .as_deref()
            .filter(|k| model::is_cleanup_selection(&self.report, k))
        {
            let members: Vec<_> = model::cleanup_members(&self.report, key)
                .into_iter()
                .cloned()
                .collect();
            if members.is_empty() {
                self.refuse("No supported cleanup members remain; refresh the report.");
                return;
            }
            if members
                .iter()
                .all(|u| self.marked.contains_key(&u.path.display().to_string()))
            {
                for u in members {
                    self.marked.remove(&u.path.display().to_string());
                }
                return;
            }
            // A failed member must not leave a silently partial group selected.
            let original = self.marked.clone();
            for u in members {
                let id = UnitId::for_artifact(&u.path);
                if self.marked.contains_key(&id.0) {
                    continue;
                }
                let mut leaf = row.clone();
                leaf.expansion_key = None;
                leaf.unit = Some(id.clone());
                leaf.label = u.path.display().to_string();
                leaf.bytes = u.bytes;
                leaf.kind = Some(swamp_core::report::ArtifactKind::BuildOutput);
                self.mark_row(&leaf);
                if !self.marked.contains_key(&id.0) {
                    self.marked = original;
                    return;
                }
            }
            return;
        }
        if let Some(id) = row.unit.clone()
            && let Some(target) = self.path_target(&id.0, row)
        {
            self.mark_reclaim_row(row, &id, target);
            return;
        }
        let Some(unit_id) = row.unit.clone() else {
            if row.signals.iter().any(|s| s == "category") {
                self.refuse("Category total: pick one of the items inside it. Nothing changed.");
                return;
            }
            if row.signals.iter().any(|s| s == "blocked") {
                self.refuse(
                    "Not a path swamp can move: this row stands for an aggregate or a daemon's record, not a folder or file. Nothing changed.",
                );
                return;
            }
            // A projects-view row is a whole project rather than one
            // path. Marking it means marking what that project can give
            // back, so the human does not have to open it first.
            if let Some(project) = row.project.clone() {
                if self.mark_project(&project, true) == ProjectMark::Nothing {
                    self.refuse("nothing reclaimable in this project");
                }
                return;
            }
            self.refuse("nothing to delete on this row");
            return;
        };
        if let Some(tx) = &self.review_progress {
            let _ = tx.send(OperationEvent::ReviewStep(PathBuf::from(&unit_id.0)));
        }
        // The ignored/untracked rows report bytes scattered across a
        // checkout under the worktree's own path. Marking one would
        // queue the whole checkout, which is not what the row says.
        if let Some(kind) = &row.kind
            && matches!(
                kind,
                swamp_core::report::ArtifactKind::Ignored
                    | swamp_core::report::ArtifactKind::Untracked
            )
            && let Err(why) = crate::units::markable(kind)
        {
            self.refuse(why);
            return;
        }
        // A Docker object is removed through the daemon, not moved to
        // Trash. Which command that is depends on the kind, and build
        // cache has none.
        let docker = match row.kind {
            Some(swamp_core::report::ArtifactKind::DockerImage) => {
                Some(swamp_core::docker::Removal::Image {
                    id: unit_id.0.clone(),
                })
            }
            Some(swamp_core::report::ArtifactKind::DockerVolume) => {
                Some(swamp_core::docker::Removal::Volume {
                    name: unit_id.0.clone(),
                })
            }
            _ => None,
        };
        if self.marked.remove(&unit_id.0).is_some() {
            return; // toggle off
        }
        // Human keep/protect intent, checked for **every** markable row
        // before anything else.
        //
        // This used to be reached only for the two row kinds that
        // happen to propose through core (`nested_artifacts` and agent
        // units), so an ordinary artifact or unowned row containing a
        // protected file marked cleanly and was refused much later, at
        // execution. The integration owner's 2026-09-21 mutation check
        // is why this is a gate rather than a side effect of proposing:
        // a one-directional protection predicate survived every test
        // precisely because no path exercised "ordinary row *contains* a
        // protected descendant".
        //
        // Both directions, from the one predicate
        // (`.oh/guardrails/protection-fails-closed.md`); protection
        // state that cannot be read is *unknown*, so it refuses too.
        if let Some(store) = self.store_dir.clone() {
            let candidate = PathBuf::from(&unit_id.0);
            match swamp_core::agents::load_protect(&store) {
                Ok(protected) => {
                    if let Some(reason) = protected.conflict(&candidate) {
                        self.refuse(&format!(
                            "human-protected path (swamp protect): {reason}; remove protection \
                             first if this unit should be actionable"
                        ));
                        return;
                    }
                }
                Err(e) => {
                    self.refuse(&format!(
                        "protection state could not be read, so nothing may be marked: {e}"
                    ));
                    return;
                }
            }
        }
        let mut warnings: Vec<String> = Vec::new();
        let worktree = row.worktree.clone().map(|wt| {
            let whole_checkout = !wt.linked;
            if whole_checkout && wt.remote.is_none() {
                warnings.push("no remote to restore from".into());
            }
            if wt.dirty == Some(true) {
                warnings.push("dirty".into());
            }
            match wt.unpushed {
                Some(n) if n > 0 => warnings.push(format!("{n} unpushed")),
                None => warnings.push("unpushed unknown".into()),
                _ => {}
            }
            if wt.locked == Some(true) {
                warnings.push("locked".into());
            }
            if whole_checkout {
                for (p, b) in swamp_core::ignore::untracked_content(&wt.path, 3, 100_000) {
                    let rel = p.strip_prefix(&wt.path).unwrap_or(&p).display().to_string();
                    warnings.push(format!("{rel} untracked {}", model::human_bytes(b)));
                }
            }
            crate::actions::WorktreeTerms {
                merge_complete: wt.merge_complete,
                pr: wt.pr.clone(),
                whole_checkout,
                remote: wt.remote.clone(),
            }
        });
        match row.track {
            Some(swamp_core::ignore::TrackState::Untracked) => {
                warnings.push("untracked: in no version control".into())
            }
            Some(swamp_core::ignore::TrackState::Tracked) if row.worktree.is_none() => {
                warnings.push("tracked source".into())
            }
            _ => {}
        }
        if row.kind == Some(swamp_core::report::ArtifactKind::Git) {
            warnings.push("git object store: history goes with it".into());
        }
        match row.kind {
            Some(swamp_core::report::ArtifactKind::DockerVolume) => warnings.push(
                "docker volume: its contents exist nowhere else, and this does not go to Trash"
                    .into(),
            ),
            Some(swamp_core::report::ArtifactKind::DockerImage) => warnings
                .push("docker image: permanent, comes back only by pulling or rebuilding".into()),
            Some(swamp_core::report::ArtifactKind::Loose) => {
                warnings.push("no project claims these bytes".into())
            }
            _ => {}
        }
        // #60/#61: consumer/current-use/recovery/reclaimability facts
        // from the row's own decision evidence (`model::Row::evidence`,
        // populated from the same `ArtifactRow`/`AgentUnit` every other
        // row field already comes from) -- distinct from the git-status
        // warnings above, and covering every markable row uniformly
        // rather than only the cargo-container-member/agent-storage
        // cases that separately call `actions::propose`/`propose_agents`
        // below.
        warnings.extend(swamp_core::render::evidence_warnings(&row.evidence));
        let label = row.label.trim().to_string();
        let unit_path = PathBuf::from(&unit_id.0);
        let cargo_unit = if self
            .report
            .nested_artifacts
            .iter()
            .any(|u| u.path == unit_path)
        {
            // `propose_checking_protection`, not `propose`: human
            // keep/protect intent has to refuse here, at the moment the
            // human marks the row, not silently at execution.
            //
            // The integration owner's 2026-09-21 mutation check is why
            // this is spelled out: a one-directional protection check
            // survived every test because nothing exercised an
            // *ordinary* row that contained a protected descendant, and
            // this was the path that would have caught it. The live
            // protect list is reloaded from `self.report.store_dir`
            // inside that function (one small control file, the same
            // cost as the `stat`s `propose` already does here), so a
            // protection added since startup is honoured.
            match swamp_core::actions::propose_checking_protection(
                &self.report,
                None,
                std::slice::from_ref(&unit_path),
                "human:tui",
                &[],
            ) {
                Ok(units) => {
                    warnings.extend(units.iter().flat_map(|u| u.warnings().iter().cloned()));
                    units.into_iter().next()
                }
                Err(e) => {
                    self.refuse(&e.to_string());
                    return;
                }
            }
        } else {
            None
        };
        // A standalone Cargo target directory (#171) is planned through
        // core like any other build output, so its confirm line says what
        // it is and what a rebuild costs, and carries the fresh open-file
        // reading taken when it was planned. Execution is the ordinary
        // path Trash move; nothing here changes what moves.
        if self.report.unowned.iter().any(|u| {
            u.reason == swamp_core::report::UnownedReason::StandaloneCargoTarget
                && Path::new(&u.path_or_object) == unit_path.as_path()
        }) {
            match swamp_core::actions::propose_checking_protection(
                &self.report,
                None,
                std::slice::from_ref(&unit_path),
                "human:tui",
                &[],
            ) {
                Ok(units) => {
                    for u in &units {
                        warnings.extend(u.warnings().iter().cloned());
                        warnings.extend(swamp_core::render::evidence_warnings(u.evidence()));
                    }
                }
                Err(e) => {
                    self.refuse(&e.to_string());
                    return;
                }
            }
        }
        // Agent-storage unit (#101's TUI wiring): every `agent_rows` row
        // carries `unit: Some(...)` regardless of whether it is
        // protected or has a supported action, so this branch is reached
        // for a protected/unsupported row too -- `propose_agents`'s own
        // refusal text (protected category, no supported action for this
        // category yet, active session...) becomes the footer, never a
        // generic "nothing to delete on this row" for a unit the human
        // can plainly see in the Agents view.
        let agent_unit_observed_at = self
            .agent_units
            .iter()
            .find(|u| u.path == unit_path)
            .map(|u| u.observed_at);
        let agent_unit = if agent_unit_observed_at.is_some() {
            match swamp_core::actions::propose_agents(
                &self.agent_units,
                std::slice::from_ref(&unit_path),
                "human:tui",
            ) {
                Ok(units) => {
                    warnings.extend(units.iter().flat_map(|u| u.warnings().iter().cloned()));
                    units.into_iter().next()
                }
                Err(e) => {
                    self.refuse(&e.to_string());
                    return;
                }
            }
        } else {
            None
        };
        // The worktree this unit lives in: where `bin/` goes when keeping
        // executables. A worktree row is its own worktree.
        let worktree_path = self
            .report
            .projects
            .iter()
            .flat_map(|p| p.worktrees.iter())
            .filter(|wt| unit_path.starts_with(&wt.path))
            .max_by_key(|wt| wt.path.as_os_str().len())
            .map(|wt| wt.path.clone())
            .unwrap_or_else(|| {
                unit_path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_default()
            });
        let selected_bytes = cargo_unit
            .as_ref()
            .or(agent_unit.as_ref())
            .map(|u| u.bytes())
            .unwrap_or(row.bytes);
        self.marked.insert(
            unit_id.0.clone(),
            MarkedUnit {
                cargo_unit: cargo_unit.filter(|u| u.cargo_group().is_some()),
                agent_unit,
                reclaim: None,
                path: unit_path,
                docker,
                worktree_path,
                bytes: selected_bytes,
                observed_at: agent_unit_observed_at.unwrap_or(self.report.observed_at),
                worktree,
                label,
                warnings,
            },
        );
        if let Some(tx) = &self.review_progress {
            let _ = tx.send(OperationEvent::ReviewReady);
        }
    }

    /// What marking this row's path needs to know, when the row is a real
    /// path that goes to the reviewed Trash flow without a project behind
    /// it: a row of the Reclaim view (a unit, or one of its listed
    /// folders) reached from Reclaim or External, a store interior folder
    /// under no checkout, or a measured folder in the Disk views. A
    /// standalone Cargo target keeps its own reviewed flow.
    fn path_target(&self, id: &str, row: &Row) -> Option<swamp_core::reclaim_trash::ReclaimTarget> {
        use swamp_core::reclaim_trash::{ReclaimTarget, find_target};
        let path = Path::new(id);
        if matches!(self.view, ViewKind::Reclaim | ViewKind::External) {
            let standalone = self.report.unowned.iter().any(|u| {
                u.reason == swamp_core::report::UnownedReason::StandaloneCargoTarget
                    && Path::new(&u.path_or_object) == path
            });
            if !standalone && let Ok(t) = find_target(&self.reclaim_view(), path) {
                return Some(t);
            }
        }
        // A build-store interior no checkout owns: there is no project to
        // plan it through, so it is the path and what its adapter said.
        let owned = self
            .report
            .projects
            .iter()
            .flat_map(|p| p.worktrees.iter())
            .any(|wt| path != wt.path && swamp_core::scope::under(path, &wt.path));
        if !owned
            && let Some(n) = self
                .report
                .nested_artifacts
                .iter()
                .chain(self.store_interiors.iter())
                .find(|n| n.present && n.path == path && n.reported_by.is_none())
        {
            return Some(ReclaimTarget::for_path(
                n.path.clone(),
                "store interior",
                Some(n.bytes),
                n.consequence.as_deref(),
                n.advisories(),
            ));
        }
        if matches!(self.view, ViewKind::Disk | ViewKind::DiskGaps) && path.is_absolute() {
            return Some(ReclaimTarget::for_path(
                path.to_path_buf(),
                "outside developer storage",
                Some(row.bytes),
                None,
                vec![format!(
                    "measured by the disk ledger ({}); swamp has no record of what uses it",
                    row.signals.first().cloned().unwrap_or_default()
                )],
            ));
        }
        None
    }

    /// Space/Backspace on a Reclaim or External row: a real path goes to
    /// the same reviewed Trash flow as every other unit. What refuses is
    /// only that it is not a real entry, the person's own `swamp protect`
    /// mark, or an overlap with another mark; everything else swamp knows
    /// or does not know is a line on the confirm. Marking again unmarks.
    fn mark_reclaim_row(
        &mut self,
        row: &Row,
        id: &UnitId,
        target: swamp_core::reclaim_trash::ReclaimTarget,
    ) {
        if self.marked.remove(&id.0).is_some() {
            return; // toggle off
        }
        let path = PathBuf::from(&id.0);
        let observed_at = self.report.observed_at;
        if let Some(other) = self
            .marked
            .values()
            .find(|m| swamp_core::scope::overlapping(&m.path, &path))
        {
            let msg = format!(
                "overlaps {}, already marked: unmark one of them, or mark the larger one alone",
                other.path.display()
            );
            self.refuse(&msg);
            return;
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let review = match swamp_core::reclaim_trash::review(
            &target,
            self.store_dir.as_deref(),
            home.as_deref(),
        ) {
            Ok(r) => r,
            Err(e) => {
                self.refuse(&e);
                return;
            }
        };
        let worktree_path = path.parent().map(Path::to_path_buf).unwrap_or_default();
        self.marked.insert(
            id.0.clone(),
            MarkedUnit {
                cargo_unit: None,
                agent_unit: None,
                reclaim: Some(actions::ReclaimMark {
                    reviewed: review.reviewed,
                    category: target.category.clone(),
                    store: self.store_dir.clone(),
                }),
                path,
                docker: None,
                worktree_path,
                bytes: target.bytes.unwrap_or(0),
                observed_at,
                worktree: None,
                label: row.label.trim().to_string(),
                warnings: review.warnings,
            },
        );
        if let Some(tx) = &self.review_progress {
            let _ = tx.send(OperationEvent::ReviewReady);
        }
    }

    /// Whether the row under the cursor has a mark to make: what the key
    /// legend names `Space mark` and `Backspace trash` for in Reclaim. A
    /// row that is not a folder shows its reason in the detail pane.
    pub fn selected_row_markable(&self) -> bool {
        self.selected_row().is_some_and(|r| r.unit.is_some())
    }

    pub fn cancel_operation(&mut self) {
        if let Some(op) = &self.operation {
            op.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            crate::worker::spawn(swamp_core::fs_gate::spawn::kill_all_children);
        }
    }

    /// Explicit inspection only: ordinary observation and drawing never call it.
    pub fn inspect_selected_cargo_profile(&mut self) {
        if self.operation.is_some() {
            return;
        }
        let path = self.selected_row().and_then(|row| {
            row.expansion_key
                .as_deref()
                .and_then(|key| key.strip_prefix("cargo:"))
                .map(PathBuf::from)
                .or_else(|| row.unit.map(|id| PathBuf::from(id.0)))
        });
        let profile = path.and_then(|path| {
            self.report
                .nested_artifacts
                .iter()
                .filter(|u| {
                    u.present
                        && u.adapter.as_deref() == Some("cargo")
                        && u.role == swamp_core::artifact::ArtifactRole::Profile
                        && path.starts_with(&u.path)
                })
                .max_by_key(|u| u.path.components().count())
                .map(|u| u.path.clone())
        });
        let Some(profile) = profile else {
            self.set_refusal("Select a Cargo profile or an item within it, then press i");
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.operation = Some(Operation {
            label: "Inspecting",
            completed: 0,
            total: 1,
            succeeded: 0,
            failed: 0,
            current: names::friendly_unit_name(&self.report, &profile),
            bytes_done: 0,
            bytes_total: 0,
            started: Instant::now(),
            cancel: cancel.clone(),
            checking_open_files: None,
        });
        self.operation_rx = Some(rx);
        crate::worker::spawn(move || {
            let inspection =
                swamp_core::cargo_artifacts::inspect_profile(&profile, Default::default(), &cancel);
            let mut lines = vec![
                format!("{}", profile.display()),
                format!(
                    "deps: {} allocated / {} unique within inspected files · {} entries · {}ms",
                    model::human_bytes(inspection.allocated_bytes),
                    model::human_bytes(inspection.unique_allocated_bytes),
                    inspection.entries_examined,
                    inspection.elapsed_ms
                ),
                inspection.accounting_note.clone(),
            ];
            lines.extend(inspection.coverage.limits.iter().cloned());
            if !inspection.coverage.complete {
                lines.push(
                    "PARTIAL: totals cover inspected entries only, not the whole profile.".into(),
                );
                lines.push("For more: swamp inspect-cargo <profile> --max-entries 262144 --max-ms 30000 --json".into());
            }
            lines.push("Target / variant (package identity only when evidenced)".into());
            for group in inspection.groups {
                lines.push(format!(
                    "{}  {}  {} · features {} · package {}",
                    model::human_bytes(group.allocated_bytes),
                    group.target.as_deref().unwrap_or("unattributed"),
                    group.target_kind.as_deref().unwrap_or("unknown"),
                    group.variant.features.as_deref().unwrap_or("unknown"),
                    group.package_id.as_deref().unwrap_or("unknown")
                ));
                if let Some(reason) = group.residual_reason {
                    lines.push(format!("  {reason}"));
                }
            }
            let _ = tx.send(OperationEvent::Inspected(lines));
        });
    }

    /// UI entry point; synchronous marking helpers run only on the worker.
    pub fn review_in_background(&mut self, all: bool, confirm: bool) {
        if self.operation.is_some() {
            return;
        }
        // A row whose installs its own manager removes (#177) never goes
        // to Trash: Backspace with nothing marked opens its sheet, Space
        // says why it cannot be marked.
        // Backspace on its row opens the manager's own list; Space marks
        // the folder for Trash like any other (the confirm says the
        // manager will not know it is gone). Backspace while this very row
        // is marked is the Trash confirm.
        if !all && let Some(manager) = self.selected_row().and_then(|r| r.tool) {
            let own_mark = self
                .selected_row()
                .and_then(|r| r.unit)
                .is_some_and(|u| self.marked.contains_key(&u.0));
            if confirm && self.marked.is_empty() {
                self.open_tool_sheet(manager);
                return;
            } else if confirm && !own_mark {
                self.set_refusal(&format!(
                    "{} removes these installs itself, one at a time: confirm or clear the \
                     marked items first.",
                    manager.name()
                ));
                return;
            }
        }
        if confirm && !self.marked.is_empty() {
            self.note_confirm_base();
            self.open_confirm();
            return;
        }
        let row = self.selected_row();
        if !all && row.is_none() {
            return;
        }
        if confirm {
            self.note_confirm_base();
        }
        let marks = self.marked.clone();
        self.start_review(all, row, confirm, marks);
    }

    /// `r` in the blocked list: the same check again, from a fresh look at
    /// the disk (something that was open may have closed). The marks the
    /// last check added are replaced by what this one finds; marks made
    /// any other way stay.
    pub fn recheck_blocked(&mut self) {
        if self.operation.is_some() {
            return;
        }
        let Some((all, row)) = self.last_check.clone() else {
            return;
        };
        let mut marks = self.marked.clone();
        for id in &self.last_check_marks {
            marks.remove(id);
        }
        let confirm = self.confirm_open;
        self.blocked_open = false;
        self.start_review(all, row, confirm, marks);
    }

    fn start_review(
        &mut self,
        all: bool,
        row: Option<Row>,
        confirm: bool,
        base_marks: BTreeMap<String, MarkedUnit>,
    ) {
        self.last_check = Some((all, row.clone()));
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut worker = App::new(self.report.clone(), self.root.clone());
        worker.marked = base_marks;
        worker.view = self.view;
        worker.filter = self.filter.clone();
        worker.selected_project = self.selected_project.clone();
        worker.collapsed = self.collapsed.clone();
        worker.track = self.track.clone();
        // The protect marks live in the store: a worker without it would
        // skip the check the main thread makes. The Reclaim view is the
        // one this screen shows, not one rebuilt from nothing.
        worker.store_dir = self.store_dir.clone();
        worker.store_interiors = self.store_interiors.clone();
        worker.agent_units = self.agent_units.clone();
        *worker.reclaim_cache.borrow_mut() = Some(self.reclaim_view());
        worker.review_cancel = Some(cancel.clone());
        worker.review_progress = Some(tx.clone());
        self.operation = Some(Operation {
            label: "Reviewing",
            completed: 0,
            total: 0,
            succeeded: 0,
            failed: 0,
            current: String::new(),
            bytes_done: 0,
            bytes_total: 0,
            started: Instant::now(),
            cancel: cancel.clone(),
            checking_open_files: None,
        });
        self.operation_rx = Some(rx);
        self.refusal = None;
        self.last_result = None;
        self.blocked.clear();
        self.blocked_open = false;
        crate::worker::spawn(move || {
            let total = if all {
                worker.count_targets(None)
            } else {
                row.as_ref().map_or(0, |r| worker.count_targets(Some(r)))
            };
            let _ = tx.send(OperationEvent::ReviewTotal(total));
            // One open-file snapshot serves every item in this pass,
            // instead of one directory-tree walk per item.
            let phase_tx = tx.clone();
            swamp_core::occupancy::OccupancySnapshot::scoped_observed(
                move |phase| {
                    let _ = phase_tx.send(OperationEvent::OpenFileCheck(
                        phase == swamp_core::occupancy::SnapshotPhase::Capturing,
                    ));
                },
                || {
                    if all {
                        worker.mark_all_in_view();
                    } else if let Some(row) = row {
                        worker.mark_row(&row);
                    }
                },
            );
            let _ = tx.send(OperationEvent::Reviewed {
                marked: worker.marked,
                blocked: worker.blocked_log,
                refusal: worker.refusal.map(|(msg, _)| msg),
                confirm: confirm || all,
                cancelled: cancel.load(std::sync::atomic::Ordering::SeqCst),
            });
        });
    }

    /// How many items a check of `row` (or of every row in the view, for
    /// `None`) will look at: what "checked N of M" counts against. Items
    /// are counted once by path (the same directory is listed under more
    /// than one row), and a row with nothing to check counts as one that
    /// will be reported blocked.
    fn count_targets(&self, only: Option<&Row>) -> usize {
        let mut ids: HashSet<String> = HashSet::new();
        let mut blocked_rows = 0usize;
        let mut add = |row: &Row, checkouts: bool| {
            if let Some(key) = row
                .expansion_key
                .as_deref()
                .filter(|k| model::is_cleanup_selection(&self.report, k))
            {
                let members = model::cleanup_members(&self.report, key);
                if members.is_empty() {
                    blocked_rows += 1;
                }
                ids.extend(members.iter().map(|u| u.path.display().to_string()));
            } else if let Some(u) = &row.unit {
                ids.insert(u.0.clone());
            } else if let Some(p) = row.project.as_deref().filter(|_| row.kind.is_none()) {
                let units = self.project_units(p, checkouts);
                if units.is_empty() {
                    blocked_rows += 1;
                }
                ids.extend(
                    units
                        .iter()
                        .filter_map(|r| r.unit.as_ref().map(|u| u.0.clone())),
                );
            }
        };
        match only {
            Some(row) => add(row, true),
            None => self.rows().iter().for_each(|r| add(r, false)),
        }
        ids.len() + blocked_rows
    }

    /// Applies what the worker reported. True when anything arrived, so the
    /// event loop paints it without waiting for a key.
    pub fn poll_operation(&mut self) -> bool {
        let mut changed = false;
        loop {
            let event = match self.operation_rx.as_ref().map(|rx| rx.try_recv()) {
                Some(Ok(e)) => e,
                Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => OperationEvent::Failed(
                    "Worker stopped unexpectedly; check the ledger before retrying cleanup".into(),
                ),
                _ => break,
            };
            changed = true;
            match event {
                OperationEvent::Inspected(lines) => {
                    self.operation = None;
                    self.operation_rx = None;
                    self.cargo_inspection = Some(lines);
                    self.cargo_inspection_scroll = 0;
                    break;
                }
                OperationEvent::OpenFileCheck(active) => {
                    if let Some(op) = &mut self.operation {
                        op.checking_open_files = active.then(Instant::now);
                    }
                }
                OperationEvent::ReviewTotal(n) => {
                    if let Some(op) = &mut self.operation {
                        op.total = n;
                    }
                }
                OperationEvent::ReviewStep(path) => {
                    let name = names::friendly_unit_name(&self.report, &path);
                    if let Some(op) = &mut self.operation {
                        op.current = name;
                    }
                }
                OperationEvent::ReviewReady => {
                    if let Some(op) = &mut self.operation {
                        op.succeeded += 1;
                        op.completed = op.succeeded + op.failed;
                    }
                }
                OperationEvent::ReviewBlocked => {
                    if let Some(op) = &mut self.operation {
                        op.failed += 1;
                        op.completed = op.succeeded + op.failed;
                    }
                }
                OperationEvent::Progress {
                    completed,
                    total,
                    path,
                    outcome,
                } => {
                    let name = names::friendly_unit_name(&self.report, &path);
                    let bytes = self
                        .marked
                        .get(&path.display().to_string())
                        .map_or(0, |u| u.bytes);
                    if let Some(op) = &mut self.operation {
                        op.completed = completed;
                        op.total = total;
                        op.current = name;
                        if outcome == Some(true) {
                            op.succeeded += 1;
                            op.bytes_done += bytes;
                        }
                        if outcome == Some(false) {
                            op.failed += 1;
                        }
                    }
                }
                OperationEvent::Reviewed {
                    marked,
                    blocked,
                    refusal,
                    confirm,
                    cancelled,
                } => {
                    let (done, of) = self
                        .operation
                        .as_ref()
                        .map_or((0, 0), |op| (op.completed, op.total));
                    let cancelled = cancelled
                        || self
                            .operation
                            .as_ref()
                            .is_some_and(|op| op.cancel.load(std::sync::atomic::Ordering::SeqCst));
                    if !cancelled {
                        let before = std::mem::take(&mut self.marked);
                        let added: Vec<String> = marked
                            .keys()
                            .filter(|k| !before.contains_key(*k))
                            .cloned()
                            .collect();
                        let removed = before.keys().filter(|k| !marked.contains_key(*k)).count();
                        self.last_check_marks = added.clone();
                        self.marked = marked;
                        self.blocked = blocked;
                        self.refusal = refusal.map(|msg| (msg, Instant::now()));
                        self.confirm_open = confirm && !self.marked.is_empty();
                        if !self.confirm_open {
                            self.confirm_base = None;
                            // Space: say where the marks stand, the row
                            // itself may not show a change (a project row
                            // stands for many units).
                            let msg = self.mark_state_line(added.len(), removed);
                            self.set_result(msg);
                        }
                    } else {
                        let progress = if of > 0 {
                            format!("at {done} of {of}")
                        } else {
                            format!("after {done}")
                        };
                        let kept = match self.marked.len() {
                            0 => String::new(),
                            k => format!(" {k} still marked."),
                        };
                        self.set_result(format!(
                            "Check stopped {progress}. Nothing was moved.{kept}"
                        ));
                    }
                    self.operation = None;
                    self.operation_rx = None;
                    break;
                }
                OperationEvent::Deleted { results, total } => {
                    self.operation = None;
                    self.operation_rx = None;
                    self.finish_delete(results, total);
                    break;
                }
                OperationEvent::Failed(msg) => {
                    self.operation = None;
                    self.operation_rx = None;
                    self.confirm_open = false;
                    self.confirm_base = None;
                    self.set_refusal(&msg);
                    break;
                }
            }
        }
        if self.operation.is_none() && self.status.as_deref() == Some(REFRESH_WAITS) {
            self.status = None;
        }
        changed
    }

    fn set_refusal(&mut self, msg: &str) {
        self.refusal = Some((format!("refused: {msg}"), Instant::now()));
    }

    pub fn set_result(&mut self, msg: String) {
        self.last_result = Some(msg);
    }

    /// The last operation's result. It stays until the next key: a timer
    /// that erased it would repaint the screen while nobody is looking.
    pub fn result_active(&self) -> Option<&str> {
        self.last_result.as_deref()
    }

    pub fn refusal_active(&self) -> Option<&str> {
        self.refusal.as_ref().and_then(|(msg, at)| {
            // While a confirm is open the refusals it carries (what `A`
            // skipped and why) stay until the human answers it.
            if self.confirm_open || at.elapsed() < REFUSAL_DISPLAY {
                Some(msg.as_str())
            } else {
                None
            }
        })
    }

    /// Lists what the last check could not include, with reasons.
    pub fn open_blocked(&mut self) {
        if !self.blocked.is_empty() {
            self.blocked_open = true;
            self.blocked_scroll = 0;
        }
    }

    pub fn open_confirm(&mut self) {
        if !self.marked.is_empty() {
            self.confirm_open = true;
        }
    }

    /// Backspace: delete what is under the cursor. If nothing is marked,
    /// mark the current row; then ask once.
    pub fn delete_here(&mut self) {
        if self.marked.is_empty()
            && let Some(row) = self.selected_row()
        {
            self.note_confirm_base();
            self.mark_row(&row);
        } else {
            self.note_confirm_base();
        }
        self.open_confirm();
    }

    /// Records the marks as they stand when a confirm is first opened. A
    /// press that finds the confirm already open keeps the first record.
    fn note_confirm_base(&mut self) {
        if !self.confirm_open {
            self.confirm_base = Some(self.marked.keys().cloned().collect());
        }
    }

    /// Esc on the confirm: nothing is deleted, and the marks the opening
    /// press made are taken back so a later Backspace on another row asks
    /// about that row. Marks made earlier (Space) stay, drawn on the rows.
    pub fn cancel_confirm(&mut self) {
        if !self.confirm_open {
            return;
        }
        self.confirm_open = false;
        self.refusal = None;
        let base = self.confirm_base.take().unwrap_or_default();
        let undone: Vec<String> = self
            .marked
            .keys()
            .filter(|k| !base.contains(*k))
            .cloned()
            .collect();
        for id in &undone {
            self.marked.remove(id);
        }
        let kept = self.marked.len();
        let msg = match (undone.len(), kept) {
            (0, 0) => "Cancelled. Nothing was deleted.".to_string(),
            (0, k) => format!(
                "Cancelled. Nothing was deleted. {k} still marked (Space unmarks, Backspace asks again)."
            ),
            (n, 0) => format!("Cancelled. Nothing was deleted. Unmarked the {n} it had marked."),
            (n, k) => format!(
                "Cancelled. Nothing was deleted. Unmarked the {n} it had marked; {k} marked earlier remain."
            ),
        };
        self.set_result(msg);
    }

    /// Where the marks stand after a Space, for the result rows.
    fn mark_state_line(&self, added: usize, removed: usize) -> String {
        let total = self.marked.len();
        let bytes: u64 = self.marked.values().map(|u| u.bytes).sum();
        let blocked = match self.blocked.len() {
            0 => String::new(),
            n => format!(" {n} blocked (b to see why)."),
        };
        if total == 0 {
            return if removed > 0 {
                format!("Unmarked {removed}. Nothing is marked.{blocked}")
            } else {
                format!("Nothing marked.{blocked}")
            };
        }
        let change = if added > 0 {
            format!("Marked {added} more. ")
        } else if removed > 0 {
            format!("Unmarked {removed}. ")
        } else {
            String::new()
        };
        format!(
            "{change}{total} marked in all ({}). Nothing has been moved. Backspace moves them to Trash after you confirm.{blocked}",
            model::human_bytes(bytes)
        )
    }

    /// Whether a terminal of this size shows what Enter would do. A plan
    /// that removes anything for good (docker) needs the sheet that says
    /// so; below that size Enter is not offered and does nothing.
    pub fn confirm_fits(&self, width: u16, height: u16) -> bool {
        let permanent = self.marked.values().any(|u| u.docker.is_some());
        let roomy = height >= CONFIRM_MIN_ROWS && width >= CONFIRM_MIN_COLS;
        if self.marked.values().any(|u| u.reclaim.is_some()) {
            return roomy && crate::ui::reclaim_plan_fits(self, width, height);
        }
        !permanent || roomy
    }

    pub fn confirm_summary(&self) -> String {
        let units: Vec<MarkedUnit> = self.marked.values().cloned().collect();
        actions::confirm_summary(&units)
    }

    /// Enter on the confirm banner: this keypress at the keyboard is the
    /// human authorization for this one plan. Drives plan -> grant ->
    /// execute -> ledger, then re-observes the affected worktrees only.
    pub fn confirm_delete(&mut self) {
        self.start_delete(
            swamp_core::fs_gate::StoreDir::resolved(),
            actions::trash_root(),
        );
    }

    fn start_delete(&mut self, store: swamp_core::fs_gate::StoreDir, trash: PathBuf) {
        if !self.confirm_open || self.operation.is_some() {
            return;
        }
        let units: Vec<MarkedUnit> = self.marked.values().cloned().collect();
        if units.is_empty() {
            self.confirm_open = false;
            return;
        }
        let planned: u64 = units.iter().map(|u| u.bytes).sum();
        // A report started before these moves must not resurrect deleted rows.
        self.pending = None;
        self.observing = None;
        // This keypress, on the summary the human just read (current
        // facts, shown a moment ago) is the human decision. There is no
        // token to mint: Enter just moves what was listed.
        let keep = self.keep_executables;
        let total = units.len();
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.operation = Some(Operation {
            label: "Deleting",
            completed: 0,
            total,
            succeeded: 0,
            failed: 0,
            current: String::new(),
            bytes_done: 0,
            bytes_total: planned,
            started: Instant::now(),
            cancel: cancel.clone(),
            checking_open_files: None,
        });
        self.operation_rx = Some(rx);
        self.confirm_open = false;
        self.confirm_base = None;
        self.last_result = None;
        self.refusal = None;
        self.blocked.clear();
        self.blocked_open = false;
        crate::worker::spawn(move || {
            let ledger = swamp_core::ledger::Ledger::resolved(&store);
            let results = actions::execute_plan_progress(
                &units,
                &ledger,
                &trash,
                keep,
                |completed, path, outcome| {
                    let _ = tx.send(OperationEvent::Progress {
                        completed,
                        total,
                        path: path.to_path_buf(),
                        outcome,
                    });
                    !cancel.load(std::sync::atomic::Ordering::SeqCst)
                },
            );
            let _ = tx.send(OperationEvent::Deleted { results, total });
        });
    }

    fn finish_delete(&mut self, results: Vec<actions::UnitResult>, total: usize) {
        // What could not be moved, kept so `b` lists each with its reason
        // instead of the result line naming only the first.
        let failed: Vec<BlockedItem> = results
            .iter()
            .filter_map(|r| {
                r.outcome.as_ref().err().map(|e| BlockedItem {
                    name: names::friendly_unit_name(&self.report, &r.path),
                    reason: e.to_string(),
                    next: "check that nothing is using it, then try again".to_string(),
                })
            })
            .collect();
        let could_not = |n: usize| {
            if n == 0 {
                String::new()
            } else {
                format!(" {n} could not be moved (b to see why).")
            }
        };
        // What moved to Trash and what was removed for good (docker), with
        // sizes, read before the marks of finished units are dropped.
        let (mut trash_n, mut trash_bytes, mut docker_n, mut docker_bytes) =
            (0usize, 0u64, 0usize, 0u64);
        let mut reclaim_moved = false;
        for r in results.iter().filter(|r| r.outcome.is_ok()) {
            match self.marked.get(&r.path.display().to_string()) {
                Some(u) if u.docker.is_some() => {
                    docker_n += 1;
                    docker_bytes += u.bytes;
                }
                Some(u) => {
                    reclaim_moved |= u.reclaim.is_some();
                    trash_n += 1;
                    trash_bytes += u.bytes;
                }
                None => trash_n += 1,
            }
        }
        // Retain refused and unprocessed selections for explicit review/retry.
        for r in &results {
            if r.outcome.is_ok() {
                self.marked.remove(&r.path.display().to_string());
            }
        }
        self.confirm_open = false;
        let items = |n: usize| {
            if n == 1 {
                "1 item".to_string()
            } else {
                format!("{n} items")
            }
        };
        let mut moved = format!(
            "Moved {} ({}) to Trash.",
            items(trash_n),
            model::human_bytes(trash_bytes)
        );
        if docker_n > 0 {
            moved.push_str(&format!(
                " Removed {} docker {} ({}) for good.",
                docker_n,
                if docker_n == 1 { "item" } else { "items" },
                model::human_bytes(docker_bytes)
            ));
        }
        if trash_n == 0 && docker_n > 0 {
            moved = moved.replacen("Moved 0 items (0B) to Trash. ", "", 1);
        }
        self.set_result(if results.len() < total {
            format!(
                "Stopped. {moved}{} {} not attempted. Items moved to Trash stay there. A docker or git command that was running was stopped and may still have finished.",
                could_not(failed.len()),
                total - results.len()
            )
        } else if trash_n > 0 {
            // No "freed" figure: on the same volume the move to Trash frees
            // nothing until Trash is emptied, so a measured change is noise.
            // Sizes of a Reclaim move are the last observation's, not a
            // measurement of what left; nothing is freed until Trash is
            // emptied, and the next observation remeasures.
            let sizes = if reclaim_moved {
                " Sizes are from the last observation."
            } else {
                ""
            };
            format!(
                "{moved}{sizes}{} Space is freed when Trash is emptied.",
                could_not(failed.len())
            )
        } else {
            format!("{moved}{}", could_not(failed.len()))
        });
        self.blocked = failed;
        // What just left the disk leaves the screen now; the store and the
        // header follow from a background incremental observe (FSEvents
        // narrows it to the touched trees), the same path startup uses.
        self.prune_removed(&results);
        self.observe_in_background();
    }

    /// Drops every row under a successfully removed path from the
    /// in-memory report -- artifacts, whole worktrees, Source directory
    /// rollups -- and takes their bytes off the header totals, so the
    /// screen is right before the re-observe lands.
    /// What left the disk leaves the Reclaim and External rows now: a unit
    /// at or under a moved path is dropped, and a moved folder of a unit
    /// comes off the unit and its size (the next observation remeasures).
    fn prune_reclaim(&mut self, removed: &[PathBuf]) {
        let under = |p: &Path| removed.iter().any(|r| p == r || p.starts_with(r));
        let mut changed = false;
        self.external_units.retain(|u| {
            let gone = under(&u.path);
            changed |= gone;
            !gone
        });
        for u in &mut self.external_units {
            let unit = u.path.display().to_string();
            let mut freed = 0i64;
            let before = u.children.len();
            u.children.retain(|c| {
                match swamp_core::reclaim_trash::row_path(&unit, Some((c.kind, &c.name))) {
                    Some(p) if under(&p) => {
                        freed += c.bytes.unwrap_or(0);
                        false
                    }
                    _ => true,
                }
            });
            if u.children.len() != before {
                u.bytes = u.bytes.saturating_sub(freed.max(0) as u64);
                changed = true;
            }
        }
        if changed {
            self.reclaim_cache.borrow_mut().take();
            self.headline_cache.borrow_mut().take();
        }
    }

    pub fn prune_removed(&mut self, results: &[actions::UnitResult]) {
        let removed: Vec<PathBuf> = results
            .iter()
            .filter(|r| r.outcome.is_ok())
            .map(|r| r.path.clone())
            .collect();
        if removed.is_empty() {
            return;
        }
        self.prune_reclaim(&removed);
        if let Some(estimate) = self.report.reconciliation.unique_estimate.as_mut() {
            estimate.needs_reconciliation = true;
        }
        let under = |p: &std::path::Path| removed.iter().any(|r| p == r || p.starts_with(r));
        // Companion paths are part of the exact group, not just the selected
        // executable. Suppress stale nested facts until the observer refreshes.
        self.report.nested_artifacts.retain(|u| {
            !under(&u.path)
                && !removed.iter().any(|r| {
                    u.path == r.with_extension("d") || u.path.starts_with(r.with_extension("dSYM"))
                })
        });
        let mut freed = 0u64;
        let wt_paths: std::collections::HashMap<String, PathBuf> = self
            .report
            .projects
            .iter()
            .flat_map(|p| p.worktrees.iter())
            .map(|w| (w.worktree_id.clone(), w.path.clone()))
            .collect();
        for p in &mut self.report.projects {
            p.worktrees.retain(|wt| {
                if under(&wt.path) {
                    freed += wt.artifacts.iter().map(|a| a.bytes).sum::<u64>();
                    false
                } else {
                    true
                }
            });
            for wt in &mut p.worktrees {
                wt.artifacts.retain(|a| {
                    if under(&a.path) {
                        freed += a.bytes;
                        false
                    } else {
                        true
                    }
                });
            }
        }
        self.report.projects.retain(|p| !p.worktrees.is_empty());
        if let Some(map) = self.report.dirs_by_worktree.as_mut() {
            for (wt_id, rows) in map.iter_mut() {
                let Some(base) = wt_paths.get(wt_id) else {
                    continue;
                };
                rows.retain(|d| !under(&base.join(&d.rel_path)));
            }
        }
        // Bytes of a removed Source directory were counted inside the
        // worktree's Source row; the re-observe corrects that row. The
        // totals shrink by what we know left.
        let rec = &mut self.report.reconciliation;
        rec.attributed = rec.attributed.saturating_sub(freed);
        rec.walked_total = rec.walked_total.saturating_sub(freed);
        // Agent and external unit rows for exactly the successful
        // outcomes. Without this the agents view kept showing storage
        // that had just been moved to Trash, until the next full
        // startup -- one of the review's TUI staleness findings.
        self.agent_units
            .retain(|u| !under(&u.path) && !u.members.iter().any(|m| under(&m.path)));
        self.external_units.retain(|u| !under(&u.path));
        for path in &removed {
            self.track.remove(path);
            self.collapsed.remove(&format!("source:{}", path.display()));
        }
        if self.selected >= self.rows().len() {
            self.selected = self.rows().len().saturating_sub(1);
        }
    }

    /// Starts an incremental observation of every root in `self.roots`
    /// on one worker thread (sequentially -- root re-walks already run
    /// each worker pool to saturation on their own, so parallelizing
    /// across roots too would only contend with itself); `event_loop`
    /// applies each root's fresh report as it would any other pending
    /// result. No-op without a store (fixture apps in tests) or while
    /// one is already running. A root whose own re-observation fails is
    /// simply absent from the returned vec -- its last-known entry in
    /// `reports_by_root` (and therefore its rows in `report`) is left
    /// exactly as it was, never erased by another root's refresh.
    pub fn observe_in_background(&mut self) {
        let Some(store) = self.store_dir.clone() else {
            return;
        };
        if self.pending.is_some() {
            return;
        }
        let Some(scope) = self.scope.clone() else {
            self.status =
                Some("refresh skipped: no resolved scope for this session; reopen swamp ui".into());
            return;
        };
        let (roots, cache) = (self.roots.clone(), self.reports_by_root.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        crate::worker::spawn(move || {
            // Single-flight with the scheduled `swamp observe`: try the
            // lock, never wait for it. Held elsewhere -> report that and
            // leave the stored data alone; the lock poller shows who has
            // it and reloads when it finishes.
            let _lock = match swamp_core::schedule::acquire_lock(&store) {
                Ok(swamp_core::schedule::LockOutcome::Acquired(g)) => Some(g),
                Ok(swamp_core::schedule::LockOutcome::HeldBy { pid, .. }) => {
                    let _ = tx.send(Err(anyhow::anyhow!(
                        "another observation is running (pid {pid})"
                    )));
                    return;
                }
                // A lock we cannot even try is no reason to show nothing.
                Err(_) => None,
            };
            // The same scope-aware entry point startup uses, with the
            // same exclusions and external pruning, and returning the
            // external/agent units from that same pass so the agent view
            // cannot drift out of date behind a "live" header.
            let res = swamp_core::report::observe_scope(
                &scope,
                swamp_core::report::ObservationParts::ALL,
                None,
                None,
                false,
                Some(&store),
                None,
                true,
                true,
                false,
                false,
                swamp_core::fs_events::platform_source().as_ref(),
                30,
                24 * 3600,
            )
            .map(|o| {
                RefreshedObservation::merged_on_worker(
                    &roots,
                    cache,
                    o.per_root.into_iter().collect(),
                    Some(o.external_units),
                    Some(o.agent_units),
                    Some(o.store_interiors),
                )
            });
            let _ = tx.send(res);
        });
        self.pending = Some(rx);
        self.observing = Some((0, 0));
        self.observing_started = Some(Instant::now());
    }

    /// The `R` key: observe now, in the background. Never while one is
    /// already running, and never on a nearly full disk.
    /// `R` while a check or a move runs: it does not start a scan, and the
    /// header says why. The line goes when the operation ends.
    pub fn say_refresh_waits(&mut self) {
        self.status = Some(REFRESH_WAITS.to_string());
    }

    pub fn refresh_now(&mut self) {
        if self.pending.is_some() {
            return;
        }
        // Another process (often the schedule) is already observing: say
        // so rather than start a second walk that would only be refused,
        // and promise what will happen (its result loads here).
        if let Some(h) = self.external_observer {
            let secs = swamp_core::entities::now().saturating_sub(h.since);
            self.set_result(format!(
                "An observation is already running (pid {}, {}). Its result loads here when it finishes.",
                h.pid,
                swamp_core::schedule::format_elapsed(secs)
            ));
            return;
        }
        if let Some(b) = &self.disk_banner {
            self.status = Some(format!("refresh skipped: {b}"));
            return;
        }
        self.status = None;
        self.observe_in_background();
    }

    /// Startup policy: the only automatic scan is the first one. With an
    /// index (stored report) at any age nothing is observed -- the
    /// schedule keeps it current, `R` refreshes on demand. Without one
    /// (and no full disk) the first scan starts in the background so the
    /// UI opens at once and shows its progress. Returns whether a scan
    /// was started.
    pub fn scan_if_no_index(&mut self, has_index: bool) -> bool {
        if has_index || self.disk_banner.is_some() {
            return false;
        }
        self.observe_in_background();
        self.pending.is_some()
    }

    /// Watches the observation lock every `every` on a worker, so the
    /// header can say a scheduled observation is running (and for how
    /// long) and the stored report reloads when it ends. `self_pid` is
    /// this process's own pid: its own observations are shown by
    /// `observing`, not as somebody else's.
    pub fn start_lock_poll(&mut self, every: Duration, self_pid: u32) {
        let (Some(store), Some(scope)) = (self.store_dir.clone(), self.scope.clone()) else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        crate::worker::spawn(move || {
            let mut prev: Option<swamp_core::schedule::LockHolder> = None;
            let mut first = true;
            loop {
                let now_holder =
                    swamp_core::schedule::peek_lock(&store).filter(|h| h.pid != self_pid);
                if (first || now_holder != prev)
                    && tx.send(LockPollMsg::Holder(now_holder)).is_err()
                {
                    return;
                }
                if prev.is_some()
                    && now_holder.is_none()
                    && let Ok(snap) = swamp_core::report::report_scope_from_store(&scope, &store)
                    && tx
                        .send(LockPollMsg::Reloaded(
                            Box::new(snap),
                            Box::new(swamp_core::volume_ledger::read_reading(&store)),
                        ))
                        .is_err()
                {
                    return;
                }
                first = false;
                prev = now_holder;
                std::thread::sleep(every);
            }
        });
        self.lock_poll_rx = Some(rx);
    }

    /// Applies whatever the lock poller has reported since last tick.
    /// Never blocks.
    pub fn drain_lock_poll(&mut self) -> bool {
        let Some(rx) = &self.lock_poll_rx else {
            return false;
        };
        let msgs: Vec<LockPollMsg> = rx.try_iter().collect();
        let changed = !msgs.is_empty();
        for m in msgs {
            match m {
                LockPollMsg::Holder(h) => self.external_observer = h,
                LockPollMsg::Reloaded(snap, ledger) => {
                    // Our own observation in flight will replace this
                    // report anyway.
                    if self.pending.is_some() {
                        continue;
                    }
                    if self.reload_must_wait() {
                        self.held_reload = Some(HeldReload::Snapshot(snap, ledger));
                        continue;
                    }
                    self.install_snapshot(*snap, *ledger);
                }
            }
        }
        changed
    }

    /// Opens the tool-managed removal sheet and reads the manager's own
    /// list on a worker (never on open of the TUI: only this key press).
    pub fn open_tool_sheet(&mut self, manager: swamp_core::tool_removal::Manager) {
        if self.tool_rx.is_some() {
            return;
        }
        self.tool_sheet = Some(crate::tool_sheet::ToolSheet::new(manager));
        let host = self.tool_host.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.tool_rx = Some(rx);
        crate::worker::spawn(move || {
            let listed = swamp_core::tool_removal::read_candidates(&host, manager);
            let _ = tx.send(crate::tool_sheet::ToolEvent::Listed(listed));
        });
    }

    /// swamp's stored sizes of directories, for the confirm's size line.
    fn tool_sizes(&self) -> Vec<(PathBuf, u64)> {
        self.external_units
            .iter()
            .map(|u| (u.path.clone(), u.bytes))
            .chain(
                self.store_interiors
                    .iter()
                    .map(|u| (u.path.clone(), u.bytes)),
            )
            .collect()
    }

    /// ↑/↓ in the sheet's list.
    pub fn tool_move(&mut self, delta: i32) {
        if let Some(sheet) = self.tool_sheet.as_mut()
            && matches!(sheet.stage, crate::tool_sheet::Stage::Choose)
        {
            let n = sheet.listing.as_ref().map_or(0, |l| l.candidates.len());
            if n > 0 {
                let at = (sheet.cursor as i64 + i64::from(delta)).clamp(0, n as i64 - 1);
                sheet.cursor = at as usize;
            }
        }
    }

    /// Enter in the sheet: review the chosen item, or, on the confirm, run
    /// exactly its command (after the re-review `execute` does).
    pub fn tool_enter(&mut self) {
        use crate::tool_sheet::{Stage, ToolEvent};
        if self.tool_rx.is_some() {
            return;
        }
        let sizes = self.tool_sizes();
        let host = self.tool_host.clone();
        let Some(sheet) = self.tool_sheet.as_mut() else {
            return;
        };
        // Enter never runs a removal: a held, queued or pasted Enter that
        // opened the review must not also confirm it. Only `Y` on the
        // confirm does ([`App::tool_remove_key`]).
        if matches!(sheet.stage, Stage::Choose) {
            {
                let Some(c) = sheet
                    .listing
                    .as_ref()
                    .and_then(|l| l.candidates.get(sheet.cursor))
                    .cloned()
                else {
                    return;
                };
                sheet.stage = Stage::Reviewing(c.label.clone());
                let (tx, rx) = std::sync::mpsc::channel();
                self.tool_rx = Some(rx);
                crate::worker::spawn(move || {
                    let r = swamp_core::tool_removal::review_target(&host, &c.target, &sizes)
                        .map(Box::new);
                    let _ = tx.send(ToolEvent::Reviewed(r));
                });
            }
        }
    }

    /// `Y` on the confirm: runs exactly the drawn preview's command, and
    /// only when the confirm has been drawn, in full, at a known size, at
    /// least [`crate::tool_sheet::HOLD_OFF`] ago.
    pub fn tool_remove_key(&mut self) {
        use crate::tool_sheet::{Stage, ToolEvent};
        if self.tool_rx.is_some() {
            return;
        }
        let store = swamp_core::fs_gate::StoreDir::resolved();
        let (w, h) = (self.width, self.height);
        let sizes = self.tool_sizes();
        let host = self.tool_host.clone();
        let Some(sheet) = self.tool_sheet.as_mut() else {
            return;
        };
        let Stage::Confirm(preview) = &sheet.stage else {
            return;
        };
        if w == 0 || h == 0 || !crate::tool_sheet::confirm_fits(preview, w, h) || !sheet.armed(w, h)
        {
            return;
        }
        let preview = preview.clone();
        sheet.stage = Stage::Running(preview.title().to_string());
        let (tx, rx) = std::sync::mpsc::channel();
        self.tool_rx = Some(rx);
        crate::worker::spawn(move || {
            let out = crate::actions::run_tool_removal(&host, &preview, &sizes, &store);
            let _ = tx.send(ToolEvent::Ran(Box::new(out)));
        });
    }

    /// True once after a confirm appears: the event loop then drops every
    /// input event already queued (typeahead, key repeat, a paste).
    pub fn take_confirm_drain(&mut self) -> bool {
        std::mem::replace(&mut self.confirm_drain, false)
    }

    /// Esc in the sheet: back one step, or close. A running removal is
    /// never interrupted from here.
    pub fn tool_back(&mut self) {
        use crate::tool_sheet::Stage;
        let Some(sheet) = self.tool_sheet.as_mut() else {
            return;
        };
        let has_list = sheet.listing.is_some();
        match sheet.stage {
            Stage::Running(_) => {}
            Stage::Listing | Stage::Reviewing(_) => {
                // Only a read-only listing or dry run is running: stop it.
                let back_to_list = has_list && matches!(sheet.stage, Stage::Reviewing(_));
                if back_to_list {
                    sheet.stage = Stage::Choose;
                }
                self.tool_rx = None;
                crate::worker::spawn(swamp_core::fs_gate::spawn::kill_all_children);
                if !back_to_list {
                    self.tool_sheet = None;
                }
            }
            Stage::Confirm(_) | Stage::Refused { .. } if has_list => sheet.stage = Stage::Choose,
            _ => self.tool_sheet = None,
        }
    }

    /// Lands a tool worker's report. True when the screen changed.
    pub fn poll_tool(&mut self) -> bool {
        use crate::tool_sheet::{Stage, ToolEvent};
        let event = match self.tool_rx.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(e)) => e,
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                self.tool_rx = None;
                if let Some(sheet) = self.tool_sheet.as_mut() {
                    let running = matches!(sheet.stage, Stage::Running(_));
                    sheet.stage = Stage::Refused {
                        what: "worker stopped".into(),
                        refusal: swamp_core::tool_removal::Refusal {
                            reason: if running {
                                "The removal's worker stopped unexpectedly; what the manager did \
                                 was not read back."
                                    .into()
                            } else {
                                "The worker stopped unexpectedly.".into()
                            },
                            next: "Open the sheet again to see the manager's list now.".into(),
                            output: Vec::new(),
                        },
                    };
                }
                return true;
            }
            _ => return false,
        };
        self.tool_rx = None;
        let Some(sheet) = self.tool_sheet.as_mut() else {
            return true;
        };
        match event {
            ToolEvent::Listed(Ok(listing)) => {
                sheet.cursor = 0;
                sheet.listing = Some(listing);
                sheet.stage = Stage::Choose;
            }
            ToolEvent::Listed(Err(refusal)) => {
                sheet.stage = Stage::Refused {
                    what: format!("{}'s list", sheet.manager.name()),
                    refusal,
                };
            }
            ToolEvent::Reviewed(Ok(preview)) => {
                sheet.stage = Stage::Confirm(preview);
                sheet.disarm();
                self.confirm_drain = true;
            }
            ToolEvent::Reviewed(Err(refusal)) => {
                let what = sheet
                    .listing
                    .as_ref()
                    .and_then(|l| l.candidates.get(sheet.cursor))
                    .map_or_else(String::new, |c| c.label.clone());
                sheet.stage = Stage::Refused { what, refusal };
            }
            ToolEvent::Ran(outcome) => {
                sheet.listing = None;
                sheet.stage = Stage::Done(outcome);
            }
        }
        true
    }

    /// Something on screen moves by itself: a check or a move, our own
    /// scan, or another process's. Only then does the UI paint on a timer.
    pub fn is_busy(&self) -> bool {
        self.tool_rx.is_some()
            || self.operation.is_some()
            || self.observing.is_some()
            || self.pending.is_some()
            || self.external_observer.is_some()
    }

    pub fn toggle_help(&mut self) {
        self.help_open = !self.help_open;
        self.help_scroll.set(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swamp_core::entities::Confidence;
    use swamp_core::report::{
        ArtifactKind, ArtifactRow, ProjectRow, Reconciliation, Source, WorktreeKind, WorktreeRow,
    };

    fn wait_operation(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.operation.is_some() {
            assert!(Instant::now() < deadline, "operation did not complete");
            app.poll_operation();
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn background_review_cancel_preserves_selection_and_never_confirms() {
        let mut app = App::new(fixture_report(), "/root".into());
        app.clear_filter();
        app.review_in_background(false, true);
        assert!(app.operation.is_some());
        // The worker may already have finished when Ctrl-C arrives, but the UI
        // has not accepted its result. Cancellation must still win.
        let result = loop {
            let event = app
                .operation_rx
                .as_ref()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            if matches!(event, OperationEvent::Reviewed { .. }) {
                break event;
            }
        };
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(result).unwrap();
        app.operation_rx = Some(rx);
        app.cancel_operation();
        wait_operation(&mut app);
        assert!(app.marked.is_empty());
        assert!(!app.confirm_open);
        assert!(app.last_result.as_ref().unwrap().contains("Check stopped"));
    }

    #[test]
    fn cancelled_delete_retains_refused_and_unattempted_marks() {
        let mut app = App::new(fixture_report(), "/root".into());
        for name in ["done", "refused", "untouched"] {
            let path = PathBuf::from(format!("/fixture/{name}"));
            app.marked.insert(
                path.display().to_string(),
                MarkedUnit {
                    cargo_unit: None,
                    agent_unit: None,
                    reclaim: None,
                    path,
                    docker: None,
                    worktree_path: PathBuf::new(),
                    bytes: 1,
                    observed_at: 0,
                    label: name.into(),
                    warnings: vec![],
                    worktree: None,
                },
            );
        }
        app.finish_delete(
            vec![
                actions::UnitResult {
                    path: "/fixture/done".into(),
                    outcome: Ok(swamp_core::execution::Outcome {
                        unit_id: String::new(),
                        status: "ok".into(),
                        reason: None,
                        intended_bytes: 1,
                        observed_free_space_delta: None,
                    }),
                },
                actions::UnitResult {
                    path: "/fixture/refused".into(),
                    outcome: Err("busy".into()),
                },
            ],
            3,
        );
        assert_eq!(app.marked.len(), 2);
        assert!(!app.marked.contains_key("/fixture/done"));
        assert!(app.marked.contains_key("/fixture/refused"));
        assert!(app.marked.contains_key("/fixture/untouched"));
        assert!(
            app.last_result
                .as_ref()
                .unwrap()
                .contains("1 not attempted")
        );
        assert!(!app.confirm_open);
        // The unit that could not move is recallable with `b`, with its
        // reason, instead of vanishing after the result line.
        assert_eq!(app.blocked.len(), 1);
        assert_eq!(app.blocked[0].reason, "busy");
        assert!(
            app.last_result
                .as_ref()
                .unwrap()
                .contains("1 could not be moved (b to see why)")
        );
        assert!(
            !app.last_result
                .as_ref()
                .unwrap()
                .contains("Free space changed"),
            "no measured free-space figure: a move to Trash frees nothing yet"
        );
        app.open_blocked();
        assert!(app.blocked_open);
    }

    #[test]
    fn background_delete_finishes_and_worker_failure_is_visible() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cache");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("data"), b"fixture").unwrap();
        let mut app = App::new(fixture_report(), tmp.path().into());
        app.marked.insert(
            path.display().to_string(),
            MarkedUnit {
                cargo_unit: None,
                agent_unit: None,
                reclaim: None,
                path: path.clone(),
                docker: None,
                worktree_path: tmp.path().into(),
                bytes: 7,
                observed_at: 0,
                label: "cache".into(),
                warnings: vec![],
                worktree: None,
            },
        );
        app.confirm_open = true;
        app.start_delete(
            swamp_core::fs_gate::StoreDir::at(tmp.path()).unwrap(),
            tmp.path().join("Trash"),
        );
        assert!(app.operation.is_some());
        assert!(!app.confirm_open);
        wait_operation(&mut app);
        assert!(!path.exists());
        assert!(app.marked.is_empty());
        assert!(app.last_result.as_ref().unwrap().contains("Moved 1 item"));
        assert_eq!(
            swamp_core::ledger::Ledger::open(tmp.path().join("ledger.parquet"))
                .unwrap()
                .all()
                .unwrap()
                .len(),
            1
        );

        app.review_in_background(true, false);
        // Replace the receiver with a disconnected worker channel.
        let (tx, rx) = std::sync::mpsc::channel();
        drop(tx);
        app.operation_rx = Some(rx);
        app.poll_operation();
        assert!(app.operation.is_none());
        assert!(
            app.refusal_active()
                .unwrap()
                .contains("Worker stopped unexpectedly")
        );
    }

    /// A real (never fixture-literal) Claude Code home under a tempdir:
    /// one actionable cache category (`shell-snapshots/`) and one
    /// protected config file (`settings.json`), discovered through the
    /// same `swamp_core::agents::discover_and_measure` path `swamp
    /// report --view agents` and the real TUI startup use -- this test
    /// exercises `App::mark_row`'s new agent-storage branch against real
    /// identification output, not a hand-built `AgentUnit` literal.
    fn fixture_agent_units(claude_home: &std::path::Path) -> Vec<swamp_core::agents::AgentUnit> {
        std::fs::create_dir_all(claude_home.join("shell-snapshots")).unwrap();
        std::fs::write(
            claude_home.join("shell-snapshots").join("snap.sh"),
            b"alias x=y",
        )
        .unwrap();
        std::fs::write(claude_home.join("settings.json"), b"{}").unwrap();

        let mut env_vars = std::collections::HashMap::new();
        env_vars.insert(
            "CLAUDE_CONFIG_DIR".to_string(),
            claude_home.display().to_string(),
        );
        let home_dummy = tempfile::tempdir().unwrap();
        let env = swamp_core::locations::Environment::fixture(
            home_dummy.path().to_path_buf(),
            env_vars,
            swamp_core::locations::Platform::MacOS,
        );
        let registry = swamp_core::locations::Registry::with_builtins();
        let cfg = swamp_core::scope::ScanConfig {
            defaults: false,
            include: Vec::new(),
            exclude: Vec::new(),
            disabled_detectors: vec![
                "cargo-home".into(),
                "rustup".into(),
                "homebrew".into(),
                "codex".into(),
                "codex-desktop".into(),
                "oh-my-pi".into(),
                "opencode".into(),
            ],
            enabled_detectors: Vec::new(),
        };
        let scope = swamp_core::scope::resolve_effective_scope(&env, &cfg, &[], &registry, 1);
        swamp_core::agents::discover_and_measure(
            &scope,
            &[],
            None,
            false,
            1_000,
            30,
            3600,
            &swamp_core::fs_events::EventCoverage::untrusted(),
        )
        .unwrap()
    }

    #[test]
    fn agents_view_mark_row_builds_an_agent_plan_and_deletes_it_via_the_ordinary_worker_path() {
        let claude_home = tempfile::tempdir().unwrap();
        let units = fixture_agent_units(claude_home.path());
        let mut app = App::new(fixture_report(), "/root".into());
        app.set_view(ViewKind::Agents);
        app.set_agent_units(units);
        let cache_row = model::agent_rows(&app.agent_units)
            .into_iter()
            .find(|r| r.label.contains("shell-snapshots"))
            .expect("cache row present");
        app.mark_row(&cache_row);
        let cache_path = claude_home.path().join("shell-snapshots");
        let marked = app
            .marked
            .get(&cache_path.display().to_string())
            .expect("cache unit marked");
        assert!(marked.agent_unit.is_some(), "agent_plan must be built");
        assert!(
            cache_path.exists(),
            "marking alone must not delete anything"
        );

        app.confirm_open = true;
        app.start_delete(
            swamp_core::fs_gate::StoreDir::at(claude_home.path()).unwrap(),
            claude_home.path().join("Trash"),
        );
        wait_operation(&mut app);
        assert!(!cache_path.exists(), "marked cache dir must be trashed");
        assert!(app.marked.is_empty());
        assert!(app.last_result.as_ref().unwrap().contains("Moved 1 item"));
        // Untouched: the protected settings.json survives the same pass.
        assert!(claude_home.path().join("settings.json").exists());
    }

    #[test]
    fn agents_view_mark_row_refuses_a_protected_unit_with_the_reason_not_a_generic_message() {
        let claude_home = tempfile::tempdir().unwrap();
        let units = fixture_agent_units(claude_home.path());
        let mut app = App::new(fixture_report(), "/root".into());
        app.set_view(ViewKind::Agents);
        app.set_agent_units(units);
        let settings_row = model::agent_rows(&app.agent_units)
            .into_iter()
            .find(|r| r.label.contains("settings.json"))
            .expect("settings row present");
        app.mark_row(&settings_row);
        assert!(
            app.marked.is_empty(),
            "a protected unit must never be marked"
        );
        assert!(
            app.refusal_active()
                .unwrap_or_default()
                .contains("protected"),
            "{:?}",
            app.refusal_active()
        );
        assert!(claude_home.path().join("settings.json").exists());
    }

    /// Shift+A over the Agents view (chunk D follow-up): the actionable
    /// cache row is marked and the protected config row is left alone,
    /// with one confirm opened for what *was* marked and a footer that
    /// names the skip -- never a silent "nothing in this view can be
    /// acted on" for a screen that plainly has one actionable row, and
    /// never a false "everything selected" that quietly includes
    /// `settings.json`.
    #[test]
    fn mark_all_in_agents_view_marks_the_cache_and_skips_the_protected_config() {
        let claude_home = tempfile::tempdir().unwrap();
        let units = fixture_agent_units(claude_home.path());
        let mut app = App::new(fixture_report(), "/root".into());
        app.set_view(ViewKind::Agents);
        app.set_agent_units(units);
        app.mark_all_in_view();

        let cache_path = claude_home.path().join("shell-snapshots");
        assert_eq!(
            app.marked.len(),
            1,
            "only the actionable cache row, not the protected config: {:?}",
            app.marked.keys().collect::<Vec<_>>()
        );
        assert!(app.marked.contains_key(&cache_path.display().to_string()));
        assert!(
            !app.marked.contains_key(
                &claude_home
                    .path()
                    .join("settings.json")
                    .display()
                    .to_string()
            ),
            "the protected unit must never be swept up by bulk marking"
        );
        assert!(app.confirm_open, "one confirm for what could be marked");
        assert!(
            app.refusal_active().is_some_and(|m| m.contains("skipped")),
            "the footer must explain the skip, not stay silent: {:?}",
            app.refusal_active()
        );
    }

    fn fixture_report() -> Report {
        Report {
            store_dir: None,
            observed_at: 1000,
            root: "/root".into(),
            projects: vec![ProjectRow {
                project_id: "p1".into(),
                name: "mole".into(),
                remote: None,
                ecosystems: Vec::new(),
                worktrees: vec![WorktreeRow {
                    worktree_id: "w1".into(),
                    path: "/root/mole".into(),
                    kind: WorktreeKind::Main,
                    artifacts: vec![
                        ArtifactRow {
                            kind: ArtifactKind::DependencyTree,
                            path: "/root/mole/node_modules".into(),
                            bytes: 200 * 1024 * 1024,
                            mtime_max: 0,
                            ecosystem: None,
                            hardlinked: false,
                            dedup_stale: false,
                            allocated_bytes: None,
                            allocated_growth_bytes: None,
                            local_bytes: 0,
                            track: None,
                            growth_bytes: Some(150 * 1024 * 1024),
                            regrowth_count: 0,
                            observed_at: 1000,
                            confidence: Confidence::High,
                            source: Source::new("test"),
                            note: None,
                            created_at: None,
                            containers: Vec::new(),
                            shared_with: Vec::new(),
                            dangling: false,
                            evidence: Vec::new(),
                        },
                        ArtifactRow {
                            kind: ArtifactKind::Source,
                            path: "/root/mole/src".into(),
                            bytes: 10 * 1024 * 1024,
                            mtime_max: 0,
                            ecosystem: None,
                            hardlinked: false,
                            dedup_stale: false,
                            allocated_bytes: None,
                            allocated_growth_bytes: None,
                            local_bytes: 0,
                            track: None,
                            growth_bytes: Some(1024),
                            regrowth_count: 0,
                            observed_at: 1000,
                            confidence: Confidence::High,
                            source: Source::new("test"),
                            note: None,
                            created_at: None,
                            containers: Vec::new(),
                            shared_with: Vec::new(),
                            dangling: false,
                            evidence: Vec::new(),
                        },
                    ],
                    signals: vec![],
                    branch: None,
                    github: None,
                    merge_complete: None,
                    idle_secs: None,
                }],
            }],
            unowned: vec![],
            reconciliation: Reconciliation {
                unique_estimate: None,
                attributed: 0,
                unowned: 0,
                walked_total: 0,
                du_total: None,
                docker_attributed: 0,
                docker_unowned: 0,
            },
            notes: vec![],
            series_by_key: Default::default(),
            total_series: Vec::new(),
            series_window_secs: 0,
            summary: Default::default(),
            dirs_by_worktree: None,
            files_by_worktree: None,
            schedule_line: None,
            github_enrichment: None,
            nested_artifacts: Vec::new(),
        }
    }

    #[test]
    fn history_span_reads_the_root_scoped_store_the_picker_needs() {
        let store = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let id = swamp_core::growth::root_scoped_volume_id(root.path());
        let now = swamp_core::entities::now();
        let mut projects = fixture_report().projects;
        swamp_core::growth::observe_and_annotate(
            &swamp_core::bus::Stage::for_tests(),
            store.path(),
            id,
            &mut projects,
            now - 7_200,
            30,
            3600,
            &HashSet::new(),
        )
        .unwrap();
        let span = crate::history_span(store.path(), root.path());
        assert!(
            span.is_some_and(|s| (7_100..=7_400).contains(&s)),
            "the picker must see the 2h of history the header shows: {span:?}"
        );
        assert_eq!(
            crate::history_span_of_roots(store.path(), &[root.path().to_path_buf()]),
            span
        );
    }

    #[test]
    fn a_project_row_marks_the_projects_artifacts_without_entering_it() {
        let mut app = App::new(fixture_report(), "/root".into());
        app.clear_filter();
        assert_eq!(app.view, ViewKind::Projects);
        app.selected = 0;
        app.mark_selected();
        assert!(
            app.refusal_active().is_none(),
            "a project row is actionable from the projects view"
        );
        assert!(!app.marked.is_empty(), "the project's artifacts are marked");
        assert!(
            app.marked.keys().any(|k| k.ends_with("node_modules")),
            "the dependency tree is included: {:?}",
            app.marked.keys().collect::<Vec<_>>()
        );
        assert!(
            !app.marked.keys().any(|k| k.ends_with("/src")),
            "the source tree is not: {:?}",
            app.marked.keys().collect::<Vec<_>>()
        );
        // A second press clears the project rather than re-marking it.
        app.mark_selected();
        assert!(app.marked.is_empty());
    }

    /// The same report with every rebuildable artifact removed: a
    /// checkout, its source and nothing else.
    fn report_with_only_source() -> Report {
        let mut report = fixture_report();
        for p in report.projects.iter_mut() {
            for wt in p.worktrees.iter_mut() {
                wt.artifacts
                    .retain(|a| a.kind == swamp_core::report::ArtifactKind::Source);
            }
        }
        report
    }

    #[test]
    fn a_project_with_nothing_rebuildable_offers_its_checkout() {
        let mut app = App::new(report_with_only_source(), "/root".into());
        app.clear_filter();
        app.selected = 0;
        app.mark_selected();
        assert!(
            app.refusal_active().is_none(),
            "the row must do something rather than refuse"
        );
        assert!(
            app.marked.contains_key("/root/mole"),
            "the checkout is the only thing this project has: {:?}",
            app.marked.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn mark_all_never_reaches_for_a_checkout() {
        let mut app = App::new(report_with_only_source(), "/root".into());
        app.clear_filter();
        app.mark_all_in_view();
        assert!(
            app.marked.is_empty(),
            "A over a screen of projects must not queue checkouts: {:?}",
            app.marked.keys().collect::<Vec<_>>()
        );
        assert!(app.refusal_active().is_some());
    }

    #[test]
    fn backspace_on_a_project_row_opens_one_confirm() {
        let mut app = App::new(fixture_report(), "/root".into());
        app.clear_filter();
        app.selected = 0;
        app.delete_here();
        assert!(
            app.confirm_open,
            "Backspace asks once for the whole project"
        );
        assert!(!app.marked.is_empty());
    }

    #[test]
    fn source_rows_mark_with_a_warning_instead_of_a_refusal() {
        let mut app = App::new(fixture_report(), "/root".into());
        app.clear_filter();
        app.set_view(ViewKind::Tree);
        // row 0 = worktree, row 1 = node_modules, row 2 = src (a Source row)
        app.selected = 2;
        app.mark_selected();
        assert_eq!(app.marked.len(), 1, "anything with a path can be marked");
        assert!(app.refusal_active().is_none());
        // Space toggles it off again.
        app.mark_selected();
        assert!(app.marked.is_empty());
    }

    #[test]
    fn mark_all_marks_what_it_can_and_opens_one_confirm() {
        let mut app = App::new(fixture_report(), "/root".into());
        app.set_view(ViewKind::Deps);
        app.mark_all_in_view();
        assert!(!app.marked.is_empty(), "dependency trees are actionable");
        assert!(app.confirm_open, "one confirm for the whole set");
    }

    #[test]
    fn r_in_the_blocked_list_checks_again_and_replaces_only_what_the_check_added() {
        let mut app = App::new(fixture_report(), "/root".into());
        app.set_view(ViewKind::Deps);
        app.review_in_background(true, true);
        wait_operation(&mut app);
        let first: Vec<String> = app.marked.keys().cloned().collect();
        assert!(!first.is_empty() && app.confirm_open);
        // A mark made another way, and one blocked item to look at.
        let mut extra = app.marked.values().next().unwrap().clone();
        extra.path = "/root/elsewhere/extra".into();
        app.marked.insert("/root/elsewhere/extra".into(), extra);
        app.blocked = vec![BlockedItem {
            name: "x".into(),
            reason: "in use".into(),
            next: "close it".into(),
        }];
        app.open_blocked();
        assert!(app.blocked_open);
        crate::handle_key(&mut app, crossterm::event::KeyCode::Char('r'));
        assert!(!app.blocked_open, "the sheet closes while it checks");
        assert!(app.operation.is_some(), "the same check runs again");
        wait_operation(&mut app);
        let again: Vec<String> = app
            .marked
            .keys()
            .filter(|k| *k != "/root/elsewhere/extra")
            .cloned()
            .collect();
        assert_eq!(again, first, "same target, same marks: not toggled off");
        assert!(
            app.marked.contains_key("/root/elsewhere/extra"),
            "marks made another way stay"
        );
        assert!(app.confirm_open, "the plan comes back");
    }

    #[test]
    fn right_goes_in_and_left_comes_back_out() {
        let mut app = App::new(fixture_report(), "/root".into());
        assert_eq!(app.view, ViewKind::Projects);
        // Left at the top level is not an exit: there is no level above.
        app.leave_row();
        assert_eq!(
            app.view,
            ViewKind::Projects,
            "left must not leave the projects view"
        );
        app.enter_row();
        assert_eq!(app.view, ViewKind::Tree, "right opens the project");
        // In the tree, left first collapses the expanded row under the
        // cursor, the way a file tree does; only then does it go out.
        app.leave_row();
        assert_eq!(app.view, ViewKind::Tree, "the first left collapsed the row");
        assert!(
            app.selected_row()
                .is_some_and(|r| r.collapsed_children.is_some()),
            "the row under the cursor is now collapsed"
        );
        app.leave_row();
        assert_eq!(
            app.view,
            ViewKind::Projects,
            "the second left comes back out"
        );
    }

    #[test]
    fn prune_removed_drops_worktrees_dirs_and_bytes_under_the_removed_paths() {
        let mut app = App::new(fixture_report(), "/root".into());
        let before = app.report.reconciliation.attributed;
        let wt_path = app.report.projects[0].worktrees[0].path.clone();
        let art = app.report.projects[0].worktrees[0].artifacts[0].clone();
        let mut dirs = std::collections::HashMap::new();
        dirs.insert(
            app.report.projects[0].worktrees[0].worktree_id.clone(),
            vec![swamp_core::report::DirRollup {
                worktree_id: app.report.projects[0].worktrees[0].worktree_id.clone(),
                track: None,
                rel_path: "node_modules/x".into(),
                parent_rel_path: None,
                allocated_total: 1,
                own_allocated: 1,
                file_count: 1,
                entry_count: 1,
                symlink_count: 0,
                mod_time_min: 0,
                complete: true,
                growth_bytes: None,
            }],
        );
        app.report.dirs_by_worktree = Some(dirs);
        app.prune_removed(&[actions::UnitResult {
            path: art.path.clone(),
            outcome: Ok(swamp_core::execution::Outcome {
                unit_id: String::new(),
                status: "ok".into(),
                reason: None,
                intended_bytes: 0,
                observed_free_space_delta: None,
            }),
        }]);
        assert!(
            app.report.projects[0].worktrees[0]
                .artifacts
                .iter()
                .all(|a| a.path != art.path)
        );
        assert_eq!(
            app.report.reconciliation.attributed,
            before.saturating_sub(art.bytes)
        );
        assert!(
            app.report
                .dirs_by_worktree
                .as_ref()
                .unwrap()
                .values()
                .all(|rows| rows.is_empty()),
            "dir rollups under the removed artifact go too"
        );
        // A whole worktree removal empties its project.
        app.prune_removed(&[actions::UnitResult {
            path: wt_path,
            outcome: Ok(swamp_core::execution::Outcome {
                unit_id: String::new(),
                status: "ok".into(),
                reason: None,
                intended_bytes: 0,
                observed_free_space_delta: None,
            }),
        }]);
        assert!(app.report.projects.is_empty());
        assert_eq!(app.selected, 0);
    }

    fn report_with_reconciled_unique_estimate() -> Report {
        let mut report = fixture_report();
        report.reconciliation.unique_estimate = Some(swamp_core::report::UniqueEstimate {
            sharing: Some(expected_sharing_summary()),
            bytes: 123_456,
            reconciled_at: 987_654,
            needs_reconciliation: false,
        });
        report
    }

    fn expected_sharing_summary() -> swamp_core::sharing::SharingSummary {
        swamp_core::sharing::SharingSummary {
            groups: vec![swamp_core::sharing::SharingGroup {
                containers: vec![PathBuf::from("/fixture/a"), PathBuf::from("/fixture/b")],
                bytes: 4096,
                unresolved_links: false,
            }],
            omitted_groups: 2,
            omitted_bytes: 8192,
        }
    }

    fn synthetic_unit_result(path: &str, succeeded: bool) -> actions::UnitResult {
        actions::UnitResult {
            path: PathBuf::from(path),
            outcome: if succeeded {
                Ok(swamp_core::execution::Outcome {
                    unit_id: "synthetic".into(),
                    status: "ok".into(),
                    reason: None,
                    intended_bytes: 10,
                    observed_free_space_delta: None,
                })
            } else {
                Err("synthetic failure".into())
            },
        }
    }

    #[test]
    fn successful_local_mutation_marks_unique_estimate_stale_without_changing_its_facts() {
        let mut app = App::new(report_with_reconciled_unique_estimate(), "/root".into());

        app.prune_removed(&[synthetic_unit_result("/outside/scope/removed", true)]);

        let estimate = app.report.reconciliation.unique_estimate.as_ref().unwrap();
        assert!(estimate.needs_reconciliation);
        assert_eq!(estimate.bytes, 123_456);
        assert_eq!(estimate.reconciled_at, 987_654);
        assert_eq!(estimate.sharing, Some(expected_sharing_summary()));
    }

    #[test]
    fn failed_only_and_empty_local_mutations_leave_unique_estimate_current() {
        let mut failed = App::new(report_with_reconciled_unique_estimate(), "/root".into());
        failed.prune_removed(&[synthetic_unit_result("/outside/scope/failed", false)]);
        assert!(
            !failed
                .report
                .reconciliation
                .unique_estimate
                .as_ref()
                .unwrap()
                .needs_reconciliation
        );

        let mut empty = App::new(report_with_reconciled_unique_estimate(), "/root".into());
        empty.prune_removed(&[]);
        assert!(
            !empty
                .report
                .reconciliation
                .unique_estimate
                .as_ref()
                .unwrap()
                .needs_reconciliation
        );
    }

    #[test]
    fn mixed_local_mutation_invalidates_estimate_when_any_unit_succeeded() {
        let mut app = App::new(report_with_reconciled_unique_estimate(), "/root".into());

        app.prune_removed(&[
            synthetic_unit_result("/outside/scope/removed", true),
            synthetic_unit_result("/outside/scope/refused", false),
        ]);

        let estimate = app.report.reconciliation.unique_estimate.as_ref().unwrap();
        assert!(estimate.needs_reconciliation);
        assert_eq!(estimate.bytes, 123_456);
        assert_eq!(estimate.reconciled_at, 987_654);
        assert_eq!(estimate.sharing, Some(expected_sharing_summary()));
    }

    #[test]
    fn backspace_marks_the_current_row_and_asks_once() {
        let mut app = App::new(fixture_report(), "/root".into());
        app.set_view(ViewKind::Tree);
        app.selected = 1; // node_modules
        app.delete_here();
        assert_eq!(app.marked.len(), 1);
        assert!(app.confirm_open, "one 'are you sure', with the facts on it");
        assert!(app.confirm_summary().contains("node_modules"));
        assert!(app.confirm_summary().contains("→ Trash"));
    }

    #[test]
    fn mark_and_confirm_full_flow() {
        let mut app = App::new(fixture_report(), "/root".into());
        app.set_view(ViewKind::Tree);
        app.selected = 1; // node_modules
        app.mark_selected();
        assert_eq!(app.marked.len(), 1);
        app.open_confirm();
        assert!(app.confirm_open);
        assert!(app.confirm_summary().contains("Move 1 item"));
    }

    #[test]
    fn bulk_refusals_are_counted_not_first_only() {
        let a = "nothing reclaimable in this project".to_string();
        let b = "protected".to_string();
        assert_eq!(summarize_refusals(&[]), None);
        assert_eq!(
            summarize_refusals(std::slice::from_ref(&a)),
            Some(a.clone())
        );
        let m = summarize_refusals(&[a.clone(), a.clone(), b.clone()]).unwrap();
        assert!(m.starts_with("3 rows skipped:"), "{m}");
        assert!(m.contains("(x2)") && m.contains("protected (x1)"), "{m}");
    }

    #[test]
    fn refusal_expires_after_display_window() {
        let mut app = App::new(fixture_report(), "/root".into());
        app.set_refusal("test reason");
        assert!(app.refusal_active().is_some());
        app.refusal = Some((
            "refused: test".into(),
            Instant::now() - Duration::from_secs(5),
        ));
        assert!(app.refusal_active().is_none());
    }

    #[test]
    fn clear_filter_shows_zero_and_none() {
        let mut app = App::new(fixture_report(), "/root".into());
        app.clear_filter();
        assert_eq!(app.filter, Filter::default());
        assert_eq!(app.filter_text, "0");
    }

    #[test]
    fn bad_filter_keeps_previous_and_sets_error() {
        let mut app = App::new(fixture_report(), "/root".into());
        let before = app.filter.clone();
        app.filter_text = "bananas".to_string();
        app.commit_filter();
        assert_eq!(app.filter, before);
        assert!(app.filter_error.is_some());
    }

    fn region(
        path: &str,
        status: swamp_core::coverage::RegionStatus,
    ) -> swamp_core::coverage::RootCoverage {
        swamp_core::coverage::RootCoverage {
            path: path.into(),
            status,
            walked_total: 0,
            projects: 0,
            mode: String::new(),
            reached_by_registry: Vec::new(),
        }
    }

    #[test]
    fn scope_note_is_none_for_one_complete_region() {
        use swamp_core::coverage::RegionStatus;
        let mut app = App::new(fixture_report(), "/root".into());
        app.set_scope_note(&[region("/root", RegionStatus::Complete)]);
        assert_eq!(app.scope_note, None);
    }

    #[test]
    fn scope_note_names_count_and_worst_status_for_a_missing_root() {
        use swamp_core::coverage::RegionStatus;
        let mut app = App::new(fixture_report(), "/root".into());
        app.set_scope_note(&[
            region("/root", RegionStatus::Complete),
            region("/gone", RegionStatus::Missing),
        ]);
        assert_eq!(app.scope_note.as_deref(), Some("2 roots (1 missing)"));
    }

    #[test]
    fn scope_note_prioritizes_inaccessible_over_missing_and_names_the_reason() {
        use swamp_core::coverage::RegionStatus;
        let mut app = App::new(fixture_report(), "/root".into());
        app.set_scope_note(&[
            region("/root", RegionStatus::Complete),
            region("/gone", RegionStatus::Missing),
            region(
                "/denied",
                RegionStatus::Inaccessible {
                    reason: "permission denied".into(),
                },
            ),
        ]);
        assert_eq!(
            app.scope_note.as_deref(),
            Some("3 roots (2 inaccessible: permission denied)")
        );
    }

    /// `RegionStatus::Partial` (part of a `Present` root was unreadable
    /// *during this walk*) is a real outcome `scope::RootStatus` alone
    /// never had -- the whole reason `set_scope_note` moved from
    /// `EffectiveScope` to post-walk `RootCoverage` (#51).
    #[test]
    fn scope_note_reports_a_partial_region_with_its_reason() {
        use swamp_core::coverage::RegionStatus;
        let mut app = App::new(fixture_report(), "/root".into());
        app.set_scope_note(&[
            region("/root", RegionStatus::Complete),
            region(
                "/flaky",
                RegionStatus::Partial {
                    reason: "2 path(s) unreadable during this walk".into(),
                },
            ),
        ]);
        assert_eq!(
            app.scope_note.as_deref(),
            Some("2 roots (1 partial: 2 path(s) unreadable during this walk)")
        );
    }

    /// A root a config `exclude` pruned still gets its own coverage
    /// row (`RootCoverage::excluded`) -- never silently absent, and
    /// never labelled "deleted".
    #[test]
    fn scope_note_names_an_excluded_region() {
        use swamp_core::coverage::RegionStatus;
        let mut app = App::new(fixture_report(), "/root".into());
        app.set_scope_note(&[
            region("/root", RegionStatus::Complete),
            region("/scratch", RegionStatus::Excluded),
        ]);
        assert_eq!(app.scope_note.as_deref(), Some("2 roots (1 excluded)"));
    }

    #[test]
    fn view_cycles_within_a_section_and_keys_jump_to_sections() {
        assert_eq!(ViewKind::Projects.next(), ViewKind::Tree);
        assert_eq!(Section::from_key('1'), Some(Section::Projects));
        assert_eq!(Section::from_key('2'), Some(Section::Tools));
        assert_eq!(Section::from_key('3'), Some(Section::Disk));
        // '0' is reserved for "clear filter" (crate::handle_key_mod), and
        // 4-9 are no longer keys at all.
        for k in ['0', '4', '9', 'c', 'D', 'I'] {
            assert_eq!(Section::from_key(k), None, "{k}");
        }
        // `v` wraps inside the section it is in.
        for s in Section::ALL {
            let mut v = s.default_view();
            for _ in 0..s.views().len() {
                assert_eq!(v.section(), s);
                v = v.next();
            }
            assert_eq!(v, s.default_view());
        }
        assert_eq!(ViewKind::Kinds.next(), ViewKind::Unowned);
        assert_eq!(ViewKind::Unowned.next(), ViewKind::Projects);
        assert_eq!(ViewKind::Reclaim.next(), ViewKind::Docker);
        assert_eq!(ViewKind::Agents.next(), ViewKind::Reclaim);
        assert_eq!(Section::Disk.next(), Section::Projects);
        assert_eq!(Section::Projects.prev(), Section::Disk);
        // Every view is in exactly one section.
        let total: usize = Section::ALL.iter().map(|s| s.views().len()).sum();
        assert_eq!(total, ViewKind::ALL.len());
        for v in ViewKind::ALL {
            assert!(v.section().views().contains(&v), "{v:?}");
        }
    }

    // -----------------------------------------------------------------
    // #51: multi-root reports, coverage inspection, and live refresh.
    // -----------------------------------------------------------------

    fn minimal_report(root: &str, project_name: &str, worktree_path: &str) -> Report {
        Report {
            store_dir: None,
            observed_at: 1000,
            root: root.into(),
            projects: vec![ProjectRow {
                project_id: format!("{project_name}-id"),
                name: project_name.into(),
                remote: None,
                ecosystems: Vec::new(),
                worktrees: vec![WorktreeRow {
                    worktree_id: format!("{project_name}-wt"),
                    path: worktree_path.into(),
                    kind: WorktreeKind::Main,
                    artifacts: Vec::new(),
                    signals: Vec::new(),
                    branch: None,
                    github: None,
                    merge_complete: None,
                    idle_secs: None,
                }],
            }],
            unowned: vec![],
            reconciliation: Reconciliation {
                unique_estimate: None,
                attributed: 0,
                unowned: 0,
                walked_total: 0,
                du_total: None,
                docker_attributed: 0,
                docker_unowned: 0,
            },
            notes: vec![],
            series_by_key: Default::default(),
            total_series: Vec::new(),
            series_window_secs: 0,
            summary: Default::default(),
            dirs_by_worktree: None,
            files_by_worktree: None,
            schedule_line: None,
            github_enrichment: None,
            nested_artifacts: Vec::new(),
        }
    }

    #[test]
    fn new_multi_root_covers_every_root_and_picks_the_first_as_primary() {
        let a = minimal_report("/roots/a", "proj-a", "/roots/a/proj-a");
        let app = App::new_multi_root(a, vec!["/roots/a".into(), "/roots/b".into()]);
        assert_eq!(app.root, PathBuf::from("/roots/a"));
        assert_eq!(
            app.roots,
            vec![PathBuf::from("/roots/a"), PathBuf::from("/roots/b")]
        );
    }

    /// The core #51 guarantee: refreshing one root's report must not
    /// erase, stale-mark, or duplicate another root's rows. This is the
    /// adversarial case a naive "just replace `self.report` wholesale"
    /// implementation would fail immediately.
    #[test]
    fn replacing_one_roots_report_leaves_every_other_root_untouched() {
        let a = minimal_report("/roots/a", "proj-a", "/roots/a/proj-a");
        let b = minimal_report("/roots/b", "proj-b", "/roots/b/proj-b");
        let mut app = App::new_multi_root(a.clone(), vec!["/roots/a".into(), "/roots/b".into()]);
        app.reports_by_root
            .insert(PathBuf::from("/roots/a"), a.clone());
        app.reports_by_root.insert(PathBuf::from("/roots/b"), b);
        app.report = swamp_core::report::merge_reports(&app.roots, &app.reports_by_root);
        assert_eq!(app.report.projects.len(), 2, "{:?}", app.report.projects);

        // A fresh observation of root A only -- root B's cached entry is
        // never read or written by this call.
        let mut a2 = a;
        a2.projects[0].worktrees[0].artifacts = Vec::new();
        a2.reconciliation.walked_total = 999;
        app.replace_report_for_root(PathBuf::from("/roots/a"), a2);

        assert_eq!(
            app.report.projects.len(),
            2,
            "root B's project must still be present after only root A refreshed: {:?}",
            app.report.projects
        );
        assert!(
            app.report.projects.iter().any(|p| p.name == "proj-b"),
            "{:?}",
            app.report.projects
        );
        assert!(app.report.projects.iter().any(|p| p.name == "proj-a"));
    }

    /// A root that stops being observable (removed from scope, access
    /// lost) simply keeps its last entry in `reports_by_root` -- nothing
    /// ever deletes an entry on its own, so its rows survive in `report`
    /// until a caller deliberately narrows `roots`/`reports_by_root`.
    /// This mirrors `coverage-changes-are-not-storage-changes`: losing
    /// *coverage* of a root is never treated as that root's data having
    /// been deleted.
    #[test]
    fn a_root_no_longer_refreshed_keeps_its_last_known_rows() {
        let a = minimal_report("/roots/a", "proj-a", "/roots/a/proj-a");
        let b = minimal_report("/roots/b", "proj-b", "/roots/b/proj-b");
        let mut app = App::new_multi_root(a.clone(), vec!["/roots/a".into(), "/roots/b".into()]);
        app.reports_by_root.insert(PathBuf::from("/roots/a"), a);
        app.reports_by_root
            .insert(PathBuf::from("/roots/b"), b.clone());
        app.report = swamp_core::report::merge_reports(&app.roots, &app.reports_by_root);

        // Root B "loses access" (its watcher/observer never fires again,
        // e.g. an unmounted volume) -- only root A ever refreshes again.
        let mut a2 = app.reports_by_root[&PathBuf::from("/roots/a")].clone();
        a2.reconciliation.walked_total = 42;
        app.replace_report_for_root(PathBuf::from("/roots/a"), a2);
        assert!(app.report.projects.iter().any(|p| p.name == "proj-b"));
        assert_eq!(
            app.reports_by_root[&PathBuf::from("/roots/b")]
                .projects
                .len(),
            b.projects.len(),
            "root B's cached entry itself must be untouched"
        );
    }

    /// A report with zero projects (no Git checkout anywhere in scope)
    /// still renders and still carries external/agent units -- the
    /// concrete #51 acceptance case "project/shared/external units
    /// available even when no Git checkout exists". Rendering must not
    /// panic on an all-unowned, project-free report.
    #[test]
    fn external_only_report_with_no_projects_still_renders() {
        let mut report = minimal_report("/roots/a", "unused", "/roots/a/unused");
        report.projects.clear();
        let mut app = App::new_multi_root(report, vec!["/roots/a".into()]);
        app.set_external_units(vec![swamp_core::external::ExternalUnit {
            detector_id: "homebrew".into(),
            detector_name: "Homebrew".into(),
            category: swamp_core::locations::StorageCategory::Downloads,
            provenance: swamp_core::locations::Provenance::BuiltinConvention,
            path: "/roots/a/.brew-cache".into(),
            bytes: 12_345,
            mtime_max: 0,
            hardlinked: false,
            growth_bytes: None,
            regrowth_count: 0,
            observed_at: 1000,
            consumers: Vec::new(),
            note: None,
            evidence: Vec::new(),
            bytes_counted_elsewhere: 0,
            overlap_count: 0,
            last_used: Default::default(),
            children: Vec::new(),
        }]);
        assert!(app.rows().is_empty(), "no projects, no project rows");
        app.set_view(ViewKind::External);
        assert_eq!(app.external_units.len(), 1);
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &app)).unwrap();
    }

    /// A machine-wide store's identified interior opens under its row in
    /// the External view, in the same family groups a project container
    /// uses -- closed until opened, every row blocked, and nothing
    /// selectable -- at both a narrow and a wide terminal.
    #[test]
    fn a_store_interior_opens_under_its_external_row_in_family_groups() {
        use swamp_core::artifact::{AccountingBasis, ArtifactRole, TimeSource};
        use swamp_core::build_adapters::{BuildContainer, NestedUnitBuilder};
        let mut report = minimal_report("/roots/a", "p", "/roots/a/p");
        report.projects.clear();
        let mut app = App::new_multi_root(report, vec!["/roots/a".into()]);
        let repo = std::path::PathBuf::from("/fixture/.m2/repository");
        app.set_external_units(vec![swamp_core::external::ExternalUnit {
            detector_id: "maven".into(),
            detector_name: "Maven".into(),
            category: swamp_core::locations::StorageCategory::Unclassified,
            provenance: swamp_core::locations::Provenance::BuiltinConvention,
            path: repo.clone(),
            bytes: 100_000,
            mtime_max: 0,
            hardlinked: false,
            growth_bytes: None,
            regrowth_count: 0,
            observed_at: 1000,
            consumers: Vec::new(),
            note: None,
            evidence: Vec::new(),
            bytes_counted_elsewhere: 0,
            overlap_count: 0,
            last_used: Default::default(),
            children: Vec::new(),
        }]);
        let c = BuildContainer::shared_store_of(
            "maven",
            repo.clone(),
            swamp_core::locations::BuildStoreKind::MavenRepository,
        );
        let root = NestedUnitBuilder::new(&c, ArtifactRole::SharedStoreEntry, repo.clone())
            .is_dir(true)
            .bytes_on_basis(100_000, AccountingBasis::Allocated)
            .supported_with_reason("fixture repository")
            .build();
        let entry = NestedUnitBuilder::new(
            &c,
            ArtifactRole::SharedStoreEntry,
            repo.join("org/example/lib/1.0"),
        )
        .is_dir(true)
        .bytes_on_basis(60_000, AccountingBasis::Allocated)
        .modified(500, TimeSource::FoldedDirectoryModification)
        .supported_with_reason("fixture artifact")
        .consequence("downloaded again on the next build")
        .no_action_because("shared")
        .build();
        app.set_store_interiors(vec![root, entry]);
        app.set_view(ViewKind::External);
        let closed = app.rows();
        assert_eq!(closed.len(), 1, "closed until opened");
        assert!(closed[0].expandable);
        app.selected = 0;
        app.enter_row();
        let open = app.rows();
        assert!(
            open.iter()
                .any(|r| r.label == swamp_core::artifact::RoleFamily::SharedStore.title()),
            "the family group appears: {:?}",
            open.iter().map(|r| r.label.clone()).collect::<Vec<_>>()
        );
        // Tempting wrong patch: interior rows are "blocked" and never
        // selectable. A family header groups paths (no single path, no
        // unit, and it no longer says "blocked" or "inspection only"); the
        // paths under it are marked on their own rows.
        for r in open.iter().skip(1) {
            assert!(
                r.unit.is_none(),
                "{}: a group header is not one path",
                r.label
            );
            assert!(r.signals.iter().all(|s| s != "blocked"), "{}", r.label);
            assert!(
                r.signals.iter().all(|s| !s.contains("inspection only")),
                "{:?}",
                r.signals
            );
        }
        for width in [80u16, 160] {
            let backend = ratatui::backend::TestBackend::new(width, 24);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            terminal.draw(|f| crate::ui::draw(f, &app)).unwrap();
        }
    }

    fn drilled_unit(
        path: &str,
        children: Vec<swamp_core::drilldown::UnitChild>,
    ) -> swamp_core::external::ExternalUnit {
        swamp_core::external::ExternalUnit {
            detector_id: "rustup".into(),
            detector_name: "rustup".into(),
            category: swamp_core::locations::StorageCategory::Installation,
            provenance: swamp_core::locations::Provenance::BuiltinConvention,
            path: path.into(),
            bytes: 1_000,
            mtime_max: 0,
            hardlinked: false,
            growth_bytes: None,
            regrowth_count: 0,
            observed_at: 1_790_000_000,
            consumers: Vec::new(),
            note: None,
            evidence: Vec::new(),
            bytes_counted_elsewhere: 0,
            overlap_count: 0,
            last_used: swamp_core::last_used::resolve(None, Some(1_783_468_800)),
            children,
        }
    }

    /// #176/#178 in the External view: the unit's last-used fact with its
    /// source, its folders as inspection-only rows that add up, and a
    /// folder that could not be read never drawn as `0B`.
    #[test]
    fn an_external_unit_opens_onto_its_depth_two_rows_and_says_when_it_was_last_used() {
        use swamp_core::drilldown::{ChildKind, ChildMeasure, UnitChild};
        let child = |name: &str, bytes: Option<i64>, measure| UnitChild {
            kind: ChildKind::Entry,
            name: name.into(),
            bytes,
            measure,
            mtime_max: 1_789_000_000,
            entries: 0,
            not_measured: 0,
            last_used: swamp_core::last_used::resolve(None, Some(1_783_468_800)),
        };
        let mut report = minimal_report("/roots/a", "p", "/roots/a/p");
        report.projects.clear();
        let mut app = App::new_multi_root(report, vec!["/roots/a".into()]);
        let mut rest = child("", Some(100), ChildMeasure::Complete);
        rest.kind = ChildKind::Remainder;
        rest.entries = 3;
        app.set_external_units(vec![drilled_unit(
            "/fixture/.rustup/toolchains",
            vec![
                child("stable", Some(600), ChildMeasure::Complete),
                child("nightly", Some(300), ChildMeasure::Complete),
                child("locked", None, ChildMeasure::NotMeasured),
                rest,
            ],
        )]);
        app.set_view(ViewKind::External);
        let closed = app.rows();
        assert_eq!(closed.len(), 1);
        assert!(closed[0].expandable, "a unit with rows opens");
        let fact = closed[0].last_used.as_deref().unwrap();
        assert!(
            fact.starts_with("Last run or opened: Jul 8") && fact.ends_with("(file access time)"),
            "{fact}"
        );
        app.selected = 0;
        app.enter_row();
        let open = app.rows();
        assert_eq!(open.len(), 5);
        let shown: u64 = open.iter().skip(1).map(|r| r.bytes).sum();
        assert_eq!(shown, 1_000, "the rows add up to the unit");
        let locked = open.iter().find(|r| r.label.contains("locked")).unwrap();
        assert!(
            locked.label.contains("not measured"),
            "never a bare size for an unreadable folder: {}",
            locked.label
        );
        // Tempting wrong patch: the folders are tagged "blocked" with no
        // unit, so what the person sees cannot be moved. Every listed
        // folder is a real path and markable; only the remainder row, which
        // is not a folder, is not, and it says why.
        for r in open.iter().skip(1) {
            assert!(r.signals.iter().all(|s| s != "blocked"), "{:?}", r.signals);
            if r.label.contains("other") || r.label.contains("3 ") {
                continue;
            }
        }
        let folders = open.iter().skip(1).filter(|r| r.unit.is_some()).count();
        assert_eq!(folders, 3, "stable, nightly and the unmeasured folder");
        let remainder = open.iter().skip(1).find(|r| r.unit.is_none()).unwrap();
        assert!(
            remainder
                .detail_lines
                .iter()
                .any(|l| l.contains("mark the unit")),
            "{:?}",
            remainder.detail_lines
        );
        let stable = open.iter().find(|r| r.label == "stable").unwrap();
        assert!(
            stable
                .last_used
                .as_deref()
                .unwrap()
                .contains("(file access time)")
        );
        // The detail pane carries the fact.
        let lines = crate::detail::lines(&closed[0], &[]);
        assert!(
            lines.iter().any(|l| l.starts_with("Last run or opened:")),
            "{lines:?}"
        );
    }

    /// Reviewer M2: the same bytes never appear twice under one row. The
    /// interior of a unit that sits under another (go-build under
    /// Library/Caches) belongs to that unit, and a unit's own interior is
    /// held under one closed header beside its folder rows.
    #[test]
    fn a_folder_or_interior_is_listed_once_and_the_visible_rows_add_up_once() {
        use swamp_core::artifact::{AccountingBasis, ArtifactRole};
        use swamp_core::build_adapters::{BuildContainer, NestedUnitBuilder};
        use swamp_core::drilldown::{ChildKind, ChildMeasure, UnitChild};
        let entry = |name: &str, bytes: i64| UnitChild {
            kind: ChildKind::Entry,
            name: name.into(),
            bytes: Some(bytes),
            measure: ChildMeasure::Complete,
            mtime_max: 5,
            entries: 0,
            not_measured: 0,
            last_used: Default::default(),
        };
        let mut caches = drilled_unit("/fixture/Caches", vec![entry("a", 600), entry("b", 400)]);
        caches.category = swamp_core::locations::StorageCategory::Unclassified;
        let mut gobuild = drilled_unit("/fixture/Caches/go-build", Vec::new());
        gobuild.detector_id = "go".into();
        gobuild.bytes = 300;
        let make = |root: &str, unit: &str| {
            let c = BuildContainer::shared_store_of(
                "go",
                root.into(),
                swamp_core::locations::BuildStoreKind::GoBuildCache,
            );
            NestedUnitBuilder::new(&c, ArtifactRole::Intermediate, unit.into())
                .is_dir(true)
                .bytes_on_basis(300, AccountingBasis::Allocated)
                .supported_with_reason("fixture")
                .consequence("recompiled")
                .no_action_because("shared")
                .build()
        };
        let mut report = minimal_report("/roots/a", "p", "/roots/a/p");
        report.projects.clear();
        let mut app = App::new_multi_root(report, vec!["/roots/a".into()]);
        app.set_external_units(vec![caches, gobuild]);
        app.set_store_interiors(vec![
            make("/fixture/Caches/go-build", "/fixture/Caches/go-build"),
            make("/fixture/Caches/go-build", "/fixture/Caches/go-build/x"),
        ]);
        app.set_view(ViewKind::External);
        app.selected = 0;
        app.enter_row();
        let open = app.rows();
        let labels: Vec<String> = open.iter().map(|r| r.label.clone()).collect();
        // Caches: its two folders, once. go-build's interior families are
        // not among them.
        // Caches, its two folders, then go-build as its own top-level unit.
        assert_eq!(open.len(), 4, "{labels:?}");
        assert!(labels[3].contains("go-build"), "{labels:?}");
        let shown: u64 = open[1..3].iter().map(|r| r.bytes).sum();
        assert_eq!(
            shown, 1_000,
            "the rows under Caches add up once: {labels:?}"
        );
        assert!(!labels.iter().any(|l| l.contains("Caches & intermediates")));
    }

    /// #171 in the External view: its own kind, markable.
    #[test]
    fn a_standalone_cargo_target_is_a_row_of_the_external_view_too() {
        let mut report = minimal_report("/roots/a", "p", "/roots/a/p");
        report.projects.clear();
        report.unowned.push(swamp_core::report::UnownedRow {
            measurement: None,
            path_or_object: "/fixture/scratch-target".into(),
            bytes: 4096,
            reason: swamp_core::report::UnownedReason::StandaloneCargoTarget,
            shared_bytes: None,
            note: Some("standalone Cargo target: rebuild with `cargo build`; swamp does not link it to a project".into()),
            docker_kind: None,
            created_at: None,
            containers: Vec::new(),
            shared_with: Vec::new(),
            dangling: false,
            evidence: Vec::new(),
        });
        let mut app = App::new_multi_root(report, vec!["/roots/a".into()]);
        app.set_view(ViewKind::External);
        let rows = app.rows();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].label.starts_with("standalone Cargo target"));
        assert!(rows[0].unit.is_some(), "plannable from this view");
    }

    /// #171: a standalone Cargo target directory marks like any unowned
    /// row and its confirm line says what it is and that Cargo remakes it.
    #[test]
    fn a_standalone_cargo_target_marks_with_its_consequence_on_the_confirm_line() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(tmp.path())
            .unwrap()
            .join("scratch-target");
        std::fs::create_dir_all(dir.join("debug")).unwrap();
        std::fs::write(dir.join("debug/blob"), vec![1u8; 4096]).unwrap();
        let mut report = minimal_report("/roots/a", "p", "/roots/a/p");
        report.projects.clear();
        report.unowned.push(swamp_core::report::UnownedRow {
            measurement: Some(swamp_core::report::UnownedMeasurement::Subtree),
            path_or_object: dir.display().to_string(),
            bytes: 4096,
            reason: swamp_core::report::UnownedReason::StandaloneCargoTarget,
            shared_bytes: None,
            note: Some("standalone Cargo target: rebuild with `cargo build`; nothing in it records which project built it, so none is named".into()),
            docker_kind: None,
            created_at: None,
            containers: Vec::new(),
            shared_with: Vec::new(),
            dangling: false,
            evidence: Vec::new(),
        });
        let mut app = App::new_multi_root(report, vec!["/roots/a".into()]);
        app.set_view(ViewKind::Unowned);
        let rows = app.rows();
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].label.contains("standalone Cargo target"),
            "{}",
            rows[0].label
        );
        assert!(rows[0].label.contains("cargo build"));
        let row = rows[0].clone();
        app.mark_row(&row);
        let marked = app
            .marked
            .values()
            .next()
            .expect("marked like any unowned row");
        let text = marked.warnings.join(" | ");
        assert!(text.contains("cargo build"), "{text}");
        assert!(text.contains("Trash"), "{text}");
    }

    // ---- adversarial review (audit/v0.7.5-adversarial) ----

    /// Backspace with nothing marked, then `r` (check again) from the
    /// blocked list, then Esc: every mark the Backspace made must go.
    #[test]
    fn adv_esc_after_check_again_undoes_every_mark_the_opening_press_made() {
        let mut app = App::new(fixture_report(), "/root".into());
        app.set_view(ViewKind::Deps);
        app.review_in_background(true, true);
        wait_operation(&mut app);
        assert!(app.confirm_open && !app.marked.is_empty());
        app.blocked = vec![BlockedItem {
            name: "x".into(),
            reason: "in use".into(),
            next: "close it".into(),
        }];
        app.open_blocked();
        crate::handle_key(&mut app, crossterm::event::KeyCode::Char('r'));
        wait_operation(&mut app);
        assert!(app.confirm_open);
        crate::handle_key(&mut app, crossterm::event::KeyCode::Esc);
        assert!(
            app.marked.is_empty(),
            "nothing was marked before the press; Esc left {} marked: {:?}",
            app.marked.len(),
            app.marked.keys().collect::<Vec<_>>()
        );
    }

    /// `A` pressed again while its own confirm is open, then Esc.
    #[test]
    fn adv_esc_after_a_repeated_while_confirm_open_undoes_the_marks() {
        let mut app = App::new(fixture_report(), "/root".into());
        app.set_view(ViewKind::Deps);
        app.review_in_background(true, true);
        wait_operation(&mut app);
        assert!(app.confirm_open);
        crate::handle_key(&mut app, crossterm::event::KeyCode::Char('A'));
        wait_operation(&mut app);
        crate::handle_key(&mut app, crossterm::event::KeyCode::Esc);
        assert!(
            app.marked.is_empty(),
            "Esc left {} marks made by the confirm's own press",
            app.marked.len()
        );
    }

    /// Evidence: plain Backspace then Esc leaves nothing; an earlier Space
    /// mark survives.
    #[test]
    fn adv_esc_keeps_earlier_space_marks_and_drops_its_own() {
        let mut app = App::new(fixture_report(), "/root".into());
        app.set_view(ViewKind::Deps);
        app.review_in_background(false, false);
        wait_operation(&mut app);
        let space: Vec<String> = app.marked.keys().cloned().collect();
        assert!(!space.is_empty());
        app.review_in_background(false, true);
        wait_operation(&mut app);
        assert!(app.confirm_open);
        app.cancel_confirm();
        assert_eq!(app.marked.keys().cloned().collect::<Vec<_>>(), space);
    }

    // ---- v0.8.0 G1: previous scope and declared roots in the header ----

    /// The header says it is showing the previous scope and points at `R`;
    /// declared roots are a clause; both fit 80 columns and change nothing
    /// below the header line (no row shifts).
    #[test]
    fn the_header_labels_a_previous_scope_and_lists_declared_roots_without_moving_rows() {
        let base = App::new_multi_root(fixture_report(), vec!["/root".into()]);
        let plain = paint(&base, 80, 24);
        let mut app = App::new_multi_root(fixture_report(), vec!["/root".into()]);
        app.previous_scope_roots = Some(3);
        app.set_declared_roots(&[
            swamp_core::roots::DeclaredRoot {
                path: "/root".into(),
                state: swamp_core::roots::DeclaredState::Present {
                    bytes: Some(10),
                    complete: true,
                },
            },
            swamp_core::roots::DeclaredRoot {
                path: "/gone".into(),
                state: swamp_core::roots::DeclaredState::Missing,
            },
        ]);
        let shown = paint(&app, 80, 24);
        let first = shown.lines().next().unwrap();
        assert!(
            first.contains("showing the previous scope (3 roots)"),
            "{first}"
        );
        assert!(first.contains("press R"), "{first}");
        for line in shown.lines() {
            // `TestBackend` prints each row between two quote marks.
            assert!(crate::model::display_width(line) <= 82, "{line}");
        }
        assert_eq!(shown.lines().count(), plain.lines().count());
        // At a wide width the declared-roots clause fits after the rest.
        let wide = paint(&app, 200, 24);
        assert!(
            wide.lines()
                .next()
                .unwrap()
                .contains("2 declared roots (1 missing)"),
            "{}",
            wide.lines().next().unwrap()
        );
        // The help screen lists them.
        let mut helped = App::new_multi_root(fixture_report(), vec!["/root".into()]);
        helped.set_declared_roots(&[swamp_core::roots::DeclaredRoot {
            path: "/gone".into(),
            state: swamp_core::roots::DeclaredState::Missing,
        }]);
        helped.toggle_help();
        assert!(paint(&helped, 100, 100).contains("Declared source roots"));
    }

    // ---- v0.7.5 adversarial fixes ----

    fn paint(app: &App, w: u16, h: u16) -> String {
        use ratatui::{Terminal, backend::TestBackend};
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| crate::ui::draw(f, app)).unwrap();
        t.backend().to_string()
    }

    /// The event loop, one iteration at a time and without a key: what
    /// arrives is applied, and the frame is painted whenever the gate says
    /// so. Returns the last painted frame once `done` holds.
    fn drive_without_keys(app: &mut App, done: impl Fn(&App) -> bool) -> String {
        let mut gate = crate::RedrawGate::default();
        // The size is already known, as it is after the first frame.
        app.width = 100;
        app.height = 30;
        let mut frame = paint(app, 100, 30);
        assert!(gate.due(app));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(Instant::now() < deadline, "never finished");
            crate::advance(app, &mut gate, Some((100, 30)));
            if gate.due(app) {
                frame = paint(app, 100, 30);
            }
            if done(app) {
                return frame;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn every_async_completion_is_on_the_very_next_frame_without_a_key() {
        // (name, start the async work, text the finished frame must show,
        //  text it must no longer show)
        type Case = (&'static str, fn(&mut App), &'static str, &'static str);
        fn check_finished(app: &mut App) {
            app.set_view(ViewKind::Deps);
            app.review_in_background(false, false);
        }
        fn confirm_opens(app: &mut App) {
            app.set_view(ViewKind::Deps);
            app.review_in_background(true, true);
        }
        fn cancelled(app: &mut App) {
            app.set_view(ViewKind::Deps);
            app.review_in_background(true, false);
            app.cancel_operation();
        }
        fn delete_finished(app: &mut App) {
            let (tx, rx) = std::sync::mpsc::channel();
            app.operation = Some(Operation {
                label: "Deleting",
                completed: 0,
                total: 1,
                succeeded: 0,
                failed: 0,
                current: String::new(),
                bytes_done: 0,
                bytes_total: 0,
                started: Instant::now(),
                cancel: Default::default(),
                checking_open_files: None,
            });
            app.operation_rx = Some(rx);
            tx.send(OperationEvent::Deleted {
                results: Vec::new(),
                total: 1,
            })
            .unwrap();
        }
        fn refresh_finished(app: &mut App) {
            let (tx, rx) = std::sync::mpsc::channel();
            app.observed_label = "3 hours ago".into();
            app.observing = Some((0, 0));
            app.pending = Some(rx);
            tx.send(Ok(RefreshedObservation {
                per_root: Vec::new(),
                merged: None,
                external_units: None,
                agent_units: None,
                store_interiors: None,
            }))
            .unwrap();
        }
        fn lock_poll_change(app: &mut App) {
            let (tx, rx) = std::sync::mpsc::channel();
            app.lock_poll_rx = Some(rx);
            tx.send(LockPollMsg::Holder(Some(
                swamp_core::schedule::LockHolder {
                    pid: 4242,
                    since: swamp_core::entities::now(),
                },
            )))
            .unwrap();
        }
        fn blocked_check_again(app: &mut App) {
            app.set_view(ViewKind::Deps);
            app.review_in_background(true, true);
            wait_operation(app);
            app.blocked = vec![BlockedItem {
                name: "x".into(),
                reason: "in use".into(),
                next: "close it".into(),
            }];
            app.open_blocked();
            crate::handle_key(app, crossterm::event::KeyCode::Char('r'));
        }
        let cases: [Case; 7] = [
            ("check finished", check_finished, "marked in all", "Checked"),
            ("confirm opens", confirm_opens, "Enter confirm", "Checked"),
            ("cancelled", cancelled, "Check stopped", "Stopping after"),
            (
                "delete finished",
                delete_finished,
                "Moved",
                "Moving to Trash",
            ),
            (
                "refresh finished",
                refresh_finished,
                "observed just now",
                "3 hours ago",
            ),
            (
                "lock poll change",
                lock_poll_change,
                "another observation running",
                "Enter confirm",
            ),
            (
                "blocked list check again",
                blocked_check_again,
                "Enter confirm",
                "check again",
            ),
        ];
        for (name, start, shows, gone) in cases {
            let mut app = App::new(fixture_report(), "/root".into());
            start(&mut app);
            let frame = drive_without_keys(&mut app, |a| match name {
                "refresh finished" => a.pending.is_none(),
                "lock poll change" => a.external_observer.is_some(),
                _ => a.operation.is_none(),
            });
            assert!(
                frame.contains(shows),
                "{name}: expected {shows:?} in\n{frame}"
            );
            assert!(
                !frame.contains(gone),
                "{name}: {gone:?} still on screen\n{frame}"
            );
        }
    }

    /// After `A`, a refreshed index that no longer lists one of the marked
    /// rows must not leave Enter able to move it, and a reload that
    /// arrives while the confirm is open waits for it.
    #[test]
    fn a_reload_during_a_confirm_waits_and_stale_marks_are_dropped_with_a_message() {
        let mut app = App::new(fixture_report(), "/root".into());
        app.set_view(ViewKind::Deps);
        app.review_in_background(true, true);
        wait_operation(&mut app);
        assert!(app.confirm_open);
        let marked_before: Vec<String> = app.marked.keys().cloned().collect();
        assert!(marked_before.iter().any(|k| k.ends_with("node_modules")));
        let summary_before = app.confirm_summary();
        let mut newer = fixture_report();
        newer.observed_at = 2000;
        for wt in newer.projects.iter_mut().flat_map(|p| &mut p.worktrees) {
            wt.artifacts.retain(|a| !a.path.ends_with("node_modules"));
        }
        let fresh = RefreshedObservation {
            per_root: Vec::new(),
            merged: Some(MergedReports {
                by_root: Default::default(),
                report: newer,
            }),
            external_units: None,
            agent_units: None,
            store_interiors: None,
        };
        app.land_observation(fresh);
        assert!(app.new_data_waiting(), "the reload waits for the confirm");
        assert_eq!(app.report.observed_at, 1000);
        assert_eq!(
            app.confirm_summary(),
            summary_before,
            "Enter's plan is unchanged"
        );
        assert!(paint(&app, 120, 30).contains("new data available"));
        // Esc closes the confirm; the held index lands and the marks the
        // press made are gone anyway.
        app.cancel_confirm();
        assert!(app.apply_held_reload());
        assert!(!app.new_data_waiting());
        assert_eq!(app.report.observed_at, 2000);
        assert!(app.marked.keys().all(|k| !k.ends_with("node_modules")));
        // Marks made earlier (Space) that the new index no longer lists are
        // dropped at once, and the result line says so.
        let mut app = App::new(fixture_report(), "/root".into());
        app.set_view(ViewKind::Deps);
        app.review_in_background(false, false);
        wait_operation(&mut app);
        assert!(!app.marked.is_empty());
        let mut newer = fixture_report();
        newer.observed_at = 2000;
        for wt in newer.projects.iter_mut().flat_map(|p| &mut p.worktrees) {
            wt.artifacts.clear();
        }
        app.land_observation(RefreshedObservation {
            per_root: Vec::new(),
            merged: Some(MergedReports {
                by_root: Default::default(),
                report: newer,
            }),
            external_units: None,
            agent_units: None,
            store_interiors: None,
        });
        assert!(app.marked.is_empty(), "{:?}", app.marked.keys());
        let msg = app.result_active().unwrap_or_default();
        assert!(msg.contains("no longer lists"), "{msg}");
    }

    #[test]
    fn r_during_a_check_says_so() {
        let mut app = App::new(fixture_report(), "/root".into());
        app.set_view(ViewKind::Deps);
        app.review_in_background(true, false);
        crate::handle_key(&mut app, crossterm::event::KeyCode::Char('R'));
        let s = paint(&app, 100, 30);
        assert!(
            s.contains("A check is running; press R after it finishes"),
            "{s}"
        );
        app.cancel_operation();
        wait_operation(&mut app);
    }
}

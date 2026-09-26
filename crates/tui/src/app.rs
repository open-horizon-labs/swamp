//! Application state and key handling. Pure aside from the action layer
//! calls the confirm step makes; the rest is unit-testable without a
//! terminal.

use crate::actions::{self, MarkedUnit};
use crate::filter::{self, Filter};
use crate::model::{self, Row, Sort};
use crate::units::UnitId;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use swamp_core::report::Report;

pub const REFUSAL_DISPLAY: Duration = Duration::from_secs(4);

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
    /// Agent-tool storage (#91/#92/#100): read-only, one row per
    /// `AgentUnit`. See `model::agent_rows`'s doc comment for why
    /// marking is not wired up in this chunk.
    Agents,
}

impl ViewKind {
    /// `'0'` is not a view digit here: it is already the global "clear
    /// filter" key (see `crate::handle_key_mod`), so `ViewKind::Agents`
    /// has no dedicated digit and is reached only by cycling with `v`
    /// (`ViewKind::next`) -- documented, not a silent omission.
    pub fn from_digit(d: char) -> Option<Self> {
        Some(match d {
            '1' => ViewKind::Projects,
            '2' => ViewKind::Tree,
            '3' => ViewKind::Builds,
            '4' => ViewKind::Deps,
            '5' => ViewKind::Docker,
            '6' => ViewKind::Kinds,
            '7' => ViewKind::Unowned,
            '8' => ViewKind::Types,
            '9' => ViewKind::External,
            _ => return None,
        })
    }
    pub fn next(self) -> Self {
        match self {
            ViewKind::Projects => ViewKind::Tree,
            ViewKind::Tree => ViewKind::Builds,
            ViewKind::Builds => ViewKind::Deps,
            ViewKind::Deps => ViewKind::Docker,
            ViewKind::Docker => ViewKind::Kinds,
            ViewKind::Kinds => ViewKind::Unowned,
            ViewKind::Unowned => ViewKind::Types,
            ViewKind::Types => ViewKind::External,
            ViewKind::External => ViewKind::Agents,
            ViewKind::Agents => ViewKind::Projects,
        }
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

pub struct App {
    /// Ephemeral on-demand details; never persisted in the observation store.
    pub cargo_inspection: Option<Vec<String>>,
    pub cargo_inspection_scroll: u16,
    pub operation: Option<Operation>,
    operation_rx: Option<std::sync::mpsc::Receiver<OperationEvent>>,
    review_progress: Option<std::sync::mpsc::Sender<OperationEvent>>,
    review_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    reviewed: usize,
    review_total: usize,
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
    pub help_open: bool,
    /// The filter picker form, when open.
    pub picker: Option<crate::picker::Picker>,
    /// Tab-completion candidates shown under the raw filter line.
    pub completions: Vec<String>,
    pub refusal: Option<(String, Instant)>,
    pub observing: Option<(u32, u32)>,
    pub last_result: Option<String>,
    pub observed_label: String,
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
    /// Seconds of observation history the store holds; bounds the growth
    /// windows a human may pick (the store cannot answer beyond it).
    pub history_secs: Option<u64>,
    /// git tracking status per absolute path, filled when a project is
    /// opened (one exclude stack per worktree, reused for its rows).
    pub track: std::collections::HashMap<PathBuf, swamp_core::ignore::TrackState>,
    /// Store dir, when known: the applied filter is persisted there so it
    /// survives relaunch (`ui_filter.txt`).
    pub store_dir: Option<PathBuf>,
    /// The live FSEvents streams covering every root in `roots` (#51):
    /// one `Watcher` per root, all feeding the single `watch_rx` below
    /// through cloned senders, so a change under *any* included root,
    /// including our own deletes, arrives here -- nothing "asks" for a
    /// refresh. Kept as a `Vec` (not one merged stream) because
    /// `fs_events::watch` is a per-path platform call; two roots on
    /// different volumes are two independent FSEvents streams no matter
    /// how this struct stores their handles.
    pub watches: Vec<swamp_core::fs_events::Watcher>,
    pub watch_rx: Option<std::sync::mpsc::Receiver<swamp_core::fs_events::WatchBatch>>,
    /// Changed directories received and not yet observed.
    pub live_changes: std::collections::HashSet<PathBuf>,
    pub live_last_event_id: u64,
    /// When the last batch arrived; observation starts once the stream has
    /// been quiet for `LIVE_QUIET`.
    pub live_last_batch: Option<Instant>,
    /// Roots whose next live refresh must walk fully, and why: the
    /// watch lost coverage (Linux: overflow, watch limit, permissions),
    /// or its epoch opened after the root's last observation, leaving a
    /// gap nothing covers. Empty on macOS, whose stream reports neither.
    pub live_full_walk:
        std::collections::HashMap<PathBuf, (swamp_core::fs_events::RefreshRefusal, String)>,
    /// Roots whose watch cannot vouch for anything any more (a watch
    /// limit, a permission gap): live refresh is off for them and the
    /// status line says why. The background refresh still covers them.
    pub live_off: std::collections::HashMap<PathBuf, String>,
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
    /// The authorized scope this TUI is showing. Every refresh --
    /// background, live watch, post-action re-observe -- goes through it,
    /// so exclusions and external pruning survive an update rather than
    /// applying only to the first render
    /// (`.oh/guardrails/tui-refresh-preserves-scope.md`).
    pub scope: Option<swamp_core::scope::EffectiveScope>,
}

pub struct Operation {
    pub label: &'static str,
    pub completed: usize,
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub current: PathBuf,
    pub started: Instant,
    pub cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

enum OperationEvent {
    Inspected(Vec<String>),
    Progress {
        completed: usize,
        total: usize,
        path: PathBuf,
        outcome: Option<bool>,
    },
    Reviewed {
        marked: BTreeMap<String, MarkedUnit>,
        refusal: Option<String>,
        confirm: bool,
        cancelled: bool,
    },
    Deleted {
        results: Vec<actions::UnitResult>,
        planned: u64,
        measured: Option<i64>,
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
            operation: None,
            operation_rx: None,
            review_progress: None,
            review_cancel: None,
            reviewed: 0,
            review_total: 0,
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
            help_open: false,
            picker: None,
            completions: Vec::new(),
            refusal: None,
            observing: None,
            last_result: None,
            observed_label: "just now".to_string(),
            actor: "human".to_string(),
            sort: Sort::None,
            reverse: false,
            keep_executables: false,
            quit: false,
            width: 0,
            store_dir: None,
            watches: Vec::new(),
            watch_rx: None,
            live_changes: std::collections::HashSet::new(),
            live_last_event_id: 0,
            live_last_batch: None,
            live_full_walk: std::collections::HashMap::new(),
            live_off: std::collections::HashMap::new(),
            track: std::collections::HashMap::new(),
            history_secs: None,
            external_units: Vec::new(),
            store_interiors: Vec::new(),
            agent_units: Vec::new(),
            scope_note: None,
            scope: None,
        }
    }

    /// Sets `external_units` for `ViewKind::External` (#43). Called once
    /// at startup, never from the event/render loop -- detector
    /// resolution and measurement are disk I/O.
    pub fn set_external_units(&mut self, units: Vec<swamp_core::external::ExternalUnit>) {
        self.external_units = units;
    }

    /// Sets the store interiors shown under `ViewKind::External`. Same
    /// contract as `set_external_units`, and always from the same pass.
    pub fn set_store_interiors(&mut self, units: Vec<swamp_core::artifact::NestedArtifact>) {
        self.store_interiors = units;
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
            ViewKind::External => model::external_rows_with(
                &self.external_units,
                &self.store_interiors,
                &self.collapsed,
                self.report.observed_at,
            ),
            ViewKind::Agents => model::agent_rows(&self.agent_units),
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
    pub fn toggle_keep_executables(&mut self) {
        self.keep_executables = !self.keep_executables;
        self.persist_ui_state();
    }

    /// Whether the active filter is one that has no data source yet.
    /// Kept for the UI: every predicate now has a data source, so a
    /// filter never lands in a "no data yet" state.
    pub fn filter_has_no_data(&self) -> bool {
        false
    }

    pub fn set_view(&mut self, v: ViewKind) {
        self.view = v;
        self.selected = 0;
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
    }

    pub fn clear_filter(&mut self) {
        self.filter_text = "0".to_string();
        self.filter = Filter::default();
        self.filter_error = None;
        self.selected = 0;
        self.persist_filter();
    }

    fn persist_filter(&self) {
        self.persist_ui_state();
    }

    fn persist_ui_state(&self) {
        if let Some(store) = &self.store_dir
            && let Ok(store) = swamp_core::fs_gate::StoreDir::at(store)
        {
            let state = UiState {
                filter: self.filter_text.clone(),
                sort: sort_to_str(self.sort).to_string(),
                reverse: self.reverse,
                keep_executables: self.keep_executables,
            };
            let _ = swamp_core::fs_gate::store::write_json(
                swamp_core::fs_gate::store::JsonFile::UiState { store: &store },
                &state,
            );
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
            ViewKind::Tree | ViewKind::External | ViewKind::Docker
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
            self.confirm_delete();
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
        let mut refused: Option<String> = None;
        let mut marked = 0usize;
        // Agents view (#91/#100/#101): `model::agent_rows` sets `unit`
        // on every row, protected/unsupported ones included, but never
        // sets `kind` (there is no `ArtifactKind` for an agent-storage
        // unit). Counted separately so the footer can say how many were
        // skipped and why, rather than folding it into the single
        // static per-kind refusal strings below.
        let mut agent_skipped = 0usize;
        for row in rows {
            let Some(kind) = row.kind.clone() else {
                // Projects view: each row stands for a whole project.
                if let Some(project) = row.project.clone() {
                    // Bulk marking never reaches for a checkout: `A` over
                    // a screen of projects would otherwise queue every
                    // checkout under the root behind one Enter.
                    let n = self.mark_project(&project, false);
                    if n == 0 {
                        refused = refused.or(Some("nothing reclaimable in this project".into()));
                    }
                    marked += n;
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
                Err(why) => refused = refused.or(Some(why.into())),
            }
        }
        if agent_skipped > 0 {
            refused = refused.or(Some(format!(
                "{agent_skipped} agent-storage row{} protected, unsupported, or active; skipped",
                if agent_skipped == 1 { " is" } else { "s are" }
            )));
        }
        if marked == 0 {
            self.set_refusal(
                refused
                    .as_deref()
                    .unwrap_or("nothing in this view can be acted on"),
            );
            return;
        }
        // Some rows were left alone (agent-storage skip, or an empty
        // project) even though at least one row *was* marked: say so,
        // rather than silently proceeding to a confirm that looks like
        // it covers everything the human saw on screen.
        if let Some(msg) = refused {
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
    fn mark_project(&mut self, project: &str, include_checkouts: bool) -> usize {
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
        if units.is_empty() {
            return 0;
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
            return 0;
        }
        let mut newly = 0usize;
        for r in units {
            if marked_already(self, &r) {
                continue;
            }
            self.mark_row(&r);
            newly += 1;
        }
        newly
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
                self.set_refusal("No supported cleanup members remain; refresh the report.");
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
        let Some(unit_id) = row.unit.clone() else {
            if row.signals.iter().any(|s| s == "category") {
                self.set_refusal(
                    "Category total: select an unchecked child group. Nothing changed.",
                );
                return;
            }
            if row.signals.iter().any(|s| s == "blocked") {
                self.set_refusal(
                    "Inspection-only: this output cannot be selected for cleanup. Nothing changed.",
                );
                return;
            }
            // A projects-view row is a whole project rather than one
            // path. Marking it means marking what that project can give
            // back, so the human does not have to open it first.
            if let Some(project) = row.project.clone() {
                if self.mark_project(&project, true) == 0 {
                    self.set_refusal("nothing reclaimable in this project");
                }
                return;
            }
            self.set_refusal("nothing to delete on this row");
            return;
        };
        if let Some(tx) = &self.review_progress {
            let _ = tx.send(OperationEvent::Progress {
                completed: self.reviewed,
                total: self.review_total,
                path: PathBuf::from(&unit_id.0),
                outcome: None,
            });
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
            self.set_refusal(why);
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
                        self.set_refusal(&format!(
                            "human-protected path (swamp protect): {reason}; remove protection \
                             first if this unit should be actionable"
                        ));
                        return;
                    }
                }
                Err(e) => {
                    self.set_refusal(&format!(
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
                    self.set_refusal(&e.to_string());
                    return;
                }
            }
        } else {
            None
        };
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
                    self.set_refusal(&e.to_string());
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
        self.reviewed += 1;
        if let Some(tx) = &self.review_progress {
            let _ = tx.send(OperationEvent::Progress {
                completed: self.reviewed,
                total: self.review_total,
                path: PathBuf::from(&unit_id.0),
                outcome: Some(true),
            });
        }
    }

    pub fn cancel_operation(&mut self) {
        if let Some(op) = &self.operation {
            op.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
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
            current: profile.clone(),
            started: Instant::now(),
            cancel: cancel.clone(),
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
        if confirm && !self.marked.is_empty() {
            self.open_confirm();
            return;
        }
        let row = self.selected_row();
        if !all && row.is_none() {
            return;
        }
        let total = if all {
            0
        } else {
            row.as_ref()
                .and_then(|r| r.expansion_key.as_deref())
                .map(|k| model::cleanup_members(&self.report, k).len())
                .unwrap_or(0)
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut worker = App::new(self.report.clone(), self.root.clone());
        worker.marked = self.marked.clone();
        worker.view = self.view;
        worker.filter = self.filter.clone();
        worker.selected_project = self.selected_project.clone();
        worker.collapsed = self.collapsed.clone();
        worker.track = self.track.clone();
        worker.review_total = total;
        worker.review_cancel = Some(cancel.clone());
        worker.review_progress = Some(tx.clone());
        self.operation = Some(Operation {
            label: "Reviewing",
            completed: 0,
            total,
            succeeded: 0,
            failed: 0,
            current: PathBuf::new(),
            started: Instant::now(),
            cancel: cancel.clone(),
        });
        self.operation_rx = Some(rx);
        self.refusal = None;
        self.last_result = None;
        crate::worker::spawn(move || {
            if all {
                worker.mark_all_in_view();
            } else if let Some(row) = row {
                worker.mark_row(&row);
            }
            let _ = tx.send(OperationEvent::Reviewed {
                marked: worker.marked,
                refusal: worker.refusal.map(|(msg, _)| msg),
                confirm: confirm || all,
                cancelled: cancel.load(std::sync::atomic::Ordering::SeqCst),
            });
        });
    }

    pub fn poll_operation(&mut self) {
        loop {
            let event = match self.operation_rx.as_ref().map(|rx| rx.try_recv()) {
                Some(Ok(e)) => e,
                Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => OperationEvent::Failed(
                    "Worker stopped unexpectedly; check the ledger before retrying cleanup".into(),
                ),
                _ => break,
            };
            match event {
                OperationEvent::Inspected(lines) => {
                    self.operation = None;
                    self.operation_rx = None;
                    self.cargo_inspection = Some(lines);
                    self.cargo_inspection_scroll = 0;
                    break;
                }
                OperationEvent::Progress {
                    completed,
                    total,
                    path,
                    outcome,
                } => {
                    if let Some(op) = &mut self.operation {
                        op.completed = completed;
                        op.total = total;
                        op.current = path;
                        if outcome == Some(true) {
                            op.succeeded += 1;
                        }
                        if outcome == Some(false) {
                            op.failed += 1;
                        }
                    }
                }
                OperationEvent::Reviewed {
                    marked,
                    refusal,
                    confirm,
                    cancelled,
                } => {
                    let cancelled = cancelled
                        || self
                            .operation
                            .as_ref()
                            .is_some_and(|op| op.cancel.load(std::sync::atomic::Ordering::SeqCst));
                    if !cancelled {
                        self.marked = marked;
                        self.refusal = refusal.map(|msg| (msg, Instant::now()));
                        self.confirm_open = confirm && !self.marked.is_empty();
                    } else {
                        self.last_result = Some(
                            "Review cancelled; previous selection preserved; nothing deleted"
                                .into(),
                        );
                    }
                    self.operation = None;
                    self.operation_rx = None;
                    break;
                }
                OperationEvent::Deleted {
                    results,
                    planned,
                    measured,
                    total,
                } => {
                    self.operation = None;
                    self.operation_rx = None;
                    self.finish_delete(results, planned, measured, total);
                    break;
                }
                OperationEvent::Failed(msg) => {
                    self.operation = None;
                    self.operation_rx = None;
                    self.confirm_open = false;
                    self.set_refusal(&msg);
                    break;
                }
            }
        }
    }

    fn set_refusal(&mut self, msg: &str) {
        self.refusal = Some((format!("refused: {msg}"), Instant::now()));
    }

    pub fn refusal_active(&self) -> Option<&str> {
        self.refusal.as_ref().and_then(|(msg, at)| {
            if at.elapsed() < REFUSAL_DISPLAY {
                Some(msg.as_str())
            } else {
                None
            }
        })
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
            self.mark_row(&row);
        }
        self.open_confirm();
    }

    pub fn cancel_confirm(&mut self) {
        self.confirm_open = false;
    }

    pub fn confirm_summary(&self) -> String {
        let units: Vec<MarkedUnit> = self.marked.values().cloned().collect();
        actions::confirm_summary(&units, self.keep_executables)
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
            current: PathBuf::new(),
            started: Instant::now(),
            cancel: cancel.clone(),
        });
        self.operation_rx = Some(rx);
        self.confirm_open = false;
        self.last_result = None;
        self.refusal = None;
        crate::worker::spawn(move || {
            let ledger = swamp_core::ledger::Ledger::resolved(&store);
            let free_before = actions::free_space_bytes(&trash);
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
            let measured = match (free_before, actions::free_space_bytes(&trash)) {
                (Some(b), Some(a)) => Some(a as i64 - b as i64),
                _ => None,
            };
            let _ = tx.send(OperationEvent::Deleted {
                results,
                planned,
                measured,
                total,
            });
        });
    }

    fn finish_delete(
        &mut self,
        results: Vec<actions::UnitResult>,
        planned: u64,
        measured: Option<i64>,
        total: usize,
    ) {
        let ok = results.iter().filter(|r| r.outcome.is_ok()).count();
        let failed: Vec<String> = results
            .iter()
            .filter_map(|r| {
                r.outcome
                    .as_ref()
                    .err()
                    .map(|e| format!("{}: {e}", r.path.display()))
            })
            .collect();
        // Retain refused and unprocessed selections for explicit review/retry.
        for r in &results {
            if r.outcome.is_ok() {
                self.marked.remove(&r.path.display().to_string());
            }
        }
        self.confirm_open = false;
        let measured_txt = measured
            .map(model::human_signed_bytes)
            .unwrap_or_else(|| "unmeasured".into());
        self.last_result = Some(if results.len() < total {
            format!(
                "Cancelled · {ok} completed · {} refused · {} not attempted; completed filesystem moves are in Trash",
                failed.len(),
                total - results.len()
            )
        } else if failed.is_empty() {
            format!(
                "{ok} deleted · planned {} · measured {measured_txt}",
                model::human_bytes(planned)
            )
        } else {
            format!(
                "{ok} deleted, {} refused · planned {} · measured {measured_txt}",
                failed.len(),
                model::human_bytes(planned)
            )
        });
        // What just left the disk leaves the screen now; the store and the
        // header follow from a background incremental observe (FSEvents
        // narrows it to the touched trees), the same path startup uses.
        // The screen is right now; the store follows through the live
        // FSEvents stream, which sees the move to Trash like any change.
        self.prune_removed(&results);
        if self.watches.is_empty() {
            self.observe_in_background();
        }
    }

    /// Drops every row under a successfully removed path from the
    /// in-memory report -- artifacts, whole worktrees, Source directory
    /// rollups -- and takes their bytes off the header totals, so the
    /// screen is right before the re-observe lands.
    pub fn prune_removed(&mut self, results: &[actions::UnitResult]) {
        let removed: Vec<PathBuf> = results
            .iter()
            .filter(|r| r.outcome.is_ok())
            .map(|r| r.path.clone())
            .collect();
        if removed.is_empty() {
            return;
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

    /// How long the stream must be quiet before its changes are observed.
    /// FSEvents already coalesces at 0.5 s; this only lets a burst (a
    /// build writing thousands of files) land as one observation.
    pub const LIVE_QUIET: Duration = Duration::from_millis(400);

    /// Starts one live FSEvents stream per root in `self.roots` (#51),
    /// all feeding the same `watch_rx` through cloned senders. A root
    /// whose stream fails to start (no FSEvents on this platform, or the
    /// path itself is gone) simply contributes no watcher -- the others
    /// still run; this is never fatal to the TUI, only to that root's
    /// live updates (it still gets refreshed by the scheduled/cached
    /// path). No-op if watches are already running.
    pub fn start_watch(&mut self) {
        self.start_watch_with(swamp_core::fs_events::watch_pending);
    }

    /// [`Self::start_watch`] with the stream factory supplied.
    ///
    /// Every root's thread is spawned **before** any readiness is
    /// collected. `FSEventStreamStart` is a synchronous, per-process
    /// serialized request to `fseventsd` that costs seconds (measured on
    /// two fresh temp directories: 1.4 s and 2.9 s on a quiet machine,
    /// 4.7 s and 6.6 s on a loaded one), so waiting for each root's
    /// stream before spawning the next one made the later roots pay the
    /// sum of those latencies against a fixed per-stream budget. That is
    /// how this ended up holding one watcher for two roots while both
    /// streams had in fact started: the second one reported ready 1.6 s
    /// after the caller had already given up on it. Spawning first
    /// bounds the wait by the slowest stream rather than by their total,
    /// and `fs_events::watch_start_budget` is now larger than the cost
    /// of the call it is bounding.
    pub(crate) fn start_watch_with(&mut self, factory: swamp_core::fs_events::WatchFactory) {
        if self.store_dir.is_none() || !self.watches.is_empty() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let pending: Vec<_> = self
            .roots
            .clone()
            .iter()
            .filter_map(|root| factory(root, tx.clone()))
            .collect();
        let budget = swamp_core::fs_events::watch_start_budget();
        for p in pending {
            if let Some(w) = p.ready(budget) {
                self.watches.push(w);
            }
        }
        if !self.watches.is_empty() {
            self.watch_rx = Some(rx);
        }
    }

    /// The root in `self.roots` that owns `path` (the longest matching
    /// prefix, so a nested root -- were one ever present -- would not be
    /// shadowed by a shorter ancestor). `None` for a path outside every
    /// known root, which a stream should not be able to report but is
    /// handled as "ignore this change" rather than a panic if it ever
    /// does (a root removed from scope between watch-start and now, for
    /// instance).
    fn root_for_path(&self, path: &std::path::Path) -> Option<PathBuf> {
        self.roots
            .iter()
            .filter(|r| path.starts_with(r))
            .max_by_key(|r| r.as_os_str().len())
            .cloned()
    }

    /// Consumes every batch the stream has delivered so far.
    pub fn drain_watch(&mut self) {
        let Some(rx) = &self.watch_rx else {
            return;
        };
        let mut batches = Vec::new();
        while let Ok(batch) = rx.try_recv() {
            batches.push(batch);
        }
        for batch in batches {
            self.apply_watch_batch(batch);
        }
    }

    /// One batch's effect. Changes accumulate for the next quiet-window
    /// refresh; a coverage loss or an epoch newer than the root's last
    /// observation turns that refresh into a full walk with the reason
    /// named, never an incremental one over an incomplete change list.
    fn apply_watch_batch(&mut self, batch: swamp_core::fs_events::WatchBatch) {
        use swamp_core::fs_events::RefreshRefusal;
        self.live_changes.extend(batch.changed_dirs);
        self.live_last_event_id = self.live_last_event_id.max(batch.last_event_id);
        self.live_last_batch = Some(Instant::now());
        if let Some((reason, detail)) = batch.coverage_lost {
            let unrecoverable = matches!(
                reason,
                RefreshRefusal::WatchLimitReached | RefreshRefusal::WatchPermissionGap
            );
            if unrecoverable {
                self.status = Some(format!(
                    "live refresh off for {}: {detail} ({})",
                    batch.root.display(),
                    reason.as_str()
                ));
                self.live_off.insert(batch.root.clone(), detail.clone());
            }
            self.live_full_walk
                .insert(batch.root.clone(), (reason, detail));
        }
        if let Some(opened_at) = batch.epoch_opened_at {
            let observed = self
                .reports_by_root
                .get(&batch.root)
                .map(|r| r.observed_at)
                .or_else(|| (self.roots.len() == 1).then_some(self.report.observed_at));
            if observed.is_none_or(|t| t < opened_at) {
                self.live_full_walk.entry(batch.root.clone()).or_insert((
                    RefreshRefusal::LiveWatchGap,
                    "the watch opened after this root's last observation".into(),
                ));
            }
        }
    }

    /// Changes are waiting, no observation is running, and the stream has
    /// been quiet long enough.
    pub fn live_observe_due(&self) -> bool {
        self.pending.is_none()
            && (!self.live_changes.is_empty() || !self.live_full_walk.is_empty())
            && self
                .live_last_batch
                .is_some_and(|t| t.elapsed() >= Self::LIVE_QUIET)
    }

    /// Observes exactly the directories the stream reported, on a worker
    /// thread, through the same pipeline as everything else: the plan is
    /// the live batch, so the store re-walks those subtrees and carries
    /// every other row forward.
    ///
    /// A live batch can name changes under more than one root (two
    /// watchers can both go quiet in the same tick); this call handles
    /// exactly *one* root per invocation -- the first, in `self.roots`
    /// order, that has any pending change -- draining only that root's
    /// changed paths from `live_changes` and leaving any other root's
    /// changes in place. `live_observe_due` stays true afterward as long
    /// as changes remain, so `event_loop` simply calls this again on its
    /// next tick to pick up the next root; no root's changes are ever
    /// silently dropped, and no two roots are ever re-walked by the same
    /// worker thread (keeping the existing single-root incremental path
    /// untouched per root).
    pub fn observe_live(&mut self) {
        let Some(store) = self.store_dir.clone() else {
            return;
        };
        if self.pending.is_some()
            || (self.live_changes.is_empty() && self.live_full_walk.is_empty())
        {
            return;
        }
        // A root owed a full walk goes first: its changes, if any, are
        // covered by that walk.
        let forced = self
            .roots
            .iter()
            .find(|r| self.live_full_walk.contains_key(*r))
            .cloned();
        let Some(root) =
            forced.or_else(|| self.live_changes.iter().find_map(|p| self.root_for_path(p)))
        else {
            // Every pending change is outside every known root (a root
            // was removed from scope since the watcher was started);
            // drop them rather than looping forever on changes nothing
            // will ever claim.
            self.live_changes.clear();
            return;
        };
        let (mine, rest): (HashSet<PathBuf>, HashSet<PathBuf>) = self
            .live_changes
            .drain()
            .partition(|p| p.starts_with(&root));
        self.live_changes = rest;
        let changed: Vec<PathBuf> = mine.into_iter().collect();
        let device = swamp_core::fs_gate::device_of(&root);
        let plan = swamp_core::fs_events::FsEventsPlan::from_live(
            changed,
            self.live_last_event_id,
            device,
        );
        // A live refresh re-walks one root -- through that root's own
        // slice of the authorized scope, so its exclusions and external
        // prune notes still apply. Without a scope there is nothing
        // authorized to observe, and refusing is the honest answer.
        let Some(scope) = self.scope.as_ref().map(|s| s.restricted_to(&root)) else {
            self.status = Some(
                "live refresh skipped: no resolved scope for this session; reopen swamp ui".into(),
            );
            return;
        };
        let (roots, cache) = (self.roots.clone(), self.reports_by_root.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        crate::worker::spawn(move || {
            // The watcher's own changes, as a replay plan: a production
            // source (`fs_events::LivePlanSource`), not a test double.
            let source = swamp_core::fs_events::LivePlanSource::new(plan);
            // `ObservationParts::WALK_ONLY`, and `None` for both unit
            // vectors.
            //
            // Narrowing the scope to the one root a watcher fired under
            // is the right fix for the *scope* half of the earlier
            // finding. It is the wrong thing to derive unit vectors
            // from: every tool home outside that root is unauthorized
            // in the narrowed scope, so `ObservationParts::ALL` came
            // back with `agent_units: Some(vec![])`, the event loop
            // applied it unconditionally, and the agent view emptied on
            // the first file save anywhere in the project (the
            // 2026-09-22 re-review's CE3).
            //
            // A part not asked for is also a part not *swept*
            // (`growth::ObservationOwnership`), so asking for fewer
            // parts here cannot invent a disappearance either. The
            // background refresh, which covers the whole scope, is what
            // refreshes those vectors.
            let res = swamp_core::report::observe_scope(
                &scope,
                swamp_core::report::ObservationParts::WALK_ONLY,
                None,
                None,
                false,
                Some(&store),
                None,
                true,
                true,
                false,
                false,
                &source,
                30,
                24 * 3600,
            )
            .map(|o| {
                RefreshedObservation::merged_on_worker(
                    &roots,
                    cache,
                    o.per_root.into_iter().collect(),
                    None,
                    None,
                    None,
                )
            });
            let _ = tx.send(res);
        });
        self.pending = Some(rx);
        self.observing = Some((0, 0));
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
    }

    pub fn toggle_help(&mut self) {
        self.help_open = !self.help_open;
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
        assert!(app.last_result.as_ref().unwrap().contains("cancelled"));
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
            None,
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
        assert!(app.last_result.as_ref().unwrap().contains("1 deleted"));
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
        assert!(app.last_result.as_ref().unwrap().contains("1 deleted"));
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

    /// A watch that lost coverage (Linux: an inotify queue overflow)
    /// never lets its partial change list drive an incremental refresh:
    /// the next live refresh of that root is a full walk naming the loss.
    #[test]
    fn a_coverage_loss_turns_the_next_live_refresh_into_a_named_full_walk() {
        use swamp_core::fs_events::RefreshRefusal;
        let mut app = App::new(fixture_report(), "/root".into());
        app.store_dir = Some(std::env::temp_dir());
        let (tx, rx) = std::sync::mpsc::channel();
        app.watch_rx = Some(rx);
        tx.send(swamp_core::fs_events::WatchBatch {
            root: PathBuf::from("/root"),
            changed_dirs: vec![PathBuf::from("/root/mole")],
            last_event_id: 3,
            coverage_lost: Some((RefreshRefusal::WatchQueueOverflow, "overflowed".into())),
            epoch_opened_at: None,
        })
        .unwrap();
        app.drain_watch();
        assert_eq!(
            app.live_full_walk
                .get(&PathBuf::from("/root"))
                .map(|(r, _)| *r),
            Some(RefreshRefusal::WatchQueueOverflow)
        );
        app.live_last_batch = Some(Instant::now() - Duration::from_secs(1));
        assert!(app.live_observe_due());
        assert!(
            app.live_off.is_empty(),
            "an overflow is recoverable: live refresh stays on"
        );

        // A watch limit is not recoverable: live refresh goes off for the
        // root, and the status line says why.
        tx.send(swamp_core::fs_events::WatchBatch {
            root: PathBuf::from("/root"),
            coverage_lost: Some((RefreshRefusal::WatchLimitReached, "no watches left".into())),
            ..Default::default()
        })
        .unwrap();
        app.drain_watch();
        assert!(app.live_off.contains_key(&PathBuf::from("/root")));
        assert!(
            app.status
                .as_deref()
                .unwrap_or("")
                .contains("watch_limit_reached")
        );
    }

    /// A watch whose epoch opened after the root's last observation
    /// cannot vouch for the gap in between: one full walk first.
    #[test]
    fn an_epoch_newer_than_the_last_observation_owes_a_full_walk() {
        use swamp_core::fs_events::RefreshRefusal;
        let mut report = fixture_report();
        report.observed_at = 1_000;
        let mut app = App::new(report, "/root".into());
        app.store_dir = Some(std::env::temp_dir());
        app.apply_watch_batch(swamp_core::fs_events::WatchBatch {
            root: PathBuf::from("/root"),
            epoch_opened_at: Some(2_000),
            ..Default::default()
        });
        assert_eq!(
            app.live_full_walk
                .get(&PathBuf::from("/root"))
                .map(|(r, _)| *r),
            Some(RefreshRefusal::LiveWatchGap)
        );

        let mut report = fixture_report();
        report.observed_at = 3_000;
        let mut app = App::new(report, "/root".into());
        app.apply_watch_batch(swamp_core::fs_events::WatchBatch {
            root: PathBuf::from("/root"),
            epoch_opened_at: Some(2_000),
            ..Default::default()
        });
        assert!(
            app.live_full_walk.is_empty(),
            "an observation inside the epoch is covered"
        );
    }

    #[test]
    fn live_changes_are_observed_after_the_stream_goes_quiet() {
        let mut app = App::new(fixture_report(), "/root".into());
        app.store_dir = Some(std::env::temp_dir());
        let (tx, rx) = std::sync::mpsc::channel();
        app.watch_rx = Some(rx);
        tx.send(swamp_core::fs_events::WatchBatch {
            root: PathBuf::from("/root"),
            changed_dirs: vec![PathBuf::from("/root/mole/node_modules")],
            last_event_id: 42,
            ..Default::default()
        })
        .unwrap();
        app.drain_watch();
        assert_eq!(app.live_changes.len(), 1);
        assert_eq!(app.live_last_event_id, 42);
        assert!(!app.live_observe_due(), "not before the quiet window");
        app.live_last_batch = Some(Instant::now() - Duration::from_secs(1));
        assert!(app.live_observe_due());
        // While an observation runs, more changes just accumulate.
        let (_ptx, prx) = std::sync::mpsc::channel();
        app.pending = Some(prx);
        assert!(!app.live_observe_due());
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

    #[test]
    fn backspace_marks_the_current_row_and_asks_once() {
        let mut app = App::new(fixture_report(), "/root".into());
        app.set_view(ViewKind::Tree);
        app.selected = 1; // node_modules
        app.delete_here();
        assert_eq!(app.marked.len(), 1);
        assert!(app.confirm_open, "one 'are you sure', with the facts on it");
        assert!(app.confirm_summary().contains("node_modules"));
        assert!(app.confirm_summary().contains("Enter yes"));
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
        assert!(app.confirm_summary().contains("delete node_modules"));
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
    fn view_cycles_and_digit_keys() {
        assert_eq!(ViewKind::Projects.next(), ViewKind::Tree);
        assert_eq!(ViewKind::from_digit('3'), Some(ViewKind::Builds));
        assert_eq!(ViewKind::from_digit('6'), Some(ViewKind::Kinds));
        assert_eq!(ViewKind::from_digit('8'), Some(ViewKind::Types));
        assert_eq!(ViewKind::from_digit('9'), Some(ViewKind::External));
        // '0' is reserved for "clear filter" (crate::handle_key_mod);
        // Agents has no dedicated digit and must not silently claim '0'.
        assert_eq!(ViewKind::from_digit('0'), None);
        // Full cycle returns to Projects, matching the CLI's view order:
        // worktrees(Projects)/tree/builds/deps/docker/kinds/unowned/
        // types/external/agents.
        let mut v = ViewKind::Projects;
        for _ in 0..10 {
            v = v.next();
        }
        assert_eq!(v, ViewKind::Projects);
        // Agents is reachable by cycling even without its own digit.
        let mut seen = std::collections::HashSet::new();
        let mut v = ViewKind::Projects;
        for _ in 0..10 {
            seen.insert(v);
            v = v.next();
        }
        assert!(seen.contains(&ViewKind::Agents));
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
    fn a_store_interior_opens_under_its_external_row_and_stays_inspection_only() {
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
        for r in open.iter().skip(1) {
            assert!(
                r.unit.is_none(),
                "{}: an interior row is never selectable",
                r.label
            );
            assert!(r.signals.iter().any(|s| s == "blocked"), "{}", r.label);
        }
        for width in [80u16, 160] {
            let backend = ratatui::backend::TestBackend::new(width, 24);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            terminal.draw(|f| crate::ui::draw(f, &app)).unwrap();
        }
    }

    /// `start_watch` opens one FSEvents stream per root in `self.roots`
    /// (#51), not just the primary one -- the concrete "multiple
    /// watchers" acceptance case.
    ///
    /// Driven through the injected stream factory rather than real
    /// FSEvents. The previous version opened two real streams and could
    /// only skip itself when it got *zero*; it got one often enough to
    /// be recorded as a flake, and the cause was not the assertion but
    /// `start_watch` itself (`fs_events::PendingWatch` documents the
    /// measurement). With the factory the assertion is about the loop --
    /// every root gets its own stream, none is shared, none is dropped
    /// -- and nothing in it depends on how fast `fseventsd` answers, so
    /// there is no sleep and no bound to lose a race against.
    #[test]
    fn start_watch_opens_one_stream_per_root() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let report = minimal_report(
            dir_a.path().to_str().unwrap(),
            "proj-a",
            dir_a.path().join("proj-a").to_str().unwrap(),
        );
        let mut app = App::new_multi_root(
            report,
            vec![dir_a.path().to_path_buf(), dir_b.path().to_path_buf()],
        );
        app.store_dir = Some(store.path().to_path_buf());
        app.start_watch_with(swamp_core::fs_events::testing::inert_watch_factory);
        assert_eq!(
            app.watches.len(),
            2,
            "one watcher per root, not one shared watcher for the whole App"
        );
        assert!(app.watch_rx.is_some());
        // Already running: a second call must not double the streams.
        app.start_watch_with(swamp_core::fs_events::testing::inert_watch_factory);
        assert_eq!(app.watches.len(), 2, "start_watch is idempotent");
    }

    /// A root whose stream never reports ready contributes no watcher,
    /// and the roots whose streams did start still do -- the partial
    /// case `start_watch`'s own doc comment promises. Asserted through a
    /// factory that fails for exactly one root, so "the other roots
    /// still run" is a property of the loop rather than of which stream
    /// `fseventsd` happened to be slow about.
    #[test]
    fn a_root_whose_stream_fails_does_not_stop_the_others() {
        fn only_the_first_starts(
            root: &std::path::Path,
            tx: std::sync::mpsc::Sender<swamp_core::fs_events::WatchBatch>,
        ) -> Option<swamp_core::fs_events::PendingWatch> {
            if root.to_string_lossy().contains("swamp-watch-refuses") {
                return None;
            }
            swamp_core::fs_events::testing::inert_watch_factory(root, tx)
        }
        let store = tempfile::tempdir().unwrap();
        let report = minimal_report("/roots/a", "proj-a", "/roots/a/proj-a");
        let mut app = App::new_multi_root(
            report,
            vec![
                PathBuf::from("/roots/a"),
                PathBuf::from("/roots/swamp-watch-refuses"),
                PathBuf::from("/roots/c"),
            ],
        );
        app.store_dir = Some(store.path().to_path_buf());
        app.start_watch_with(only_the_first_starts);
        assert_eq!(app.watches.len(), 2, "two of three roots opened a stream");
        assert!(app.watch_rx.is_some());
    }

    /// A live batch naming changes under two different roots is handled
    /// one root at a time: `observe_live` drains only the changed paths
    /// under the root it picks, leaving the other root's changes intact
    /// for the next call -- never silently dropped, never merged into
    /// the wrong root's re-walk.
    #[test]
    fn observe_live_handles_one_roots_changes_at_a_time() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let report = minimal_report(
            dir_a.path().to_str().unwrap(),
            "proj-a",
            dir_a.path().join("proj-a").to_str().unwrap(),
        );
        let mut app = App::new_multi_root(
            report,
            vec![dir_a.path().to_path_buf(), dir_b.path().to_path_buf()],
        );
        app.store_dir = Some(store.path().to_path_buf());
        // A refresh without an authorized scope is refused by design
        // (`.oh/guardrails/tui-refresh-preserves-scope.md`), so the
        // fixture supplies the one this App is showing -- exactly what
        // `tui::run`/`run_scope` do at startup.
        app.scope = Some(swamp_core::scope::resolve_effective_scope(
            &swamp_core::locations::Environment::fixture(
                dir_a.path().to_path_buf(),
                std::collections::HashMap::new(),
                swamp_core::locations::Platform::MacOS,
            ),
            &swamp_core::scope::ScanConfig::default(),
            &[dir_a.path().to_path_buf(), dir_b.path().to_path_buf()],
            &swamp_core::locations::Registry::with_builtins(),
            1_000,
        ));
        app.live_changes.insert(dir_a.path().join("changed-a"));
        app.live_changes.insert(dir_b.path().join("changed-b"));
        app.live_last_batch = Some(Instant::now() - App::LIVE_QUIET - Duration::from_millis(10));
        assert!(app.live_observe_due());
        app.observe_live();
        assert!(app.pending.is_some(), "one root's re-walk was started");
        // Exactly one root's change was drained; the other is still
        // pending for a subsequent call.
        assert_eq!(app.live_changes.len(), 1, "{:?}", app.live_changes);
    }
}

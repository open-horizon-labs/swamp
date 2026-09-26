#![cfg_attr(
    not(test),
    deny(clippy::disallowed_methods, clippy::disallowed_types, unsafe_code)
)]

mod collect;
mod output;
mod schedule;

#[macro_export]
macro_rules! safe_print {
    ($($arg:tt)*) => {{
        $crate::output::write_stdout(format_args!($($arg)*))?;
    }};
}

#[macro_export]
macro_rules! safe_println {
    ($($arg:tt)*) => {{
        $crate::output::write_stdout_line(format_args!($($arg)*))?;
    }};
}

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use swamp_core::{
    filter,
    render::{
        OverviewSort, render_kinds, render_overview_sorted, render_project_tree_with_agents,
        render_types, render_view_builds, render_view_deps, render_view_docker,
        render_view_reconciliation, render_view_unowned, render_worktree_signals, render_worktrees,
    },
    report::Report,
    scan::{ScanOptions, observation},
    store::Store,
};

/// `--view` at root or with `--project`. `--kinds`/`--docker` remain as
/// aliases for `--view kinds`/`--view docker` (#33). `Worktrees` at root
/// is #35's git-enriched one-line-per-worktree listing
/// (`render_worktrees`); with `--project` it is the tree drill (#33),
/// also the default when `--project` is given with no `--view`.
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum View {
    Worktrees,
    Builds,
    Deps,
    Docker,
    Kinds,
    Unowned,
    Reconciliation,
    /// Per-ecosystem rollup: projects wearing the tag, artifacts it
    /// generates, bytes and growth.
    Types,
    /// Nested Cargo target/build units with physical-accounting and
    /// evidence/unknown details. Inspection only.
    Rust,
    /// Ranked list of every discovered project: name, id, total bytes,
    /// growth, checkout+worktree count, remote. JSON only.
    Projects,
    /// Rows with growth > 0 since the window, sorted desc, plus an
    /// unowned-bytes summary and coverage (walked/du/unowned totals,
    /// permission-denied count, history span). JSON only.
    Grown,
    /// External/shared storage units (#43): Cargo registry, rustup
    /// toolchains, Homebrew, and other detector-resolved locations with
    /// no containing project. Identity, category, size/growth/regrowth
    /// history and declared consumers. Read-only: this view never
    /// removes anything, in this command or any other.
    External,
    /// Agent-tool storage (#91-#99/#100): sessions, caches, logs,
    /// checkpoints and protected config for every named coding-agent
    /// tool (Claude Code, Codex, Oh My Pi, OpenCode, Gemini CLI, Pi,
    /// Aider, GitHub Copilot CLI, Cursor, Windsurf, Cline, Roo Code,
    /// Continue), grouped tool → category → unit with size/growth/age
    /// and project linkage. `--project` filters to units linked to that
    /// project. Redaction-aware by construction (this view never has
    /// session content to print). Read-only from this command; removal
    /// is TUI-only (Space/Backspace/Enter) -- see `swamp protect` for
    /// the human-keep-intent surface this view respects.
    Agents,
}

impl View {
    /// The name this view is addressed by in `--view` and echoed back in
    /// `report --json`'s `"view"` field -- derived from clap's own
    /// kebab-case rendering of the variant so the flag value and the
    /// JSON contract never drift apart.
    fn name(self) -> String {
        use clap::ValueEnum;
        self.to_possible_value()
            .map(|v| v.get_name().to_string())
            .unwrap_or_else(|| "worktrees".to_string())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum SortArg {
    Growth,
    Size,
    Name,
    Type,
    Age,
}

impl From<SortArg> for OverviewSort {
    fn from(s: SortArg) -> Self {
        match s {
            SortArg::Growth => OverviewSort::Growth,
            SortArg::Size => OverviewSort::Size,
            SortArg::Name => OverviewSort::Name,
            SortArg::Type => OverviewSort::Type,
            SortArg::Age => OverviewSort::Age,
        }
    }
}

#[derive(Parser)]
#[command(name = "swamp", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum ConfigAction {
    Show,
    Path,
    Init,
}

#[derive(Subcommand)]
enum Command {
    /// On-demand, bounded inspection of an existing Cargo profile. Does not run Cargo.
    InspectCargo {
        profile: PathBuf,
        #[arg(long)]
        json: bool,
        #[arg(long, default_value_t = 262144)]
        max_entries: usize,
        #[arg(long, default_value_t = 5000)]
        max_ms: u64,
    },
    /// Diffstat-ledger terminal UI (ratatui). Default when no
    /// subcommand is given.
    Ui {
        /// Defaults to the configured effective scope's first present
        /// root when omitted (see `swamp scope`); an explicit root
        /// still replaces the configured scope for this invocation.
        root: Option<PathBuf>,
    },
    Scan {
        #[arg(default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        store: Option<PathBuf>,
    },
    /// Project x worktree x artifact growth report -- a pure read of
    /// what `swamp observe` last wrote (R12): never walks a directory,
    /// stats a file, or spawns a subprocess. Exits 2 (JSON:
    /// `{"error":"no_observation", ...}`) when the scope has never been
    /// observed; run `swamp observe` first.
    Report {
        /// Defaults to the configured effective scope's first present
        /// root when omitted (see `swamp scope`); an explicit root
        /// still replaces the configured scope for this invocation,
        /// though configured exclusions still apply.
        root: Option<PathBuf>,
        #[arg(long)]
        json: bool,
        /// Print the `du -skPx` total the last `swamp observe
        /// --verify-du` stored, when present. Never runs `du` itself --
        /// `report` never spawns a subprocess.
        #[arg(long)]
        verify_du: bool,
        /// Drill into one project: worktree -> kind -> path -> bytes ->
        /// growth -> regrowth -> signals. Text output only.
        #[arg(long)]
        project: Option<String>,
        /// Bytes and count per artifact kind across the root. Text output
        /// only. Deprecated alias for `--view kinds`.
        #[arg(long)]
        kinds: bool,
        /// Named view, at root or narrowed with `--project`: worktrees
        /// (git-enriched one-line-per-worktree listing at root, the tree
        /// drill with `--project`), builds, deps, docker, kinds, unowned,
        /// reconciliation, rust. Every question the issue lists is exactly one
        /// command through this flag. Text output only.
        #[arg(long, value_enum)]
        view: Option<View>,
        /// Signals for one worktree (matched by exact or root-relative
        /// path), the one-command surface for a question the maintainer
        /// rule comment lists explicitly. Text output only.
        #[arg(long)]
        worktree: Option<PathBuf>,
        /// Filter worktrees/artifacts, e.g. "merge-complete idle > 48h".
        /// See `swamp_core::filter` for the grammar. Only consulted
        /// by `--view worktrees` at root.
        #[arg(long)]
        filter: Option<String>,
        /// Show every project row instead of the default top-N. Text
        /// output only.
        #[arg(long)]
        all: bool,
        /// List every unjoined Docker object individually instead of the
        /// default one-line-per-kind summary. Text output only.
        #[arg(long)]
        docker: bool,
        /// R4c: print each worktree's subdirectory growth table (and any
        /// large files that grew) instead of the overview. Combine with
        /// `--project` to narrow to one project; text output only, kept
        /// separate from `render.rs`'s overview rendering.
        #[arg(long)]
        dirs: bool,
        /// With `--dirs`, only print directories up to this many path
        /// components deep (relative to the worktree root). Unset shows
        /// every directory.
        #[arg(long)]
        depth: Option<usize>,
        /// Order of the overview's project rows: growth (default), size,
        /// name, type (grouped by ecosystem), age (oldest artifact first).
        #[arg(long, value_enum, default_value = "growth")]
        sort: SortArg,
        /// Reverse the sort order (smallest first, newest first, …).
        #[arg(long)]
        reverse: bool,
        /// Bound a JSON array-shaped result (the `result` array with
        /// `--view`, or the `projects` array without one) to this many
        /// rows. Only consulted with `--json`; the envelope's `total`
        /// and `truncated` fields say whether this is a partial page.
        #[arg(long)]
        limit: Option<usize>,
        /// Skip this many rows of a JSON array-shaped result before
        /// applying `--limit`. Only consulted with `--json`.
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// With `--view docker --json`, restrict to Docker objects with
        /// no join evidence to a project (never attributed by name
        /// similarity in this mode).
        #[arg(long)]
        unowned_only: bool,
    },
    /// Observe-only: walk `root`s, write the growth store, and refresh
    /// GitHub enrichment live for every GitHub-remote worktree found
    /// (concurrent, coalesced per repo -- see `github::observe_all`). No
    /// rendering. This is what a scheduled LaunchAgent run executes, and
    /// the only `swamp` command that calls `gh` on your behalf by
    /// default; `report` reads whatever this last wrote.
    Observe {
        /// Defaults to every present root in the configured effective
        /// scope when omitted (see `swamp scope`); explicit roots still
        /// replace the configured scope for this invocation, though
        /// configured exclusions still apply.
        roots: Vec<PathBuf>,
        /// Skip the FSEvents-driven incremental attempt and force a full
        /// walk (also re-anchors the stored event id for next time).
        #[arg(long)]
        full: bool,
        #[arg(long)]
        docker_facts: Option<PathBuf>,
        /// Also run `du -skPx` on each root as an independent total
        /// (slow); `swamp report --verify-du` prints whatever this
        /// stores.
        #[arg(long)]
        verify_du: bool,
        /// How far back to look for the growth baseline (e.g. "24h",
        /// "30m", "7d"). Overrides the `since` setting in config.toml.
        #[arg(long)]
        since: Option<String>,
        /// Refresh GitHub enrichment live before persisting it, instead
        /// of leaving `enrich.parquet` as-is. Every `swamp observe`
        /// already shells out to `gh` for enrichment (see the command's
        /// own doc); this additionally forces a live refresh rather
        /// than trusting the cache's TTL.
        #[arg(long)]
        enrich: bool,
    },
    /// Linux: watch the scope's roots with inotify until stopped and keep
    /// a bounded change list, so a later `observe`/`report` can walk only
    /// what changed (#82). Opt-in, user-owned, foreground; refuses on
    /// macOS, where FSEvents already keeps the history.
    Collect {
        /// Defaults to every present root in the configured scope.
        roots: Vec<PathBuf>,
        /// Print each root's checkpoint and whether its collector is
        /// running, then exit. Reads only.
        #[arg(long)]
        status: bool,
        /// With --status: the CLI's JSON contract instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Install, report on, or remove the opt-in per-user LaunchAgent that
    /// runs `observe` on a fixed interval (#31).
    Schedule {
        /// Install (or replace) the schedule with this interval, e.g.
        /// "30m", "1h", "12h", "1d".
        #[arg(long)]
        every: Option<String>,
        /// Remove the schedule.
        #[arg(long)]
        off: bool,
        /// Linux: also install the optional `swamp collect` user service,
        /// which keeps a live change list between scheduled runs so they
        /// can walk only what changed. Refused on macOS (not needed).
        #[arg(long)]
        collector: bool,
        roots: Vec<PathBuf>,
    },
    /// The configuration file: `config show` prints effective values,
    /// `config path` where it lives, `config init` writes one with every
    /// key and its meaning (never overwrites an existing file).
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Print the effective scan scope (#41): every root swamp would use
    /// for this invocation, its status (present/missing/unreadable/
    /// skipped-as-nested/excluded) and every reason it is in scope --
    /// built-in default, detector (with id/category/provenance),
    /// configured include, or explicit command root -- plus the full
    /// detector catalog (including disabled/not-present/unresolved
    /// entries) and the detector catalog version. With explicit roots,
    /// shows what those roots resolve to (configured exclusions still
    /// apply) instead of the configured scope. This is the one shared
    /// resolution every scope-aware command (`report`, `observe`, `ui`,
    /// `schedule`) uses when no explicit root is given -- never a
    /// separate ad hoc computation.
    Scope {
        roots: Vec<PathBuf>,
        #[arg(long)]
        json: bool,
        /// Show every root, including folded-as-nested and Unclassified
        /// detector locations, which the default text view hides as
        /// noise (#R13 item B). `--json` is always the full, unfiltered
        /// scope -- this flag only changes the text renderer.
        #[arg(long)]
        verbose: bool,
    },
    /// Human keep/protect intent for agent-storage paths (#100/#101):
    /// survives refresh and is never itself inferred from observation --
    /// only this command changes it. The TUI's mark step refuses to
    /// queue anything under a protected path for the Trash.
    Protect {
        #[command(subcommand)]
        cmd: ProtectCmd,
    },
}

#[derive(Subcommand)]
enum ProtectCmd {
    Add {
        path: PathBuf,
    },
    Remove {
        path: PathBuf,
    },
    List {
        #[arg(long)]
        json: bool,
    },
}

/// The resolved swamp dir (`$SWAMP_DIR`, else `~/.local/share/swamp`):
/// the gate's one resolver, `fs_gate::store::StoreDir::resolved`. Same on
/// Linux and macOS; ported from the Linux track's XDG-aware
/// `platform::data_dir` deliberately narrowed back to this one resolver
/// so there remains exactly one place that decides where swamp's state
/// lives (see the port session note).
fn swamp_dir() -> PathBuf {
    swamp_core::fs_gate::store::StoreDir::resolved()
        .path()
        .to_path_buf()
}

/// The one shared resolution every scope-aware command (#41) goes
/// through: built-in defaults, detector results, and configured
/// include/exclude/disabled-detectors, or -- when `explicit` is
/// non-empty -- exactly those roots (configured exclusions still
/// apply). A malformed `config.toml` is a hard error here (nonzero
/// exit via `main`'s `Result`, message on stderr): scope resolution
/// never silently falls back to a broader default on invalid config.
fn resolve_scope(explicit: &[PathBuf]) -> Result<swamp_core::scope::EffectiveScope> {
    let store_dir = swamp_dir();
    let cfg = swamp_core::growth::load_config_checked(&store_dir)?;
    let env = swamp_core::locations::Environment::from_process();
    let registry = swamp_core::locations::Registry::with_builtins();
    Ok(swamp_core::scope::resolve_effective_scope(
        &env,
        &cfg.scan,
        explicit,
        &registry,
        swamp_core::entities::now(),
    ))
}

/// Persists the just-resolved scope and, when a previous one exists,
/// prints a one-line coverage-change note to stderr (#41's "explain
/// effective coverage and baseline changes"). This never touches byte
/// history: it is coverage bookkeeping only, per
/// `.oh/guardrails/coverage-changes-are-not-storage-changes.md`.
fn note_and_persist_scope(store_dir: &Path, scope: &swamp_core::scope::EffectiveScope) {
    if let Some(previous) = swamp_core::scope::load_last_effective_scope(store_dir) {
        let changes = swamp_core::scope::coverage_changes(&previous, scope);
        if !changes.is_empty() {
            let summary = changes
                .iter()
                .map(|c| {
                    let sign = match c.kind {
                        swamp_core::scope::CoverageChangeKind::Added => '+',
                        swamp_core::scope::CoverageChangeKind::Removed => '-',
                    };
                    format!("{sign}root {} ({})", c.path.display(), c.reason)
                })
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!("coverage changed since last observation: {summary}");
        }
    }
    if let Err(e) = swamp_core::scope::persist_effective_scope(store_dir, scope) {
        eprintln!("note: could not persist effective scope for next run: {e}");
    }
}

/// One line per non-`Complete` root from a `report_scope` call (#42):
/// missing/excluded/inaccessible/partial regions, so a coherent
/// multi-root observation never silently under-reports without saying
/// why. A `Complete` root prints nothing -- the ordinary case should not
/// be noisy.
fn print_scope_coverage_note(coverage: &[swamp_core::coverage::RootCoverage]) {
    use swamp_core::coverage::RegionStatus;
    let incomplete: Vec<&swamp_core::coverage::RootCoverage> = coverage
        .iter()
        .filter(|c| !matches!(c.status, RegionStatus::Complete))
        .collect();
    if incomplete.is_empty() {
        return;
    }
    // Missing/excluded/detector-only paths are the usual state of a
    // machine that has fewer tools installed than the catalog knows:
    // one count, `swamp scope` for the list. Only a root that *should*
    // have been walked and was not (partial, inaccessible) is named.
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    let mut named = Vec::new();
    for c in &incomplete {
        match &c.status {
            RegionStatus::Partial { .. } | RegionStatus::Inaccessible { .. } => {
                named.push(format!("{} ({})", c.path.display(), c.status.label()))
            }
            RegionStatus::Missing => *counts.entry("missing").or_default() += 1,
            RegionStatus::Excluded => *counts.entry("excluded").or_default() += 1,
            RegionStatus::DetectorOnly => *counts.entry("detector-only").or_default() += 1,
            RegionStatus::Complete => {}
        }
    }
    let mut parts: Vec<String> = counts.iter().map(|(k, n)| format!("{n} {k}")).collect();
    parts.extend(named);
    eprintln!(
        "scope coverage: {} (swamp scope for the list)",
        parts.join(", ")
    );
}

fn render_scope_text(scope: &swamp_core::scope::EffectiveScope, verbose: bool) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "platform {} · catalog {} · defaults={} · disabled=[{}] · {}",
        // The same fact `--json` carries in `platform`: whose conventions
        // produced these roots. Without it, a scope printed on one OS and
        // read on the other is a list of missing paths with no
        // explanation.
        swamp_core::platform::Os::from(scope.platform).as_str(),
        scope.catalog_version,
        scope.defaults_enabled,
        scope.disabled_detectors.join(", "),
        if scope.explicit {
            "explicit roots (configured exclusions still apply)"
        } else {
            "configured scope"
        }
    );

    fn reason_labels(root: &swamp_core::scope::ScopeRoot) -> Vec<String> {
        root.reasons
            .iter()
            .map(|r| match r {
                swamp_core::scope::RootReason::BuiltinDefault => "built-in default".to_string(),
                swamp_core::scope::RootReason::Detector {
                    detector_id,
                    category,
                    provenance,
                } => format!(
                    "detector:{detector_id} ({}, {})",
                    swamp_core::external::category_label(*category),
                    swamp_core::locations::provenance_label(provenance)
                ),
                swamp_core::scope::RootReason::Included => "include".to_string(),
                swamp_core::scope::RootReason::ExplicitCommand => "explicit".to_string(),
                swamp_core::scope::RootReason::NestedFrom { path } => {
                    format!("covers nested {}", path.display())
                }
            })
            .collect()
    }
    fn status_label(status: &swamp_core::scope::RootStatus) -> String {
        match status {
            swamp_core::scope::RootStatus::Present => "present".to_string(),
            swamp_core::scope::RootStatus::Missing => "missing".to_string(),
            swamp_core::scope::RootStatus::Unreadable { reason } => {
                format!("unreadable ({reason})")
            }
            swamp_core::scope::RootStatus::SkippedAsNested { parent } => {
                format!("skipped-as-nested (folded into {})", parent.display())
            }
            swamp_core::scope::RootStatus::Excluded { pattern } => format!("excluded ({pattern})"),
        }
    }
    // Noise the default view hides (#R13 item B): a root already folded
    // into a parent's walk tells a reader nothing beyond what its
    // parent's own row already says -- `--verbose`/`--json` still show
    // it in full.
    fn is_noise(root: &swamp_core::scope::ScopeRoot) -> bool {
        matches!(
            root.status,
            swamp_core::scope::RootStatus::SkippedAsNested { .. }
        )
    }

    // Two classes (#R13 item B): project roots get ordinary Git/
    // ecosystem discovery and an unowned remainder; detector locations
    // are measured only as external units. `swamp report`'s coverage
    // (`RegionStatus::DetectorOnly`) is this same split at observe time.
    let (project_roots, detector_roots): (Vec<_>, Vec<_>) =
        scope.roots.iter().partition(|r| r.is_project_root());

    let _ = writeln!(
        out,
        "project roots (Git/ecosystem discovery, unowned remainder):"
    );
    for root in &project_roots {
        if !verbose && is_noise(root) {
            continue;
        }
        let _ = writeln!(
            out,
            "  {:<10} {}  [{}]",
            status_label(&root.status),
            root.path.display(),
            reason_labels(root).join("; ")
        );
    }
    let _ = writeln!(
        out,
        "detector locations (measured as external units only, never a project):"
    );
    for root in &detector_roots {
        if !verbose && is_noise(root) {
            continue;
        }
        let _ = writeln!(
            out,
            "  {:<10} {}  [{}]",
            status_label(&root.status),
            root.path.display(),
            reason_labels(root).join("; ")
        );
    }
    if !verbose {
        let hidden = scope.roots.iter().filter(|r| is_noise(r)).count();
        if hidden > 0 {
            let _ = writeln!(
                out,
                "({hidden} nested/Unclassified root(s) hidden; --verbose to show)"
            );
        }
    }

    if !scope.pruned_subtrees.is_empty() {
        let _ = writeln!(out, "pruned subtrees (excluded, inside an in-scope root):");
        for p in &scope.pruned_subtrees {
            let _ = writeln!(out, "  {} under {}", p.pattern, p.root.display());
        }
    }
    let _ = writeln!(out, "detectors:");
    for d in &scope.detectors {
        for loc in d.locations_for_display() {
            let status = match &loc.status {
                swamp_core::locations::LocationStatus::Resolved => "resolved".to_string(),
                swamp_core::locations::LocationStatus::NotPresent => "not-present".to_string(),
                // `disabled (default off)` names *why* this one is off
                // without the config saying so -- a system-wide install
                // tree like Homebrew (`Detector::default_enabled`) --
                // distinct from a plain `disabled`, which the user's own
                // `disabled_detectors` caused. `[scan] enabled_detectors
                // = ["<id>"]` turns either kind back on.
                swamp_core::locations::LocationStatus::Disabled => {
                    if scope.default_off_detectors.contains(&d.detector_id) {
                        "disabled (default off)".to_string()
                    } else {
                        "disabled".to_string()
                    }
                }
                swamp_core::locations::LocationStatus::UnresolvedWithReason { reason } => {
                    format!("unresolved ({reason})")
                }
            };
            let path = loc
                .path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "-".to_string());
            let _ = writeln!(out, "  {:<12} {:<10} {}", d.detector_id, status, path);
        }
    }
    // "Not applicable" is its own answer, distinct from "found nothing"
    // and from "could not tell" (#84). Omitting these would leave a
    // Linux user wondering whether Xcode detection failed rather than
    // knowing it does not apply.
    if !scope.not_applicable_detectors.is_empty() {
        let _ = writeln!(
            out,
            "not applicable on this platform (not failures, and not absences):"
        );
        for d in &scope.not_applicable_detectors {
            let applies: Vec<&str> = d
                .applies_to
                .iter()
                .map(|p| swamp_core::platform::Os::from(*p).as_str())
                .collect();
            let _ = writeln!(
                out,
                "  {:<12} {:<10} {} (applies to: {})",
                d.detector_id,
                "n/a",
                d.name,
                applies.join(", ")
            );
        }
    }
    out
}

/// `swamp protect add|remove|list`: the human keep list. Read-only
/// otherwise, this is the one CLI surface that still writes state, since
/// it never deletes anything -- it only ever keeps a path out of the
/// TUI's Trash.
fn cmd_protect(cmd: ProtectCmd) -> Result<()> {
    let store_dir = swamp_dir();
    match cmd {
        ProtectCmd::Add { path } => {
            // Resolve a relative argument against the cwd before
            // storing, rather than printing "protected: debug" for an
            // entry that protects nothing (the 2026-09-22 re-review's
            // CE5). `protect_add` refuses a non-absolute path outright;
            // doing the join here means `swamp protect add debug` from
            // inside a tool home does the obvious thing and *says* which
            // path it protected.
            let resolved = if path.is_absolute() {
                path.clone()
            } else {
                std::env::current_dir()?.join(&path)
            };
            swamp_core::agents::protect_add(&store_dir, &resolved)?;
            safe_println!("protected: {}", resolved.display());
        }
        ProtectCmd::Remove { path } => {
            let resolved = if path.is_absolute() {
                path.clone()
            } else {
                std::env::current_dir()?.join(&path)
            };
            swamp_core::agents::protect_remove(&store_dir, &resolved)?;
            safe_println!("no longer protected: {}", resolved.display());
        }
        ProtectCmd::List { json } => {
            let listing = swamp_core::agents::protect_listing(&store_dir)?;
            if json {
                safe_println!("{}", serde_json::to_string_pretty(&listing)?);
            } else if listing.is_empty() {
                safe_println!("no protected agent-storage paths");
            } else {
                safe_println!("{listing}");
            }
        }
    }
    Ok(())
}

/// Whether `u`'s project linkage names `project` (case-insensitive,
/// matching this codebase's other `--project` matching): only a
/// `Linked` unit can match; every other linkage state (unresolved,
/// missing, not-a-project, moved, remote, shared, not-applicable) is
/// filtered out by a project filter, never silently included. `None`
/// (no filter given) matches everything.
fn agent_unit_matches_project(u: &swamp_core::agents::AgentUnit, project: Option<&str>) -> bool {
    let Some(project) = project else { return true };
    matches!(
        &u.project_link,
        swamp_core::agents::ProjectLinkState::Linked { project_name, .. }
            if project_name.eq_ignore_ascii_case(project)
    )
}

/// The bounded, documented JSON contract behind `report --json` (see
/// `skills/swamp/references/commands-and-json.md`): with `--view`, an
/// envelope `{view, project, result, observed_at, since,
/// index_refreshed, total, truncated}` (plus `coverage` for `--view
/// grown`); without one, the full (optionally project-scoped) report
/// with the same `since`/`index_refreshed`/`total`/`truncated` fields
/// added and its top-level `projects` array bounded by
/// `--limit`/`--offset`. `--filter`, when given, narrows the whole
/// report before any view is computed -- the same order the retired MCP
/// `report` tool applied it in, so a filtered view and a filtered full
/// report agree on what rows exist. This function is the only place
/// that builds `report --json` output; every branch below funnels
/// through it so `--view`/`--project`/`--filter` can never again be
/// silently ignored in JSON mode the way the whole-report dump used to
/// ignore them.
#[allow(clippy::too_many_arguments)]
fn report_json_envelope(
    r: &Report,
    root: &Path,
    view: Option<View>,
    project: Option<&str>,
    parsed_filter: Option<&filter::Filter>,
    since: Option<&str>,
    index_refreshed: bool,
    unowned_only: bool,
    limit: Option<usize>,
    offset: usize,
    scope_coverage: &[swamp_core::coverage::RootCoverage],
    external_units: &[swamp_core::external::ExternalUnit],
    agent_units: &[swamp_core::agents::AgentUnit],
    store_interiors: &[swamp_core::artifact::NestedArtifact],
) -> Result<serde_json::Value> {
    let store_dir = swamp_dir();
    let since_str = swamp_core::agent_json::effective_since(&store_dir, since);
    let mut rr = r.clone();
    if let Some(f) = parsed_filter {
        swamp_core::agent_json::apply_filter_to_report(&mut rr, f);
    }
    let observed_at = rr.observed_at;

    if let Some(v) = view {
        let name = v.name();
        let mut result = match v {
            View::Grown => swamp_core::agent_json::what_grew_payload(&rr, project),
            View::Projects => swamp_core::agent_json::list_projects_payload(&rr, project),
            View::Worktrees => swamp_core::agent_json::list_worktrees_payload(
                &rr,
                &filter::Filter::default(),
                project,
            ),
            View::Docker => {
                swamp_core::agent_json::docker_objects_payload(&rr, unowned_only, project)
            }
            View::Rust => {
                let units: Vec<_> = rr
                    .nested_artifacts
                    .iter()
                    .filter(|unit| {
                        project.is_none_or(|wanted| {
                            swamp_core::render::nested_artifact_project_name(&rr, unit)
                                == Some(wanted)
                        })
                    })
                    .collect();
                serde_json::json!(units)
            }
            View::External => serde_json::json!({
                "units": external_units,
                "total_bytes": swamp_core::external::total_bytes(external_units),
                // Each machine-wide build store's identified interior,
                // keyed by the external unit's path, in the shape
                // `--view builds --json` uses for a project container.
                "interiors": external_units
                    .iter()
                    .filter_map(|u| {
                        swamp_core::agent_json::interior_json(&u.path, store_interiors)
                            .map(|i| (u.path.display().to_string(), i))
                    })
                    .collect::<serde_json::Map<String, serde_json::Value>>(),
            }),
            View::Agents => {
                let filtered: Vec<&swamp_core::agents::AgentUnit> = agent_units
                    .iter()
                    .filter(|u| agent_unit_matches_project(u, project))
                    .collect();
                serde_json::json!({
                    "units": filtered,
                    "total_bytes": filtered.iter().map(|u| u.bytes).sum::<u64>(),
                })
            }
            _ => swamp_core::agent_json::view_payload(&rr, &name, project),
        };
        let page = swamp_core::agent_json::paginate(&mut result, limit, offset);
        let mut envelope = serde_json::json!({
            "view": name,
            "project": project,
            "result": result,
            "observed_at": observed_at,
            "since": since_str,
            "index_refreshed": index_refreshed,
        });
        if let Some(p) = page {
            envelope["total"] = serde_json::json!(p.total);
            envelope["truncated"] = serde_json::json!(p.truncated);
        }
        if !scope_coverage.is_empty() {
            envelope["scope_coverage"] = serde_json::json!(scope_coverage);
        }
        if v == View::Docker && project.is_none() {
            envelope["buildkit"] = swamp_core::agent_json::buildkit_payload(&rr);
        }
        if v == View::Grown {
            envelope["coverage"] = serde_json::json!({
                "walked_total": rr.reconciliation.walked_total,
                "unique_estimate": rr.reconciliation.unique_estimate,
                "du_total": rr.reconciliation.du_total,
                "unowned_total": rr.reconciliation.unowned,
                "attributed_total": rr.reconciliation.attributed,
                "observed_at": observed_at,
                "since": since_str,
                "index_refreshed": index_refreshed,
                "history": swamp_core::agent_json::history_block(&store_dir, root, Some(&since_str)),
            });
        }
        return Ok(envelope);
    }

    if let Some(name) = project {
        swamp_core::agent_json::scope_to_project(&mut rr, name);
    }
    let mut value = serde_json::to_value(&rr)?;
    value["since"] = serde_json::json!(since_str);
    value["index_refreshed"] = serde_json::json!(index_refreshed);
    if !scope_coverage.is_empty() {
        value["scope_coverage"] = serde_json::json!(scope_coverage);
    }
    // `--project NAME --json` (no `--view`): include this project's own
    // linked agent-storage units inline, same linkage-state contract as
    // `--view agents --project NAME` (#100's "the CLI `report --project
    // X --json` includes linked agent units with linkage states" --
    // this used to be silently absent whenever `--view agents` was not
    // also passed).
    if project.is_some() && !agent_units.is_empty() {
        let filtered: Vec<&swamp_core::agents::AgentUnit> = agent_units
            .iter()
            .filter(|u| agent_unit_matches_project(u, project))
            .collect();
        value["agent_storage"] = serde_json::json!({
            "units": filtered,
            "total_bytes": filtered.iter().map(|u| u.bytes).sum::<u64>(),
        });
    }
    if let Some(mut projects) = value.get("projects").cloned() {
        let page = swamp_core::agent_json::paginate(&mut projects, limit, offset);
        value["projects"] = projects;
        if let Some(p) = page {
            value["total"] = serde_json::json!(p.total);
            value["truncated"] = serde_json::json!(p.truncated);
        }
    }
    Ok(value)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Ui { root: None }) {
        Command::InspectCargo {
            profile,
            json,
            max_entries,
            max_ms,
        } => {
            use std::sync::atomic::AtomicBool;
            let result = swamp_core::cargo_artifacts::inspect_profile(
                &profile,
                swamp_core::cargo_artifacts::CargoProfileInspectionLimits {
                    max_entries,
                    max_duration: std::time::Duration::from_millis(max_ms),
                    ..Default::default()
                },
                &AtomicBool::new(false),
            );
            if json {
                safe_println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                safe_println!(
                    "Cargo profile: {} ({} bytes allocated; {} unique; coverage {})",
                    profile.display(),
                    result.allocated_bytes,
                    result.unique_allocated_bytes,
                    if result.coverage.complete {
                        "complete"
                    } else {
                        "partial"
                    }
                );
                safe_println!("{}", result.accounting_note);
                for limit in &result.coverage.limits {
                    safe_println!("Limit: {limit}");
                }
                for group in result.groups {
                    safe_println!(
                        "{} [{}; features {}; package {}]: {} bytes allocated{}",
                        group.target.unwrap_or_else(|| "unknown/residual".into()),
                        group.target_kind.as_deref().unwrap_or("unknown kind"),
                        group.variant.features.as_deref().unwrap_or("unknown"),
                        group.package_id.as_deref().unwrap_or("unknown"),
                        group.allocated_bytes,
                        group
                            .residual_reason
                            .map(|r| format!(" ({r})"))
                            .unwrap_or_default()
                    );
                }
            }
        }
        Command::Ui { root } => {
            if let Some(explicit) = root {
                swamp_tui::run(&explicit)?;
            } else {
                // No explicit root: the TUI opens the *whole* configured
                // multi-root scope (#51) -- project/shared/external/
                // agent-tool storage from every present root at once,
                // including a root with no Git checkout in it at all,
                // not just the first present root the CLI used to pick
                // before the TUI even started.
                let scope = resolve_scope(&[])?;
                if scope.scan_paths().is_empty() {
                    if scope.is_empty_scope() {
                        anyhow::bail!(
                            "effective scan scope is empty: no built-in default, detector, or configured include is enabled. This is explicit, not a fallback to the current directory -- see `swamp scope --json`, or pass a root explicitly."
                        );
                    }
                    anyhow::bail!(
                        "configured scope has no present root (every candidate is missing/unreadable/excluded) -- see `swamp scope --json`, or pass a root explicitly."
                    );
                }
                note_and_persist_scope(&swamp_dir(), &scope);
                swamp_tui::run_scope(&scope)?;
            }
        }
        Command::Scan { root, store } => {
            let obs = observation(&ScanOptions {
                roots: vec![root],
                cross_device: false,
                max_depth: None,
            })?;
            if let Some(s) = store {
                Store::open(s)?.write(&obs)?;
            }
            safe_println!("{}", serde_json::to_string_pretty(&obs)?);
        }
        Command::Report {
            root,
            json,
            verify_du,
            project,
            kinds,
            view,
            worktree,
            filter: filter_expr,
            all,
            docker,
            dirs,
            depth,
            sort,
            reverse,
            limit,
            offset,
            unowned_only,
        } => {
            let explicit_root = root.clone();
            // `--kinds`/`--docker` are deprecated aliases folded under
            // `--view` (#33); an explicit `--view` wins if somehow both
            // are given.
            let view = view.or_else(|| {
                if kinds {
                    Some(View::Kinds)
                } else if docker && project.is_none() {
                    Some(View::Docker)
                } else {
                    None
                }
            });
            let store_dir = swamp_dir();
            // R12: `report` is a pure read. The scope this invocation
            // names (an explicit root is a scope of one, #42) is
            // resolved only to know *which* stored snapshot to read and
            // how to describe it if there is none -- resolving a scope
            // stats each candidate root for presence but never walks
            // one, so this is not the "no filesystem walk" the pipeline
            // otherwise avoids.
            let scope_roots: Vec<PathBuf> = explicit_root.clone().into_iter().collect();
            let scope = resolve_scope(&scope_roots)?;
            if scope.scan_paths().is_empty() {
                if scope.is_empty_scope() {
                    anyhow::bail!(
                        "effective scan scope is empty: no built-in default, detector, or configured include is enabled. This is explicit, not a fallback to the current directory -- see `swamp scope --json`, or pass a root explicitly."
                    );
                }
                anyhow::bail!(
                    "configured scope has no present root (every candidate is missing/unreadable/excluded) -- see `swamp scope --json`, or pass a root explicitly."
                );
            }
            let snapshot = match swamp_core::report::report_scope_from_store(&scope, &store_dir) {
                Ok(s) => s,
                Err(e) => {
                    if json {
                        safe_println!(
                            "{}",
                            serde_json::to_string_pretty(&serde_json::json!({
                                "error": "no_observation",
                                "scope": e.scope_description,
                            }))?
                        );
                    } else {
                        eprintln!("{e}");
                    }
                    std::process::exit(2);
                }
            };
            let r = snapshot.report;
            // An explicit root is a scope of one (#42): it never carried
            // scope-level coverage noise even when the underlying pass
            // is now the same coherent scope pipeline the catalog uses,
            // so that contract is preserved here rather than in storage.
            let coverage = if explicit_root.is_some() {
                Vec::new()
            } else {
                snapshot.coverage
            };
            let external_units = snapshot.external_units;
            let agent_units = snapshot.agent_units;
            let store_interiors = snapshot.store_interiors;
            let root = r.root.clone();
            if !coverage.is_empty() {
                print_scope_coverage_note(&coverage);
            }
            if !json
                && r.projects
                    .iter()
                    .flat_map(|p| &p.worktrees)
                    .flat_map(|w| &w.artifacts)
                    .any(|a| a.dedup_stale)
            {
                eprintln!(
                    "Unique-byte totals were not recomputed this pass; use `swamp observe --full` to reconcile. Allocated sizes are current and may count hardlinks multiple times."
                );
            }
            let parsed_filter = match filter_expr.as_deref().map(filter::parse) {
                Some(Ok(f)) => Some(f),
                Some(Err(e)) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                None => None,
            };
            if json {
                safe_println!(
                    "{}",
                    serde_json::to_string_pretty(&report_json_envelope(
                        &r,
                        &root,
                        view,
                        project.as_deref(),
                        parsed_filter.as_ref(),
                        None,
                        false,
                        unowned_only,
                        limit,
                        offset,
                        &coverage,
                        &external_units,
                        &agent_units,
                        &store_interiors,
                    )?)?
                );
            } else if let Some(wt_path) = worktree {
                match render_worktree_signals(&r, &wt_path) {
                    Some(text) => safe_print!("{text}"),
                    None => {
                        eprintln!(
                            "no worktree at {} found under {}",
                            wt_path.display(),
                            root.display()
                        );
                        std::process::exit(1);
                    }
                }
            } else if dirs {
                match render_dirs(&r, project.as_deref(), depth) {
                    Ok(text) => safe_print!("{text}"),
                    Err(name) => {
                        eprintln!("no project named {name:?} found under {}", root.display());
                        std::process::exit(1);
                    }
                }
            } else if let Some(name) = project {
                match view {
                    None | Some(View::Worktrees) => {
                        match render_project_tree_with_agents(&r, &name, &agent_units) {
                            Some(text) => safe_print!("{text}"),
                            None => {
                                eprintln!(
                                    "no project named {name:?} found under {}",
                                    root.display()
                                );
                                std::process::exit(1);
                            }
                        }
                    }
                    Some(View::Builds) => safe_print!("{}", render_view_builds(&r, Some(&name))),
                    Some(View::Deps) => safe_print!("{}", render_view_deps(&r, Some(&name))),
                    Some(View::Docker) => safe_print!("{}", render_view_docker(&r, Some(&name))),
                    Some(View::Kinds) => safe_print!("{}", render_kinds(&r)),
                    Some(View::Types) => safe_print!("{}", render_types(&r)),
                    Some(View::Rust) => {
                        safe_print!(
                            "{}",
                            swamp_core::render::render_view_rust_with_limit(
                                &r,
                                Some(&name),
                                if all { None } else { Some(30) }
                            )
                        )
                    }
                    Some(View::Unowned) => safe_print!("{}", render_view_unowned(&r)),
                    Some(View::Reconciliation) => safe_print!("{}", render_view_reconciliation(&r)),
                    Some(View::External) => {
                        safe_print!(
                            "{}",
                            swamp_core::render::render_view_external_with(
                                &external_units,
                                &store_interiors,
                                r.observed_at,
                            )
                        )
                    }
                    Some(View::Agents) => {
                        safe_print!(
                            "{}",
                            swamp_core::render::render_view_agents(
                                &agent_units,
                                Some(&name),
                                all,
                                r.observed_at
                            )
                        )
                    }
                    Some(v @ (View::Projects | View::Grown)) => {
                        eprintln!("--view {} is JSON only; add --json", v.name());
                        std::process::exit(1);
                    }
                }
            } else {
                match view {
                    Some(View::Worktrees) => safe_print!(
                        "{}",
                        render_worktrees(&r, &parsed_filter.unwrap_or_default())
                    ),
                    Some(View::Kinds) => safe_print!("{}", render_kinds(&r)),
                    Some(View::Builds) => safe_print!("{}", render_view_builds(&r, None)),
                    Some(View::Deps) => safe_print!("{}", render_view_deps(&r, None)),
                    Some(View::Docker) => safe_print!("{}", render_view_docker(&r, None)),
                    Some(View::Types) => safe_print!("{}", render_types(&r)),
                    Some(View::Rust) => {
                        safe_print!(
                            "{}",
                            swamp_core::render::render_view_rust_with_limit(
                                &r,
                                None,
                                if all { None } else { Some(30) }
                            )
                        )
                    }
                    Some(View::Unowned) => safe_print!("{}", render_view_unowned(&r)),
                    Some(View::Reconciliation) => safe_print!("{}", render_view_reconciliation(&r)),
                    Some(View::External) => {
                        safe_print!(
                            "{}",
                            swamp_core::render::render_view_external_with(
                                &external_units,
                                &store_interiors,
                                r.observed_at,
                            )
                        )
                    }
                    Some(View::Agents) => {
                        safe_print!(
                            "{}",
                            swamp_core::render::render_view_agents(
                                &agent_units,
                                None,
                                all,
                                r.observed_at
                            )
                        )
                    }
                    Some(v @ (View::Projects | View::Grown)) => {
                        eprintln!("--view {} is JSON only; add --json", v.name());
                        std::process::exit(1);
                    }
                    None => safe_print!(
                        "{}",
                        render_overview_sorted(&r, all, verify_du, docker, sort.into(), reverse)
                    ),
                }
            }
        }
        Command::Protect { cmd } => cmd_protect(cmd)?,
        Command::Config { action } => {
            let dir = swamp_dir();
            let path = dir.join("config.toml");
            match action {
                ConfigAction::Path => safe_println!("{}", path.display()),
                ConfigAction::Show => {
                    safe_print!(
                        "{}",
                        swamp_core::growth::load_config_checked(&dir)?.to_toml()
                    );
                    if !swamp_core::fs_gate::exists(&path) {
                        eprintln!(
                            "(defaults; no file at {} — `swamp config init` writes one)",
                            path.display()
                        );
                    }
                }
                ConfigAction::Init => {
                    if swamp_core::fs_gate::exists(&path) {
                        eprintln!("{} already exists; not overwriting", path.display());
                        std::process::exit(1);
                    }
                    swamp_core::fs_gate::store::write_text(
                        swamp_core::fs_gate::store::TextFile::Config {
                            store: &swamp_core::fs_gate::store::StoreDir::resolved(),
                        },
                        &swamp_core::growth::GrowthConfig::default().to_toml(),
                    )?;
                    safe_println!("wrote {}", path.display());
                }
            }
        }
        Command::Scope {
            roots,
            json,
            verbose,
        } => {
            let scope = resolve_scope(&roots)?;
            if json {
                safe_println!("{}", serde_json::to_string_pretty(&scope)?);
            } else {
                safe_print!("{}", render_scope_text(&scope, verbose));
                if scope.is_empty_scope() {
                    eprintln!(
                        "effective scan scope is empty: no built-in default, detector, or configured include is enabled -- this is explicit, never a silent fallback to cwd or home."
                    );
                }
            }
        }
        Command::Observe {
            roots,
            full,
            docker_facts,
            verify_du,
            since,
            enrich,
        } => {
            let store_dir = swamp_dir();
            let scope = resolve_scope(&roots)?;
            if scope.scan_paths().is_empty() {
                if scope.is_empty_scope() {
                    anyhow::bail!(
                        "effective scan scope is empty: no built-in default, detector, or configured include is enabled. This is explicit, not a fallback to the current directory -- see `swamp scope --json`, or pass a root explicitly."
                    );
                }
                anyhow::bail!(
                    "no present root to observe (every candidate is missing/unreadable/excluded) -- see `swamp scope --json`, or pass a root explicitly."
                );
            }
            note_and_persist_scope(&store_dir, &scope);
            let progress =
                spawn_progress_line(std::io::IsTerminal::is_terminal(&std::io::stderr()));
            let result = schedule::cmd_observe(
                store_dir,
                scope,
                full,
                docker_facts,
                verify_du,
                since,
                enrich,
            );
            progress.stop();
            result?;
        }
        Command::Collect {
            roots,
            status,
            json,
        } => {
            let store_dir = swamp_dir();
            let scope = resolve_scope(&roots)?;
            let (authorized, _) = scope.authorized_roots();
            let present: Vec<collect::Root> = authorized
                .into_iter()
                .filter(|r| r.nested_in.is_none() && swamp_core::fs_gate::is_dir(&r.path))
                .map(|r| collect::Root {
                    excluded: r.pruned_subtrees.clone(),
                    path: swamp_core::fs_gate::canonicalize(&r.path).unwrap_or(r.path),
                })
                .collect();
            if present.is_empty() {
                anyhow::bail!(
                    "no present root to collect for -- see `swamp scope --json`, or pass a root explicitly."
                );
            }
            if status {
                let roots = present.into_iter().map(|r| r.path).collect();
                collect::cmd_collect_status(store_dir, roots, json)?;
            } else {
                collect::cmd_collect(store_dir, present)?;
            }
        }
        Command::Schedule {
            every,
            off,
            collector,
            roots,
        } => {
            let store_dir = swamp_dir();
            // No explicit roots: install `observe` with none baked into
            // the plist's argv at all (#42/#50), so every scheduled fire
            // re-resolves the configured scope itself (same code path
            // `swamp observe` with no roots already takes) instead of
            // replaying whatever was present at `schedule --every` time.
            // A config edit therefore takes effect on the next scheduled
            // run, not only after `schedule --every` is run again. This
            // is a validate-then-install check only: it fails fast on an
            // empty scope now rather than installing a schedule that can
            // never do anything, but it does not freeze the resolved
            // list into the plist -- explicit roots on the command line
            // still do, exactly as an explicit root has always replaced
            // the configured scope for one invocation.
            if roots.is_empty() && !off && every.is_some() {
                let scope = resolve_scope(&[])?;
                if scope.scan_paths().is_empty() {
                    anyhow::bail!(
                        "effective scan scope is empty; nothing to schedule -- see `swamp scope --json`, or pass roots explicitly."
                    );
                }
            }
            schedule::cmd_schedule(store_dir, every, off, collector, roots)?;
        }
    }
    Ok(())
}

/// R4c `--dirs` drill: per worktree, subdirectories sorted by growth desc
/// then bytes desc, with `changed <age>` derived from `mod_time_min`;
/// large files that grew are listed beneath. Kept in the CLI (not
/// `render.rs`) per this slice's scope: it is a minimal text view over
/// `dirs_by_worktree`/`files_by_worktree`, not part of the overview
/// renderer's contract.
///
/// Returns `Err(name)` when `only_project` names a project not present
/// in the report, mirroring `render_project_tree`'s `None` case.
fn render_dirs(
    report: &Report,
    only_project: Option<&str>,
    depth: Option<usize>,
) -> Result<String, String> {
    use std::fmt::Write as _;

    let empty_dirs = std::collections::HashMap::new();
    let empty_files = std::collections::HashMap::new();
    let dirs_by_worktree = report.dirs_by_worktree.as_ref().unwrap_or(&empty_dirs);
    let files_by_worktree = report.files_by_worktree.as_ref().unwrap_or(&empty_files);

    if let Some(name) = only_project
        && !report.projects.iter().any(|p| p.name == name)
    {
        return Err(name.to_string());
    }

    let mut out = String::new();
    for project in &report.projects {
        if let Some(name) = only_project
            && project.name != name
        {
            continue;
        }
        for worktree in &project.worktrees {
            let mut rows: Vec<&swamp_core::report::DirRollup> = dirs_by_worktree
                .get(&worktree.worktree_id)
                .map(|v| v.iter().collect())
                .unwrap_or_default();
            if let Some(max_depth) = depth {
                rows.retain(|d| {
                    d.rel_path.is_empty() || d.rel_path.matches('/').count() < max_depth
                });
            }
            rows.sort_by(|a, b| {
                b.growth_bytes
                    .unwrap_or(0)
                    .cmp(&a.growth_bytes.unwrap_or(0))
                    .then(b.allocated_total.cmp(&a.allocated_total))
            });

            let _ = writeln!(
                out,
                "{} [{}] {}",
                project.name,
                worktree.worktree_id,
                worktree.path.display()
            );
            for row in &rows {
                let label = if row.rel_path.is_empty() {
                    ".".to_string()
                } else {
                    row.rel_path.clone()
                };
                let growth = row
                    .growth_bytes
                    .map(|g| format!(" ({g:+} bytes)"))
                    .unwrap_or_default();
                let track = row
                    .track
                    .map(|t| format!("  [{}]", t.label()))
                    .unwrap_or_default();
                let _ = writeln!(
                    out,
                    "  {label:<40} {:>12} bytes{growth}  changed {}{track}",
                    row.allocated_total,
                    age_from_mod_time_min(row.mod_time_min)
                );
            }

            let mut file_rows: Vec<&swamp_core::report::FileRow> = files_by_worktree
                .get(&worktree.worktree_id)
                .map(|v| {
                    v.iter()
                        .filter(|f| f.growth_bytes.unwrap_or(0) > 0)
                        .collect()
                })
                .unwrap_or_default();
            file_rows.sort_by_key(|a| std::cmp::Reverse(a.growth_bytes));
            for f in file_rows {
                let _ = writeln!(
                    out,
                    "    large file {:<38} {:>12} bytes (+{} bytes)  changed {}",
                    f.rel_path,
                    f.allocated,
                    f.growth_bytes.unwrap_or(0),
                    age_from_mod_time_min(f.mod_time_min)
                );
            }
        }
    }
    Ok(out)
}

/// A rough human age string ("3d", "5h", "12m", "just now") from minutes
/// since the Unix epoch, relative to now.
fn age_from_mod_time_min(mod_time_min: i32) -> String {
    let now_min = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        / 60) as i64;
    let age_min = (now_min - mod_time_min as i64).max(0);
    if age_min < 1 {
        "just now".to_string()
    } else if age_min < 60 {
        format!("{age_min}m")
    } else if age_min < 60 * 24 {
        format!("{}h", age_min / 60)
    } else {
        format!("{}d", age_min / (60 * 24))
    }
}

/// A one-line stderr progress readout while a walk runs: bytes and
/// directories seen so far from `walk::progress`, redrawn in place ten
/// times a second, erased when done. Off when stderr is not a terminal or
/// the caller wants machine output.
struct ProgressLine {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ProgressLine {
    fn stop(mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn spawn_progress_line(enabled: bool) -> ProgressLine {
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    if !enabled {
        return ProgressLine { stop, handle: None };
    }
    let flag = stop.clone();
    let handle = std::thread::spawn(move || {
        use std::io::Write;
        let mut drew = false;
        while !flag.load(std::sync::atomic::Ordering::Relaxed) {
            let (bytes, dirs, active) = swamp_core::walk::progress::snapshot();
            if active {
                let _ = write!(
                    std::io::stderr(),
                    "\r\x1b[2Kobserving… {} · {dirs} dirs",
                    swamp_core::render::human_bytes_pub(bytes)
                );
                drew = true;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if drew {
            let _ = write!(std::io::stderr(), "\r\x1b[2K");
        }
        let _ = std::io::stderr().flush();
    });
    ProgressLine {
        stop,
        handle: Some(handle),
    }
}

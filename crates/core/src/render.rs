//! Text rendering for [`crate::report::Report`].
//!
//! Signals only, never verdict vocabulary ("safe", "needs reconciliation", "unused",
//! "abandoned", ...). Output is column-aligned ASCII that fits 100 cols
//! and needs no terminal color support.

use crate::report::{ArtifactKind, Report, UnownedReason, WorktreeKind};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

/// Default number of project rows shown in the overview before folding
/// the rest into a "… and N more" footer.
const DEFAULT_TOP_N: usize = 25;

/// Decimal (SI, ÷1000) so a "47.9GB" this renderer prints means the same
/// 47.9 * 10^9 a raw byte count means everywhere else (e.g. `walked_total`
/// printed verbatim in the `observe` log line, or read out of `report
/// --json`). Before this fix the divisor was 1024 (binary/GiB) under a
/// decimal ("GB") label, so the *same* observation could read "51.3GB"
/// from one surface (the raw integer) and "47.9GB" from this one -- a
/// 1024-vs-1000 unit mismatch masquerading as stale/re-read data.
/// A project's display name: `owner/repo` when its remote names one, so
/// two clones of different repos sharing a basename are told apart on
/// sight. Shared by the TUI and CLI (including its `--json` output).
pub fn project_display_name(p: &crate::report::ProjectRow) -> String {
    let owner_repo = p.remote.as_deref().and_then(|r| {
        let parts: Vec<&str> = r.trim_end_matches('/').split('/').collect();
        (parts.len() >= 3).then(|| format!("{}/{}", parts[parts.len() - 2], parts[parts.len() - 1]))
    });
    match owner_repo {
        Some(or) if or.to_lowercase().ends_with(&p.name.to_lowercase()) => or,
        _ => p.name.clone(),
    }
}

/// The one byte formatter in this product (decimal, SI-labelled). The TUI
/// re-exports it; a second implementation is a defect (source audit).
pub fn human_bytes_pub(bytes: u64) -> String {
    human_bytes(bytes)
}

/// Signed human units for growth/delta figures: `+1.2GB`, `-300.0MB`, `0B`.
pub fn human_bytes_signed(delta: i64) -> String {
    if delta == 0 {
        return "0B".into();
    }
    let sign = if delta < 0 { "-" } else { "+" };
    format!("{sign}{}", human_bytes(delta.unsigned_abs()))
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KB", "MB", "GB", "TB", "PB", "EB"];
    if bytes < 1_000 {
        return format!("{bytes}B");
    }

    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1_000.0 && unit < UNITS.len() - 1 {
        value /= 1_000.0;
        unit += 1;
    }
    if unit == 1 {
        // Whole KB avoids false precision in the range where counts move
        // quickly. Carry a rounded boundary into MB instead of printing
        // the misleading `1000KB`.
        let rounded = value.round() as u64;
        if rounded >= 1_000 {
            return "1.0MB".to_string();
        }
        return format!("{rounded}KB");
    }
    let rounded_tenths = (value * 10.0).round() / 10.0;
    if rounded_tenths >= 1_000.0 && unit < UNITS.len() - 1 {
        return format!("1.0{}", UNITS[unit + 1]);
    }
    format!("{rounded_tenths:.1}{}", UNITS[unit])
}

/// An exact count with thousands separators. Unlike a byte measure, a
/// count should not be rounded or abbreviated.
pub fn human_count(count: u64) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn human_signed_bytes(delta: i64) -> String {
    let sign = if delta > 0 {
        "+"
    } else if delta < 0 {
        "-"
    } else {
        ""
    };
    format!("{sign}{}", human_bytes(delta.unsigned_abs()))
}

pub fn allocation_note(row: &crate::report::ArtifactRow) -> String {
    if !row.dedup_stale {
        return String::new();
    }
    let allocated = row
        .allocated_bytes
        .map(human_bytes)
        .unwrap_or_else(|| "unknown".into());
    let growth = row
        .allocated_growth_bytes
        .map(|g| format!("; allocated growth {}", human_signed_bytes(g)))
        .unwrap_or_default();
    format!(" [unique not recomputed; allocated {allocated}{growth}]")
}

pub fn kind_label(kind: &ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::BuildOutput => "build",
        ArtifactKind::DependencyTree => "deps",
        ArtifactKind::Git => "git",
        ArtifactKind::Cache => "cache",
        ArtifactKind::Source => "source",
        ArtifactKind::Ignored => "ignored",
        ArtifactKind::Untracked => "untracked",
        ArtifactKind::DockerImage => "docker-image",
        ArtifactKind::DockerBuildCache => "docker-cache",
        ArtifactKind::DockerVolume => "docker-volume",
        ArtifactKind::Loose => "loose",
        ArtifactKind::Unknown => "unknown",
    }
}

pub fn reason_label(reason: &UnownedReason) -> &'static str {
    match reason {
        UnownedReason::OutsideAnyCheckout => "outside-any-checkout",
        UnownedReason::OwnedByNothing => "owned-by-nothing",
        UnownedReason::InconclusiveEvidence => "inconclusive-evidence",
        UnownedReason::NoContainingRepo => "no-containing-repo",
        UnownedReason::SharedCache => "shared-cache",
        UnownedReason::PermissionDenied => "permission-denied",
        UnownedReason::NotMeasured => "not-measured",
        UnownedReason::StandaloneCargoTarget => "standalone-cargo-target",
        UnownedReason::DockerNoJoin => "docker-no-join",
    }
}

struct ProjectTotals {
    bytes: u64,
    growth: Option<i64>,
    worktrees: usize,
    top_kind: Option<(ArtifactKind, u64)>,
}

fn project_totals(project: &crate::report::ProjectRow) -> ProjectTotals {
    let mut bytes = 0u64;
    let mut growth: Option<i64> = None;
    let mut have_growth = false;
    let mut kind_bytes: BTreeMap<String, (ArtifactKind, u64)> = BTreeMap::new();
    for wt in &project.worktrees {
        for a in &wt.artifacts {
            bytes += a.bytes;
            if let Some(g) = a.growth_bytes {
                have_growth = true;
                growth = Some(growth.unwrap_or(0) + g);
            }
            let entry = kind_bytes
                .entry(kind_label(&a.kind).to_string())
                .or_insert((a.kind.clone(), 0));
            entry.1 += a.bytes;
        }
    }
    let top_kind = kind_bytes
        .into_values()
        .max_by_key(|(_, b)| *b)
        .filter(|(_, b)| *b > 0);
    ProjectTotals {
        bytes,
        growth: if have_growth { growth } else { None },
        worktrees: project.worktrees.len(),
        top_kind,
    }
}

fn header(report: &Report, verify_du: bool) -> String {
    let mut out = String::new();
    let worktree_count: usize = report.projects.iter().map(|p| p.worktrees.len()).sum();
    let _ = writeln!(out, "root: {}", report.root.display());
    if report.unowned.iter().any(|u| {
        u.measurement
            .is_some_and(|m| m.unique_needs_reconciliation())
    }) {
        out.push_str(
            "unowned unique-byte estimates not reconciled; swamp observe --full to reconcile\n",
        );
    }
    if let Some(u) = &report.reconciliation.unique_estimate {
        let _ = writeln!(
            out,
            "filesystem unique: {} ({}, last reconciled {}) · not a reclaimable-space estimate",
            human_bytes(u.bytes),
            if u.needs_reconciliation {
                "needs reconciliation"
            } else {
                "reconciled"
            },
            timestamp_label(u.reconciled_at)
        );
    }
    let _ = writeln!(
        out,
        "observed {} · {} projects · {} worktrees",
        timestamp_label(report.observed_at),
        human_count(report.projects.len() as u64),
        human_count(worktree_count as u64)
    );
    let _ = write!(
        out,
        "filesystem: {} attributed · {} unowned · {} walked",
        human_bytes(report.reconciliation.attributed),
        human_bytes(report.reconciliation.unowned),
        human_bytes(report.reconciliation.walked_total)
    );
    if verify_du {
        let _ = write!(
            out,
            " · du verification {}",
            report
                .reconciliation
                .du_total
                .map(human_bytes)
                .unwrap_or_else(|| "not measured (use observe --verify-du)".to_string())
        );
    }
    let _ = writeln!(out);
    if let Some(line) = &report.schedule_line {
        let _ = writeln!(out, "{line}");
    }
    out
}

/// A persisted Unix timestamp shown as a calendar date and time.
fn timestamp_label(secs: u64) -> String {
    if secs == 0 {
        return "time unknown".to_string();
    }
    format!(
        "{} {:02}:{:02} UTC",
        crate::schedule::utc_date(secs),
        (secs % 86_400) / 3_600,
        (secs % 3_600) / 60
    )
}

/// The zero-flag, one-screen overview: header, then one line per project
/// sorted by growth desc then bytes desc, capped at `top_n` rows (0 means
/// no cap) with a "… and N more" footer.
/// How `render_overview` orders project rows. `Growth` is the default the
/// tool always had (growth desc, then bytes); the rest mirror the TUI's
/// `g/s/n/t/a` keys and clean-dev-dirs' `--sort size|age|name|type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverviewSort {
    #[default]
    Growth,
    Size,
    Name,
    Type,
    Age,
}

impl std::str::FromStr for OverviewSort {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "growth" => OverviewSort::Growth,
            "size" | "bytes" => OverviewSort::Size,
            "name" => OverviewSort::Name,
            "type" => OverviewSort::Type,
            "age" => OverviewSort::Age,
            other => {
                return Err(format!(
                    "unknown sort {other:?}; use growth|size|name|type|age"
                ));
            }
        })
    }
}

pub fn render_overview(
    report: &Report,
    show_all: bool,
    verify_du: bool,
    show_docker: bool,
) -> String {
    render_overview_sorted(
        report,
        show_all,
        verify_du,
        show_docker,
        OverviewSort::Growth,
        false,
    )
}

/// `--view types`: one line per ecosystem from `Report.summary`.
pub fn render_types(report: &Report) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<6} {:<14} {:>9} {:>10} {:>12} {:>10}",
        "type", "name", "projects", "artifacts", "bytes", "growth"
    );
    let mut rows: Vec<_> = report.summary.by_type.iter().collect();
    rows.sort_by_key(|a| std::cmp::Reverse(a.1.bytes));
    if rows.is_empty() {
        let _ = writeln!(
            out,
            "No ecosystem totals were recorded in this observation."
        );
        return out;
    }
    for (tag, t) in rows {
        let _ = writeln!(
            out,
            "{:<6} {:<14} {:>9} {:>10} {:>12} {:>10}",
            tag,
            t.name,
            human_count(t.projects as u64),
            human_count(t.artifacts as u64),
            human_bytes(t.bytes),
            t.growth_bytes
                .map(human_signed_bytes)
                .unwrap_or_else(|| "—".to_string())
        );
    }
    out
}

pub fn render_overview_sorted(
    report: &Report,
    show_all: bool,
    verify_du: bool,
    show_docker: bool,
    sort: OverviewSort,
    reverse: bool,
) -> String {
    let mut out = header(report, verify_du);
    let _ = writeln!(out);

    if report.projects.is_empty() {
        let _ = writeln!(
            out,
            "No projects were discovered under {}. Check the effective roots with `swamp scope` or add a source root with `swamp config add-root <path>`.",
            report.root.display()
        );
        return out;
    }

    let mut rows: Vec<(&crate::report::ProjectRow, ProjectTotals)> = report
        .projects
        .iter()
        .map(|p| (p, project_totals(p)))
        .collect();
    let type_rank = |p: &crate::report::ProjectRow| {
        p.ecosystems
            .first()
            .and_then(|t| crate::ecosystem::ECOSYSTEMS.iter().position(|e| e.tag == t))
            .unwrap_or(usize::MAX)
    };
    let age_key = |p: &crate::report::ProjectRow| {
        let m = p
            .worktrees
            .iter()
            .flat_map(|w| w.artifacts.iter())
            .map(|a| a.mtime_max)
            .max()
            .unwrap_or(0);
        if m == 0 { u64::MAX } else { m }
    };
    match sort {
        OverviewSort::Growth => rows.sort_by(|a, b| {
            let ga = a.1.growth.unwrap_or(i64::MIN);
            let gb = b.1.growth.unwrap_or(i64::MIN);
            gb.cmp(&ga).then_with(|| b.1.bytes.cmp(&a.1.bytes))
        }),
        OverviewSort::Size => rows.sort_by_key(|a| std::cmp::Reverse(a.1.bytes)),
        OverviewSort::Name => rows.sort_by_key(|a| a.0.name.to_lowercase()),
        OverviewSort::Type => rows.sort_by(|a, b| {
            type_rank(a.0)
                .cmp(&type_rank(b.0))
                .then_with(|| b.1.bytes.cmp(&a.1.bytes))
        }),
        OverviewSort::Age => rows.sort_by_key(|a| age_key(a.0)),
    }
    if reverse {
        rows.reverse();
    }

    let _ = writeln!(
        out,
        "{:<10} {:<28} {:>10} {:>10} {:>4} {:<20}",
        "type", "project", "bytes", "growth", "wts", "top kind"
    );
    let total = rows.len();
    let limit = if show_all { total } else { DEFAULT_TOP_N };
    for (project, totals) in rows.iter().take(limit) {
        let growth_str = totals
            .growth
            .map(human_signed_bytes)
            .unwrap_or_else(|| "—".to_string());
        let top_kind_str = totals
            .top_kind
            .as_ref()
            .map(|(k, b)| format!("{} ({})", kind_label(k), human_bytes(*b)))
            .unwrap_or_else(|| "—".to_string());
        let bytes_str = if totals.bytes == 0 {
            "0B".to_string()
        } else {
            human_bytes(totals.bytes)
        };
        let _ = writeln!(
            out,
            "{:<10} {:<28} {:>10} {:>10} {:>4} {:<20}",
            crate::ecosystem::tags(&project.ecosystems),
            project.name,
            bytes_str,
            growth_str,
            totals.worktrees,
            top_kind_str
        );
    }
    if !show_all && total > limit {
        let _ = writeln!(
            out,
            "… and {} more (use --all)",
            human_count((total - limit) as u64)
        );
    }
    let _ = writeln!(out);
    render_unowned_summary(report, &mut out, show_docker);
    out
}

/// `--docker` shows every unjoined Docker object individually; by default
/// they fold into one line per object kind (a real multi-project `~/src`
/// scan can have hundreds of unjoined build-cache entries, which used to
/// print one row each here).
fn render_unowned_summary(report: &Report, out: &mut String, show_docker: bool) {
    if report.unowned.iter().any(|u| {
        u.measurement
            .is_some_and(|m| m.unique_needs_reconciliation())
    }) {
        out.push_str(
            "unowned unique-byte estimates need reconciliation; use swamp observe --full\n",
        );
    }
    // Aggregate filesystem rows by their top-level directory *relative to
    // root* (never an absolute-path segment like `Users`, which every row
    // shares and which says nothing about where the bytes live) and by
    // reason; shared caches are listed separately. Docker objects are not
    // filesystem paths at all and are aggregated by kind instead. Never
    // per-file/per-object rows in the default summary.
    #[derive(Default)]
    struct UnownedTotal {
        bytes: u64,
        not_measured: bool,
    }
    impl UnownedTotal {
        fn add(&mut self, row: &crate::report::UnownedRow) {
            self.bytes = self.bytes.saturating_add(row.bytes);
            self.not_measured |= matches!(
                &row.reason,
                UnownedReason::PermissionDenied | UnownedReason::NotMeasured
            );
        }

        fn label(&self) -> String {
            match (self.bytes, self.not_measured) {
                (0, true) => "not measured".to_string(),
                (bytes, true) => format!("at least {} + not measured", human_bytes(bytes)),
                (bytes, false) => human_bytes(bytes),
            }
        }
    }

    let mut by_dir: BTreeMap<String, UnownedTotal> = BTreeMap::new();
    let mut by_reason: BTreeMap<&'static str, UnownedTotal> = BTreeMap::new();
    let mut shared_caches_bytes = 0u64;
    let mut shared_caches_count = 0u64;
    let mut docker_by_kind: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut docker_rows: Vec<&crate::report::UnownedRow> = Vec::new();

    for row in &report.unowned {
        if row.reason == UnownedReason::DockerNoJoin {
            let kind = row
                .docker_kind
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            let entry = docker_by_kind.entry(kind).or_insert((0, 0));
            entry.0 += 1;
            entry.1 += row.bytes;
            docker_rows.push(row);
            continue;
        }
        if row.reason == UnownedReason::SharedCache {
            shared_caches_bytes += row.bytes;
            shared_caches_count += 1;
            continue;
        }
        let rel = Path::new(&row.path_or_object)
            .strip_prefix(&report.root)
            .unwrap_or_else(|_| Path::new(&row.path_or_object));
        // Under the first root: its top-level directory. Under another
        // root of a multi-root scope (`strip_prefix` failed, `rel` is the
        // whole path): the row's own parent directory -- never "/", which
        // the aim review saw as a meaningless 94 GB row.
        let top_dir = if rel.is_absolute() {
            rel.parent()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| row.path_or_object.clone())
        } else {
            rel.components()
                .next()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .unwrap_or_else(|| row.path_or_object.clone())
        };
        by_dir.entry(top_dir).or_default().add(row);
        by_reason
            .entry(reason_label(&row.reason))
            .or_default()
            .add(row);
    }
    let _ = writeln!(out, "unowned by top-level dir:");
    if by_dir.is_empty() {
        let _ = writeln!(out, "  (none)");
    } else {
        for (dir, bytes) in &by_dir {
            let _ = writeln!(out, "  {:<30} {:>10}", dir, bytes.label());
        }
    }
    let _ = writeln!(out, "unowned by reason:");
    if by_reason.is_empty() {
        let _ = writeln!(out, "  (none)");
    } else {
        for (reason, bytes) in &by_reason {
            let _ = writeln!(out, "  {:<30} {:>10}", reason, bytes.label());
        }
    }
    if shared_caches_count > 0 {
        let _ = writeln!(
            out,
            "shared caches: {} ({} items)",
            human_bytes(shared_caches_bytes),
            human_count(shared_caches_count)
        );
    }
    if !docker_by_kind.is_empty() {
        let _ = writeln!(out, "docker unowned:");
        for (kind, (count, bytes)) in &docker_by_kind {
            let _ = writeln!(
                out,
                "  {:<30} {:>10} ({} items)",
                kind,
                human_bytes(*bytes),
                human_count(*count)
            );
        }
        if !show_docker {
            let _ = writeln!(out, "  (use --docker to list each object)");
        }
    }
    if show_docker {
        for row in docker_rows {
            let _ = writeln!(
                out,
                "  {:<40} {:>10}",
                row.path_or_object,
                human_bytes(row.bytes)
            );
        }
    }
}

fn worktree_kind_label(kind: &WorktreeKind) -> &'static str {
    match kind {
        WorktreeKind::Main => "main",
        WorktreeKind::Linked => "linked",
        WorktreeKind::Clone => "clone",
    }
}

/// `--project <name>` drill: worktree → kind → path → bytes → growth →
/// regrowth → signals.
///
/// Worktree identity is shown as its path relative to the report root
/// plus a short 8-char id (the full 64-hex `worktree_id` is noise on a
/// terminal screen and never needed to tell rows apart here); a
/// duplicate relative path -- two truly distinct identities that happen
/// to render the same, which the 8-char id then disambiguates -- keeps
/// the full path as a fallback suffix.
pub fn render_project(report: &Report, name: &str) -> Option<String> {
    let project = report.projects.iter().find(|p| p.name == name)?;
    let mut out = String::new();
    let _ = writeln!(out, "project: {}", project.name);
    if project.worktrees.is_empty() {
        let _ = writeln!(out, "  (no worktrees)");
        return Some(out);
    }
    for wt in &project.worktrees {
        let kind = worktree_kind_label(&wt.kind);
        let rel = wt
            .path
            .strip_prefix(&report.root)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| wt.path.display().to_string());
        let short_id = &wt.worktree_id[..wt.worktree_id.len().min(8)];
        let _ = writeln!(out, "worktree: {rel} ({short_id}) [{kind}]");
        for a in &wt.artifacts {
            let growth_str = a
                .growth_bytes
                .map(human_signed_bytes)
                .unwrap_or_else(|| "—".to_string());
            let _ = writeln!(
                out,
                "  {:<14} {:<40} {:>10} {:>10} regrowth={}",
                kind_label(&a.kind),
                a.path.display(),
                human_bytes(a.bytes),
                growth_str,
                human_count(a.regrowth_count as u64),
            );
        }
        if wt.artifacts.is_empty() {
            let _ = writeln!(out, "  no artifact rows were recorded for this worktree");
        }
        if !wt.signals.is_empty() {
            let signals = wt
                .signals
                .iter()
                .map(|s| format!("{}: {}", s.name, s.value))
                .collect::<Vec<_>>()
                .join(" · ");
            let _ = writeln!(out, "  signals: {signals}");
        }
    }
    Some(out)
}

/// `--kinds`: bytes and count per artifact kind across the root.
pub fn render_kinds(report: &Report) -> String {
    let mut out = String::new();
    let mut agg: BTreeMap<&'static str, (u64, u64)> = BTreeMap::new();
    for project in &report.projects {
        for wt in &project.worktrees {
            for a in &wt.artifacts {
                let entry = agg.entry(kind_label(&a.kind)).or_insert((0, 0));
                entry.0 += a.bytes;
                entry.1 += 1;
            }
        }
    }
    let _ = writeln!(out, "{:<16} {:>12} {:>8}", "kind", "bytes", "count");
    if agg.is_empty() {
        let _ = writeln!(out, "No artifact kinds were recorded in this observation.");
        return out;
    }
    let mut rows: Vec<(&str, (u64, u64))> = agg.into_iter().collect();
    rows.sort_by_key(|a| std::cmp::Reverse(a.1.0));
    for (kind, (bytes, count)) in rows {
        let _ = writeln!(
            out,
            "{:<16} {:>12} {:>8}",
            kind,
            human_bytes(bytes),
            human_count(count)
        );
    }
    out
}

/// `--view worktrees [--filter '...']`: one line per matching worktree,
/// with the literal human command to remove it. Never executed by this
/// tool -- the text is printed for a person to run themselves.
pub fn render_worktrees(report: &Report, filter: &crate::filter::Filter) -> String {
    use crate::github::{GithubFacts, MergeComplete, PrStatus, TriState};
    let mut out = String::new();
    let mut shown = 0usize;
    let empty_pr = PrStatus::Unknown;
    for project in &report.projects {
        for wt in &project.worktrees {
            let (pr, verdict, merge_complete_terms) = match (&wt.github, &wt.merge_complete) {
                (
                    Some(GithubFacts { pull_request, .. }),
                    Some(MergeComplete { verdict, terms }),
                ) => (pull_request, *verdict, Some(terms.clone())),
                (Some(GithubFacts { pull_request, .. }), None) => {
                    (pull_request, TriState::Unknown, None)
                }
                (None, _) => (&empty_pr, TriState::Unknown, None),
            };

            let facts = crate::filter::WorktreeFacts {
                merge_complete: verdict == TriState::Yes,
                idle_secs: wt.idle_secs,
                pr,
            };
            if !filter.matches_worktree(project, wt, &facts) {
                continue;
            }
            shown += 1;

            let branch = wt
                .branch
                .clone()
                .unwrap_or_else(|| "(detached)".to_string());
            let idle_str = wt
                .idle_secs
                .map(|s| format!("idle {}", human_duration(s)))
                .unwrap_or_else(|| "idle unknown".to_string());
            let verdict_str = match verdict {
                TriState::Yes => "yes",
                TriState::No => "no",
                TriState::Unknown => "unknown",
            };
            let mc_str = match merge_complete_terms {
                Some(terms) => format!("merge-complete: {verdict_str} ({})", terms.join(", ")),
                None => "merge-complete: unknown (no GitHub remote)".to_string(),
            };
            let pr_str = render_pr(pr);

            let _ = writeln!(
                out,
                "{}  {}  branch={}  {}  {}  {}",
                project.name,
                wt.path.display(),
                branch,
                idle_str,
                mc_str,
                pr_str,
            );
            match wt.kind {
                WorktreeKind::Linked => {
                    let _ = writeln!(out, "  git worktree remove {}", wt.path.display());
                }
                WorktreeKind::Main | WorktreeKind::Clone => {
                    let _ = writeln!(out, "  main checkout -- not removable as a worktree");
                }
            }
        }
    }
    if shown == 0 {
        let _ = writeln!(
            out,
            "No worktrees match this filter. Broaden or remove `--filter`, or check the effective roots with `swamp scope`."
        );
    }
    out
}

/// A coarse worktree idle duration.
fn human_duration(secs: u64) -> String {
    if secs < 60 {
        "under 1m".to_string()
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

fn render_pr(pr: &crate::github::PrStatus) -> String {
    use crate::github::{PrState, PrStatus, ReviewDecision};
    match pr {
        PrStatus::None => "no PR".to_string(),
        PrStatus::Unknown => "PR unknown".to_string(),
        PrStatus::Some(pr) => {
            let state = match pr.state {
                PrState::Open => "open",
                PrState::Closed => "closed",
                PrState::Merged => "merged",
            };
            let decision = match pr.review_decision {
                ReviewDecision::Approved => Some("approved"),
                ReviewDecision::ChangesRequested => Some("changes requested"),
                ReviewDecision::ReviewRequired => Some("review required"),
                ReviewDecision::None | ReviewDecision::Unknown => None,
            };
            match decision {
                Some(d) => format!("PR #{} {state} ({d})", pr.number),
                None => format!("PR #{} {state}", pr.number),
            }
        }
    }
}

/// Legacy flat renderer kept for the golden test's exact-format
/// expectations (R2-R5 fixture output); the CLI's default surface is
/// [`render_overview`].
pub fn render_text(report: &Report) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "root: {}", report.root.display());
    let _ = writeln!(out, "observed: {}", timestamp_label(report.observed_at));
    let _ = writeln!(out);
    if report.projects.is_empty() {
        let _ = writeln!(
            out,
            "No projects were discovered under {}. Check the effective roots with `swamp scope` or add a source root with `swamp config add-root <path>`.",
            report.root.display()
        );
        return out;
    }
    let _ = writeln!(
        out,
        "{:<12} {:<10} {:<10} {:<8} {:<40} {:>12} {:>12} {:>8}",
        "project", "worktree", "kind", "artifact", "path", "bytes", "growth", "regrowth"
    );
    for project in &report.projects {
        for worktree in &project.worktrees {
            let kind = worktree_kind_label(&worktree.kind);
            for artifact in &worktree.artifacts {
                let _ = writeln!(
                    out,
                    "{:<12} {:<10} {:<10} {:<8} {:<40} {:>12} {:>12} {:>8}",
                    project.name,
                    &worktree.worktree_id[..worktree.worktree_id.len().min(10)],
                    kind,
                    kind_label(&artifact.kind),
                    artifact.path.display(),
                    human_bytes(artifact.bytes),
                    artifact
                        .growth_bytes
                        .map(human_signed_bytes)
                        .unwrap_or_else(|| "-".to_string()),
                    human_count(artifact.regrowth_count as u64),
                );
            }
            if !worktree.signals.is_empty() {
                let signals = worktree
                    .signals
                    .iter()
                    .map(|s| format!("{}={}", s.name, s.value))
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = writeln!(out, "  signals[{}]: {}", worktree.worktree_id, signals);
            }
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{:<40} {:>12} {:<20}",
        "unowned path/object", "bytes", "reason"
    );
    for row in &report.unowned {
        let _ = writeln!(
            out,
            "{:<40} {:>12} {:<20}",
            row.path_or_object,
            human_bytes(row.bytes),
            reason_label(&row.reason)
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "reconciliation: attributed={} unowned={} walked_total={} du_total={}",
        report.reconciliation.attributed,
        report.reconciliation.unowned,
        report.reconciliation.walked_total,
        report
            .reconciliation
            .du_total
            .map(|v| v.to_string())
            .unwrap_or_else(|| "n/a".to_string()),
    );
    out
}

/// `--project <name>` (or `--project <name> --view worktrees`, the
/// default view): the project's tree, checkouts/worktrees down to
/// (folded) artifact rows, per #33. Replaces the old flat
/// `render_project` drill; paths on every row are relative (never
/// absolute, which the flat drill used to leak), and a signal is
/// printed as its value only, never `name: name: value`.
pub fn render_project_tree(report: &Report, name: &str) -> Option<String> {
    render_project_tree_with_agents(report, name, &[])
}

/// Same as [`render_project_tree`], additionally rendering the
/// collapsed "Agent storage (linked)" row(s) #100 requires when
/// `agent_units` names any unit linked to this project. Callers that
/// have not computed agent-storage units get identical output to
/// `render_project_tree`'s (an empty slice never adds a row).
pub fn render_project_tree_with_agents(
    report: &Report,
    name: &str,
    agent_units: &[crate::agents::AgentUnit],
) -> Option<String> {
    let project = report.projects.iter().find(|p| p.name == name)?;
    let tree = crate::tree::build_project_tree(project, &report.root, agent_units);
    let mut out = String::new();
    let growth_str = tree
        .growth_bytes
        .map(|g| {
            // `human_signed_bytes` already carries the sign. Stripping a
            // leading `+` and then hardcoding one printed `+-18.9MB` for
            // a shrink; only an exact zero needs a sign added.
            let signed = human_signed_bytes(g);
            let signed = if g == 0 { format!("+{signed}") } else { signed };
            format!(" ({signed}/24h)")
        })
        .unwrap_or_default();
    let _ = writeln!(
        out,
        "{}  {}{}",
        tree.name,
        human_bytes(tree.bytes),
        growth_str
    );
    if tree.worktrees.is_empty() {
        let _ = writeln!(out, "  (no worktrees)");
        write_agent_rows(&mut out, &tree.agent_rows);
        return Some(out);
    }
    let last_idx = tree.worktrees.len() - 1;
    for (i, wt) in tree.worktrees.iter().enumerate() {
        let branch = if i == last_idx { "└─" } else { "├─" };
        let child_prefix = if i == last_idx { "   " } else { "│  " };
        let kind = worktree_kind_label(&wt.kind);
        let growth = wt
            .growth_bytes
            .map(human_signed_bytes)
            .unwrap_or_else(|| "—".to_string());
        let signals = if wt.signals.is_empty() {
            String::new()
        } else {
            format!(
                "   {}",
                wt.signals
                    .iter()
                    .map(|s| s.value.clone())
                    .collect::<Vec<_>>()
                    .join(" · ")
            )
        };
        let _ = writeln!(
            out,
            "{branch} {:<8} {:<28} {:>10}  ({growth})   {signals}",
            kind,
            wt.rel_path,
            human_bytes(wt.bytes),
        );
        let row_last = wt.rows.len().checked_sub(1);
        for (j, row) in wt.rows.iter().enumerate() {
            let row_branch = if Some(j) == row_last {
                "└─"
            } else {
                "├─"
            };
            let growth = row
                .growth_bytes
                .map(human_signed_bytes)
                .unwrap_or_else(|| "—".to_string());
            let label = if row.folded_count > 1 {
                format!(
                    "{} (x{})",
                    row.rel_path,
                    human_count(row.folded_count as u64)
                )
            } else {
                row.rel_path.clone()
            };
            let _ = writeln!(
                out,
                "{child_prefix}{row_branch} {:<10} {:<30} {:>10}  ({growth})",
                row.kind_label,
                label,
                human_bytes(row.bytes),
            );
        }
    }
    write_agent_rows(&mut out, &tree.agent_rows);
    Some(out)
}

/// Appends one collapsed line per tool contributing linked agent
/// storage to this project (#100), or nothing when `rows` is empty --
/// absence of agent-storage linkage is never rendered as a zero row.
/// This is a summary only: per-session/transcript detail stays in
/// `render_view_agents`/the TUI Agents view, never duplicated here.
fn write_agent_rows(out: &mut String, rows: &[crate::tree::ProjectAgentToolRow]) {
    if rows.is_empty() {
        return;
    }
    let _ = writeln!(out, "Agent storage (linked):");
    let last = rows.len() - 1;
    for (i, row) in rows.iter().enumerate() {
        let branch = if i == last { "└─" } else { "├─" };
        let growth = row
            .growth_bytes
            .map(human_signed_bytes)
            .unwrap_or_else(|| "—".to_string());
        let count = if row.unit_count == 1 {
            "1 unit".to_string()
        } else {
            format!("{} units", human_count(row.unit_count as u64))
        };
        let _ = writeln!(
            out,
            "{branch} {:<20} {:>10}  ({growth})   {count}",
            row.tool_name,
            human_bytes(row.bytes),
        );
    }
}

/// `--view builds`: every `BuildOutput`/`Cache` row across the project
/// (or the whole root when `only_project` is `None`), sorted by bytes
/// desc.
pub fn render_view_builds(report: &Report, only_project: Option<&str>) -> String {
    let mut out = String::from(
        "Build bytes are inode-deduplicated charges; Rust drilldown uses path allocations, which may count hardlinks more than once. Neither guarantees reclaimed space.\n",
    );
    out.push_str(&render_kind_view(
        report,
        only_project,
        &[ArtifactKind::BuildOutput, ArtifactKind::Cache],
    ));
    out.push_str(&render_build_containers(report, only_project));
    out
}

/// The build-adapter drill-down under `--view builds`: for each
/// identified container, one collapsed row per role family.
///
/// The ordering is what the row is *for*. A person looking at a 24 GiB
/// `target/` wants to know which kind of thing it is made of before they
/// want the numbers, so the family and its consequence come first and
/// the count/size/oldest follow. Every row states its accounting basis,
/// because #65 forbids adding an allocated number to a logical one and
/// the only way to honour that outside this function is to print which
/// is which.
///
/// Absence is never rendered as a zero row: a report with no identified
/// containers prints nothing here, and a family with no members prints
/// no line.
fn render_build_containers(report: &Report, only_project: Option<&str>) -> String {
    let mut out = String::new();
    if report.nested_artifacts.is_empty() {
        return out;
    }
    // Group by container, keeping the container's own row aside: it is
    // the total the families sit inside, not one of them.
    let mut containers: std::collections::BTreeMap<&str, Vec<&crate::artifact::NestedArtifact>> =
        std::collections::BTreeMap::new();
    for u in &report.nested_artifacts {
        let Some(id) = u.container_id.as_deref() else {
            continue;
        };
        // A daemon's records are `--view docker`'s, not a build
        // directory's interior.
        if u.reported_by.is_some() {
            continue;
        }
        let project = report
            .projects
            .iter()
            .find(|p| p.worktrees.iter().any(|w| u.path.starts_with(&w.path)));
        if let Some(wanted) = only_project
            && project.map(|p| p.name.as_str()) != Some(wanted)
        {
            continue;
        }
        containers.entry(id).or_default().push(u);
    }
    if containers.is_empty() {
        return out;
    }
    let _ = writeln!(
        out,
        "\nInside these build containers (identification only -- no cleanup is offered here):"
    );
    let mut sections: Vec<(u64, String)> = Vec::new();
    for units in containers.values() {
        let owned: Vec<crate::artifact::NestedArtifact> =
            units.iter().map(|u| (*u).clone()).collect();
        if let Some(section) = render_container_section(&owned, report.observed_at, "") {
            sections.push(section);
        }
    }
    sections.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (_, s) in sections {
        out.push_str(&s);
    }
    out
}

/// One container's interior as text: the container line, its limits,
/// one guidance + numbers pair per family, and the residual. The one
/// renderer `--view builds`, `--view external` and `--view docker`
/// share, so a Maven repository's interior reads exactly like a
/// `node_modules`'s. `None` when there is nothing to show. Returns the
/// container's bytes for ordering.
fn render_container_section(
    units: &[crate::artifact::NestedArtifact],
    observed_at: u64,
    indent: &str,
) -> Option<(u64, String)> {
    use crate::build_adapters::summarize_container;
    // The container's own row is the one whose id equals the container
    // id; everything else hangs off it.
    let root = units
        .iter()
        .find(|u| Some(u.id.as_str()) == u.container_id.as_deref())?;
    let summary = summarize_container(&root.path, units);
    let residual = summary.unsupported_bytes.unwrap_or(0) + summary.unaccounted_bytes.unwrap_or(0);
    if summary.families.is_empty() && summary.unsupported_count == 0 && residual == 0 {
        return None;
    }
    let mut section = String::new();
    let _ = writeln!(
        section,
        "\n{indent}{} ({}, {} {}{})",
        root.path.display(),
        root.adapter.clone().unwrap_or_else(|| "unknown".into()),
        human_bytes(root.bytes),
        root.basis.label(),
        // Two numbers describe one directory: the row above counts a
        // hardlinked file once, this one counts every link. Say which.
        if root.basis == crate::artifact::AccountingBasis::Allocated {
            "; each hardlink counted, the row above counts a shared file once"
        } else {
            ""
        }
    );
    for limit in &root.coverage.limits {
        let _ = writeln!(section, "{indent}  limit: {limit}");
    }
    for f in &summary.families {
        // Guidance and consequence first: that is the question. The
        // numbers follow on their own line.
        let consequence = match (&f.consequence, f.other_consequences) {
            (Some(c), 0) => c.clone(),
            (Some(c), 1) => format!("{c} (and 1 other consequence inside)"),
            (Some(c), n) => format!(
                "{c} (and {} other consequences inside)",
                human_count(n as u64)
            ),
            (None, _) => "consequence not established".to_string(),
        };
        let size = match f.basis {
            crate::artifact::AccountingBasis::Unknown => {
                "size mixed-basis (not summed)".to_string()
            }
            basis => format!("{} {}", human_bytes(f.bytes), basis.label()),
        };
        let time_word = if root.reported_by.is_some() {
            "oldest created (daemon record)"
        } else {
            "oldest modified"
        };
        let oldest = match f.oldest_modified {
            Some(t) => format!("{time_word} {}", age_label(t, observed_at)),
            None => "no known modification time".to_string(),
        };
        let unknowns = if f.unknown_age > 0 {
            format!(", {} of unknown age", human_count(f.unknown_age as u64))
        } else {
            String::new()
        };
        let _ = writeln!(
            section,
            "{indent}  {:<24} {} -- {consequence}",
            f.family.title(),
            f.recommendation
        );
        let _ = writeln!(
            section,
            "{indent}  {:<24} {} item(s), {size}, {oldest}{unknowns}{}; no cleanup rule",
            "",
            human_count(f.count as u64),
            if f.complete {
                ""
            } else {
                " (measurement incomplete)"
            }
        );
    }
    if summary.unsupported_count > 0 || residual > 0 {
        let _ = writeln!(
            section,
            "{indent}  {:<24} {} -- {} unrecognised entr{}, {}",
            crate::artifact::RoleFamily::Residual.title(),
            crate::build_adapters::family_guidance(crate::artifact::RoleFamily::Residual),
            human_count(summary.unsupported_count as u64),
            if summary.unsupported_count == 1 {
                "y"
            } else {
                "ies"
            },
            match summary.unaccounted_bytes {
                Some(b) => format!(
                    "{} {} in total, {} of it claimed by no unit",
                    human_bytes(residual),
                    root.basis.label(),
                    human_bytes(b)
                ),
                None => format!(
                    "{} {} unrecognised; the remainder of the container was not reconciled",
                    human_bytes(residual),
                    root.basis.label()
                ),
            }
        );
    }
    Some((root.bytes, section))
}

/// `--view docker`'s BuildKit section: each builder's records, in the
/// daemon's own terms. The family rows say what the records are and
/// cost; the record lines say which record, with the daemon's flags --
/// shared, in use, reclaimable -- and its parents, because a native
/// prune removes a record with its dependents. The host disk image that
/// holds these is `--view external`'s, never added here.
fn render_buildkit_records(report: &Report) -> String {
    let mut out = String::new();
    let mut builders: std::collections::BTreeMap<&str, Vec<crate::artifact::NestedArtifact>> =
        std::collections::BTreeMap::new();
    for u in report
        .nested_artifacts
        .iter()
        .filter(|u| u.reported_by.is_some())
    {
        if let Some(c) = u.container_id.as_deref() {
            builders.entry(c).or_default().push(u.clone());
        }
    }
    if builders.is_empty() {
        return out;
    }
    let _ = writeln!(
        out,
        "\nBuildKit build cache, as the daemon reports it (logical sizes, each record's own; \
         no cleanup rule):"
    );
    for units in builders.values() {
        if let Some((_, section)) = render_container_section(units, report.observed_at, "") {
            out.push_str(&section);
        }
        let mut records: Vec<&crate::artifact::NestedArtifact> = units
            .iter()
            .filter(|u| Some(u.id.as_str()) != u.container_id.as_deref())
            .collect();
        records.sort_by_key(|a| std::cmp::Reverse(a.bytes));
        for r in records.iter().take(25) {
            let flags: Vec<&str> = r
                .coverage
                .limits
                .iter()
                .filter_map(|l| {
                    if l.contains("in use") {
                        Some("in use")
                    } else if l.contains("reports this record shared") {
                        Some("shared")
                    } else {
                        None
                    }
                })
                .collect();
            let parents = r
                .producer_evidence
                .iter()
                .filter(|e| e.source == "buildkit-parent")
                .count();
            let description = r
                .producer_evidence
                .iter()
                .find(|e| e.source == "buildkit-description")
                .map(|e| e.detail.as_str())
                .unwrap_or("");
            let last_used = r
                .producer_evidence
                .iter()
                .find(|e| e.source == crate::build_adapters::LAST_USED_EVIDENCE)
                .map(|e| format!(" · last used {} (daemon)", e.detail))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "    {:<14} {:<18} {:>10}  created {}{last_used}{}{} {}",
                r.relative_path.chars().take(14).collect::<String>(),
                r.variant.configuration.as_deref().unwrap_or("unknown type"),
                human_bytes(r.bytes),
                age_label(r.mtime_max, report.observed_at),
                if flags.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", flags.join(", "))
                },
                if parents > 0 {
                    format!(" · {parents} parent(s)")
                } else {
                    String::new()
                },
                description
            );
        }
        if records.len() > 25 {
            let _ = writeln!(out, "    ... and {} more records", records.len() - 25);
        }
    }
    out
}

/// `--view deps`: every `DependencyTree` row across the project (or
/// root), sorted by bytes desc. Shared dependency caches that touch this
/// project surface as ordinary `DependencyTree` rows already joined at
/// discovery time; this view does not re-derive that join.
pub fn render_view_deps(report: &Report, only_project: Option<&str>) -> String {
    render_kind_view(report, only_project, &[ArtifactKind::DependencyTree])
}

pub fn render_view_rust_with_limit(
    report: &Report,
    only_project: Option<&str>,
    limit: Option<usize>,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Cargo storage: category totals include children; do not add a parent to its descendants. Disjoint paths can be summed as allocation, not guaranteed reclaimable space."
    );
    let _ = writeln!(
        out,
        "inspect-groups = category; unchecked = review required, not proven unused. Run `swamp ui` to inspect cleanup groups; Space marks a group for review."
    );
    let _ = writeln!(
        out,
        "{:<18} {:<20} {:<10} {:>10} {:>10} {:>10}  path / evidence",
        "project", "role", "profile", "allocated", "charged", "growth"
    );
    let mut rows = Vec::new();
    for unit in &report.nested_artifacts {
        let project = nested_artifact_project_name(report, unit).unwrap_or("unknown");
        if only_project.is_some_and(|wanted| wanted != project) {
            continue;
        }
        let profile = unit.variant.profile.clone().unwrap_or_else(|| "?".into());
        let evidence = unit
            .producer_evidence
            .first()
            .map(|e| e.source.as_str())
            .unwrap_or("unknown");
        rows.push((
            project.to_string(),
            unit.role.label(),
            profile,
            unit.bytes,
            unit.physical_total,
            unit,
            evidence,
        ));
    }
    rows.sort_by(|a, b| {
        b.3.cmp(&a.3)
            .then_with(|| a.5.relative_path.cmp(&b.5.relative_path))
    });
    if rows.is_empty() {
        if let Some(name) = only_project {
            let _ = writeln!(
                out,
                "No Cargo target rows or supported nested facts were recorded for project {name}."
            );
        } else {
            let _ = writeln!(
                out,
                "No Cargo target rows or supported nested facts were recorded."
            );
        }
        return out;
    }
    let count = rows.len();
    for (project, role, profile, bytes, charged, unit, evidence) in
        rows.into_iter().take(limit.unwrap_or(usize::MAX))
    {
        let unknown = if unit.variant.unknowns.is_empty() {
            String::new()
        } else {
            format!("; unknown: {}", unit.variant.unknowns.join(", "))
        };
        let _ = writeln!(
            out,
            "{:<18} {:<20} {:<10} {:>10} {:>10} {:>10}  {} · {} [{}{}]",
            project,
            role,
            profile,
            human_bytes(bytes),
            if matches!(
                unit.membership,
                crate::artifact::Membership::Unknown | crate::artifact::Membership::SharedHardlink
            ) {
                "—".into()
            } else {
                human_bytes(charged)
            },
            unit.growth_bytes
                .map(human_bytes_signed)
                .unwrap_or_else(|| "—".into()),
            match crate::cargo_cleanup::guidance(unit).next_action.as_str() {
                "inspect_groups" => "inspect-groups",
                "review_cleanup" => "unchecked",
                _ => "inspection-only",
            },
            unit.path.display(),
            evidence,
            unknown
        );
    }
    if limit.is_some_and(|n| count > n) {
        let _ = writeln!(
            out,
            "Showing {} of {} rows, largest first. Use --all for all rows, --json for structured cleanup guidance, or `swamp ui` to inspect cleanup groups.",
            human_count(limit.unwrap() as u64),
            human_count(count as u64)
        );
    }
    let _ = writeln!(
        out,
        "Parent rows include their children: do not sum them. Charged bytes are inode-deduplicated allocation, not reclaimable space. debug/release name output directories, not unique dev/test/bench configurations."
    );
    out
}

/// Project owning a nested artifact, using the same worktree containment
/// rule for text and JSON report views.
pub fn nested_artifact_project_name<'a>(
    report: &'a Report,
    unit: &crate::artifact::NestedArtifact,
) -> Option<&'a str> {
    report
        .projects
        .iter()
        .flat_map(|p| p.worktrees.iter().map(move |w| (p, w)))
        .filter(|(_, w)| unit.path.starts_with(&w.path))
        .max_by_key(|(_, w)| w.path.components().count())
        .map(|(p, _)| p.name.as_str())
}

fn render_kind_view(report: &Report, only_project: Option<&str>, kinds: &[ArtifactKind]) -> String {
    let mut out = String::new();
    let subject = if kinds == [ArtifactKind::DependencyTree] {
        "Dependencies"
    } else {
        "Build output and caches"
    };
    let scope = only_project
        .map(|name| format!(" for project {name}"))
        .unwrap_or_else(|| " across all projects".to_string());
    let _ = writeln!(out, "{subject}{scope}");
    let _ = writeln!(
        out,
        "{:<20} {:<10} {:<40} {:>10} {:>10}",
        "project", "kind", "path", "bytes", "growth"
    );
    let mut rows: Vec<(String, &'static str, String, u64, Option<i64>)> = Vec::new();
    for project in &report.projects {
        if let Some(name) = only_project
            && project.name != name
        {
            continue;
        }
        for wt in &project.worktrees {
            for a in &wt.artifacts {
                if !kinds.contains(&a.kind) {
                    continue;
                }
                let rel = a
                    .path
                    .strip_prefix(&wt.path)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| a.path.display().to_string());
                rows.push((
                    project.name.clone(),
                    kind_label(&a.kind),
                    format!("{rel}{}", allocation_note(a)),
                    a.bytes,
                    a.growth_bytes,
                ));
            }
        }
    }
    rows.sort_by_key(|a| std::cmp::Reverse(a.3));
    if rows.is_empty() {
        let empty = if let Some(name) = only_project {
            if kinds == [ArtifactKind::DependencyTree] {
                format!(
                    "No dependency folders were recorded for project {name} in this observation."
                )
            } else {
                format!(
                    "No build-output or cache folders were recorded for project {name} in this observation."
                )
            }
        } else if kinds == [ArtifactKind::DependencyTree] {
            "No dependency folders were recorded in this observation.".to_string()
        } else {
            "No build-output or cache folders were recorded in this observation.".to_string()
        };
        let _ = writeln!(out, "{empty}");
        return out;
    }
    for (project, kind, path, bytes, growth) in rows {
        let growth_str = growth
            .map(human_signed_bytes)
            .unwrap_or_else(|| "—".to_string());
        let _ = writeln!(
            out,
            "{:<20} {:<10} {:<40} {:>10} {:>10}",
            project,
            kind,
            path,
            human_bytes(bytes),
            growth_str
        );
    }
    out
}

/// `--view docker`: every Docker object joined to the project (or, at
/// root, every joined Docker object across every project) plus, when
/// `only_project` is set, unowned Docker objects whose name family
/// resembles the project -- shown clearly labelled as unowned and never
/// attributed, per #33/#19 (no verdict words, name similarity is never
/// evidence). Sorted by bytes (unique bytes) desc.
pub fn render_view_docker(report: &Report, only_project: Option<&str>) -> String {
    let mut out = String::new();
    let scope = only_project
        .map(|name| format!(" for project {name}"))
        .unwrap_or_else(|| " across all projects".to_string());
    let _ = writeln!(out, "Docker objects{scope}");
    let _ = writeln!(
        out,
        "{:<24} {:<20} {:<14} {:>10} {:>10}  detail",
        "project", "object", "kind", "bytes", "shared"
    );

    /// One `--view docker` row's detail column, assembled from
    /// created_at / shared_with / containers / dangling / note so every
    /// object -- joined or unowned -- shows the same fields (#33).
    fn detail_string(
        created_at: &Option<String>,
        shared_with: &[String],
        containers: &[String],
        dangling: bool,
        note: &Option<String>,
    ) -> String {
        let mut bits: Vec<String> = Vec::new();
        if let Some(c) = created_at {
            bits.push(format!("created {c}"));
        }
        if dangling {
            bits.push("dangling".to_string());
        }
        if !shared_with.is_empty() {
            bits.push(format!("shared_with={}", shared_with.join(",")));
        }
        if containers.is_empty() {
            bits.push("no containers reference it".to_string());
        } else {
            bits.push(format!("containers={}", containers.join("; ")));
        }
        if let Some(n) = note {
            bits.push(n.clone());
        }
        bits.join(" · ")
    }

    struct Row {
        project: String,
        object: String,
        kind: String,
        bytes: u64,
        shared_bytes: u64,
        detail: String,
    }
    let mut rows: Vec<Row> = Vec::new();
    for project in &report.projects {
        if let Some(name) = only_project
            && project.name != name
        {
            continue;
        }
        for wt in &project.worktrees {
            for a in &wt.artifacts {
                if !matches!(
                    a.kind,
                    ArtifactKind::DockerImage
                        | ArtifactKind::DockerBuildCache
                        | ArtifactKind::DockerVolume
                ) {
                    continue;
                }
                rows.push(Row {
                    project: project.name.clone(),
                    object: a.path.display().to_string(),
                    kind: kind_label(&a.kind).to_string(),
                    bytes: a.bytes,
                    shared_bytes: 0,
                    detail: detail_string(
                        &a.created_at,
                        &a.shared_with,
                        &a.containers,
                        a.dangling,
                        &a.note,
                    ),
                });
            }
        }
    }
    for row in &report.unowned {
        if row.reason != UnownedReason::DockerNoJoin {
            continue;
        }
        // At root (no project filter): list every unowned object too, so
        // `--view docker` at root is a complete listing per the issue.
        // With `--project`: only include a candidate whose reference
        // string contains the project name -- shown as unowned, never
        // attributed.
        let project_label = match only_project {
            None => String::new(),
            Some(name) => {
                if row
                    .path_or_object
                    .to_lowercase()
                    .contains(&name.to_lowercase())
                {
                    format!("{name} (unowned, name-alike)")
                } else {
                    continue;
                }
            }
        };
        rows.push(Row {
            project: project_label,
            object: row.path_or_object.clone(),
            kind: row.docker_kind.as_deref().unwrap_or("unknown").to_string(),
            bytes: row.bytes,
            shared_bytes: row.shared_bytes.unwrap_or(0),
            detail: detail_string(
                &row.created_at,
                &row.shared_with,
                &row.containers,
                row.dangling,
                &row.note,
            ),
        });
    }
    rows.sort_by_key(|a| std::cmp::Reverse(a.bytes));
    if rows.is_empty() {
        if let Some(name) = only_project {
            let _ = writeln!(
                out,
                "No Docker objects matched project {name} in this observation."
            );
        } else {
            let _ = writeln!(
                out,
                "No Docker objects were recorded in this observation. Refresh with `swamp observe` to check current daemon facts."
            );
        }
        return out;
    }
    for row in rows {
        let _ = writeln!(
            out,
            "{:<24} {:<20} {:<14} {:>10} {:>10}  {}",
            row.project,
            row.object,
            row.kind,
            human_bytes(row.bytes),
            if row.shared_bytes > 0 {
                human_bytes(row.shared_bytes)
            } else {
                "—".to_string()
            },
            row.detail,
        );
    }
    if only_project.is_none() {
        out.push_str(&render_buildkit_records(report));
    }
    out
}

/// `--view reconciliation`: attributed / unowned / walked / du (when
/// computed) / docker in one line, exactly the "one command" the
/// maintainer rule comment asks for instead of hand-assembled jq over
/// separate fields.
pub fn render_view_reconciliation(report: &Report) -> String {
    let mut out = String::new();
    let r = &report.reconciliation;
    let _ = writeln!(
        out,
        "Filesystem (allocated bytes): {} attributed · {} unowned · {} walked",
        human_bytes(r.attributed),
        human_bytes(r.unowned),
        human_bytes(r.walked_total)
    );
    let _ = writeln!(
        out,
        "Docker (separate from walked filesystem): {} attributed · {} unowned",
        human_bytes(r.docker_attributed),
        human_bytes(r.docker_unowned),
    );
    let _ = writeln!(
        out,
        "du verification: {}",
        r.du_total
            .map(human_bytes)
            .unwrap_or_else(|| { "not measured (use `swamp observe --verify-du`)".to_string() })
    );
    match &r.unique_estimate {
        Some(u) => {
            let _ = writeln!(
                out,
                "Filesystem unique: {} · {} · last reconciled {} · not a reclaimable-space estimate",
                human_bytes(u.bytes),
                if u.needs_reconciliation {
                    "needs reconciliation; swamp observe --full to reconcile"
                } else {
                    "reconciled"
                },
                timestamp_label(u.reconciled_at)
            );
        }
        None => out.push_str(
            "Filesystem unique: not measured; run `swamp observe --full` to reconcile.\n",
        ),
    }
    for line in render_sharing_lines(r.unique_estimate.as_ref(), None) {
        let _ = writeln!(out, "{line}");
    }
    out
}

/// Bounded display of reconciliation evidence; never a reclaimability claim.
pub fn render_sharing_lines(
    estimate: Option<&crate::report::UniqueEstimate>,
    path: Option<&Path>,
) -> Vec<String> {
    let Some(estimate) = estimate else {
        return Vec::new();
    };
    let Some(summary) = &estimate.sharing else {
        return Vec::new();
    };
    let matching: Vec<_> = summary
        .groups
        .iter()
        .filter(|g| path.is_none_or(|p| g.containers.iter().any(|c| c.starts_with(p))))
        .collect();
    let mut lines = Vec::new();
    for group in matching.iter().take(4) {
        let peers: Vec<_> = group
            .containers
            .iter()
            .filter(|c| path.is_none_or(|p| !c.starts_with(p)))
            .collect();
        let names = peers
            .iter()
            .take(3)
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!(
            "{} shared {}{}{} · not reclaimable bytes",
            human_bytes(group.bytes),
            if names.is_empty() {
                "within selection / unresolved peers"
            } else {
                "with "
            },
            names,
            if peers.len() > 3 {
                format!(" (+{} containers)", peers.len() - 3)
            } else {
                String::new()
            },
        ));
        lines.push(format!(
            "Sharing observed {}{}",
            timestamp_label(estimate.reconciled_at),
            if estimate.needs_reconciliation {
                "; needs reconciliation"
            } else {
                ""
            }
        ));
        if group.unresolved_links {
            lines.push(
                "Other links may exist outside observed scope; peer list is incomplete".into(),
            );
        }
    }
    if matching.len() > 4 {
        lines.push(format!(
            "{} more sharing groups; see reconciliation JSON",
            matching.len() - 4
        ));
    }
    if summary.omitted_groups > 0 {
        lines.push(format!(
            "Sharing summary bounded: {} groups / {} omitted",
            summary.omitted_groups,
            human_bytes(summary.omitted_bytes)
        ));
    }
    lines
}

/// `--worktree <path>`: signals for one worktree, matched by exact path
/// or by its relative path under the report root, since a project's
/// `WorktreeRow.path` is stored absolute. One command, no jq over the
/// whole report needed to answer "signals for this worktree".
pub fn render_worktree_signals(report: &Report, path: &Path) -> Option<String> {
    let worktree = report.projects.iter().find_map(|p| {
        p.worktrees.iter().find(|w| {
            w.path == path
                || w.path
                    .strip_prefix(&report.root)
                    .map(|rel| rel == path)
                    .unwrap_or(false)
        })
    })?;
    let mut out = String::new();
    let _ = writeln!(out, "worktree: {}", worktree.path.display());
    let bytes: u64 = worktree.artifacts.iter().map(|a| a.bytes).sum();
    let stale = if worktree.artifacts.iter().any(|a| a.dedup_stale) {
        "; unique-byte estimate needs reconciliation, use --full"
    } else {
        ""
    };
    let _ = writeln!(
        out,
        "  measured size: {} (not a free-space estimate{stale})",
        human_bytes(bytes)
    );
    if worktree.signals.is_empty() {
        let _ = writeln!(out, "  (no signals)");
    } else {
        for s in &worktree.signals {
            let _ = writeln!(out, "  {}: {}", s.name, s.value);
        }
    }
    Some(out)
}

/// `--view unowned` at root: the same aggregation `render_overview`
/// prints below the project table, standalone so it is one command
/// rather than a post-processed slice of the overview.
pub fn render_view_unowned(report: &Report) -> String {
    let mut out = String::new();
    render_unowned_summary(report, &mut out, true);
    out
}

/// `--view external` (#43): detector-resolved storage with no containing
/// project (Cargo registry, rustup toolchains, Homebrew, ...), one line
/// per unit plus a total that is explicitly *not* folded into any
/// `reconciliation` total above -- external units are measured
/// independently of the walked root(s), so summing the two would double
/// nothing (they never overlap) but would also conflate two different
/// bases; kept visibly separate instead.
pub fn render_view_external(units: &[crate::external::ExternalUnit]) -> String {
    render_view_external_with(units, &[], crate::entities::now())
}

/// `--view external`, with each machine-wide build store's identified
/// interior (`ScopeObservation::store_interiors`) under its unit.
pub fn render_view_external_with(
    units: &[crate::external::ExternalUnit],
    interiors: &[crate::artifact::NestedArtifact],
    observed_at: u64,
) -> String {
    let mut out = String::new();
    if units.is_empty() {
        let _ = writeln!(
            out,
            "No external storage units were recorded. Check detector coverage with `swamp scope`."
        );
        return out;
    }
    let mut sorted: Vec<&crate::external::ExternalUnit> = units.iter().collect();
    sorted.sort_by_key(|a| std::cmp::Reverse(a.bytes));
    let mut total = 0u64;
    for u in &sorted {
        total += u.bytes;
        let growth = u
            .growth_bytes
            .map(human_bytes_signed)
            .unwrap_or_else(|| "—".to_string());
        let known_consumer = |link: bool| {
            u.evidence
                .iter()
                .filter(|e| {
                    e.kind == crate::evidence::FactKind::Consumer
                        && matches!(e.status, crate::evidence::FactStatus::Known(_))
                        && (e.subtype == crate::evidence::FactSubtype::RecordedLink) == link
                })
                .count()
        };
        let consumer_facts = known_consumer(false);
        let link_facts = known_consumer(true);
        let consumers = if u.consumers.is_empty() && consumer_facts > 0 {
            // The declarations are attached as evidence rows (rendered
            // just below), not as the unit's own consumer list: say so
            // rather than contradicting them (the aim review's "no
            // declared consumers" above three declared consumers).
            format!("consumers: {consumer_facts} (from declarations, below)")
        } else if u.consumers.is_empty() {
            "no declared consumers".to_string()
        } else {
            format!(
                "consumers: {}",
                u.consumers
                    .iter()
                    .map(|c| c.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let consumers = if link_facts > 0 {
            // Second tier: what the tool recorded, counted apart from
            // what a project declared.
            format!("{consumers}; recorded links: {link_facts} (the tool's own metadata, below)")
        } else {
            consumers
        };
        let note = u
            .display_note()
            .map(|n| format!("  [{n}]"))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "{:<10} {:>+10}  {}  {}  {} ({})  {}{note}",
            human_bytes(u.bytes),
            growth,
            crate::external::category_str(u.category),
            u.path.display(),
            u.detector_name,
            u.detector_id,
            consumers,
        );
        if let Some(r) = crate::locations::remainder_of(&u.detector_id) {
            let _ = writeln!(
                out,
                "    the rest of this location after the {} units (a file hardlinked between \
                 them is counted in those, once); to report it whole: {}",
                r.of, r.include_all
            );
        }
        let _ = writeln!(out, "    {}", u.last_used.describe(observed_at));
        for line in render_evidence_lines(&u.evidence) {
            let _ = writeln!(out, "    {line}");
        }
        for line in render_unit_children(u, observed_at) {
            let _ = writeln!(out, "    {line}");
        }
        let inside: Vec<crate::artifact::NestedArtifact> = interiors
            .iter()
            .filter(|i| i.present && i.path.starts_with(&u.path))
            // Another unit's interior (go-build under Library/Caches) is
            // listed once, under that unit.
            .filter(|i| {
                !sorted.iter().any(|o| {
                    o.path != u.path && o.path.starts_with(&u.path) && i.path.starts_with(&o.path)
                })
            })
            .cloned()
            .collect();
        if let Some((_, section)) = render_container_section(&inside, observed_at, "    ") {
            let _ = write!(
                out,
                "    inside (identification only -- no cleanup is offered here):{section}"
            );
        }
        let models = crate::build_adapters::model_stores::model_rows(&u.path, &inside, observed_at);
        for line in model_lines(&models) {
            let _ = writeln!(out, "    {line}");
        }
    }
    let _ = writeln!(
        out,
        "\nexternal storage total: {} ({} units, independent of walked_total above)",
        human_bytes(total),
        sorted.len()
    );
    out
}

/// A model store's models, one block each: name, revision or tag, size
/// and what it is; then when its weights were last read, what getting
/// it back costs, its revisions, the Hub's answer, and the facts about
/// its bytes. When Hub facts are off, one line says how to turn them on.
pub fn model_lines(models: &[crate::build_adapters::model_stores::ModelRow]) -> Vec<String> {
    let mut out = Vec::new();
    if models.is_empty() {
        return out;
    }
    out.push(format!(
        "models ({}), largest first; what each is comes from its own files:",
        human_count(models.len() as u64)
    ));
    for m in models {
        let rev = m
            .revision
            .as_deref()
            .map(|r| format!("@{r}"))
            .unwrap_or_default();
        out.push(format!(
            "  {:>10}  {}{rev}  ({})  {}",
            human_bytes(m.bytes),
            m.name,
            m.kind,
            m.about
                .as_deref()
                .unwrap_or("what it is: no field stated in its files")
        ));
        out.push(format!("      last read: {}", m.last_read));
        out.push(format!("      regeneration: {}", m.regeneration));
        if let Some(r) = &m.revisions {
            out.push(format!("      {r}"));
        }
        if let Some(c) = &m.card {
            out.push(format!("      card: {c}"));
        }
        if let Some(h) = m.hub.as_deref().filter(|h| *h != "off") {
            out.push(format!("      {h}"));
        }
        for f in &m.facts {
            out.push(format!("      {f}"));
        }
    }
    if models.iter().any(|m| m.hub.as_deref() == Some("off")) {
        out.push(crate::hub_api::OFF_LINE.to_string());
    }
    out
}

/// The standalone Cargo target directories the walk found, as their own
/// kind (#171): rows of the unowned list that carry Cargo's own signature.
/// Their bytes are counted under `unowned`, never in the external total.
pub fn standalone_cargo_targets(
    unowned: &[crate::report::UnownedRow],
) -> Vec<&crate::report::UnownedRow> {
    let mut rows: Vec<&crate::report::UnownedRow> = unowned
        .iter()
        .filter(|u| u.reason == UnownedReason::StandaloneCargoTarget)
        .collect();
    rows.sort_by_key(|u| std::cmp::Reverse(u.bytes));
    rows
}

/// `--view external`'s section for [`standalone_cargo_targets`]: empty
/// when there are none.
pub fn render_standalone_targets(unowned: &[crate::report::UnownedRow]) -> String {
    let rows = standalone_cargo_targets(unowned);
    if rows.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    let _ = writeln!(
        out,
        "\nstandalone Cargo targets (their own kind; counted under unowned, not in the external total above):"
    );
    for row in &rows {
        let _ = writeln!(
            out,
            "{:<10}  standalone-cargo-target  {}  [{}]",
            human_bytes(row.bytes),
            row.path_or_object,
            row.note.as_deref().unwrap_or("")
        );
        for line in render_evidence_lines(&row.evidence) {
            let _ = writeln!(out, "    {line}");
        }
    }
    let _ = writeln!(
        out,
        "standalone Cargo targets: {} in {} director{}",
        human_bytes(rows.iter().map(|r| r.bytes).sum()),
        human_count(rows.len() as u64),
        if rows.len() == 1 { "y" } else { "ies" }
    );
    out
}

/// The text of one drilldown row after its byte figure: the folder's
/// name exactly as on disk with its facts, or what a remainder or
/// adjustment row stands for. Shared with the TUI so the two say the same
/// thing.
pub fn describe_unit_child(child: &crate::drilldown::UnitChild, now: u64) -> String {
    use crate::drilldown::{ChildKind, ChildMeasure};
    match child.kind {
        ChildKind::Remainder => {
            let mut text = format!(
                "remainder: {} other entr{} (the other folders, and files directly inside)",
                human_count(child.entries as u64),
                if child.entries == 1 { "y" } else { "ies" }
            );
            if child.not_measured > 0 {
                text.push_str(&format!(
                    "; {} folder{} not measured",
                    human_count(child.not_measured as u64),
                    if child.not_measured == 1 { "" } else { "s" }
                ));
            }
            text
        }
        ChildKind::Adjustment => {
            "adjustment: hardlinked files are counted once in this unit's total".to_string()
        }
        ChildKind::NotRemeasured => {
            "not re-measured this pass: the unit keeps its last complete total".to_string()
        }
        ChildKind::Entry => {
            let mut text = child.name.clone();
            match child.measure {
                ChildMeasure::NotMeasured => {
                    text.push_str("  (not measured: this folder could not be read)");
                    return text;
                }
                ChildMeasure::Partial => {
                    text.push_str("  (partly measured: a folder below it could not be read)");
                }
                ChildMeasure::Complete => {}
            }
            text.push_str(&format!("  modified {}", age_label(child.mtime_max, now)));
            let last_used_is_unsupported =
                child.last_used.source == crate::last_used::LastUsedSource::Unsupported;
            if !last_used_is_unsupported || child.access_evidence.is_none() {
                text.push_str(&format!("  {}", child.last_used.describe(now)));
            }
            if let Some(access) = &child.access_evidence {
                text.push_str(&format!(
                    "  directory access time {}",
                    access_summary(access)
                ));
                if last_used_is_unsupported {
                    text.push_str("  use tracking unsupported");
                }
            }
            text
        }
    }
}

/// The byte column of a drilldown row: the size, a signed figure for an
/// adjustment, and words -- never `0B` -- for a folder that was not
/// measured.
pub fn unit_child_size(child: &crate::drilldown::UnitChild) -> String {
    match (child.bytes, child.kind) {
        (None, _) => "not measured".to_string(),
        (
            Some(b),
            crate::drilldown::ChildKind::Adjustment | crate::drilldown::ChildKind::NotRemeasured,
        ) => human_bytes_signed(b),
        (Some(b), _) if child.measure == crate::drilldown::ChildMeasure::Partial => {
            format!(">={}", human_bytes(b.max(0) as u64))
        }
        (Some(b), _) => human_bytes(b.max(0) as u64),
    }
}

/// A unit's depth-2 drilldown as text lines: the top folders largest
/// first, then a remainder row, so the sizes add up to the total the walk
/// reported for it. Empty for a unit that is not drilled into.
fn render_unit_children(u: &crate::external::ExternalUnit, now: u64) -> Vec<String> {
    if u.children.is_empty() {
        return Vec::new();
    }
    let mut lines =
        vec!["inside, largest first (rows add up to the total the walk measured):".to_string()];
    for child in &u.children {
        lines.push(format!(
            "  {:>13}  {}",
            unit_child_size(child),
            describe_unit_child(child, now)
        ));
        if let Some(access) = &child.access_evidence {
            lines.push(format!(
                "                 {}",
                render_access_evidence(access)
            ));
        }
    }
    let sum = crate::drilldown::rows_total(&u.children);
    if sum != u.bytes as i64 {
        lines.push(format!(
            "  the rows above sum to {} measured this pass; this unit's total above is {}",
            human_bytes(sum.max(0) as u64),
            human_bytes(u.bytes)
        ));
    }
    lines
}

/// One line's worth of a modification age, correctly labelled: unknown
/// (`mtime_max == 0`, e.g. a residual row with nothing measured) is
/// never rendered as ancient, and a future timestamp is never rendered
/// as old (guardrail: label modification time correctly).
fn age_label(mtime_max: u64, now: u64) -> String {
    if mtime_max == 0 {
        return "age unknown".to_string();
    }
    // A tree written to while the pass ran carries an mtime a little
    // after `now` (the observation's own timestamp, taken at its start):
    // that is "just now", not a clock problem. Only an hour or more
    // ahead is worth calling out (R19; the aim review saw `~/.claude`,
    // `~/.codex` and `/opt/homebrew` labelled "clock skew?" on every
    // pass).
    if mtime_max > now + 3600 {
        return "modified in the future (clock skew?)".to_string();
    }
    let secs = now.saturating_sub(mtime_max);
    format!("{} ago", crate::schedule::coarse_age(secs))
}

/// Public form of the same age wording used for unit-child modifications.
pub fn modification_age_label(mtime_max: u64, now: u64) -> String {
    age_label(mtime_max, now)
}

/// One short, source-qualified line per decision-evidence fact (#60):
/// what kind of fact, its value or its explicit unknown/unavailable/
/// conflicting reason, and where it came from. Never a verdict word --
/// `agent_interface_facts_not_verdicts` audits this file for exactly
/// that.
/// The human-facing name of one fact's subtype. Plain words, not the
/// enum's spelling: a reader sees "estimated reclaimable", not
/// `EstimatedReclaimable`.
pub fn evidence_subtype_label(subtype: crate::evidence::FactSubtype) -> &'static str {
    use crate::evidence::FactSubtype as S;
    match subtype {
        S::Modified => "modified",
        S::Accessed => "accessed",
        S::ToolReportedUse => "tool-reported use",
        S::DeclaredConsumer => "declared consumer",
        S::InferredConsumer => "inferred consumer",
        S::RecordedLink => "recorded link",
        S::Process => "process",
        S::OpenFile => "open file",
        S::Lock => "lock",
        S::RunningContainer => "running container",
        S::Mounted => "mounted",
        S::Booted => "booted",
        S::Rebuild => "rebuild",
        S::NetworkFetch => "network fetch",
        S::LocalReinstall => "local reinstall",
        S::TrashRecovery => "Trash recovery",
        S::BackupDependent => "backup dependent",
        S::PotentiallyUniqueLocalState => "potentially unique local state",
        S::UnknownPrerequisites => "unknown prerequisites",
        S::LogicalBytes => "logical bytes",
        S::AllocatedBytes => "allocated bytes",
        S::EstimatedReclaimable => "estimated reclaimable",
        S::ObservedFreed => "observed freed",
    }
}

/// A human-readable label for an [`EvidenceSource`], with its detail
/// text where it has one -- never `{:?}`, which used to print the
/// struct literally (`FilesystemMetadata { detail: "…" }`, `Inferred {
/// basis: "…" }`) on this exact user-facing evidence line (item 3).
fn evidence_source_label(source: &crate::evidence::EvidenceSource) -> String {
    use crate::evidence::EvidenceSource as ES;
    match source {
        ES::FilesystemMetadata { detail } => format!("filesystem metadata ({detail})"),
        ES::ToolReported { tool, detail } => format!("{tool} ({detail})"),
        ES::ProcessQuery { tool } => format!("{tool} process query"),
        ES::ManagerLock { tool, path } => format!("{tool} lock ({path})"),
        ES::ConfigDeclaration { path } => format!("config declaration ({path})"),
        ES::Lockfile { ecosystem, path } => format!("{ecosystem} lockfile ({path})"),
        ES::BuildMetadata { path } => format!("build metadata ({path})"),
        ES::DockerApi { detail } => format!("Docker API ({detail})"),
        ES::Statvfs => "statvfs".to_string(),
        ES::Inferred { basis } => format!("inferred ({basis})"),
    }
}

/// The order the five evidence domains are printed in, so a reader
/// always finds the same question in the same place: what happened to
/// it, who refers to it, whether anything holds it right now, how it
/// comes back, and what removing it would actually free. A row's
/// evidence arrives in whatever order the sources that filled it ran
/// (`report::attach_decision_evidence`, then the Docker join, then
/// `consumer_wiring`, then a fresh current-use reading at proposal
/// time), so without this the same facts render in a different order
/// depending on which pass touched the row.
const EVIDENCE_KIND_ORDER: &[crate::evidence::FactKind] = &[
    crate::evidence::FactKind::Activity,
    crate::evidence::FactKind::Consumer,
    crate::evidence::FactKind::CurrentUse,
    crate::evidence::FactKind::Recovery,
    crate::evidence::FactKind::Reclaimability,
];

pub fn render_evidence_lines(evidence: &[crate::evidence::Evidence]) -> Vec<String> {
    use crate::evidence::{FactKind, FactStatus, FactValue};
    // Grouped by domain through the shared accessor rather than by
    // re-filtering here, so "which facts belong to this question" has
    // exactly one definition (`evidence::of_kind`) across the renderer,
    // the agent-facing JSON shaper and the action sinks.
    let mut grouped: Vec<&crate::evidence::Evidence> = EVIDENCE_KIND_ORDER
        .iter()
        .flat_map(|kind| crate::evidence::of_kind(evidence, *kind))
        .collect();
    // A fact whose kind this order does not name is appended rather than
    // dropped: adding a sixth `FactKind` must never silently delete a
    // line from what a human reads.
    grouped.extend(
        evidence
            .iter()
            .filter(|e| !EVIDENCE_KIND_ORDER.contains(&e.kind)),
    );
    grouped
        .iter()
        .map(|e| {
            let kind = match e.kind {
                FactKind::Activity => "activity",
                FactKind::Consumer => "consumer",
                FactKind::CurrentUse => "current-use",
                FactKind::Recovery => "recovery",
                FactKind::Reclaimability => "reclaimability",
            };
            let value = match &e.status {
                FactStatus::Known(FactValue::Timestamp(t)) => age_label(*t, e.observed_at),
                FactStatus::Known(FactValue::Bool(b)) => b.to_string(),
                FactStatus::Known(FactValue::Text(t)) => t.clone(),
                FactStatus::Known(FactValue::Bytes(b)) => human_bytes(*b),
                FactStatus::Known(FactValue::SignedBytes(b)) => human_bytes_signed(*b),
                FactStatus::Known(FactValue::Count(c)) => c.to_string(),
                FactStatus::Known(FactValue::List(l)) => l.join(", "),
                FactStatus::Unknown { reason } => format!("unknown ({reason})"),
                FactStatus::Unavailable { reason } => format!("unavailable ({reason})"),
                FactStatus::Conflicting { candidates, reason } => format!(
                    "conflicting [{}] ({reason})",
                    candidates
                        .iter()
                        .map(|c| match c {
                            FactValue::Text(t) => t.clone(),
                            FactValue::Bytes(b) => human_bytes(*b),
                            other => format!("{other:?}"),
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            // The subtype is what distinguishes two facts of the same
            // kind, and leaving it out made "allocated bytes" and
            // "estimated reclaimable bytes" render as byte-identical
            // lines -- which is the one distinction a reader deciding
            // what to remove most needs (the PR #123 review's
            // rendered_reclaimability counterexample).
            let subtype = evidence_subtype_label(e.subtype);
            // Not `{:?}` (item 3): that printed the struct literally,
            // e.g. `FilesystemMetadata { detail: "…" }`, on this exact
            // user-facing evidence line.
            let source = evidence_source_label(&e.source);
            let coverage = e
                .freshness
                .coverage_note
                .as_deref()
                .map(|n| format!("; coverage: {n}"))
                .unwrap_or_default();
            let note = e
                .note
                .as_deref()
                .map(|n| format!("; note: {n}"))
                .unwrap_or_default();
            format!("{kind} ({subtype}): {value}  [source: {source}]{coverage}{note}")
        })
        .collect()
}

/// Access-time detail for a stored observation. Uses calendar timestamps
/// so reopening an old report cannot make its access event look recent.
pub fn render_access_evidence(e: &crate::evidence::Evidence) -> String {
    use crate::evidence::{FactStatus, FactValue};
    let value = match &e.status {
        FactStatus::Known(FactValue::Timestamp(t)) => {
            format!("{} (event)", timestamp_label(*t))
        }
        FactStatus::Known(v) => format!("{v:?}"),
        FactStatus::Unknown { reason } => format!("unknown ({reason})"),
        FactStatus::Unavailable { reason } => format!("unavailable ({reason})"),
        FactStatus::Conflicting { reason, .. } => format!("conflicting ({reason})"),
    };
    let source = evidence_source_label(&e.source);
    let note = e
        .note
        .as_deref()
        .map(|n| format!("; note: {n}"))
        .unwrap_or_default();
    format!(
        "{value} [source: {source}; observed {}{note}]",
        timestamp_label(e.observed_at)
    )
}

/// Compact access-time value for a row label; details stay in the evidence
/// pane/indented line so source and caveat do not crowd a large cache list.
pub fn access_summary(e: &crate::evidence::Evidence) -> String {
    use crate::evidence::{FactStatus, FactValue};
    match &e.status {
        FactStatus::Known(FactValue::Timestamp(t)) => timestamp_label(*t),
        FactStatus::Known(_) => "value recorded".into(),
        FactStatus::Unknown { .. } => "unknown".into(),
        FactStatus::Unavailable { .. } => "unavailable".into(),
        FactStatus::Conflicting { .. } => "conflicting".into(),
    }
}

/// Confirm-time warnings derived from a unit's decision evidence (#60,
/// #61's `PlanUnit.evidence`): consumer/current-use/recovery/
/// reclaimability facts a human should see before authorizing removal,
/// distinct from `PlanUnit::warnings`' git-status facts (dirty,
/// unpushed, no remote). Only facts worth a human's attention are
/// surfaced -- a routine "known rebuildable, no consumers" unit
/// produces no line here, never a generic "review this" nag for every
/// selection.
pub fn evidence_warnings(evidence: &[crate::evidence::Evidence]) -> Vec<String> {
    use crate::evidence::{FactKind, FactStatus, FactValue};
    let mut out = Vec::new();
    for e in evidence {
        match (e.kind, &e.status) {
            (FactKind::Consumer, FactStatus::Known(FactValue::Text(who))) => {
                if e.subtype == crate::evidence::FactSubtype::RecordedLink {
                    // A link the tool recorded is not a declaration: it
                    // says where a build came from, not who depends on it.
                    out.push(format!("recorded link (the tool's own metadata): {who}"));
                } else {
                    out.push(format!("declared consumer: {who}"));
                }
            }
            (FactKind::Consumer, FactStatus::Known(FactValue::List(who))) => {
                out.push(format!(
                    "declared by {} projects: {}",
                    who.len(),
                    who.join(", ")
                ));
            }
            (FactKind::CurrentUse, FactStatus::Known(FactValue::Bool(true))) => {
                out.push("currently in use".to_string());
            }
            // A current-use question that could not be *answered* must
            // not look like one answered "no". The PR #123 review:
            // `Unavailable`/`Unknown` produced no confirmation line at
            // all, so a unit whose occupancy probe failed read exactly
            // like a unit confirmed idle
            // (`.oh/guardrails/occupancy-is-tristate-at-sinks.md`).
            (FactKind::CurrentUse, FactStatus::Unavailable { reason }) => {
                out.push(format!(
                    "could not check whether this is in use right now: {reason}"
                ));
            }
            (FactKind::CurrentUse, FactStatus::Unknown { reason }) => {
                out.push(format!("whether this is in use is unresolved: {reason}"));
            }
            (FactKind::Recovery, FactStatus::Known(FactValue::Text(path)))
                if path == "PotentiallyUniqueLocalState" || path == "BackupDependent" =>
            {
                out.push("may hold state that exists nowhere else".to_string());
            }
            (FactKind::Recovery, FactStatus::Unknown { reason }) => {
                out.push(format!("recovery path unknown: {reason}"));
            }
            (FactKind::Reclaimability, FactStatus::Conflicting { reason, .. }) => {
                out.push(format!("reclaimable space is a bound, not exact: {reason}"));
            }
            (FactKind::Reclaimability, FactStatus::Unknown { reason }) => {
                out.push(format!("reclaimable space unknown: {reason}"));
            }
            _ => {}
        }
    }
    // A short-lived fact (a process/lock/container/booted reading) that
    // is already past its own recheck window must not read like a
    // current one. `evidence::stale` is the single definition of "past
    // its stated expiry" -- the same one the action sinks recheck
    // against -- so the confirmation line and the sink cannot disagree
    // about which readings still stand.
    let now = crate::entities::now();
    for e in crate::evidence::stale(evidence, now) {
        let window = e.freshness.expires_after_secs.unwrap_or_default();
        out.push(format!(
            "this {} reading is past its {} recheck window (about {} old); it is checked fresh before any action",
            evidence_subtype_label(e.subtype),
            human_duration(window),
            human_duration(now.saturating_sub(e.observed_at)),
        ));
    }
    out
}

fn agent_link_label(link: &crate::agents::ProjectLinkState) -> String {
    use crate::agents::ProjectLinkState as L;
    match link {
        L::Linked {
            project_name,
            project_path,
            source,
            fallback_reason,
            ..
        } => match source {
            crate::agents::LinkSource::Declared => {
                format!("project: {project_name} ({})", project_path.display())
            }
            crate::agents::LinkSource::Inferred => {
                let fallback = fallback_reason
                    .as_deref()
                    .map(|reason| format!("; cwd fallback: {reason}"))
                    .unwrap_or_default();
                format!(
                    "project: {project_name} ({}) [inferred from the tool's project folder name, \
                     not declared{fallback}]",
                    project_path.display()
                )
            }
        },
        L::Unresolved { reason } => format!("project: unresolved ({reason})"),
        L::Missing { path } => format!("project: missing ({})", path.display()),
        L::NotAProject { path } => format!("project: not a project ({})", path.display()),
        L::Moved { from, to } => format!("project: moved ({} -> {})", from.display(), to.display()),
        L::Remote { host, path } => format!("project: remote ({host}:{})", path.display()),
        L::Shared { project_ids } => format!("project: shared ({} projects)", project_ids.len()),
        L::NotApplicable => "project: n/a (tool-wide)".to_string(),
    }
}

fn agent_unit_matches_project(u: &crate::agents::AgentUnit, project: Option<&str>) -> bool {
    let Some(project) = project else { return true };
    matches!(
        &u.project_link,
        crate::agents::ProjectLinkState::Linked { project_name, .. }
            if project_name.eq_ignore_ascii_case(project)
    )
}

/// `--view agents` (#91/#100): tool → category → unit drill-down.
/// Redaction-aware by construction, not just by convention -- an
/// `AgentUnit` never carries a session title, first prompt, or any
/// content field to begin with (see `crate::agents`'s privacy
/// contract), so there is nothing here to accidentally print. Bounded
/// to `PER_CATEGORY_LIMIT` per category, oldest-modified first (this
/// product's existing "old is enough to suggest review" convention),
/// unless `all` is set. `--project` narrows to units whose linkage
/// names that project; every other linkage state (unresolved, missing,
/// not-a-project, shared, not-applicable) is filtered out by a project
/// filter, never silently included.
const AGENT_PER_CATEGORY_LIMIT: usize = 20;

pub fn render_view_agents(
    units: &[crate::agents::AgentUnit],
    project: Option<&str>,
    all: bool,
    now: u64,
) -> String {
    let mut out = String::new();
    let filtered: Vec<&crate::agents::AgentUnit> = units
        .iter()
        .filter(|u| agent_unit_matches_project(u, project))
        .collect();
    if filtered.is_empty() {
        let _ = writeln!(
            out,
            "No agent-storage units were recorded{}.",
            project
                .map(|p| format!(" for project {p}"))
                .unwrap_or_default()
        );
        return out;
    }
    let mut by_tool: BTreeMap<&str, Vec<&crate::agents::AgentUnit>> = BTreeMap::new();
    for u in &filtered {
        by_tool.entry(u.tool_name.as_str()).or_default().push(u);
    }
    let mut grand_total = 0u64;
    for (tool, tool_units) in &by_tool {
        let tool_total: u64 = tool_units.iter().map(|u| u.bytes).sum();
        grand_total += tool_total;
        let _ = writeln!(out, "{tool}  ({} total)", human_bytes(tool_total));
        let mut by_category: BTreeMap<&'static str, Vec<&crate::agents::AgentUnit>> =
            BTreeMap::new();
        for u in tool_units {
            by_category.entry(u.category.label()).or_default().push(u);
        }
        for (cat, cat_units) in &by_category {
            let cat_total: u64 = cat_units.iter().map(|u| u.bytes).sum();
            let _ = writeln!(
                out,
                "  {cat}  ({} total, {} unit{})",
                human_bytes(cat_total),
                human_count(cat_units.len() as u64),
                if cat_units.len() == 1 { "" } else { "s" }
            );
            let mut sorted = cat_units.clone();
            // Largest first, then oldest-modified: the capped default
            // view has to show the rows a decision hinges on (the aim
            // review: oldest-first buried a 900 MB category's large
            // members under twenty 40 KB files). Units with an unknown
            // age (0) sort after known ages at equal size, never
            // masquerading as the oldest.
            sorted.sort_by(|a, b| {
                b.bytes
                    .cmp(&a.bytes)
                    .then_with(|| match (a.mtime_max, b.mtime_max) {
                        (0, 0) => std::cmp::Ordering::Equal,
                        (0, _) => std::cmp::Ordering::Greater,
                        (_, 0) => std::cmp::Ordering::Less,
                        (x, y) => x.cmp(&y),
                    })
            });
            let limit = if all {
                sorted.len()
            } else {
                AGENT_PER_CATEGORY_LIMIT.min(sorted.len())
            };
            for u in sorted.iter().take(limit) {
                let growth = u
                    .growth_bytes
                    .map(human_bytes_signed)
                    .unwrap_or_else(|| "—".to_string());
                let protect = if u.protected { "  [protected]" } else { "" };
                let note = u
                    .note
                    .as_deref()
                    .map(|n| format!("  [{n}]"))
                    .unwrap_or_default();
                let _ = writeln!(
                    out,
                    "    {:<10} {:>+10}  {}  {}  {}{protect}{note}",
                    human_bytes(u.bytes),
                    growth,
                    age_label(u.mtime_max, now),
                    u.relative_path,
                    agent_link_label(&u.project_link),
                );
            }
            if sorted.len() > limit {
                let _ = writeln!(
                    out,
                    "    … and {} more (--all for the rest)",
                    human_count((sorted.len() - limit) as u64)
                );
            }
        }
    }
    let _ = writeln!(
        out,
        "\nagent storage total: {} ({} units, independent of walked_total above)",
        human_bytes(grand_total),
        human_count(filtered.len() as u64)
    );
    out
}

/// The activity-evidence inventory (`activity::ACTIVITY_EVIDENCE_INVENTORY`)
/// rendered as the Markdown table `docs/usage.md` carries.
///
/// The constant is the #54 "inventory" deliverable and, until
/// 2026-09-22, it had **zero** readers: `activity.rs` claimed that
/// `crates/core/tests/evidence_contract.rs` asserted the doc table
/// listed every entry, and no test referenced the constant at all. A
/// rendered form is what makes the claim checkable -- the test below
/// regenerates this table and compares it to the file, so editing either
/// side alone fails.
pub fn activity_evidence_inventory_markdown() -> String {
    let mut out =
        String::from("| Domain | Activity evidence this pass can establish |\n|---|---|\n");
    for (domain, evidence) in crate::activity::ACTIVITY_EVIDENCE_INVENTORY {
        out.push_str(&format!("| {domain} | {evidence} |\n"));
    }
    out
}

#[cfg(test)]
mod child_access_display_tests {
    use super::describe_unit_child;
    use crate::drilldown::{ChildKind, ChildMeasure, UnitChild};
    use crate::evidence::{Evidence, EvidenceSource, FactKind, FactSubtype, FactValue};
    use crate::last_used::{LastUsed, LastUsedSource};

    fn access_evidence() -> Evidence {
        Evidence::known(
            FactKind::Activity,
            FactSubtype::Accessed,
            FactValue::Timestamp(1_700_000_000),
            EvidenceSource::FilesystemMetadata {
                detail: "access time of this directory".into(),
            },
            1_700_000_100,
        )
        .with_event_at(1_700_000_000)
        .with_note("listing may refresh this directory's access time; not proof of prior use")
    }

    fn child(last_used: LastUsed) -> UnitChild {
        UnitChild {
            kind: ChildKind::Entry,
            name: "large".into(),
            bytes: Some(4),
            measure: ChildMeasure::Complete,
            mtime_max: 1_699_000_000,
            entries: 0,
            not_measured: 0,
            last_used,
            access_evidence: Some(access_evidence()),
        }
    }

    #[test]
    fn directory_access_does_not_turn_unsupported_use_tracking_into_last_used() {
        let text = describe_unit_child(
            &child(LastUsed {
                source: LastUsedSource::Unsupported,
                ..LastUsed::default()
            }),
            1_800_000_000,
        );
        assert!(text.contains("modified"));
        assert!(text.contains("directory access time"));
        assert!(text.contains("use tracking unsupported"));
        assert!(!text.contains("Last run or opened: tracking unsupported"));
        assert!(
            !text.contains("ago [source:"),
            "stored access event must not be rendered as a fresh relative age"
        );
        let detail = super::render_access_evidence(&access_evidence());
        assert!(detail.contains("listing may refresh"));
        assert!(detail.contains("source: filesystem metadata"));
        assert!(detail.contains("observed"));
    }

    #[test]
    fn supported_tool_use_remains_visible_alongside_directory_access() {
        let text = describe_unit_child(
            &child(LastUsed {
                at: Some(1_600_000_000),
                source: LastUsedSource::ToolNative("fixture-tool".into()),
                ..LastUsed::default()
            }),
            1_800_000_000,
        );
        assert!(text.contains("Last run or opened:"));
        assert!(text.contains("directory access time"));
    }
}

#[cfg(test)]
mod evidence_warnings_tests {
    use super::evidence_warnings;
    use crate::evidence::{Evidence, EvidenceSource, FactKind, FactSubtype, FactValue};

    fn now() -> u64 {
        crate::entities::now()
    }

    #[test]
    fn known_consumer_list_names_every_project() {
        let ev = vec![Evidence::known(
            FactKind::Consumer,
            FactSubtype::DeclaredConsumer,
            FactValue::List(vec!["a".into(), "b".into()]),
            EvidenceSource::Lockfile {
                ecosystem: "cargo".into(),
                path: "serde@1.0".into(),
            },
            now(),
        )];
        let warnings = evidence_warnings(&ev);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains('a') && warnings[0].contains('b'));
    }

    #[test]
    fn current_use_false_produces_no_warning() {
        // The tempting shortcut this rejects: warning on every CurrentUse
        // fact regardless of value, which would nag on a unit that is
        // plainly *not* in use right now.
        let ev = vec![Evidence::known(
            FactKind::CurrentUse,
            FactSubtype::OpenFile,
            FactValue::Bool(false),
            EvidenceSource::ProcessQuery {
                tool: "lsof".into(),
            },
            now(),
        )];
        assert!(evidence_warnings(&ev).is_empty());
    }

    #[test]
    fn recovery_rebuild_is_routine_not_a_warning() {
        // A confirmed Rebuild path (source present) is the unremarkable
        // case -- it must not appear as a warning line beside a genuine
        // Unknown/PotentiallyUniqueLocalState one.
        let ev = vec![Evidence::known(
            FactKind::Recovery,
            FactSubtype::Rebuild,
            FactValue::Text("Rebuild".into()),
            EvidenceSource::FilesystemMetadata {
                detail: "source present".into(),
            },
            now(),
        )];
        assert!(evidence_warnings(&ev).is_empty());
    }

    #[test]
    fn recovery_unknown_and_unique_state_both_warn() {
        let unknown = vec![Evidence::unknown(
            FactKind::Recovery,
            FactSubtype::UnknownPrerequisites,
            EvidenceSource::Inferred {
                basis: "no signal".into(),
            },
            now(),
            "no lockfile found",
        )];
        assert_eq!(evidence_warnings(&unknown).len(), 1);

        let unique = vec![Evidence::known(
            FactKind::Recovery,
            FactSubtype::PotentiallyUniqueLocalState,
            FactValue::Text("PotentiallyUniqueLocalState".into()),
            EvidenceSource::FilesystemMetadata {
                detail: "docker volume".into(),
            },
            now(),
        )];
        assert_eq!(evidence_warnings(&unique).len(), 1);
    }

    #[test]
    fn reclaimability_known_exact_is_not_a_warning_bounded_and_unknown_are() {
        let known = vec![Evidence::known(
            FactKind::Reclaimability,
            FactSubtype::EstimatedReclaimable,
            FactValue::Bytes(1024),
            EvidenceSource::FilesystemMetadata {
                detail: "allocation".into(),
            },
            now(),
        )];
        assert!(evidence_warnings(&known).is_empty());

        let bounded = vec![Evidence::conflicting(
            FactKind::Reclaimability,
            FactSubtype::EstimatedReclaimable,
            vec![FactValue::Bytes(100), FactValue::Bytes(500)],
            EvidenceSource::Inferred {
                basis: "hardlink membership unresolved".into(),
            },
            now(),
            "unresolved hardlink membership",
        )];
        assert_eq!(evidence_warnings(&bounded).len(), 1);
    }
}

#[cfg(test)]
mod human_format_tests {
    use super::{age_label, human_bytes_pub, human_count};

    #[test]
    fn modification_ages_share_coarse_buckets_and_preserve_skew_handling() {
        assert_eq!(age_label(1, 1), "<1m ago");
        assert_eq!(age_label(0, 1), "age unknown");
        assert_eq!(
            age_label(10_000_000, 1),
            "modified in the future (clock skew?)"
        );
        assert_eq!(age_label(100, 100 + 30 * 86_400), "1mo ago");
        assert_eq!(age_label(100, 100 + 365 * 86_400), "1y ago");
    }

    #[test]
    fn byte_values_are_coarse_and_carry_rounded_unit_boundaries() {
        assert_eq!(human_bytes_pub(999), "999B");
        assert_eq!(human_bytes_pub(1_000), "1KB");
        assert_eq!(human_bytes_pub(1_499), "1KB");
        assert_eq!(human_bytes_pub(1_500), "2KB");
        assert_eq!(human_bytes_pub(999_499), "999KB");
        assert_eq!(human_bytes_pub(999_500), "1.0MB");
        assert_eq!(human_bytes_pub(1_250_000), "1.3MB");
        assert_eq!(human_bytes_pub(1_000_000_000_000), "1.0TB");
        assert_eq!(human_bytes_pub(u64::MAX), "18.4EB");
    }

    #[test]
    fn counts_remain_exact_and_grouped() {
        assert_eq!(human_count(0), "0");
        assert_eq!(human_count(999), "999");
        assert_eq!(human_count(1_000), "1,000");
        assert_eq!(human_count(u64::MAX), "18,446,744,073,709,551,615");
    }
}

//! Flattens a [`Report`] into displayable rows per view. Pure functions:
//! no I/O, no ratatui types, so this is unit-testable on its own.

use crate::filter::{self, Filter};
use crate::units::UnitId;
use std::collections::BTreeMap;
use swamp_core::report::{ArtifactKind, Report, UnownedReason};

/// Byte formatting is defined once, in core, so the TUI and the CLI can
/// never disagree about what "1.8GB" means (they did: one divided by 1024
/// under a decimal label while the other divided by 1000).
pub use swamp_core::render::human_bytes_pub as human_bytes;

pub use swamp_core::render::human_bytes_signed as human_signed_bytes;

/// Truncates `s` to `width` chars, keeping the tail: `foo…bar` rather
/// than `foo…`, per DESIGN.md ("truncated with `…` in the middle,
/// keeping the tail").
pub fn truncate_middle(s: &str, width: usize) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    if display_width(s) <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let head_budget = (width.saturating_sub(1) / 8).min(6);
    let mut head = String::new();
    for g in s.graphemes(true) {
        if display_width(&head) + display_width(g) > head_budget {
            break;
        }
        head.push_str(g);
    }
    let tail_budget = width - display_width(&head) - 1;
    let mut tail = String::new();
    for g in s.graphemes(true).rev() {
        if display_width(&tail) + display_width(g) > tail_budget {
            break;
        }
        tail.insert_str(0, g);
    }
    format!("{head}…{tail}")
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

/// `g` / `s`: sort the current view's rows by growth or by size. `None`
/// keeps report order (the tree view ignores sort -- its rows are a
/// hierarchy, not a flat ranked list).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    None,
    Growth,
    Size,
    Name,
    /// Grouped by ecosystem (tags in table order), then by size.
    Type,
    /// Oldest first: time since the unit was last written.
    Age,
}

/// One renderable line: a diffstat row. `unit` is set when this row is a
/// single artifact that Backspace can act on (whether or not it is
/// currently markable — refusal is decided at mark time so the reason is
/// specific).
/// Facts a worktree row carries for the mark/remove decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeMark {
    pub path: std::path::PathBuf,
    pub linked: bool,
    /// The project's remote, when it has one: a whole checkout may only be
    /// archived if there is somewhere to restore it from.
    pub remote: Option<String>,
    pub dirty: Option<bool>,
    pub unpushed: Option<u32>,
    pub locked: Option<bool>,
    pub merge_complete: bool,
    pub pr: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub depth: usize,
    /// Box-drawing rail prefix (`├─ `, `└─ `, `│  `, ...) plus, for an
    /// expandable row, the collapse/expand glyph. Empty for a top-level
    /// (depth 0) row: those views have no tree to draw a rail for.
    pub rail: String,
    pub label: String,
    pub bytes: u64,
    pub growth: Option<i64>,
    /// Individual signal values (e.g. `"dirty"`, `"14m"`), rendered as a
    /// joined phrase at wide widths or a single glyph column at 80x24
    /// (DESIGN.md: "signals drop to a single glyph column").
    pub signals: Vec<String>,
    pub unit: Option<UnitId>,
    pub kind: Option<ArtifactKind>,
    /// Set on a worktree row: what Backspace needs to decide whether this
    /// worktree may be marked for removal, and the terms to record.
    pub worktree: Option<WorktreeMark>,
    /// git tracking status of this path, when known: tracked / ignored /
    /// untracked. Untracked bytes are in no version control and covered by
    /// no ignore rule — the fact that most changes what a human decides.
    pub track: Option<swamp_core::ignore::TrackState>,
    /// Byte history over the growth window, from the store (sparkline).
    pub series: Option<Vec<Option<u64>>>,
    /// Glyph badges drawn before the name: ecosystem glyphs, 🐳 when the
    /// project has Docker objects joined, 🔨 when it holds build output,
    /// ⎇ N for N linked worktrees. Empty for rows without facts to badge.
    pub badges: String,
    /// Ecosystem tags, for the type sort.
    pub ecosystems: Vec<String>,
    /// Newest mtime inside the unit (0 unknown), for the age sort/filter.
    pub mtime_max: u64,
    /// Present for a worktree row: how many artifact children are hidden
    /// because the row is collapsed.
    pub collapsed_children: Option<usize>,
    pub expandable: bool,
    pub expansion_key: Option<String>,
    pub cleanup_summary: Option<String>,
    pub allocated: bool,
    /// Set on a projects-view row: the project's own name (not its
    /// display name), so marking can expand the row into that project's
    /// artifacts without parsing the rendered label back into an
    /// identity.
    pub project: Option<String>,
    /// Decision evidence (#53/#60) for the row's own unit -- an
    /// `ArtifactRow`/`ExternalUnit`/`AgentUnit`'s already-populated
    /// `evidence`, cloned at row-build time (never a fresh scan). Empty
    /// for a structural row with no single unit backing it (a project
    /// header, a worktree row, an aggregated kind/type bucket).
    pub evidence: Vec<swamp_core::evidence::Evidence>,
}

impl Row {
    fn leaf(depth: usize, label: String, bytes: u64, growth: Option<i64>) -> Self {
        Row {
            depth,
            rail: String::new(),
            label,
            bytes,
            growth,
            signals: Vec::new(),
            unit: None,
            kind: None,
            worktree: None,
            track: None,
            series: None,
            badges: String::new(),
            ecosystems: Vec::new(),
            mtime_max: 0,
            collapsed_children: None,
            expandable: false,
            expansion_key: None,
            cleanup_summary: None,
            allocated: false,
            project: None,
            evidence: Vec::new(),
        }
    }
}

/// Display width of a string in terminal cells (emoji count as 2), the
/// only correct way to pad a column that holds glyph badges.
pub fn display_width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(s)
}

/// Pads or truncates `s` to exactly `width` display cells.
pub fn pad_display(s: &str, width: usize) -> String {
    let s = truncate_middle(s, width);
    format!("{s}{}", " ".repeat(width.saturating_sub(display_width(&s))))
}

/// The badge string for a set of ecosystem tags plus project facts.
pub fn badges(
    ecosystems: &[String],
    docker: bool,
    builds: bool,
    linked_worktrees: usize,
) -> String {
    let mut b: String = ecosystems
        .iter()
        .take(3)
        .map(|t| swamp_core::ecosystem::glyph_for(t))
        .collect::<Vec<_>>()
        .join(" ");
    if docker && !ecosystems.iter().any(|t| t == "docker") {
        if !b.is_empty() {
            b.push(' ');
        }
        b.push('🐳');
    }
    if builds {
        if !b.is_empty() {
            b.push(' ');
        }
        b.push('🔨');
    }
    if linked_worktrees > 0 {
        if !b.is_empty() {
            b.push(' ');
        }
        // Give the branching glyph breathing room: terminal fonts can
        // overhang its cell and crowd the first digit, especially in bold.
        b.push_str(&format!("⎇ {linked_worktrees}"));
    }
    b
}

fn passes_filter(growth: Option<i64>, filter: &Filter) -> bool {
    filter::growth_passes(filter, growth)
}

/// Bytes below which a change is noise on a disk of this size: drawn as
/// a tick and never a bar, and the number beside it is dimmed.
pub const NOISE_FLOOR: i64 = 1_000_000;

/// One row's change as a diverging bar around a fixed centre axis:
/// shrink extends left, growth extends right, so the direction is the
/// geometry and the colour only reinforces it. Length is logarithmic
/// over `max_abs`, because a linear scale across four orders of
/// magnitude renders everything below the largest row as the same
/// one-cell sliver — 107 MB and 3 MB looked identical while 13.6 GB
/// filled the column.
///
/// Returns `(left, axis, right)`: the caller styles the two sides
/// separately. Each side is `half` cells wide; `axis` is one cell.
pub fn diverging_bar(growth: Option<i64>, max_abs: i64, half: usize) -> (String, char, String) {
    // Eighth-blocks for the right side, which fills away from the axis,
    // and one half-block for the left, which fills toward it. Only
    // U+2580..U+259F here: the U+1FB8x "eighth block" range that would
    // mirror the steps exactly is Unicode 13 and renders as tofu in many
    // terminals, which is worse than a coarser left edge.
    const STEPS: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
    const PARTIAL_L: char = '▐';
    let blank = " ".repeat(half);
    let g = growth.unwrap_or(0);
    if half == 0 {
        return (String::new(), '│', String::new());
    }
    if g == 0 || max_abs <= 0 {
        return (blank.clone(), '│', blank);
    }
    let magnitude = g.unsigned_abs() as f64;
    let grew = g > 0;

    // Below the floor: one tick hugging the axis, never scaled. A change
    // nobody would act on must not look like one that someone would.
    if magnitude < NOISE_FLOOR as f64 {
        return if grew {
            (blank, '│', STEPS[0].to_string() + &" ".repeat(half - 1))
        } else {
            (" ".repeat(half - 1) + &PARTIAL_L.to_string(), '│', blank)
        };
    }

    // log1p over the range, so the floor is a visible nub rather than
    // nothing and the largest change is exactly full.
    let span =
        ((max_abs.max(NOISE_FLOOR) as f64).ln() - (NOISE_FLOOR as f64).ln()).max(f64::EPSILON);
    let frac = ((magnitude.ln() - (NOISE_FLOOR as f64).ln()) / span).clamp(0.0, 1.0);
    let eighths = ((frac * half as f64) * 8.0).round().max(1.0) as usize;
    let full = (eighths / 8).min(half);
    let rem = if full == half { 0 } else { eighths % 8 };

    if grew {
        let mut bar = "█".repeat(full);
        if rem > 0 {
            bar.push(STEPS[rem - 1]);
        }
        let pad = half.saturating_sub(bar.chars().count());
        (blank, '│', bar + &" ".repeat(pad))
    } else {
        // Anchored at the axis: the partial cell is the outer one, inked
        // on its inner edge so the bar stays continuous.
        let partial = if rem > 0 { 1 } else { 0 };
        let pad = half.saturating_sub(full + partial);
        let mut bar = " ".repeat(pad);
        if partial == 1 {
            bar.push(PARTIAL_L);
        }
        bar.push_str(&"█".repeat(full));
        (bar, '│', blank)
    }
}

/// Whether this change is below the noise floor, so the row's number can
/// be dimmed with it.
pub fn is_noise(growth: Option<i64>) -> bool {
    growth.is_some_and(|g| g != 0 && g.unsigned_abs() < NOISE_FLOOR as u64)
}

/// Per-bucket change of a byte series: `series[i] - series[i-1]`, `None`
/// where either side was unobserved. This is what a row's sparkline
/// draws: *when* bytes moved and how much, not how big the tree is (a
/// 16 GB tree drawn as size is a solid brick that says nothing).
pub fn deltas(series: &[Option<u64>]) -> Vec<Option<i64>> {
    series
        .windows(2)
        .map(|w| match (w[0], w[1]) {
            (Some(a), Some(b)) => Some(b as i64 - a as i64),
            _ => None,
        })
        .collect()
}

/// Downsample deltas to `width` points. Each point is the bin's change
/// of largest magnitude, sign kept, so a spike survives; a bin with no
/// observation stays `None`.
pub fn spark_deltas(series: &[Option<u64>], width: usize) -> Vec<Option<i64>> {
    let d = deltas(series);
    if d.is_empty() || width == 0 {
        return Vec::new();
    }
    if d.len() <= width {
        return d;
    }
    (0..width)
        .map(|i| {
            let lo = i * d.len() / width;
            let hi = ((i + 1) * d.len() / width).max(lo + 1);
            d[lo..hi.min(d.len())]
                .iter()
                .filter_map(|v| *v)
                .max_by_key(|v| v.unsigned_abs())
        })
        .collect()
}

/// A series is flat when nothing moved between any two observations:
/// nothing worth a glyph.
pub fn is_flat(series: &[Option<u64>]) -> bool {
    deltas(series).iter().all(|d| d.is_none_or(|d| d == 0))
}

/// Net change over the observed part of a series: last minus first
/// observed value. `None` with fewer than two observations.
pub fn net_change(series: &[Option<u64>]) -> Option<i64> {
    let mut it = series.iter().filter_map(|v| *v);
    let first = it.next()? as i64;
    let last = it.next_back()? as i64;
    Some(last - first)
}

/// Trend of a series: +1 rising, -1 falling, 0 flat (first vs last observed).
pub fn trend(series: &[Option<u64>]) -> i8 {
    match net_change(series) {
        Some(d) if d > 0 => 1,
        Some(d) if d < 0 => -1,
        _ => 0,
    }
}

pub fn max_abs_growth<'a>(rows: impl Iterator<Item = &'a Row>) -> i64 {
    rows.filter_map(|r| r.growth)
        .map(|g| g.abs())
        .max()
        .unwrap_or(0)
}

/// Applies `sort` to a flat (non-hierarchical) row list. Growth and size
/// sort largest first, name alphabetical, type grouped by first ecosystem
/// tag in table order then size, age oldest first (unknown age last). A
/// stable sort keeps report order as the tiebreak; `reverse` flips the
/// whole order.
pub fn apply_sort(rows: &mut [Row], sort: Sort, reverse: bool) {
    fn type_rank(r: &Row) -> usize {
        r.ecosystems
            .first()
            .and_then(|t| {
                swamp_core::ecosystem::ECOSYSTEMS
                    .iter()
                    .position(|e| e.tag == t)
            })
            .unwrap_or(usize::MAX)
    }
    match sort {
        Sort::None => {}
        // Signed, not by magnitude: the question "what grew" is answered
        // by what arrived, and bytes that left are the opposite of the
        // answer. Sorting by magnitude put a project that shrank by 3GB
        // above one that grew by 1GB, at the top of a screen the human
        // is reading for things to delete. Shrinkage sorts last.
        Sort::Growth => {
            rows.sort_by_key(|a| std::cmp::Reverse(a.growth.unwrap_or(0)));
        }
        Sort::Size => rows.sort_by_key(|a| std::cmp::Reverse(a.bytes)),
        Sort::Name => rows.sort_by(|a, b| {
            // Labels may carry ecosystem tags ("[rs][js] owner/repo");
            // sort on the name after the last tag so tags don't cluster rows.
            let key = |l: &str| l.rsplit("] ").next().unwrap_or(l).to_lowercase();
            key(&a.label).cmp(&key(&b.label))
        }),
        Sort::Type => rows.sort_by(|a, b| {
            type_rank(a)
                .cmp(&type_rank(b))
                .then_with(|| b.bytes.cmp(&a.bytes))
        }),
        Sort::Age => rows.sort_by(|a, b| {
            let key = |r: &Row| {
                if r.mtime_max == 0 {
                    u64::MAX
                } else {
                    r.mtime_max
                }
            };
            key(a).cmp(&key(b))
        }),
    }
    if reverse && sort != Sort::None {
        rows.reverse();
    }
}

/// Projects view: one row per project, aggregated bytes/growth.
pub fn projects_rows(report: &Report, filter: &Filter) -> Vec<Row> {
    let mut out = Vec::new();
    for p in &report.projects {
        if let Some(name) = filter::project_name(filter)
            && !p
                .name
                .to_ascii_lowercase()
                .contains(&name.to_ascii_lowercase())
        {
            continue;
        }
        // `type:` predicates are facts about the project.
        if !filter::type_passes(filter, p) {
            continue;
        }
        // Worktree-level predicates (idle, merge-complete, pr) hold for a
        // project when at least one of its worktrees satisfies them.
        if filter::has_worktree_predicates(filter)
            && !p.worktrees.iter().any(|wt| worktree_passes(filter, wt))
        {
            continue;
        }
        let mut bytes = 0u64;
        let mut growth = 0i64;
        let mut have_growth = false;
        for wt in &p.worktrees {
            for a in &wt.artifacts {
                bytes += a.bytes;
                if let Some(g) = a.growth_bytes {
                    growth += g;
                    have_growth = true;
                }
            }
        }
        let growth = have_growth.then_some(growth);
        if !passes_filter(growth, filter) || !filter::size_passes(filter, bytes) {
            continue;
        }
        let mtime_max = p
            .worktrees
            .iter()
            .flat_map(|wt| wt.artifacts.iter())
            .map(|a| a.mtime_max)
            .max()
            .unwrap_or(0);
        if filter::has_age_predicate(filter)
            && !p
                .worktrees
                .iter()
                .flat_map(|wt| wt.artifacts.iter())
                .any(|a| filter::age_passes(filter, a.mtime_max))
        {
            continue;
        }
        let docker = p
            .worktrees
            .iter()
            .flat_map(|wt| wt.artifacts.iter())
            .any(|a| {
                matches!(
                    a.kind,
                    ArtifactKind::DockerImage
                        | ArtifactKind::DockerBuildCache
                        | ArtifactKind::DockerVolume
                )
            });
        let builds = p
            .worktrees
            .iter()
            .flat_map(|wt| wt.artifacts.iter())
            .any(|a| a.kind == ArtifactKind::BuildOutput);
        let linked = p
            .worktrees
            .iter()
            .filter(|w| w.kind == swamp_core::report::WorktreeKind::Linked)
            .count();
        let series = sum_series(p.worktrees.iter().flat_map(|wt| {
            wt.artifacts.iter().filter_map(move |a| {
                let rel = a
                    .path
                    .strip_prefix(&wt.path)
                    .map(|r| r.display().to_string())
                    .unwrap_or_default();
                report.series_by_key.get(&swamp_core::growth::series_key(
                    &p.project_id,
                    &wt.worktree_id,
                    &format!("{:?}", a.kind),
                    &rel,
                ))
            })
        }));
        out.push(Row {
            depth: 0,
            rail: String::new(),
            label: project_display_name(p),
            bytes,
            growth,
            signals: Vec::new(),
            unit: None,
            kind: None,
            worktree: None,
            track: None,
            series,
            badges: badges(&p.ecosystems, docker, builds, linked),
            ecosystems: p.ecosystems.clone(),
            mtime_max,
            collapsed_children: None,
            expandable: true,
            expansion_key: None,
            cleanup_summary: None,
            allocated: false,
            project: Some(p.name.clone()),
            // No single unit backs a project header (it aggregates every
            // worktree/artifact below it); drill into the tree/worktree
            // rows for evidence, same as every other per-project fact.
            evidence: Vec::new(),
        });
    }
    out
}

/// Tree view for one project: checkout/worktree -> (folded) artifact
/// rows, built from the same [`swamp_core::tree::build_project_tree`]
/// the CLI's `--project` drill uses, so the two never drift apart (#33).
/// This function only adds the TUI-specific rail glyphs
/// (`├─`/`└─`/`│`, DESIGN.md's graph-rail grammar), the collapse/expand
/// state, and unit ids for marking. `collapsed` names worktree paths (as
/// strings) currently collapsed.
pub fn tree_rows(
    report: &Report,
    project_name: &str,
    filter: &Filter,
    collapsed: &std::collections::HashSet<String>,
    track: &std::collections::HashMap<std::path::PathBuf, swamp_core::ignore::TrackState>,
) -> Vec<Row> {
    tree_rows_with_agents(report, project_name, filter, collapsed, track, &[])
}

/// Same as [`tree_rows`], additionally appending the collapsed "Agent
/// storage (linked)" summary row(s) #100 requires when `agent_units`
/// names any unit linked to this project. The row is informational
/// only (`unit: None`): acting on agent storage stays the dedicated
/// Agents view's job (full per-unit protections/occupancy checks), not
/// something the project tree can mark.
pub fn tree_rows_with_agents(
    report: &Report,
    project_name: &str,
    filter: &Filter,
    collapsed: &std::collections::HashSet<String>,
    track: &std::collections::HashMap<std::path::PathBuf, swamp_core::ignore::TrackState>,
    agent_units: &[swamp_core::agents::AgentUnit],
) -> Vec<Row> {
    let mut out = Vec::new();
    let Some(p) = report.projects.iter().find(|p| p.name == project_name) else {
        return out;
    };
    let tree = swamp_core::tree::build_project_tree(p, &report.root, agent_units);
    let wt_count = tree.worktrees.len();
    for (wi, wt) in tree.worktrees.iter().enumerate() {
        // Look up the underlying `WorktreeRow` for its absolute path (the
        // tree model's `rel_path` is relative, but marking/collapse keys
        // and folded-row unit ids need a real path on disk).
        let Some(source_wt) = p.worktrees.iter().find(|w| w.worktree_id == wt.worktree_id) else {
            continue;
        };
        if !worktree_passes(filter, source_wt) {
            continue;
        }
        let wt_last = wi + 1 == wt_count;
        let wt_key = source_wt.path.display().to_string();
        let is_collapsed = collapsed.contains(&wt_key);
        let signals: Vec<String> = wt.signals.iter().map(|s| s.value.clone()).collect();
        let wt_connector = if wt_last { "└─ " } else { "├─ " };
        let expand_glyph = if is_collapsed { "▸" } else { "▾" };
        let raw = source_wt.raw_signals();
        let mark = WorktreeMark {
            path: source_wt.path.clone(),
            linked: matches!(wt.kind, swamp_core::report::WorktreeKind::Linked),
            remote: p.remote.clone(),
            dirty: raw.0,
            unpushed: raw.1,
            locked: raw.2,
            merge_complete: source_wt
                .merge_complete
                .as_ref()
                .is_some_and(|m| m.verdict == swamp_core::github::TriState::Yes),
            pr: source_wt
                .github
                .as_ref()
                .and_then(|g| match &g.pull_request {
                    swamp_core::github::PrStatus::Some(pr) => {
                        Some(format!("PR #{} {:?}", pr.number, pr.state).to_lowercase())
                    }
                    _ => None,
                }),
        };
        let wt_series = sum_series(source_wt.artifacts.iter().filter_map(|a| {
            let rel = a
                .path
                .strip_prefix(&source_wt.path)
                .map(|r| r.display().to_string())
                .unwrap_or_default();
            report.series_by_key.get(&swamp_core::growth::series_key(
                &p.project_id,
                &source_wt.worktree_id,
                &format!("{:?}", a.kind),
                &rel,
            ))
        }));
        out.push(Row {
            depth: 1,
            rail: format!("{wt_connector}{expand_glyph} "),
            label: format!(
                "{} {}",
                format!("{:?}", wt.kind).to_lowercase(),
                source_wt.path.display()
            ),
            bytes: wt.bytes,
            growth: wt.growth_bytes,
            signals,
            unit: Some(UnitId::for_artifact(&source_wt.path)),
            kind: None,
            worktree: Some(mark),
            track: None,
            series: wt_series,
            badges: String::new(),
            ecosystems: Vec::new(),
            mtime_max: 0,
            collapsed_children: is_collapsed.then_some(wt.rows.len()),
            expandable: !wt.rows.is_empty(),
            expansion_key: Some(wt_key.clone()),
            cleanup_summary: None,
            allocated: false,
            project: None,
            // #60: the worktree's own `Source` row is where
            // `consumer_wiring::attach_associations` (#56/#57) attaches
            // this project's toolchain-declaration/dependency-lockfile
            // facts -- surfacing it here means selecting the worktree
            // itself, not just one artifact under it, shows them.
            evidence: source_wt
                .artifacts
                .iter()
                .find(|a| a.kind == ArtifactKind::Source)
                .map(|a| a.evidence.clone())
                .unwrap_or_default(),
        });
        if is_collapsed {
            continue;
        }
        let child_prefix = if wt_last { "   " } else { "│  " };
        let visible: Vec<&swamp_core::tree::TreeRow> = wt
            .rows
            .iter()
            .filter(|row| {
                filter::kind_passes(filter, row.kind_label, row.kind.as_ref())
                    && passes_filter(row.growth_bytes, filter)
                    && filter::size_passes(filter, row.bytes)
                    && (row.kind_label == "source" || filter::age_passes(filter, row.mtime_max))
            })
            .collect();
        let n = visible.len();
        for (ri, row) in visible.into_iter().enumerate() {
            let r_last = ri + 1 == n;
            let connector = if r_last { "└─ " } else { "├─ " };
            let label = if row.folded_count > 1 {
                format!(
                    "{} {} (x{})",
                    row.kind_label, row.rel_path, row.folded_count
                )
            } else {
                format!("{} {}", row.kind_label, row.rel_path)
            };
            let abs = source_wt.path.join(&row.rel_path);
            let series_key = swamp_core::growth::series_key(
                &p.project_id,
                &source_wt.worktree_id,
                &row.kind
                    .as_ref()
                    .map(|k| format!("{k:?}"))
                    .unwrap_or_default(),
                &row.rel_path,
            );
            let series = report.series_by_key.get(&series_key).cloned();
            let is_source = row.kind_label == "source";
            let source_key = format!("source:{}", source_wt.path.display());
            let source_collapsed = collapsed.contains(&source_key);
            let children: Vec<&swamp_core::report::DirRollup> = if is_source {
                source_children(report, &source_wt.worktree_id)
            } else {
                Vec::new()
            };
            let mut out_row = Row::leaf(2, label, row.bytes, row.growth_bytes);
            out_row.rail = format!(
                "{child_prefix}{connector}{}",
                if is_source && !children.is_empty() {
                    if source_collapsed { "▸ " } else { "▾ " }
                } else {
                    ""
                }
            );
            out_row.kind = row.kind.clone();
            out_row.series = series;
            out_row.mtime_max = row.mtime_max;
            if let Some(t) = &row.ecosystem {
                out_row.badges = swamp_core::ecosystem::glyph_for(t).to_string();
                out_row.ecosystems = vec![t.clone()];
            }
            // `.git` is git's own store, not content it tracks: annotating
            // it "untracked" is noise, so it carries no status.
            out_row.track = (row.kind_label != "git")
                .then(|| track.get(&abs).copied())
                .flatten();
            out_row.expandable = is_source && !children.is_empty();
            out_row.expansion_key = out_row.expandable.then_some(source_key.clone());
            out_row.collapsed_children = (is_source && source_collapsed).then_some(children.len());
            // A folded group of several artifacts has no single owning
            // path to mark; only an unfolded row is markable.
            if row.folded_count == 1 {
                out_row.unit = Some(UnitId::for_artifact(&abs));
                // #60: a folded group has no single evidence source, but
                // an unfolded row maps to exactly one `ArtifactRow` --
                // look it up by its already-known absolute path rather
                // than adding an `evidence` field to `TreeRow` for a
                // fact this same worktree's `artifacts` already holds.
                if let Some(a) = source_wt.artifacts.iter().find(|a| a.path == abs) {
                    out_row.evidence = a.evidence.clone();
                }
            }
            // Any row an adapter identified the interior of expands,
            // not only `BuildOutput`. An installed dependency tree is a
            // `DependencyTree` row, and `node_modules` is the row a
            // person most often wants to open.
            let identified_interior = report
                .nested_artifacts
                .iter()
                .any(|u| u.path.starts_with(&abs) && u.path != abs && u.adapter.is_some());
            // How the interior is presented follows the roles its units
            // carry, never an adapter id: a container in the neutral
            // vocabulary gets family groups, one whose units the Cargo
            // cleanup module speaks for keeps its purpose groups.
            let neutral_interior = identified_interior
                && !report.nested_artifacts.iter().any(|u| {
                    u.path.starts_with(&abs) && swamp_core::cargo_cleanup::speaks_for(&u.role)
                });
            let cargo_children = if neutral_interior {
                family_tree_children(
                    report,
                    &abs,
                    3,
                    &format!("{child_prefix}{}", if r_last { "   " } else { "│  " }),
                    collapsed,
                )
            } else if row.kind == Some(ArtifactKind::BuildOutput)
                || identified_interior
                || (row.folded_count == 1
                    && report.nested_artifacts.iter().any(|u| {
                        u.path == abs && u.role == swamp_core::artifact::ArtifactRole::Container
                    }))
            {
                cargo_tree_children(
                    report,
                    &abs,
                    3,
                    &format!("{child_prefix}{}", if r_last { "   " } else { "│  " }),
                    collapsed,
                )
            } else {
                Vec::new()
            };
            if !cargo_children.is_empty() {
                let key = format!("cargo:{}", abs.display());
                let closed = collapsed.contains(&key);
                out_row.expandable = true;
                out_row.expansion_key = Some(key);
                out_row.rail = format!(
                    "{child_prefix}{connector}{} ",
                    if closed { "▸" } else { "▾" }
                );
                out_row.collapsed_children = closed.then_some(cargo_children.len());
                out_row.signals = vec!["build groups below · allocated, not freeable bytes".into()];
                out.push(out_row);
                if !closed {
                    out.extend(cargo_children);
                }
            } else {
                out.push(out_row);
            }
            // A Source tree is one row only because nothing inside it is a
            // classified artifact -- which is exactly when its contents are
            // worth seeing. Expanded, it lists its own top-level
            // directories with their git tracking status.
            if is_source && !source_collapsed {
                let shown = children.len().min(8);
                for (ci, d) in children.iter().take(shown).enumerate() {
                    let c_last = ci + 1 == shown && children.len() <= shown;
                    let c_connector = if c_last { "└─ " } else { "├─ " };
                    let dir_abs = source_wt.path.join(&d.rel_path);
                    let mut child =
                        Row::leaf(3, format!("dir {}", d.rel_path), d.allocated_total, None);
                    child.rail = format!("{child_prefix}   {c_connector}");
                    child.track = track.get(&dir_abs).copied();
                    child.unit = Some(UnitId::for_artifact(&dir_abs));
                    child.kind = Some(swamp_core::report::ArtifactKind::Unknown);
                    out.push(child);
                }
                if children.len() > shown {
                    let rest: u64 = children.iter().skip(shown).map(|d| d.allocated_total).sum();
                    let mut more = Row::leaf(
                        3,
                        format!("… and {} more directories", children.len() - shown),
                        rest,
                        None,
                    );
                    more.rail = format!("{child_prefix}   └─ ");
                    out.push(more);
                }
            }
        }
    }
    if !tree.agent_rows.is_empty() {
        let total_bytes: u64 = tree.agent_rows.iter().map(|r| r.bytes).sum();
        let total_growth = tree
            .agent_rows
            .iter()
            .filter_map(|r| r.growth_bytes)
            .reduce(|a, b| a + b);
        let tools = tree
            .agent_rows
            .iter()
            .map(|r| r.tool_name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let mut row = Row::leaf(
            1,
            format!("agent storage (linked)   {tools}"),
            total_bytes,
            total_growth,
        );
        row.rail = "└─ ".into();
        // Deliberately no `unit`/`kind`: this collapsed summary row is
        // never a mark target from the project tree (see doc comment
        // above) -- the Agents view is where a specific unit's own
        // protections/occupancy are checked before it can be marked.
        out.push(row);
    }
    out
}

pub use swamp_core::render::project_display_name;

/// How many members an expanded family group lists before summarizing
/// the rest in one "… and N more" row. A `node_modules` with thousands of
/// packages is one group, not thousands of rows.
const FAMILY_MEMBERS_SHOWN: usize = 25;

/// The interior of a container identified in the ecosystem-neutral role
/// vocabulary (Node, Gradle, Maven, ...): one collapsed row per role
/// family, then one "Not identified" row for what no supported unit
/// accounts for.
///
/// Each group row leads with **review guidance and what removing it
/// costs** (`family_guidance`), then the count and oldest modification,
/// so a narrow terminal gives up the numbers before the meaning -- the
/// same ordering as Cargo's purpose groups. The adapter's own
/// consequence, the accounting basis and the action capability are the
/// row's signals. Groups open on demand (they start closed): a family is
/// the answer to "what is this made of", and its members are the
/// follow-up question.
///
/// Nothing here is selectable. No neutral-vocabulary adapter has an
/// executor yet (#73), so every row carries the `blocked` signal and no
/// `UnitId`, which is what keeps the confirmation path unreachable.
fn family_tree_children(
    report: &Report,
    container: &std::path::Path,
    depth: usize,
    prefix: &str,
    collapsed: &std::collections::HashSet<String>,
) -> Vec<Row> {
    family_tree_children_of(
        &report.nested_artifacts,
        report.observed_at,
        container,
        depth,
        prefix,
        collapsed,
    )
}

/// [`family_tree_children`] over any set of units: a project container's,
/// a machine-wide store's (`ViewKind::External`) or a BuildKit builder's
/// (`ViewKind::Docker`). One presentation for every interior.
fn family_tree_children_of(
    all: &[swamp_core::artifact::NestedArtifact],
    observed_at: u64,
    container: &std::path::Path,
    depth: usize,
    prefix: &str,
    collapsed: &std::collections::HashSet<String>,
) -> Vec<Row> {
    use swamp_core::build_adapters::{family_members, summarize_container};
    let units: Vec<swamp_core::artifact::NestedArtifact> = all
        .iter()
        .filter(|u| u.present && u.path.starts_with(container))
        .cloned()
        .collect();
    let summary = summarize_container(container, &units);
    let residual = summary.unsupported_bytes.unwrap_or(0) + summary.unaccounted_bytes.unwrap_or(0);
    let show_residual = summary.unsupported_count > 0 || residual > 0;
    let group_count = summary.families.len() + usize::from(show_residual);
    let mut rows = Vec::new();
    for (i, f) in summary.families.iter().enumerate() {
        let last = i + 1 == group_count;
        let key = format!("family-open:{}:{}", container.display(), f.family.label());
        let open = collapsed.contains(&key);
        let members = family_members(container, &units, f.family);
        let mut row = Row::leaf(depth, f.family.title().to_string(), f.bytes, None);
        row.rail = format!(
            "{prefix}{}{}",
            if last { "└─ " } else { "├─ " },
            if open { "▾ " } else { "▸ " }
        );
        row.expandable = true;
        row.expansion_key = Some(key);
        row.collapsed_children = (!open).then_some(members.len());
        row.allocated = true;
        row.mtime_max = f.oldest_modified.unwrap_or(0);
        row.cleanup_summary = Some(format!(
            "{} · {} {} · oldest {}{}",
            f.recommendation,
            f.count,
            if f.count == 1 { "item" } else { "items" },
            match f.oldest_modified {
                Some(t) => age_label(Some(observed_at.saturating_sub(t))),
                None => "unknown".into(),
            },
            if f.unknown_age > 0 {
                format!(" · {} of unknown age", f.unknown_age)
            } else {
                String::new()
            }
        ));
        let mut signals = vec![match (&f.consequence, f.other_consequences) {
            (Some(c), 0) => c.clone(),
            (Some(c), n) => format!("{c} (and {n} other consequences inside)"),
            (None, _) => "consequence not established".into(),
        }];
        let actionable = members
            .iter()
            .filter(|u| u.action == swamp_core::artifact::NestedActionCapability::TrashPath)
            .count();
        signals.push(if actionable > 0 {
            format!("Space marks {actionable} exact paths; remaining items are inspection only")
        } else {
            "inspection only: selective cleanup unsupported here".into()
        });
        signals.push(match f.basis {
            swamp_core::artifact::AccountingBasis::Unknown => {
                "mixed accounting bases: not summed".to_string()
            }
            basis => format!("{} bytes", basis.label()),
        });
        if !f.complete {
            signals.push("measurement incomplete".into());
        }
        if actionable == 0 {
            signals.push("blocked".into());
        }
        row.signals = signals;
        rows.push(row);
        if open {
            let child_prefix = format!("{prefix}{}", if last { "   " } else { "│  " });
            let shown = members.len().min(FAMILY_MEMBERS_SHOWN);
            for (j, u) in members.iter().take(shown).enumerate() {
                let m_last = j + 1 == shown && members.len() <= shown;
                rows.push(family_member_row(
                    u,
                    depth + 1,
                    &child_prefix,
                    m_last,
                    observed_at,
                ));
            }
            if members.len() > shown {
                let rest: u64 = members.iter().skip(shown).map(|u| u.bytes).sum();
                let mut more = Row::leaf(
                    depth + 1,
                    format!("… and {} more", members.len() - shown),
                    rest,
                    None,
                );
                more.rail = format!("{child_prefix}└─ ");
                more.allocated = true;
                more.signals = vec!["blocked".into()];
                rows.push(more);
            }
        }
    }
    if show_residual {
        let mut row = Row::leaf(
            depth,
            swamp_core::artifact::RoleFamily::Residual
                .title()
                .to_string(),
            residual,
            None,
        );
        row.rail = format!("{prefix}└─ ");
        row.allocated = true;
        row.cleanup_summary = Some(format!(
            "{}{}{}",
            swamp_core::build_adapters::family_guidance(swamp_core::artifact::RoleFamily::Residual),
            match summary.unsupported_count {
                0 => String::new(),
                1 => " · 1 unrecognised entry".to_string(),
                n => format!(" · {n} unrecognised entries"),
            },
            match summary.unaccounted_bytes {
                Some(b) if b > 0 => format!(" · {} no unit claims", human_bytes(b)),
                Some(_) => String::new(),
                None => " · remainder not reconciled".into(),
            }
        ));
        row.signals = vec![
            "not identified: an unsupported layout or bytes no unit accounts for".into(),
            "blocked".into(),
        ];
        rows.push(row);
    }
    rows
}

fn family_member_row(
    u: &swamp_core::artifact::NestedArtifact,
    depth: usize,
    prefix: &str,
    last: bool,
    observed_at: u64,
) -> Row {
    let name = u
        .path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let label = match (&u.variant.package, &u.variant.version) {
        (Some(p), Some(v)) if *p != name => format!("{name} ({p}@{v})"),
        (Some(p), None) if *p != name => format!("{name} ({p})"),
        (Some(_), Some(v)) => format!("{name}@{v}"),
        _ => name,
    };
    let mut row = Row::leaf(depth, label, u.bytes, u.growth_bytes);
    row.rail = format!("{prefix}{}", if last { "└─ " } else { "├─ " });
    row.allocated = true;
    row.mtime_max = u.mtime_max;
    let age = (u.time_source != swamp_core::artifact::TimeSource::Unknown && u.mtime_max > 0)
        .then(|| observed_at.saturating_sub(u.mtime_max));
    row.cleanup_summary = Some(format!(
        "{} · {} {}",
        u.consequence
            .clone()
            .unwrap_or_else(|| "consequence not established".into()),
        // A daemon's record time is the daemon's, never a file's.
        if u.reported_by.is_some() {
            "created (daemon)"
        } else {
            "modified"
        },
        match age {
            Some(a) => age_label(Some(a)),
            None => "unknown".into(),
        }
    ));
    let action = match &u.action {
        swamp_core::artifact::NestedActionCapability::TrashPath => {
            "Space marks this exact path for Trash".into()
        }
        swamp_core::artifact::NestedActionCapability::Unsupported { reason } => {
            format!("selective cleanup unsupported: {reason}")
        }
        swamp_core::artifact::NestedActionCapability::InspectionOnly => "inspection only".into(),
    };
    row.signals = vec![
        u.role.label().to_string(),
        action,
        format!(
            "{} bytes ({})",
            u.basis.label(),
            u.adapter
                .clone()
                .unwrap_or_else(|| "unknown adapter".into())
        ),
    ];
    row.signals.extend(u.coverage.limits.iter().cloned());
    if u.action == swamp_core::artifact::NestedActionCapability::TrashPath {
        row.unit = Some(UnitId::for_artifact(&u.path));
        row.kind = Some(ArtifactKind::BuildOutput);
        row.evidence = u.decision_evidence.clone();
    } else {
        row.signals.push("blocked".into());
    }
    row
}

fn cargo_tree_children(
    report: &Report,
    parent: &std::path::Path,
    depth: usize,
    prefix: &str,
    collapsed: &std::collections::HashSet<String>,
) -> Vec<Row> {
    let mut by_parent: BTreeMap<&std::path::Path, Vec<&swamp_core::artifact::NestedArtifact>> =
        BTreeMap::new();
    for unit in report.nested_artifacts.iter().filter(|u| u.present) {
        if let Some(parent) = unit.path.parent() {
            by_parent.entry(parent).or_default().push(unit);
        }
    }
    cargo_children_from_index(
        report,
        &by_parent,
        parent,
        depth,
        prefix,
        collapsed,
        report.observed_at,
    )
}

fn cargo_children_from_index(
    report: &Report,
    by_parent: &BTreeMap<&std::path::Path, Vec<&swamp_core::artifact::NestedArtifact>>,
    parent: &std::path::Path,
    depth: usize,
    prefix: &str,
    collapsed: &std::collections::HashSet<String>,
    observed_at: u64,
) -> Vec<Row> {
    let mut children = by_parent.get(parent).cloned().unwrap_or_default();
    children.sort_by(|a, b| {
        if swamp_core::cargo_cleanup::candidate(a) && swamp_core::cargo_cleanup::candidate(b) {
            swamp_core::cargo_cleanup::cleanup_order(a, b, observed_at)
        } else {
            swamp_core::cargo_cleanup::candidate(b)
                .cmp(&swamp_core::cargo_cleanup::candidate(a))
                .then_with(|| b.bytes.cmp(&a.bytes))
                .then_with(|| a.path.cmp(&b.path))
        }
    });
    let mut rows = Vec::new();
    for (index, unit) in children.iter().enumerate() {
        let last = index + 1 == children.len();
        let key = format!("cargo:{}", unit.path.display());
        let count = by_parent.get(unit.path.as_path()).map_or(0, Vec::len);
        let has_children = count > 0;
        let closed = has_children && collapsed.contains(&key);
        let name = unit.path.file_name().unwrap_or_default().to_string_lossy();
        let mut row = Row::leaf(
            depth,
            if name == unit.role.label() {
                name.into_owned()
            } else {
                format!("{} {name}", unit.role.label())
            },
            unit.bytes,
            unit.growth_bytes,
        );
        row.rail = format!(
            "{prefix}{}{}",
            if last { "└─ " } else { "├─ " },
            if has_children {
                if closed { "▸ " } else { "▾ " }
            } else {
                ""
            }
        );
        row.expandable = has_children;
        row.expansion_key = has_children.then_some(key);
        row.collapsed_children = closed.then_some(count);
        let guidance = swamp_core::cargo_cleanup::guidance_at(unit, observed_at);
        row.signals = vec![guidance.recommendation, guidance.consequence];
        row.allocated = true;
        let (candidates, bytes, oldest) = candidate_summary(by_parent, &unit.path, observed_at);
        let advice = match unit.role {
            swamp_core::artifact::ArtifactRole::Incremental => "Start here: slower next build",
            swamp_core::artifact::ArtifactRole::Dependency
            | swamp_core::artifact::ArtifactRole::TestExecutable
            | swamp_core::artifact::ArtifactRole::Example => "Review: rebuild before rerunning",
            swamp_core::artifact::ArtifactRole::BuildScriptOutput => {
                "Lower priority: reruns scripts"
            }
            swamp_core::artifact::ArtifactRole::Profile => "Review supported groups only",
            _ => "Expand to choose cleanup groups",
        };
        row.cleanup_summary = Some(if unit.bytes == 0 {
            "Empty".into()
        } else if swamp_core::cargo_cleanup::candidate(unit) {
            format!(
                "{advice} · modified {}",
                age_label(swamp_core::cargo_cleanup::modified_age_secs(
                    unit,
                    observed_at
                ))
            )
        } else if candidates > 0 {
            row.signals.push(format!("{candidates} reviewable groups · {} allocated · oldest modification {}. Expand to select groups; not guaranteed freed space.", human_bytes(bytes), age_label(oldest)));
            format!(
                "{advice} · {candidates} {} · {} · oldest {}",
                if candidates == 1 {
                    "candidate"
                } else {
                    "candidates"
                },
                human_bytes(bytes),
                age_label(oldest)
            )
        } else if unit.role == swamp_core::artifact::ArtifactRole::FinalOutput {
            row.signals.insert(
                0,
                "Compiled output: review manually; selective removal not supported here".into(),
            );
            format!(
                "Inspect only: removes built output · modified {}",
                age_label(swamp_core::cargo_cleanup::modified_age_secs(
                    unit,
                    observed_at
                ))
            )
        } else {
            "Selective cleanup unsupported".into()
        });
        row.mtime_max = unit.mtime_max;
        if swamp_core::cargo_cleanup::candidate(unit) {
            row.unit = Some(UnitId::for_artifact(&unit.path));
            row.kind = Some(ArtifactKind::BuildOutput);
        }
        rows.push(row);
        if has_children && !closed {
            let child_prefix = format!("{prefix}{}", if last { "   " } else { "│  " });
            if unit.role == swamp_core::artifact::ArtifactRole::Profile {
                rows.extend(cleanup_group_rows(
                    report,
                    &unit.path,
                    depth + 1,
                    &child_prefix,
                    collapsed,
                ));
                let layout_key = format!("layout:{}", unit.path.display());
                let mut layout =
                    Row::leaf(depth + 1, "Inspect directories".into(), unit.bytes, None);
                layout.rail = format!(
                    "{child_prefix}└─ {}",
                    if collapsed.contains(&layout_key) {
                        "▸ "
                    } else {
                        "▾ "
                    }
                );
                layout.expandable = true;
                layout.expansion_key = Some(layout_key.clone());
                layout.allocated = true;
                layout.cleanup_summary = Some("Same bytes by path; not extra storage".into());
                rows.push(layout);
                if !collapsed.contains(&layout_key) {
                    rows.extend(cargo_children_from_index(
                        report,
                        by_parent,
                        &unit.path,
                        depth + 2,
                        &format!("{child_prefix}   "),
                        collapsed,
                        observed_at,
                    ));
                }
                continue;
            }
            rows.extend(cargo_children_from_index(
                report,
                by_parent,
                &unit.path,
                depth + 1,
                &format!("{prefix}{}", if last { "   " } else { "│  " }),
                collapsed,
                observed_at,
            ));
        }
    }
    rows
}

/// Virtual selections resolve to evidenced, disjoint action units, never a
/// containing directory. Keys are scoped to a single profile and are not stored.
pub(crate) fn cleanup_members<'a>(
    report: &'a Report,
    key: &str,
) -> Vec<&'a swamp_core::artifact::NestedArtifact> {
    use swamp_core::artifact::ArtifactRole as R;
    if let Some((container, family)) = key
        .strip_prefix("family-open:")
        .and_then(|s| s.rsplit_once(':'))
    {
        let Some(family) = report
            .nested_artifacts
            .iter()
            .map(|u| u.role.family())
            .find(|f| f.label() == family)
        else {
            return Vec::new();
        };
        return swamp_core::build_adapters::family_members(
            std::path::Path::new(container),
            &report.nested_artifacts,
            family,
        )
        .into_iter()
        .filter(|u| u.action == swamp_core::artifact::NestedActionCapability::TrashPath)
        .collect();
    }
    let selection = key
        .strip_prefix("cleanup:")
        .and_then(|k| k.split_once(':'))
        .or_else(|| {
            key.strip_prefix("cargo:")
                .filter(|path| {
                    report.nested_artifacts.iter().any(|u| {
                        u.present && u.role == R::Profile && u.path == std::path::Path::new(path)
                    })
                })
                .map(|path| ("profile", path))
        });
    let Some((kind, profile)) = selection else {
        return Vec::new();
    };
    let profile = std::path::Path::new(profile);
    let mut members: Vec<_> = report
        .nested_artifacts
        .iter()
        .filter(|u| {
            u.present
                && u.bytes > 0
                && u.path.starts_with(profile)
                && swamp_core::cargo_cleanup::candidate(u)
                && match kind {
                    "profile" => true,
                    "cache" => u.role == R::Incremental,
                    "runnable" => matches!(u.role, R::TestExecutable | R::Example),
                    "tests" => u.role == R::TestExecutable,
                    "examples" => u.role == R::Example,
                    "scripts" => u.role == R::BuildScriptOutput,
                    _ => false,
                }
        })
        .collect();
    members.sort_by(|a, b| a.path.cmp(&b.path));
    members.dedup_by(|a, b| a.path == b.path);
    let paths: std::collections::HashSet<_> = members
        .iter()
        .filter(|u| u.is_dir)
        .map(|u| u.path.clone())
        .collect();
    members.retain(|u| !u.path.ancestors().skip(1).any(|p| paths.contains(p)));
    members.sort_by(|a, b| swamp_core::cargo_cleanup::cleanup_order(a, b, report.observed_at));
    members
}

pub(crate) fn is_cleanup_selection(report: &Report, key: &str) -> bool {
    (key.starts_with("family-open:") && !cleanup_members(report, key).is_empty())
        || key.starts_with("cleanup:")
        || key.strip_prefix("cargo:").is_some_and(|path| {
            report.nested_artifacts.iter().any(|u| {
                u.present
                    && u.role == swamp_core::artifact::ArtifactRole::Profile
                    && u.path == std::path::Path::new(path)
            })
        })
}

fn cleanup_group_rows(
    report: &Report,
    profile: &std::path::Path,
    depth: usize,
    prefix: &str,
    collapsed: &std::collections::HashSet<String>,
) -> Vec<Row> {
    let mut rows = Vec::new();
    for (kind, label, effect) in [
        ("cache", "Compiler caches", "Start here: slower next build"),
        (
            "runnable",
            "Compiled tests & examples",
            "Rebuild before rerunning",
        ),
        (
            "scripts",
            "Build-script output",
            "Reruns scripts; tools may be needed",
        ),
    ] {
        append_cleanup_group(
            report, profile, kind, label, effect, depth, prefix, false, collapsed, &mut rows,
        );
    }
    rows
}

#[allow(clippy::too_many_arguments)]
fn append_cleanup_group(
    report: &Report,
    profile: &std::path::Path,
    kind: &str,
    label: &str,
    effect: &str,
    depth: usize,
    prefix: &str,
    last: bool,
    collapsed: &std::collections::HashSet<String>,
    rows: &mut Vec<Row>,
) {
    let key = format!("cleanup:{kind}:{}", profile.display());
    let members = cleanup_members(report, &key);
    if members.is_empty() {
        return;
    }
    let bytes = members.iter().map(|u| u.bytes).sum();
    let oldest = members
        .iter()
        .filter_map(|u| swamp_core::cargo_cleanup::modified_age_secs(u, report.observed_at))
        .max();
    let mut row = Row::leaf(depth, label.into(), bytes, None);
    row.rail = format!(
        "{prefix}{}{}",
        if last { "└─ " } else { "├─ " },
        if collapsed.contains(&key) {
            "▸ "
        } else {
            "▾ "
        }
    );
    row.expandable = true;
    row.expansion_key = Some(key.clone());
    row.allocated = true;
    row.cleanup_summary = Some(format!(
        "{effect} · {} groups · oldest {}",
        members.len(),
        age_label(oldest)
    ));
    row.signals = vec![
        effect.into(),
        format!(
            "Space marks {} exact groups for review; → inspects members. Source and unrelated dependencies are not selected. Allocated size is not guaranteed freed space.",
            members.len()
        ),
    ];
    rows.push(row);
    if collapsed.contains(&key) {
        return;
    }
    let prefix = format!("{prefix}{}", if last { "   " } else { "│  " });
    if kind == "runnable" {
        let has_examples =
            !cleanup_members(report, &format!("cleanup:examples:{}", profile.display())).is_empty();
        append_cleanup_group(
            report,
            profile,
            "tests",
            "Tests",
            effect,
            depth + 1,
            &prefix,
            !has_examples,
            collapsed,
            rows,
        );
        append_cleanup_group(
            report,
            profile,
            "examples",
            "Examples",
            effect,
            depth + 1,
            &prefix,
            true,
            collapsed,
            rows,
        );
    } else {
        for (i, u) in members.iter().enumerate() {
            let mut row = Row::leaf(
                depth + 1,
                u.path
                    .strip_prefix(profile)
                    .unwrap_or(&u.path)
                    .display()
                    .to_string(),
                u.bytes,
                u.growth_bytes,
            );
            row.rail = format!(
                "{prefix}{}",
                if i + 1 == members.len() {
                    "└─ "
                } else {
                    "├─ "
                }
            );
            row.unit = Some(UnitId::for_artifact(&u.path));
            row.kind = Some(ArtifactKind::BuildOutput);
            row.allocated = true;
            row.mtime_max = u.mtime_max;
            row.cleanup_summary = Some(format!(
                "{effect} · modified {}",
                age_label(swamp_core::cargo_cleanup::modified_age_secs(
                    u,
                    report.observed_at
                ))
            ));
            row.signals =
                vec![swamp_core::cargo_cleanup::guidance_at(u, report.observed_at).consequence];
            rows.push(row);
        }
    }
}

pub(crate) fn age_label(age: Option<u64>) -> String {
    match age {
        Some(s) if s >= 86400 => format!("{}d", s / 86400),
        Some(s) if s >= 3600 => format!("{}h", s / 3600),
        Some(s) if s >= 60 => format!("{}m", s / 60),
        Some(_) => "<1m".into(),
        None => "?".into(),
    }
}

/// Count only disjoint, selectable descendants. Stop at a candidate directory,
/// so neither summaries nor nested members count the same selected scope twice.
fn candidate_summary(
    by_parent: &BTreeMap<&std::path::Path, Vec<&swamp_core::artifact::NestedArtifact>>,
    parent: &std::path::Path,
    now: u64,
) -> (usize, u64, Option<u64>) {
    let mut result = (0, 0u64, None);
    for child in by_parent.get(parent).into_iter().flatten() {
        let next = if swamp_core::cargo_cleanup::candidate(child) {
            (
                usize::from(child.bytes > 0),
                child.bytes,
                (child.bytes > 0)
                    .then(|| swamp_core::cargo_cleanup::modified_age_secs(child, now))
                    .flatten(),
            )
        } else {
            candidate_summary(by_parent, &child.path, now)
        };
        result.0 += next.0;
        result.1 = result.1.saturating_add(next.1);
        result.2 = result.2.max(next.2);
    }
    result
}

/// Top-level directories of a worktree's Source tree, biggest first.
/// Empty when the report was built without directory rollups.
fn source_children<'a>(
    report: &'a Report,
    worktree_id: &str,
) -> Vec<&'a swamp_core::report::DirRollup> {
    let Some(dirs) = report
        .dirs_by_worktree
        .as_ref()
        .and_then(|m| m.get(worktree_id))
    else {
        return Vec::new();
    };
    let mut top: Vec<&swamp_core::report::DirRollup> = dirs
        .iter()
        .filter(|d| !d.rel_path.is_empty() && d.rel_path != "." && !d.rel_path.contains('/'))
        .collect();
    top.sort_by_key(|a| std::cmp::Reverse(a.allocated_total));
    top
}

/// Kinds view: bytes/count per artifact kind across the whole root.
pub fn kinds_rows(report: &Report, filter: &Filter) -> Vec<Row> {
    let mut totals: BTreeMap<String, (u64, i64, u32)> = BTreeMap::new();
    for p in &report.projects {
        for wt in &p.worktrees {
            for a in &wt.artifacts {
                let e = totals.entry(kind_label(&a.kind).to_string()).or_default();
                e.0 += a.bytes;
                e.1 += a.growth_bytes.unwrap_or(0);
                e.2 += 1;
            }
        }
    }
    totals
        .into_iter()
        .filter(|(k, _)| filter::kind_passes(filter, k, None))
        .map(|(k, (bytes, growth, count))| Row {
            depth: 0,
            rail: String::new(),
            label: format!("{k} ({count})"),
            bytes,
            growth: Some(growth),
            signals: Vec::new(),
            unit: None,
            kind: None,
            worktree: None,
            track: None,
            series: None,
            badges: String::new(),
            ecosystems: Vec::new(),
            mtime_max: 0,
            collapsed_children: None,
            expandable: false,
            expansion_key: None,
            cleanup_summary: None,
            allocated: false,
            project: None,
            evidence: Vec::new(),
        })
        .collect()
}

/// Bytes belonging to Docker objects with no join evidence
/// (`UnownedReason::DockerNoJoin`), reported separately from the rest of
/// unowned per PRODUCT.md's honest-coverage principle.
pub fn docker_unowned_bytes(report: &Report) -> u64 {
    report
        .unowned
        .iter()
        .filter(|u| u.reason == UnownedReason::DockerNoJoin)
        .map(|u| u.bytes)
        .sum()
}

/// Docker view: unowned docker rows plus a per-project docker rollup.
pub fn docker_rows(report: &Report) -> Vec<Row> {
    docker_rows_with(report, &std::collections::HashSet::new())
}

/// [`docker_rows`], plus one row per BuildKit builder with its records
/// in family groups -- sizes the daemon's logical figures, times the
/// daemon's records, nothing selectable.
pub fn docker_rows_with(
    report: &Report,
    collapsed: &std::collections::HashSet<String>,
) -> Vec<Row> {
    let mut out = docker_object_rows(report);
    let mut builders: Vec<&swamp_core::artifact::NestedArtifact> = report
        .nested_artifacts
        .iter()
        .filter(|u| u.reported_by.is_some() && Some(u.id.as_str()) == u.container_id.as_deref())
        .collect();
    builders.sort_by(|a, b| a.path.cmp(&b.path));
    for b in builders {
        let name = b
            .path
            .to_string_lossy()
            .strip_prefix(swamp_core::build_adapters::DAEMON_STORE_SCHEME)
            .unwrap_or_default()
            .to_string();
        let mut row = Row::leaf(
            0,
            format!("buildkit · builder {name} (daemon-reported, logical)"),
            b.bytes,
            None,
        );
        let key = format!("store-open:{}", b.path.display());
        let open = collapsed.contains(&key);
        let children = family_tree_children_of(
            &report.nested_artifacts,
            report.observed_at,
            &b.path,
            1,
            "",
            collapsed,
        );
        row.expandable = !children.is_empty();
        row.expansion_key = Some(key);
        row.rail = if open { "▾ ".into() } else { "▸ ".into() };
        row.collapsed_children = (!open).then_some(children.len());
        row.signals = b.coverage.limits.clone();
        row.signals.push("blocked".into());
        out.push(row);
        if open {
            out.extend(children);
        }
    }
    out
}

fn docker_object_rows(report: &Report) -> Vec<Row> {
    let mut out = Vec::new();
    for p in &report.projects {
        for wt in &p.worktrees {
            for a in &wt.artifacts {
                if !matches!(
                    a.kind,
                    ArtifactKind::DockerImage
                        | ArtifactKind::DockerBuildCache
                        | ArtifactKind::DockerVolume
                ) {
                    continue;
                }
                let mut row = Row::leaf(
                    0,
                    format!("{} · {} {}", p.name, kind_label(&a.kind), a.path.display()),
                    a.bytes,
                    a.growth_bytes,
                );
                row.kind = Some(a.kind.clone());
                row.unit = Some(UnitId::for_artifact(&a.path));
                row.evidence = a.evidence.clone();
                out.push(row);
            }
        }
    }
    for u in &report.unowned {
        if u.reason != UnownedReason::DockerNoJoin {
            continue;
        }
        let mut row = Row::leaf(0, format!("unowned · {}", u.path_or_object), u.bytes, None);
        // An unjoined object is still a real object: it can be acted on,
        // it just belongs to no project. The kind decides what happens.
        row.kind = Some(match u.docker_kind.as_deref() {
            Some("volume") => ArtifactKind::DockerVolume,
            Some("build-cache") => ArtifactKind::DockerBuildCache,
            _ => ArtifactKind::DockerImage,
        });
        row.unit = Some(UnitId::for_artifact(std::path::Path::new(
            &u.path_or_object,
        )));
        row.evidence = u.evidence.clone();
        out.push(row);
    }
    out
}

/// Builds view: every `BuildOutput`/`Cache` row across the whole root,
/// same kind set as the CLI's `--view builds` (#33).
pub fn builds_rows(report: &Report, filter: &Filter) -> Vec<Row> {
    let mut rows = kind_filtered_rows(
        report,
        &[ArtifactKind::BuildOutput, ArtifactKind::Cache],
        filter,
    );
    append_cargo_breakdowns(report, filter, &mut rows);
    append_build_family_breakdowns(report, filter, &mut rows);
    rows
}

/// Deps view: every `DependencyTree` row across the whole root, same as
/// the CLI's `--view deps`.
pub fn deps_rows(report: &Report, filter: &Filter) -> Vec<Row> {
    let mut rows = kind_filtered_rows(report, &[ArtifactKind::DependencyTree], filter);
    // An installed dependency tree is a `DependencyTree` row, not a
    // build row, so its family breakdown belongs here as well as in
    // Builds -- a `node_modules` that is 70% pnpm store is the question
    // this view exists to answer.
    append_build_family_breakdowns(report, filter, &mut rows);
    rows
}

fn kind_filtered_rows(report: &Report, kinds: &[ArtifactKind], filter: &Filter) -> Vec<Row> {
    let mut out = Vec::new();
    for p in &report.projects {
        if !filter::type_passes(filter, p) {
            continue;
        }
        if let Some(name) = filter::project_name(filter)
            && !swamp_core::filter::name_matches(name, &p.name)
        {
            continue;
        }
        for wt in &p.worktrees {
            for a in &wt.artifacts {
                if !kinds.contains(&a.kind)
                    || !filter::size_passes(filter, a.bytes)
                    || !filter::age_passes(filter, a.mtime_max)
                    || !passes_filter(a.growth_bytes, filter)
                {
                    continue;
                }
                let mut row = Row::leaf(
                    0,
                    format!(
                        "{} · {} {}",
                        project_display_name(p),
                        kind_label(&a.kind),
                        a.path.display()
                    ),
                    a.bytes,
                    a.growth_bytes,
                );
                row.kind = Some(a.kind.clone());
                row.unit = Some(UnitId::for_artifact(&a.path));
                row.mtime_max = a.mtime_max;
                row.evidence = a.evidence.clone();
                row.label.push_str(&swamp_core::render::allocation_note(a));
                if let Some(t) = &a.ecosystem {
                    row.badges = swamp_core::ecosystem::glyph_for(t).to_string();
                    row.ecosystems = vec![t.clone()];
                }
                out.push(row);
            }
        }
    }
    out
}

/// Adds one collapsed row per role family below a build row whose
/// interior an adapter identified in the neutral role vocabulary (#68
/// Node, #67 Gradle/Maven).
///
/// The ordering inside a row is deliberate and is the same one the
/// Cargo purpose groups use: **what this is and what losing it costs**
/// comes first, because that is the question, and the count, size and
/// oldest modification follow when the width allows. A row that has to
/// be truncated loses the numbers, not the consequence.
///
/// These rows are not selectable. Cargo's groups carry a `unit` because
/// `cargo_cleanup` can check and plan them; no other adapter has an
/// action yet (#73), so these rows say "inspection only" and offer no
/// `UnitId` -- which is what stops the confirmation path ever being
/// reached for a unit with no executor behind it.
fn append_build_family_breakdowns(report: &Report, filter: &Filter, rows: &mut Vec<Row>) {
    let mut additions: Vec<(usize, Vec<Row>)> = Vec::new();
    let none = std::collections::HashSet::new();
    for p in &report.projects {
        if !filter::type_passes(filter, p) {
            continue;
        }
        if let Some(name) = filter::project_name(filter)
            && !swamp_core::filter::name_matches(name, &p.name)
        {
            continue;
        }
        for wt in &p.worktrees {
            for a in &wt.artifacts {
                if !matches!(
                    a.kind,
                    ArtifactKind::BuildOutput | ArtifactKind::Cache | ArtifactKind::DependencyTree
                ) {
                    continue;
                }
                // Neutral-vocabulary interiors only; a container whose
                // units the Cargo cleanup module speaks for has its own
                // breakdown (`append_cargo_breakdowns`). Decided by the
                // roles present, never by an adapter id.
                let mut inside = report
                    .nested_artifacts
                    .iter()
                    .filter(|u| u.present && u.path.starts_with(&a.path) && u.path != a.path);
                let mut any = false;
                let mut cargo_vocabulary = false;
                for u in inside.by_ref() {
                    any |= u.adapter.is_some();
                    cargo_vocabulary |= swamp_core::cargo_cleanup::speaks_for(&u.role);
                }
                if !any || cargo_vocabulary {
                    continue;
                }
                let Some(parent_index) = rows
                    .iter()
                    .position(|r| r.unit == Some(UnitId::for_artifact(&a.path)))
                else {
                    continue;
                };
                // The same group rows as the project tree, flattened: the
                // Builds view is a list, so the rows neither expand nor
                // carry an expansion glyph.
                let mut children = family_tree_children(report, &a.path, 1, "", &none);
                for row in &mut children {
                    row.rail = row.rail.replace("▸ ", "").replace("▾ ", "");
                    row.expandable = false;
                    row.expansion_key = None;
                    row.collapsed_children = None;
                }
                if !children.is_empty() {
                    additions.push((parent_index + 1, children));
                }
            }
        }
    }
    for (index, mut children) in additions.into_iter().rev() {
        rows.splice(index..index, children.drain(..));
    }
}

/// Adds a compact, non-actionable Cargo breakdown below build rows.  The
/// common build row remains the accounting/authorization boundary; these
/// aggregate children explain the logical storage without turning the TUI
/// into a second cleanup authority or rendering every hashed leaf.
fn append_cargo_breakdowns(report: &Report, filter: &Filter, rows: &mut Vec<Row>) {
    let mut additions = Vec::new();
    for p in &report.projects {
        if !filter::type_passes(filter, p) {
            continue;
        }
        if let Some(name) = filter::project_name(filter)
            && !swamp_core::filter::name_matches(name, &p.name)
        {
            continue;
        }
        for wt in &p.worktrees {
            for a in &wt.artifacts {
                if a.kind != ArtifactKind::BuildOutput {
                    continue;
                }
                let Some(parent_index) = rows.iter().position(|r| {
                    r.kind.as_ref() == Some(&ArtifactKind::BuildOutput)
                        && r.unit == Some(UnitId::for_artifact(&a.path))
                }) else {
                    continue;
                };
                let mut children: Vec<_> = report
                    .nested_artifacts
                    .iter()
                    .filter(|u| u.path != a.path && u.path.starts_with(&a.path))
                    .filter(|u| {
                        matches!(
                            u.role,
                            swamp_core::artifact::ArtifactRole::Profile
                                | swamp_core::artifact::ArtifactRole::Dependency
                                | swamp_core::artifact::ArtifactRole::Example
                                | swamp_core::artifact::ArtifactRole::BuildScriptOutput
                                | swamp_core::artifact::ArtifactRole::Incremental
                                | swamp_core::artifact::ArtifactRole::Residual
                                | swamp_core::artifact::ArtifactRole::TestExecutable
                        )
                    })
                    .filter(|u| {
                        u.path
                            .strip_prefix(&a.path)
                            .map(|p| {
                                p.components().count() <= 2
                                    || swamp_core::cargo_cleanup::candidate(u)
                                    || u.role == swamp_core::artifact::ArtifactRole::TestExecutable
                                    || (!u.is_dir
                                        && u.role == swamp_core::artifact::ArtifactRole::Example)
                            })
                            .unwrap_or(false)
                    })
                    .collect();
                children.sort_by_key(|u| (u.path.components().count(), u.path.clone()));
                let additions_for_row: Vec<_> = children
                    .into_iter()
                    .map(|u| {
                        let mut row = Row::leaf(
                            1,
                            format!(
                                "{} · {}",
                                match swamp_core::cargo_cleanup::guidance(u).next_action.as_str() {
                                    "inspect_groups" => "category",
                                    "review_cleanup" => "unchecked",
                                    _ => "inspection-only",
                                },
                                u.path.strip_prefix(&a.path).unwrap_or(&u.path).display(),
                            ),
                            u.bytes,
                            u.growth_bytes,
                        );
                        row.mtime_max = u.mtime_max;
                        row.series = report
                            .series_by_key
                            .get(&format!("Nested:{}", u.id))
                            .cloned();
                        if swamp_core::cargo_cleanup::guidance(u).check_status == "unchecked" {
                            row.unit = Some(UnitId::for_artifact(&u.path));
                            row.kind = Some(ArtifactKind::BuildOutput);
                            let guidance =
                                swamp_core::cargo_cleanup::guidance_at(u, report.observed_at);
                            row.signals = vec![
                                guidance.recommendation,
                                guidance.consequence,
                                "review required".into(),
                            ];
                        } else {
                            row.signals = if u.coverage.supported {
                                vec![
                                    if swamp_core::cargo_cleanup::guidance(u).scope == "summary" {
                                        "category"
                                    } else {
                                        "blocked"
                                    }
                                    .into(),
                                ]
                            } else {
                                vec!["coverage-limited".into()]
                            };
                        }
                        let guidance = swamp_core::cargo_cleanup::guidance(u);
                        if guidance.check_status != "unchecked" {
                            row.signals
                                .extend([guidance.recommendation, guidance.consequence]);
                        }
                        row.signals
                            .push("allocated bytes; reclaimable space unknown".into());
                        row
                    })
                    .collect();
                additions.push((parent_index + 1, additions_for_row));
            }
        }
    }
    for (index, mut children) in additions.into_iter().rev() {
        rows.splice(index..index, children.drain(..));
    }
}

/// Types view: one row per ecosystem, with the projects wearing the tag
/// and the bytes/growth of the artifacts it generates (`Report.summary`).
pub fn types_rows(report: &Report, filter: &Filter) -> Vec<Row> {
    let mut rows: Vec<Row> = report
        .summary
        .by_type
        .iter()
        .filter(|(tag, _)| {
            filter::type_passes(
                filter,
                &swamp_core::report::ProjectRow {
                    project_id: String::new(),
                    name: String::new(),
                    worktrees: Vec::new(),
                    ecosystems: vec![(*tag).clone()],
                    remote: None,
                },
            )
        })
        .map(|(tag, t)| {
            let mut row = Row::leaf(
                0,
                format!(
                    "{} · {} project{} · {} artifact{}",
                    t.name,
                    t.projects,
                    if t.projects == 1 { "" } else { "s" },
                    t.artifacts,
                    if t.artifacts == 1 { "" } else { "s" }
                ),
                t.bytes,
                t.growth_bytes,
            );
            row.badges = if tag == "other" {
                String::new()
            } else {
                swamp_core::ecosystem::glyph_for(tag).to_string()
            };
            row.ecosystems = vec![tag.clone()];
            row
        })
        .collect();
    rows.sort_by_key(|a| std::cmp::Reverse(a.bytes));
    rows
}

/// Unowned view.
pub fn unowned_rows(report: &Report) -> Vec<Row> {
    report
        .unowned
        .iter()
        .filter(|u| u.reason != UnownedReason::DockerNoJoin)
        .map(|u| {
            let mut row = Row::leaf(
                0,
                format!("{:?} · {}", u.reason, u.path_or_object),
                u.bytes,
                None,
            );
            // Bytes nothing claims are still bytes, and the path is real:
            // it can be marked like any other unit and goes to Trash.
            // Except one the walk could not even read — there is nothing
            // to stand behind.
            if u.reason != UnownedReason::PermissionDenied {
                row.kind = Some(ArtifactKind::Loose);
                row.unit = Some(UnitId::for_artifact(std::path::Path::new(
                    &u.path_or_object,
                )));
            }
            row
        })
        .collect()
}

/// External/shared storage view (#43/#51/#60): the same flat shape
/// `unowned_rows` renders, one row per detector-resolved unit. Read-only
/// by construction (`unit: None`, never markable) -- selection refusal
/// for these units already lives at the action layer
/// (`actions::execute` refuses every `PlanUnit::external_category`
/// unconditionally); the TUI simply never offers a delete affordance
/// the action layer would refuse anyway, rather than inventing a
/// confirm flow only to refuse it.
///
/// Each machine-wide build store's identified interior sits under it:
/// closed until opened (`Enter`), then the same family groups a project
/// container shows. Every interior row is inspection only (`blocked`,
/// no `UnitId`), exactly as there.
pub fn external_rows_with(
    units: &[swamp_core::external::ExternalUnit],
    interiors: &[swamp_core::artifact::NestedArtifact],
    collapsed: &std::collections::HashSet<String>,
    observed_at: u64,
) -> Vec<Row> {
    let mut sorted: Vec<&swamp_core::external::ExternalUnit> = units.iter().collect();
    sorted.sort_by_key(|a| std::cmp::Reverse(a.bytes));
    let mut rows = Vec::new();
    for u in sorted {
        let consumers = if u.consumers.is_empty() {
            "no declared consumers".to_string()
        } else {
            format!("{} consumer(s)", u.consumers.len())
        };
        let mut row = Row::leaf(
            0,
            format!(
                "{:?} · {} ({}) · {consumers}",
                u.category,
                u.path.display(),
                u.detector_id
            ),
            u.bytes,
            u.growth_bytes,
        );
        row.evidence = u.evidence.clone();
        let has_interior = interiors
            .iter()
            .any(|i| i.path != u.path && i.path.starts_with(&u.path));
        if has_interior {
            let key = format!("store-open:{}", u.path.display());
            let open = collapsed.contains(&key);
            let children =
                family_tree_children_of(interiors, observed_at, &u.path, 1, "", collapsed);
            row.expandable = !children.is_empty();
            row.expansion_key = Some(key);
            row.rail = if open { "▾ ".into() } else { "▸ ".into() };
            row.collapsed_children = (!open).then_some(children.len());
            row.signals = vec!["store interior below · inspection only".into()];
            rows.push(row);
            if open {
                rows.extend(children);
            }
        } else {
            rows.push(row);
        }
    }
    rows
}

/// Agent-tool storage view (#91/#100): one row per `AgentUnit`, grouped
/// tool → category via the label text (a flat list, same shape as
/// `unowned_rows`/`external_rows_with`; a real tool → category → unit tree is
/// left to `render::render_view_agents`'s CLI drill-down). `unit` is set
/// for **every** row, protected/unsupported ones included: `app::mark_row`
/// hands the exact path to `actions::propose_agents` either way, and that
/// call's own refusal (protected category, no supported action, active
/// session, ...) becomes the footer text -- never a silent "nothing to
/// delete on this row" for a unit the human can plainly see. Backspace's
/// confirmation and Enter's execution reuse the ordinary
/// `MarkedUnit`/background-worker path (`crate::actions::execute_plan_progress`),
/// same as every other markable view; nothing here blocks on the
/// event/render thread.
pub fn agent_rows(units: &[swamp_core::agents::AgentUnit]) -> Vec<Row> {
    let mut rows: Vec<Row> = units
        .iter()
        .map(|u| {
            let link = match &u.project_link {
                swamp_core::agents::ProjectLinkState::Linked {
                    project_name,
                    source: swamp_core::agents::LinkSource::Declared,
                    ..
                } => format!("project: {project_name}"),
                swamp_core::agents::ProjectLinkState::Linked {
                    project_name,
                    source: swamp_core::agents::LinkSource::Inferred,
                    fallback_reason,
                    ..
                } => match fallback_reason.as_deref() {
                    Some(reason) => format!("project: {project_name} (inferred; {reason})"),
                    None => format!("project: {project_name} (inferred)"),
                },
                swamp_core::agents::ProjectLinkState::NotApplicable => "tool-wide".to_string(),
                other => format!("{other:?}"),
            };
            let protect = if u.protected { " [protected]" } else { "" };
            let mut row = Row::leaf(
                0,
                format!(
                    "{} · {} · {} · {link}{protect}",
                    u.tool_name,
                    u.category.label(),
                    u.relative_path
                ),
                u.bytes,
                u.growth_bytes,
            );
            row.mtime_max = u.mtime_max;
            row.unit = Some(crate::units::UnitId::for_artifact(&u.path));
            row.evidence = u.evidence.clone();
            row
        })
        .collect();
    rows.sort_by_key(|a| std::cmp::Reverse(a.bytes));
    rows
}

/// Element-wise sum of several equal-length series; `None` if none.
/// Elementwise sum of child series. A bucket is `None` only when no child
/// had been observed yet at that time.
fn sum_series<'a>(it: impl Iterator<Item = &'a Vec<Option<u64>>>) -> Option<Vec<Option<u64>>> {
    let mut acc: Option<Vec<Option<u64>>> = None;
    for s in it {
        match acc.as_mut() {
            None => acc = Some(s.clone()),
            Some(a) => {
                for (x, y) in a.iter_mut().zip(s.iter()) {
                    if let Some(y) = y {
                        *x = Some(x.unwrap_or(0) + y);
                    }
                }
            }
        }
    }
    acc
}

/// Worktree-level predicates against a report row's facts.
fn worktree_passes(filter: &Filter, wt: &swamp_core::report::WorktreeRow) -> bool {
    let merge_complete = wt
        .merge_complete
        .as_ref()
        .is_some_and(|m| m.verdict == swamp_core::github::TriState::Yes);
    let pr_state = wt.github.as_ref().and_then(|g| match &g.pull_request {
        swamp_core::github::PrStatus::Some(pr) => Some(&pr.state),
        _ => None,
    });
    filter::worktree_passes(filter, wt.idle_secs, merge_complete, pr_state)
}

/// Parses the rendered worktree signals back into facts for the mark
/// decision: (dirty, unpushed, locked). `None` = unknown.
trait RawWorktreeSignals {
    fn raw_signals(&self) -> (Option<bool>, Option<u32>, Option<bool>);
}
impl RawWorktreeSignals for swamp_core::report::WorktreeRow {
    fn raw_signals(&self) -> (Option<bool>, Option<u32>, Option<bool>) {
        let mut dirty = None;
        let mut unpushed = None;
        let mut locked = None;
        for s in &self.signals {
            match s.name.as_str() {
                "dirty" => {
                    dirty = match s.value.as_str() {
                        "dirty" => Some(true),
                        "clean" => Some(false),
                        _ => None,
                    }
                }
                "unpushed" => {
                    unpushed = s
                        .value
                        .split_whitespace()
                        .next()
                        .and_then(|n| n.parse::<u32>().ok());
                }
                "locked" => {
                    locked = match s.value.as_str() {
                        "locked" => Some(true),
                        "unlocked" => Some(false),
                        _ => None,
                    }
                }
                _ => {}
            }
        }
        (dirty, unpushed, locked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_badges_separate_glyphs_from_multi_digit_counts() {
        assert_eq!(badges(&[], false, false, 0), "");
        assert_eq!(badges(&[], false, true, 0), "🔨");
        for count in [1, 9, 10, 12, 99, 100] {
            assert_eq!(badges(&[], false, false, count), format!("⎇ {count}"));
            let badge = badges(&[], false, true, count);
            assert_eq!(badge, format!("🔨 ⎇ {count}"));
            assert_eq!(display_width(&badge), 5 + count.to_string().len());
        }
    }

    #[test]
    fn human_bytes_formats_units() {
        // Decimal, matching the SI labels the product prints (a GB is
        // 1_000_000_000 bytes, not a GiB under a GB label).
        assert_eq!(human_bytes(500), "500B");
        assert_eq!(human_bytes(1536), "1.5KB");
        assert_eq!(human_bytes(1_288_490_188), "1.3GB");
        assert_eq!(human_bytes(1_000_000_000), "1.0GB");
        // The exact figure behind the reported arithmetic bug.
        assert_eq!(human_bytes(1_951_580_160), "2.0GB");
    }

    #[test]
    fn signed_bytes_show_sign_and_dash() {
        assert_eq!(human_signed_bytes(0), "0B");
        assert_eq!(human_signed_bytes(184_320_000), "+184.3MB");
        assert_eq!(human_signed_bytes(-1024), "-1.0KB");
    }

    #[test]
    fn diverging_bar_puts_direction_in_the_geometry() {
        let max = 10_000_000_000;
        let (l, axis, r) = diverging_bar(Some(max), max, 8);
        assert_eq!(axis, '│');
        assert_eq!(r, "████████", "the largest growth fills the right side");
        assert_eq!(l, "        ", "and leaves the left side empty");
        let (l, _, r) = diverging_bar(Some(-max), max, 8);
        assert_eq!(l, "████████", "shrink of the same size fills the left");
        // The left bar hugs the axis: its padding is on the outside.
        let (l, _, _) = diverging_bar(Some(-107_000_000), max, 8);
        assert!(l.starts_with(' ') && l.ends_with('█'), "{l:?}");
        assert_eq!(r, "        ");
        let (l, _, r) = diverging_bar(None, max, 8);
        assert_eq!((l.trim(), r.trim()), ("", ""), "no measurement, no bar");
        assert_eq!(diverging_bar(Some(0), max, 8).2.trim(), "");
    }

    #[test]
    fn a_log_scale_separates_the_sizes_a_linear_one_flattened() {
        // The frame that prompted this: 13.6GB, 107MB and 3MB shared a
        // column, and the last two were the same single sliver.
        let max = 13_600_000_000;
        let big = diverging_bar(Some(13_600_000_000), max, 20)
            .2
            .trim_end()
            .chars()
            .count();
        let mid = diverging_bar(Some(107_000_000), max, 20)
            .2
            .trim_end()
            .chars()
            .count();
        let small = diverging_bar(Some(3_000_000), max, 20)
            .2
            .trim_end()
            .chars()
            .count();
        assert_eq!(big, 20);
        assert!(mid < big && small < mid, "{big} {mid} {small}");
        assert!(
            mid >= small + 2,
            "107MB must be clearly longer than 3MB: {mid} vs {small}"
        );
    }

    #[test]
    fn noise_is_a_tick_not_a_bar() {
        let max = 10_000_000_000;
        let (_, _, r) = diverging_bar(Some(4_096), max, 20);
        assert_eq!(r.trim_end(), "▏", "a 4KB change is one tick");
        let (l, _, _) = diverging_bar(Some(-4_096), max, 20);
        assert_eq!(l.trim_start(), "▐", "and on the left it hugs the axis too");
        assert!(is_noise(Some(4_096)) && is_noise(Some(-4_096)));
        assert!(!is_noise(Some(0)), "no change is not noise, it is nothing");
        assert!(!is_noise(Some(NOISE_FLOOR)) && !is_noise(None));
    }

    #[test]
    fn spark_deltas_keep_the_biggest_move_per_bin_with_its_sign() {
        let s: Vec<Option<u64>> = vec![
            None,
            Some(10),
            Some(10),
            Some(19),
            Some(2),
            Some(2),
            Some(3),
        ];
        assert_eq!(
            deltas(&s),
            vec![None, Some(0), Some(9), Some(-17), Some(0), Some(1)]
        );
        assert_eq!(spark_deltas(&s, 3), vec![Some(0), Some(-17), Some(1)]);
        assert!(spark_deltas(&[], 5).is_empty());
    }

    #[test]
    fn flatness_means_nothing_moved() {
        assert!(is_flat(&[Some(5_186_904_064), Some(5_186_904_064)]));
        assert!(is_flat(&[None, None, Some(7)]));
        assert!(!is_flat(&[Some(100), Some(99)]));
        assert!(!is_flat(&[None, Some(0), Some(50)]));
    }

    #[test]
    fn trend_and_net_ignore_unobserved_buckets() {
        assert_eq!(trend(&[None, Some(1), Some(2)]), 1);
        assert_eq!(trend(&[Some(2), Some(1)]), -1);
        assert_eq!(trend(&[Some(3), None, Some(3)]), 0);
        assert_eq!(net_change(&[None, Some(10), Some(4)]), Some(-6));
        assert_eq!(net_change(&[None, Some(10)]), None);
    }

    #[test]
    fn truncate_middle_keeps_tail() {
        let long = "/Users/dev/src/some-really-long-project-name/node_modules";
        let t = truncate_middle(long, 20);
        assert!(t.len() <= long.len());
        assert!(t.ends_with("node_modules"));
        assert!(t.contains('…'));
    }

    #[test]
    fn truncate_middle_leaves_short_strings_alone() {
        assert_eq!(truncate_middle("short", 20), "short");
    }

    #[test]
    fn unicode_columns_are_cell_bounded_even_at_tiny_widths() {
        for text in ["project 🦀 🔨 ⎇ 123", "工程/長い名前", "a👩‍💻b e\u{301} tail"]
        {
            for width in 0..80 {
                assert!(display_width(&truncate_middle(text, width)) <= width);
                assert_eq!(display_width(&pad_display(text, width)), width);
            }
        }
        assert!(badges(&["rust".into()], false, true, 12).contains(" ⎇ 12"));
    }

    #[test]
    fn apply_sort_growth_puts_what_arrived_first_and_what_left_last() {
        let mut rows = vec![
            Row::leaf(0, "a".into(), 10, Some(5)),
            Row::leaf(0, "b".into(), 20, Some(-50)),
            Row::leaf(0, "c".into(), 30, Some(1)),
        ];
        apply_sort(&mut rows, Sort::Growth, false);
        assert_eq!(rows[0].label, "a", "the largest growth leads");
        assert_eq!(rows[1].label, "c");
        assert_eq!(
            rows[2].label, "b",
            "a big shrink sorts last, not first: it is not what grew"
        );
    }

    #[test]
    fn apply_sort_size_orders_by_bytes() {
        let mut rows = vec![
            Row::leaf(0, "a".into(), 10, None),
            Row::leaf(0, "b".into(), 300, None),
            Row::leaf(0, "c".into(), 20, None),
        ];
        apply_sort(&mut rows, Sort::Size, false);
        assert_eq!(rows[0].label, "b");
        assert_eq!(rows[1].label, "c");
        assert_eq!(rows[2].label, "a");
    }

    fn art(kind: ArtifactKind, path: &str, bytes: u64) -> swamp_core::report::ArtifactRow {
        swamp_core::report::ArtifactRow {
            kind,
            path: path.into(),
            bytes,
            mtime_max: 0,
            ecosystem: None,
            hardlinked: false,
            dedup_stale: false,
            allocated_bytes: None,
            allocated_growth_bytes: None,
            local_bytes: 0,
            track: None,
            growth_bytes: None,
            regrowth_count: 0,
            observed_at: 0,
            confidence: swamp_core::entities::Confidence::High,
            source: swamp_core::report::Source::new("t"),
            note: None,
            created_at: None,
            containers: Vec::new(),
            shared_with: Vec::new(),
            dangling: false,
            evidence: Vec::new(),
        }
    }

    #[test]
    fn tree_rail_marks_last_sibling_with_an_elbow() {
        let report = Report {
            store_dir: None,
            observed_at: 0,
            root: "/r".into(),
            projects: vec![swamp_core::report::ProjectRow {
                project_id: "p".into(),
                name: "proj".into(),
                remote: None,
                ecosystems: Vec::new(),
                worktrees: vec![
                    swamp_core::report::WorktreeRow {
                        worktree_id: "w1".into(),
                        path: "/r/proj".into(),
                        kind: swamp_core::report::WorktreeKind::Main,
                        artifacts: vec![
                            art(ArtifactKind::BuildOutput, "/r/proj/target", 10),
                            art(ArtifactKind::DependencyTree, "/r/proj/node_modules", 20),
                        ],
                        signals: vec![],
                        branch: None,
                        github: None,
                        merge_complete: None,
                        idle_secs: None,
                    },
                    swamp_core::report::WorktreeRow {
                        worktree_id: "w2".into(),
                        path: "/r/proj/.worktrees/x".into(),
                        kind: swamp_core::report::WorktreeKind::Linked,
                        artifacts: vec![],
                        signals: vec![],
                        branch: None,
                        github: None,
                        merge_complete: None,
                        idle_secs: None,
                    },
                ],
            }],
            unowned: vec![],
            reconciliation: swamp_core::report::Reconciliation {
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
        };
        let rows = tree_rows(
            &report,
            "proj",
            &Filter::default(),
            &Default::default(),
            &Default::default(),
        );
        // First worktree is not last -> ├─; second worktree is last -> └─.
        assert!(rows[0].rail.starts_with("├─"));
        assert!(rows[3].rail.starts_with("└─"));
        // Its two artifact children: first ├─, last └─, both under a │ rail
        // (first worktree is not the last sibling).
        assert!(rows[1].rail.starts_with("│  ├─"));
        assert!(rows[2].rail.starts_with("│  └─"));

        // Nested candidates must be selectable, while whole dependency groups
        // stay inspection-only. Inserting several children must not displace
        // the following worktree's rows.
        let tmp = tempfile::tempdir().unwrap();
        let target = std::fs::canonicalize(tmp.path()).unwrap().join("target");
        std::fs::create_dir_all(target.join("debug/incremental/crate-a")).unwrap();
        std::fs::create_dir_all(target.join("debug/deps")).unwrap();
        std::fs::write(target.join("debug/incremental/crate-a/state"), b"state").unwrap();
        let mut report = report;
        report.projects[0].worktrees[0].artifacts[0].path = target.clone();
        report.nested_artifacts =
            swamp_core::cargo_artifacts::inspect_target(&target, Some(&target)).units;
        report.projects[0].worktrees[0].artifacts[0].dedup_stale = true;
        report.projects[0].worktrees[0].artifacts[0].allocated_bytes = Some(4096);
        let rows = builds_rows(&report, &Filter::default());
        assert!(
            rows.iter()
                .any(|r| r.label.contains("unique not recomputed; allocated"))
        );
        let selected = UnitId::for_artifact(&target.join("debug/incremental/crate-a"));
        assert!(rows.iter().any(|r| r.unit == Some(selected.clone())));
        let deps = UnitId::for_artifact(&target.join("debug/deps"));
        assert!(!rows.iter().any(|r| r.unit == Some(deps.clone())));
    }

    #[test]
    fn docker_unowned_bytes_sums_only_docker_no_join() {
        let report = Report {
            store_dir: None,
            observed_at: 0,
            root: "/r".into(),
            projects: vec![],
            unowned: vec![
                swamp_core::report::UnownedRow {
                    measurement: None,
                    path_or_object: "img".into(),
                    bytes: 100,
                    reason: UnownedReason::DockerNoJoin,
                    docker_kind: None,
                    shared_bytes: None,
                    note: None,
                    created_at: None,
                    containers: Vec::new(),
                    shared_with: Vec::new(),
                    dangling: false,
                    evidence: Vec::new(),
                },
                swamp_core::report::UnownedRow {
                    measurement: None,
                    path_or_object: "cache".into(),
                    bytes: 200,
                    reason: UnownedReason::SharedCache,
                    docker_kind: None,
                    shared_bytes: None,
                    note: None,
                    created_at: None,
                    containers: Vec::new(),
                    shared_with: Vec::new(),
                    dangling: false,
                    evidence: Vec::new(),
                },
            ],
            reconciliation: swamp_core::report::Reconciliation {
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
        };
        assert_eq!(docker_unowned_bytes(&report), 100);
    }
}

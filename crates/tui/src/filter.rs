//! The TUI's filter line speaks the same grammar as `--filter` on the CLI
//! (interactive and `--json`) — it *is* `swamp_core::filter`. This module only
//! adds the TUI's conveniences: the default line, the `0` = no filter
//! shorthand, and small accessors the row builders need.
//!
//! Grammar (whitespace-conjunction): `growth [><] <size> in <duration>`,
//! `kind:<k>`, `project:<name>`, `idle > <dur>`, `merge-complete`,
//! `pr:open|merged|closed|none`.

pub use swamp_core::filter::Filter;
use swamp_core::filter::Predicate;
use swamp_core::report::ArtifactKind;

/// The filter shown on open, per DESIGN.md.
pub fn default_filter_text() -> &'static str {
    "growth > 100MB in 7d"
}

pub fn default_filter() -> Filter {
    swamp_core::filter::parse(default_filter_text()).expect("default filter parses")
}

/// Parses one filter line. `0` or an empty line means "no filter".
/// Returns `Err(message)` on a grammar error; the caller keeps the
/// previous filter applied and shows the message inline.
pub fn parse(input: &str) -> Result<Filter, String> {
    let raw = input.trim();
    if raw.is_empty() || raw == "0" {
        return Ok(Filter::default());
    }
    swamp_core::filter::parse(raw).map_err(|e| e.to_string())
}

/// The growth window named by the filter, for the header's `since`.
pub fn growth_window_secs(f: &Filter) -> Option<u64> {
    f.predicates.iter().find_map(|p| match p {
        Predicate::Growth { within_secs, .. } if *within_secs != u64::MAX => Some(*within_secs),
        _ => None,
    })
}

pub fn project_name(f: &Filter) -> Option<&str> {
    f.predicates.iter().find_map(|p| match p {
        Predicate::Project(n) => Some(n.as_str()),
        _ => None,
    })
}

/// Does a row of this kind pass every `kind:` predicate? Accepts the
/// enum's Debug name (`BuildOutput`) or the rendered label (`build`),
/// case-insensitively.
pub fn kind_passes(f: &Filter, label: &str, kind: Option<&ArtifactKind>) -> bool {
    f.predicates.iter().all(|p| match p {
        Predicate::Kind(k) => {
            label.eq_ignore_ascii_case(k)
                || kind.is_some_and(|kk| format!("{kk:?}").eq_ignore_ascii_case(k))
        }
        _ => true,
    })
}

/// Every `growth` predicate against this row's growth figure.
pub fn growth_passes(f: &Filter, growth: Option<i64>) -> bool {
    f.predicates.iter().all(|p| match p {
        Predicate::Growth { greater, bytes, .. } => {
            let g = growth.unwrap_or(0);
            if *greater {
                g > 0 && g.unsigned_abs() > *bytes
            } else {
                g < 0 && g.unsigned_abs() > *bytes
            }
        }
        _ => true,
    })
}

/// Every worktree-level predicate (`idle >`, `merge-complete`, `pr:`)
/// against this worktree's facts. `pr:` needs the GitHub facts; without
/// them only `pr:none` matches.
pub fn worktree_passes(
    f: &Filter,
    idle_secs: Option<u64>,
    merge_complete: bool,
    pr_state: Option<&swamp_core::github::PrState>,
) -> bool {
    use swamp_core::filter::PrFilter;
    use swamp_core::github::PrState;
    f.predicates.iter().all(|p| match p {
        Predicate::IdleGreaterThan(secs) => idle_secs.is_some_and(|i| i > *secs),
        Predicate::MergeComplete => merge_complete,
        Predicate::Pr(want) => matches!(
            (want, pr_state),
            (PrFilter::None, None)
                | (PrFilter::Open, Some(PrState::Open))
                | (PrFilter::Merged, Some(PrState::Merged))
                | (PrFilter::Closed, Some(PrState::Closed))
        ),
        _ => true,
    })
}

/// Every `size` predicate against a rollup's bytes.
pub fn size_passes(f: &Filter, bytes: u64) -> bool {
    swamp_core::filter::size_passes(f, bytes)
}

/// Every `age >` predicate against an artifact's newest mtime. Unknown
/// (0) never passes: a filter for old things must not match things whose
/// age nobody measured.
pub fn age_passes(f: &Filter, mtime_max: u64) -> bool {
    f.predicates.iter().all(|p| match p {
        Predicate::AgeGreaterThan(secs) => {
            mtime_max > 0 && swamp_core::entities::now().saturating_sub(mtime_max) > *secs
        }
        _ => true,
    })
}

/// True when the filter has an `age >` predicate, so a rollup row (a
/// project, a worktree) passes only if one of its artifacts does.
pub fn has_age_predicate(f: &Filter) -> bool {
    f.predicates
        .iter()
        .any(|p| matches!(p, Predicate::AgeGreaterThan(_)))
}

/// True when the filter has any worktree-level predicate, so a projects
/// view (which has no per-worktree rows) must evaluate them per worktree.
pub fn has_worktree_predicates(f: &Filter) -> bool {
    f.predicates.iter().any(|p| {
        matches!(
            p,
            Predicate::IdleGreaterThan(_) | Predicate::MergeComplete | Predicate::Pr(_)
        )
    })
}

/// Every `type:` predicate against the project's ecosystem tags.
pub fn type_passes(f: &Filter, project: &swamp_core::report::ProjectRow) -> bool {
    f.predicates.iter().all(|p| match p {
        Predicate::Type(t) => {
            let dummy = swamp_core::report::ArtifactRow {
                kind: swamp_core::report::ArtifactKind::Source,
                path: Default::default(),
                bytes: 0,
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
                source: swamp_core::report::Source::new("filter"),
                note: None,
                created_at: None,
                containers: Vec::new(),
                shared_with: Vec::new(),
                dangling: false,
                evidence: Vec::new(),
            };
            Filter {
                predicates: vec![Predicate::Type(t.clone())],
            }
            .matches_artifact(project, &dummy)
        }
        _ => true,
    })
}

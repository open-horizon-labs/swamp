//! Depth-2 drilldown of a unit: its immediate child folders with size,
//! modification time and last-used, the top N of them, and one remainder
//! row, so what is shown always adds up to the unit's own total.
//!
//! ```text
//! ~/Library/Caches   36.8GB
//!   com.example.app              4.1GB   mtime Sep 3
//!   ...                          (top N, largest first)
//!   remainder                    9.2GB   212 other entries
//! ```
//!
//! Built from the per-directory rows the one folded walk already
//! produces ([`crate::folded_measurement::observe_unit_with_dirs`]); no
//! second traversal, no file contents. A child folder the walk could not
//! list is `not measured`, never zero.
//!
//! Every row's `bytes` is a signed count so the identity is exact:
//! `sum(rows) == the walk's total for the unit`. A hardlinked file the
//! walk counted once but a directory rollup counted under each parent is
//! the only reason the two could differ, and it appears as an
//! [`ChildKind::Adjustment`] row instead of vanishing.

use crate::last_used::LastUsed;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// A unclassified root smaller than this is one row, not a drilldown.
pub const DRILLDOWN_MIN_BYTES: u64 = 1 << 30;
/// How many child folders a drilldown lists before the remainder row.
pub const DRILLDOWN_TOP_N: usize = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildKind {
    /// One immediate child folder.
    Entry,
    /// Everything not listed: the other folders, and the files directly
    /// inside the unit.
    Remainder,
    /// A signed correction so the rows still sum to the unit's total
    /// (a hardlinked file counted once, not once per parent).
    Adjustment,
}

impl ChildKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Entry => "entry",
            Self::Remainder => "remainder",
            Self::Adjustment => "adjustment",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        Some(match label {
            "entry" => Self::Entry,
            "remainder" => Self::Remainder,
            "adjustment" => Self::Adjustment,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildMeasure {
    Complete,
    /// Some folder below this one could not be read: `bytes` is a lower
    /// bound.
    Partial,
    /// This folder itself could not be listed. `bytes` is absent.
    NotMeasured,
}

impl ChildMeasure {
    pub fn label(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::NotMeasured => "not_measured",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        Some(match label {
            "complete" => Self::Complete,
            "partial" => Self::Partial,
            "not_measured" => Self::NotMeasured,
            _ => return None,
        })
    }
}

/// One row of a unit's drilldown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitChild {
    pub kind: ChildKind,
    /// The folder's name exactly as it is on disk; empty for a remainder
    /// or adjustment row.
    pub name: String,
    /// Allocated bytes; `None` exactly when the folder was not measured.
    pub bytes: Option<i64>,
    pub measure: ChildMeasure,
    /// Newest modification inside the folder, seconds; 0 when unknown.
    pub mtime_max: u64,
    /// Remainder only: how many entries (folders and files) it stands for.
    pub entries: u32,
    /// Remainder only: how many of those folders were not measured.
    pub not_measured: u32,
    pub last_used: LastUsed,
}

impl UnitChild {
    fn row(kind: ChildKind, bytes: i64) -> Self {
        UnitChild {
            kind,
            name: String::new(),
            bytes: Some(bytes),
            measure: ChildMeasure::Complete,
            mtime_max: 0,
            entries: 0,
            not_measured: 0,
            last_used: LastUsed::default(),
        }
    }
}

/// Whether a unit's walk should keep its per-directory rows so a
/// drilldown can be built: a unit whose detector declared a last-use
/// source always (its children are the depth-2 rows of a reclaim view),
/// an unclassified root unless its last stored size was under
/// [`DRILLDOWN_MIN_BYTES`] (so a small root keeps its replay instead of
/// being re-walked for rows it would not show).
pub(crate) fn wants_children(
    unclassified: bool,
    declares_last_use: bool,
    last_stored_bytes: Option<u64>,
) -> bool {
    declares_last_use
        || (unclassified && last_stored_bytes.is_none_or(|b| b >= DRILLDOWN_MIN_BYTES))
}

/// Whether a unit measured at `bytes` is listed as children at all:
/// always for a declared unit, at or above the threshold otherwise.
pub(crate) fn worth_listing(declares_last_use: bool, bytes: u64) -> bool {
    declares_last_use || bytes >= DRILLDOWN_MIN_BYTES
}

/// The exact sum of a drilldown's rows: what must equal the walk's total
/// for the unit. A not-measured folder contributes nothing to it (its
/// bytes are inside the remainder's only when it was not listed; see
/// [`children_of`]).
pub fn rows_total(children: &[UnitChild]) -> i64 {
    children.iter().filter_map(|c| c.bytes).sum()
}

/// The drilldown of the unit at `unit_path`, from its raw per-directory
/// rows and the folded total the walk reported for it. Empty when the
/// walk produced no root row (nothing to drill into).
///
/// `top_n` bounds the entry rows; not-measured folders fill whatever
/// room the measured ones leave and are otherwise counted in the
/// remainder row, never dropped without a count.
pub fn children_of(
    unit_path: &Path,
    dirs: Vec<crate::report::DirRollup>,
    unit_bytes: u64,
    top_n: usize,
) -> Vec<UnitChild> {
    // Whether each folder's *own* listing worked, before completeness
    // rolls up: only a folder that could not be listed itself is
    // `not measured`; one with an unreadable folder below it is partial.
    let own_listed: HashMap<String, bool> = dirs
        .iter()
        .map(|d| (d.rel_path.clone(), d.complete))
        .collect();
    let root_entries = dirs
        .iter()
        .find(|d| d.rel_path.is_empty())
        .map(|d| d.entry_count);
    let folded = crate::build_stores::folded_dirs(unit_path, dirs);
    let Some(root) = folded.iter().find(|d| d.path == unit_path) else {
        return Vec::new();
    };
    let root_total = root.allocated_total;

    let mut measured: Vec<UnitChild> = Vec::new();
    let mut unlisted: Vec<UnitChild> = Vec::new();
    for dir in &folded {
        let Some(parent) = dir.path.parent() else {
            continue;
        };
        if parent != unit_path || dir.path == unit_path {
            continue;
        }
        let name = dir
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let listed = own_listed.get(&name).copied().unwrap_or(true);
        let mut child = UnitChild::row(ChildKind::Entry, dir.allocated_total as i64);
        child.name = name;
        child.mtime_max = dir.mtime_max;
        if !listed {
            child.bytes = None;
            child.measure = ChildMeasure::NotMeasured;
            child.mtime_max = 0;
            unlisted.push(child);
        } else {
            if !dir.complete {
                child.measure = ChildMeasure::Partial;
            }
            measured.push(child);
        }
    }
    measured.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
    unlisted.sort_by(|a, b| a.name.cmp(&b.name));
    let total_entries = measured.len() + unlisted.len();

    let mut shown: Vec<UnitChild> = measured.into_iter().take(top_n).collect();
    let room = top_n.saturating_sub(shown.len());
    let hidden_unlisted = unlisted.len().saturating_sub(room);
    shown.extend(unlisted.into_iter().take(room));

    let shown_bytes: i64 = shown.iter().filter_map(|c| c.bytes).sum();
    let mut rows = shown;
    let remainder_bytes = root_total as i64 - shown_bytes;
    let other_entries = root_entries
        .map(|n| n.saturating_sub(rows.len() as u32))
        .unwrap_or_else(|| total_entries.saturating_sub(rows.len()) as u32);
    if remainder_bytes != 0 || other_entries != 0 {
        let mut rest = UnitChild::row(ChildKind::Remainder, remainder_bytes);
        rest.entries = other_entries;
        rest.not_measured = hidden_unlisted as u32;
        rows.push(rest);
    }
    let adjustment = unit_bytes as i64 - root_total as i64;
    if adjustment != 0 {
        rows.push(UnitChild::row(ChildKind::Adjustment, adjustment));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::DirRollup;

    fn row(rel: &str, own: u64, files: u32, dirs: u32, complete: bool) -> DirRollup {
        let parent = if rel.is_empty() {
            None
        } else {
            Some(match Path::new(rel).parent() {
                Some(p) => p.display().to_string(),
                None => String::new(),
            })
        };
        DirRollup {
            worktree_id: "build-store".into(),
            track: None,
            rel_path: rel.into(),
            parent_rel_path: parent,
            allocated_total: own,
            own_allocated: own,
            file_count: files,
            entry_count: files + dirs,
            symlink_count: 0,
            mod_time_min: 100,
            complete,
            growth_bytes: None,
        }
    }

    #[test]
    fn the_size_threshold_gates_an_unclassified_root_but_never_a_declared_unit() {
        // The tempting wrong patch: drilling every root (a re-walk of
        // every small cache root on every pass) or none (the 36.8 GB
        // Library/Caches stays one number).
        assert!(worth_listing(false, DRILLDOWN_MIN_BYTES));
        assert!(!worth_listing(false, DRILLDOWN_MIN_BYTES - 1));
        assert!(worth_listing(true, 1));
        // Never seen, or last seen big: keep the rows to list.
        assert!(wants_children(true, false, None));
        assert!(wants_children(true, false, Some(DRILLDOWN_MIN_BYTES)));
        // Last seen small: keep its replay.
        assert!(!wants_children(true, false, Some(DRILLDOWN_MIN_BYTES - 1)));
        // Not unclassified and declares nothing: never a drilldown.
        assert!(!wants_children(false, false, None));
        assert!(wants_children(false, true, Some(0)));
    }

    #[test]
    fn rows_add_up_to_the_unit_total_exactly() {
        // The tempting wrong patch: a remainder taken from a rounded or
        // separately measured total, so the parts drift from the whole.
        let unit = Path::new("/u");
        let mut dirs = vec![row("", 7, 3, 4, true)];
        for (i, own) in [900u64, 500, 300, 40].iter().enumerate() {
            dirs.push(row(&format!("d{i}"), *own, 1, 0, true));
        }
        dirs.push(row("d0/sub", 60, 1, 0, true));
        let total = 7 + 900 + 500 + 300 + 40 + 60;
        let kids = children_of(unit, dirs, total, 2);
        assert_eq!(rows_total(&kids), total as i64);
        assert_eq!(kids[0].name, "d0");
        assert_eq!(
            kids[0].bytes,
            Some(960),
            "a child includes what is below it"
        );
        assert_eq!(kids[1].name, "d1");
        let rest = kids.last().unwrap();
        assert_eq!(rest.kind, ChildKind::Remainder);
        assert_eq!(rest.bytes, Some(7 + 300 + 40));
        assert_eq!(rest.entries, 7 - 2);
    }

    #[test]
    fn an_unreadable_child_is_not_measured_not_zero_and_never_dropped() {
        let unit = Path::new("/u");
        let dirs = vec![
            row("", 0, 0, 3, true),
            row("ok", 100, 1, 0, true),
            row("locked", 0, 0, 0, false),
            row("partial", 50, 1, 1, true),
            row("partial/deep", 0, 0, 0, false),
        ];
        let kids = children_of(unit, dirs, 150, 15);
        let locked = kids.iter().find(|c| c.name == "locked").unwrap();
        assert_eq!(locked.measure, ChildMeasure::NotMeasured);
        assert_eq!(locked.bytes, None, "not measured is absent, not 0");
        let partial = kids.iter().find(|c| c.name == "partial").unwrap();
        assert_eq!(partial.measure, ChildMeasure::Partial);
        assert_eq!(partial.bytes, Some(50));
        assert_eq!(rows_total(&kids), 150);
    }

    #[test]
    fn a_huge_child_count_stays_bounded_and_still_adds_up() {
        let unit = Path::new("/u");
        let n = 20_000u64;
        let mut dirs = vec![row("", 0, 0, n as u32, true)];
        let mut total = 0;
        for i in 0..n {
            dirs.push(row(&format!("c{i:05}"), i + 1, 1, 0, true));
            total += i + 1;
        }
        let kids = children_of(unit, dirs, total, DRILLDOWN_TOP_N);
        assert_eq!(
            kids.len(),
            DRILLDOWN_TOP_N + 1,
            "top N plus one remainder row"
        );
        assert_eq!(rows_total(&kids), total as i64);
        assert_eq!(kids[0].bytes, Some(n as i64));
    }

    #[test]
    fn a_hardlink_counted_once_is_a_named_adjustment_not_a_vanished_byte() {
        let unit = Path::new("/u");
        let dirs = vec![
            row("", 0, 0, 2, true),
            row("a", 100, 1, 0, true),
            row("b", 100, 1, 0, true),
        ];
        // The walk counted a shared file once: 150, not 200.
        let kids = children_of(unit, dirs, 150, 15);
        assert_eq!(rows_total(&kids), 150);
        assert!(
            kids.iter()
                .any(|c| c.kind == ChildKind::Adjustment && c.bytes == Some(-50))
        );
    }

    #[test]
    fn not_measured_folders_beyond_the_room_are_counted_in_the_remainder() {
        let unit = Path::new("/u");
        let mut dirs = vec![row("", 0, 0, 5, true), row("big", 500, 1, 0, true)];
        for i in 0..4 {
            dirs.push(row(&format!("x{i}"), 0, 0, 0, false));
        }
        let kids = children_of(unit, dirs, 500, 2);
        assert_eq!(
            kids.iter().filter(|c| c.kind == ChildKind::Entry).count(),
            2
        );
        let rest = kids.last().unwrap();
        assert_eq!(rest.kind, ChildKind::Remainder);
        assert_eq!(rest.not_measured, 3);
        assert_eq!(rows_total(&kids), 500);
    }
}

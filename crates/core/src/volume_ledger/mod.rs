//! The volume ledger (#169, #170): where the disk went, for the whole
//! machine, measured once in a scheduled pass and read back without
//! touching the disk.
//!
//! * [`pass`] measures. It runs only inside `swamp observe` (never on
//!   `swamp ui` open, never in `report`), at background priority, under a
//!   time budget, with a cursor that resumes where the last run stopped.
//! * [`system`] asks `diskutil`/`tmutil` (read-only, allow-listed in
//!   `fs_gate::spawn`) about the other volumes in the APFS container,
//!   purgeable space and local snapshots. A query that fails is a note on
//!   its row, never a zero.
//! * This module holds the row vocabulary and [`account`], the pure
//!   function that turns stored rows into the reading: accounted,
//!   everything else, system volumes, not measured, and a *named*
//!   residual so the parts add up to the container's used bytes.
//!
//! Nothing here says a byte can be removed: the ledger reports.

pub mod pass;
pub mod system;
mod view;

pub use view::{NOT_MEASURED_YET, disk_json, render_disk_view};

use crate::growth::{VolumeLedgerRow, VolumeMetaRow};
use std::path::PathBuf;

/// Rows that a report lists by name are capped so a machine with
/// hundreds of protected folders cannot make the ledger or the report
/// unbounded. The count is exact; the names shown are the first ones.
pub const MAX_NAMED_ROWS: usize = 200;

/// How many of the largest "everything else" rows a report shows.
pub const TOP_ELSEWHERE: usize = 5;

/// The one sentence about Full Disk Access: said, never requested.
pub const FDA_NOTE: &str = "macOS keeps some folders (Photos, Mail, Messages, Safari, Group Containers, Containers) unreadable without Full Disk Access; swamp does not ask for it, so their size is not measured";

/// The named residual: what the parts do not explain.
pub const RESIDUAL_NAME: &str = "unattributed: APFS accounting, TCC-blocked, clones";

/// The estimate for what could not be read, from the Data volume's own
/// consumed bytes minus everything measured.
pub const NOT_MEASURED_ESTIMATE_NAME: &str = "not measured, estimated by elimination";

/// Row method of an accounted location: taken from the observation, not
/// walked again.
pub const METHOD_OBSERVATION: &str = "observation";
/// Row method of a coarse walk: allocated bytes (`st_blocks`), `lstat`
/// only, hardlinks once, no file contents.
pub const METHOD_WALK: &str = "walk: allocated bytes, lstat only";
/// A folder that took a whole run alone: it is measured as its children
/// from now on. Structure, never a size.
pub const METHOD_EXPANDED: &str = "expanded";
/// A single folder larger than one run's time budget.
pub const METHOD_OVER_BUDGET: &str = "over_budget";

/// What a row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// A built-in catalog location (a toolchain, a cache, an agent home).
    Catalog,
    /// A source root the user declared (or the built-in default root).
    Declared,
    /// A coarse folder outside both.
    Other,
    /// Another volume in the APFS container, or a sealed-system note.
    System,
    /// Purgeable space (a subset of bytes already in the rows above).
    Purgeable,
    /// Local snapshots.
    Snapshot,
    /// A directory that could not be read.
    Unreadable,
    /// A mounted disk image: a view of bytes stored elsewhere.
    Mount,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Catalog => "catalog",
            Category::Declared => "declared",
            Category::Other => "other",
            Category::System => "system",
            Category::Purgeable => "purgeable",
            Category::Snapshot => "snapshot",
            Category::Unreadable => "unreadable",
            Category::Mount => "mount",
        }
    }

    fn parse(s: &str) -> Option<Category> {
        Some(match s {
            "catalog" => Category::Catalog,
            "declared" => Category::Declared,
            "other" => Category::Other,
            "system" => Category::System,
            "purgeable" => Category::Purgeable,
            "snapshot" => Category::Snapshot,
            "unreadable" => Category::Unreadable,
            "mount" => Category::Mount,
            _ => return None,
        })
    }
}

/// How sure a row's bytes are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Exactness {
    /// Allocated bytes read from the volume when the row was measured.
    Exact,
    /// A lower bound or a figure that moved while it was read.
    Estimated,
    /// No figure. Never zero.
    NotMeasured,
}

impl Exactness {
    pub fn as_str(self) -> &'static str {
        match self {
            Exactness::Exact => "exact",
            Exactness::Estimated => "estimated",
            Exactness::NotMeasured => "not_measured",
        }
    }

    fn parse(s: &str) -> Option<Exactness> {
        Some(match s {
            "exact" => Exactness::Exact,
            "estimated" => Exactness::Estimated,
            "not_measured" => Exactness::NotMeasured,
            _ => return None,
        })
    }
}

/// One ledger row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Row {
    pub path: String,
    pub category: Category,
    /// `None` exactly when the row is `not_measured`.
    pub bytes: Option<u64>,
    /// The part of `bytes` an enclosing row already counts.
    pub overlap_bytes: u64,
    pub entries: Option<u64>,
    /// Directories and files inside this row that could not be read.
    pub unreadable: u64,
    pub measured_at: u64,
    pub method: String,
    pub exactness: Exactness,
    pub note: Option<String>,
}

impl Row {
    /// The bytes this row adds to a total.
    pub fn additive(&self) -> u64 {
        self.bytes.unwrap_or(0).saturating_sub(self.overlap_bytes)
    }

    pub(crate) fn to_stored(&self) -> VolumeLedgerRow {
        VolumeLedgerRow {
            path: self.path.clone(),
            category: self.category.as_str().to_string(),
            allocated_bytes: self.bytes,
            overlap_bytes: self.overlap_bytes,
            entry_count: self.entries,
            unreadable_count: self.unreadable,
            measured_at: self.measured_at,
            method: self.method.clone(),
            exactness: self.exactness.as_str().to_string(),
            note: self.note.clone(),
        }
    }

    /// `None` for a row whose category or exactness this build does not
    /// know (a newer store): skipped, never guessed.
    pub(crate) fn from_stored(r: &VolumeLedgerRow) -> Option<Row> {
        Some(Row {
            path: r.path.clone(),
            category: Category::parse(&r.category)?,
            bytes: r.allocated_bytes,
            overlap_bytes: r.overlap_bytes,
            entries: r.entry_count,
            unreadable: r.unreadable_count,
            measured_at: r.measured_at,
            method: r.method.clone(),
            exactness: Exactness::parse(&r.exactness)?,
            note: r.note.clone(),
        })
    }

    fn is_structure(&self) -> bool {
        self.method == METHOD_EXPANDED
    }
}

/// A location the observation already measured: its total is reused, not
/// walked again.
///
/// Locations are disjoint by construction (an observation measures a
/// unit *without* the units nested inside it, so `/opt/homebrew` and the
/// formulae under it add up), with two exceptions this type carries:
/// [`Self::subset_of_enclosing`] and the mounted images
/// [`accounted_rows`] takes.
#[derive(Debug, Clone)]
pub struct Accounted {
    pub path: PathBuf,
    pub bytes: u64,
    pub category: Category,
    /// The bytes are a finer view of a location that may already hold
    /// them (an agent tool's sessions and caches inside its home, which is
    /// also a catalog location): not added when one encloses it.
    pub subset_of_enclosing: bool,
    pub measured_at: u64,
    /// `true` when the total is a lower bound (a coverage note says why).
    pub incomplete: bool,
    pub note: Option<String>,
}

/// What a mounted filesystem is, for the accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountKind {
    /// A volume with its own storage (a mounted disk image): its bytes
    /// are the image file, counted where that file is stored.
    OwnStorage,
    /// Another volume of this disk's APFS container: its bytes are in the
    /// system-volume lines, and `statfs` would report the whole
    /// container's.
    SameContainer,
    /// A network share: not on this disk at all.
    Remote,
    /// Mounted on another device, not in the mount table.
    Unknown,
}

/// One mount the accounting needs to know about.
#[derive(Debug, Clone)]
pub struct MountView {
    pub path: PathBuf,
    pub kind: MountKind,
    /// Bytes in use, for [`MountKind::OwnStorage`].
    pub used: Option<u64>,
}

/// The non-additive row for a mount.
pub fn mount_row(m: &MountView, measured_at: u64) -> Row {
    let (bytes, exactness, note) = match (m.kind, m.used) {
        (MountKind::OwnStorage, Some(b)) => (
            Some(b),
            Exactness::Exact,
            "a mounted disk image: a view of a file counted where that file is stored, not added",
        ),
        (MountKind::OwnStorage, None) => (
            None,
            Exactness::NotMeasured,
            "a mounted disk image whose size could not be read; its bytes are the image file",
        ),
        (MountKind::SameContainer, _) => (
            None,
            Exactness::NotMeasured,
            "another volume of this disk's container: counted in the system volume lines",
        ),
        (MountKind::Remote, _) => (
            None,
            Exactness::NotMeasured,
            "a network share: not on this disk, not measured",
        ),
        (MountKind::Unknown, _) => (
            None,
            Exactness::NotMeasured,
            "mounted on another device: not walked, not measured",
        ),
    };
    Row {
        path: m.path.display().to_string(),
        category: Category::Mount,
        bytes,
        overlap_bytes: 0,
        entries: None,
        unreadable: 0,
        measured_at,
        method: "statfs".to_string(),
        exactness,
        note: Some(note.to_string()),
    }
}

/// Rows for the accounted locations, with double counting removed:
///
/// * a `subset_of_enclosing` location inside (or equal to) another
///   accounted location is shown, and added zero times;
/// * mounted disk images inside a location (an observation that walked a
///   unit through its mount points counted their bytes) are taken back
///   out as overlap: the image files are counted where they are stored.
pub fn accounted_rows(list: &[Accounted], mounts: &[MountView]) -> Vec<Row> {
    let mut order: Vec<&Accounted> = list.iter().collect();
    order.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then(b.bytes.cmp(&a.bytes))
            .then((a.category as u8).cmp(&(b.category as u8)))
    });
    let mut rows = Vec::new();
    for a in &order {
        let mut overlap = 0u64;
        let mut notes: Vec<String> = a.note.iter().cloned().collect();
        if a.subset_of_enclosing
            && list.iter().any(|o| {
                !o.subset_of_enclosing && !std::ptr::eq(o, *a) && a.path.starts_with(&o.path)
            })
        {
            overlap = a.bytes;
            notes.push("inside another location and counted there".to_string());
        }
        let views: u64 = mounts
            .iter()
            .filter(|m| m.kind == MountKind::OwnStorage && m.path.starts_with(&a.path))
            .filter_map(|m| m.used)
            .sum();
        let views = views.min(a.bytes - overlap);
        if views > 0 {
            overlap += views;
            notes.push(format!(
                "{} of it is mounted disk images, views of image files counted where they are stored",
                crate::render::human_bytes_pub(views)
            ));
        }
        rows.push(Row {
            path: a.path.display().to_string(),
            category: a.category,
            bytes: Some(a.bytes),
            overlap_bytes: overlap,
            entries: None,
            unreadable: 0,
            measured_at: a.measured_at,
            method: METHOD_OBSERVATION.to_string(),
            exactness: if a.incomplete {
                Exactness::Estimated
            } else {
                Exactness::Exact
            },
            note: (!notes.is_empty()).then(|| notes.join("; ")),
        });
    }
    rows
}

/// The accounting identity, from stored rows. Every figure names what it
/// is; the residual is the one place that can be negative.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Account {
    pub measured_at: u64,
    pub cycle_complete_at: u64,
    /// The cycle covered every location; `false` while a budget-limited
    /// pass is still working through them.
    pub complete: bool,
    pub budget_secs: u64,
    pub budget_used_ms: u64,
    pub statfs_at: u64,
    pub container: Container,
    pub accounted: Part,
    pub everything_else: Elsewhere,
    pub system_volumes: SystemVolumes,
    pub purgeable: Option<Row>,
    pub snapshots: Option<Row>,
    pub not_measured: NotMeasured,
    pub residual: Residual,
    /// Mounted disk images: their bytes live in the image files counted
    /// where they are stored, so these are never added.
    pub mounted_views: Vec<Row>,
    pub notes: Vec<String>,
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Container {
    pub total: Option<u64>,
    pub used: Option<u64>,
    pub free: Option<u64>,
    /// The Data volume's own consumed bytes (`diskutil`).
    pub data_volume_used: Option<u64>,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Part {
    pub bytes: u64,
    pub locations: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Elsewhere {
    pub bytes: u64,
    pub folders: usize,
    pub top: Vec<Row>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SystemVolumes {
    pub bytes: u64,
    pub volumes: Vec<Row>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct NotMeasured {
    /// Directories that could not be read (exact count).
    pub count: usize,
    /// The first [`MAX_NAMED_ROWS`] of them.
    pub names: Vec<String>,
    /// Data volume consumed bytes minus everything measured, when the
    /// Data volume's figure is known. An estimate by elimination.
    pub estimate_bytes: Option<u64>,
    pub estimate_name: &'static str,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Residual {
    pub name: &'static str,
    /// Container used minus every part above; negative when the parts
    /// add up to more than the container holds (APFS clones and shared
    /// extents are counted once per file). `None` without a container
    /// figure.
    pub bytes: Option<i64>,
    pub percent_of_used: Option<f64>,
    /// `|residual|` is within 1% of the container's used bytes.
    pub within_one_percent: Option<bool>,
}

/// Rows and meta into the reading. Pure: no disk, no clock.
pub fn account(rows: &[Row], meta: &VolumeMetaRow) -> Account {
    let live: Vec<&Row> = rows.iter().filter(|r| !r.is_structure()).collect();
    let accounted_rows: Vec<&Row> = live
        .iter()
        .copied()
        .filter(|r| matches!(r.category, Category::Catalog | Category::Declared))
        .collect();
    let accounted = Part {
        bytes: accounted_rows.iter().map(|r| r.additive()).sum(),
        locations: accounted_rows.len(),
    };
    let mut elsewhere_rows: Vec<&Row> = live
        .iter()
        .copied()
        .filter(|r| r.category == Category::Other && r.bytes.is_some())
        .collect();
    let else_bytes: u64 = elsewhere_rows.iter().map(|r| r.additive()).sum();
    elsewhere_rows.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.path.cmp(&b.path)));
    let top: Vec<Row> = elsewhere_rows
        .iter()
        .take(TOP_ELSEWHERE)
        .map(|r| (*r).clone())
        .collect();
    let volumes: Vec<Row> = live
        .iter()
        .copied()
        .filter(|r| r.category == Category::System && r.bytes.is_some())
        .cloned()
        .collect();
    let system_bytes: u64 = volumes.iter().map(|r| r.additive()).sum();
    let purgeable = live
        .iter()
        .copied()
        .find(|r| r.category == Category::Purgeable)
        .cloned();
    let snapshots = live
        .iter()
        .copied()
        .find(|r| r.category == Category::Snapshot)
        .cloned();
    let snapshot_bytes = snapshots.as_ref().and_then(|r| r.bytes).unwrap_or(0);
    let unreadable: Vec<&Row> = live
        .iter()
        .copied()
        .filter(|r| r.category == Category::Unreadable)
        .collect();
    let mounted_views: Vec<Row> = live
        .iter()
        .copied()
        .filter(|r| r.category == Category::Mount)
        .cloned()
        .collect();

    let measured_data_side = accounted.bytes + else_bytes;
    let estimate = meta
        .data_volume_used
        .map(|used| used.saturating_sub(measured_data_side));
    let parts_sum =
        accounted.bytes + else_bytes + system_bytes + snapshot_bytes + estimate.unwrap_or(0);
    let (residual_bytes, percent, within) = match meta.container_used {
        Some(used) => {
            let r = used as i64 - parts_sum as i64;
            let pct = if used == 0 {
                0.0
            } else {
                r as f64 * 100.0 / used as f64
            };
            (Some(r), Some(pct), Some(r.unsigned_abs() * 100 <= used))
        }
        None => (None, None, None),
    };

    let mut notes: Vec<String> = Vec::new();
    for r in &live {
        // Purgeable space and snapshots carry their own note on their own
        // line; a system-volume row that could not be read has no line.
        if let (Some(n), true) = (
            &r.note,
            matches!(r.exactness, Exactness::NotMeasured) && r.category == Category::System,
        ) {
            notes.push(format!("{}: {n}", r.path));
        }
    }
    if !unreadable.is_empty() {
        notes.push(FDA_NOTE.to_string());
    }
    if meta.container_used.is_none() {
        notes.push(
            "container size not available: the parts are listed but not reconciled".to_string(),
        );
    }
    if meta.data_volume_used.is_none() {
        notes.push(
            "the Data volume's own size is not known (diskutil), so what could not be read is not estimated; it is inside the unattributed line".to_string(),
        );
    }
    if !meta.complete {
        notes.push(
            "the pass has not finished: rows keep the time they were measured, and the next observe continues".to_string(),
        );
    }

    Account {
        measured_at: meta.measured_at,
        cycle_complete_at: meta.cycle_complete_at,
        complete: meta.complete,
        budget_secs: meta.budget_secs,
        budget_used_ms: meta.budget_used_ms,
        statfs_at: meta.statfs_at,
        container: Container {
            total: meta.container_total,
            used: meta.container_used,
            free: meta.container_free,
            data_volume_used: meta.data_volume_used,
        },
        accounted,
        everything_else: Elsewhere {
            bytes: else_bytes,
            folders: elsewhere_rows.len(),
            top,
        },
        system_volumes: SystemVolumes {
            bytes: system_bytes,
            volumes,
        },
        purgeable,
        snapshots,
        not_measured: NotMeasured {
            // One row can stand for many folders (the tail past the named
            // ones): the count is theirs, the names are the first ones.
            count: unreadable
                .iter()
                .map(|r| r.unreadable.max(1) as usize)
                .sum(),
            names: unreadable
                .iter()
                .take(MAX_NAMED_ROWS)
                .map(|r| r.path.clone())
                .collect(),
            estimate_bytes: estimate,
            estimate_name: NOT_MEASURED_ESTIMATE_NAME,
        },
        residual: Residual {
            name: RESIDUAL_NAME,
            bytes: residual_bytes,
            percent_of_used: percent,
            within_one_percent: within,
        },
        mounted_views,
        notes,
        rows: rows.to_vec(),
    }
}

/// The stored ledger as a reading. `Ok(None)` when no pass ever wrote
/// one. Two small Parquet reads; no listing, no stat, no spawn.
pub fn read_account(swamp_dir: &std::path::Path) -> anyhow::Result<Option<Account>> {
    let (stored, meta) = crate::growth::read_volume_ledger(swamp_dir)?;
    let Some(meta) = meta else {
        return Ok(None);
    };
    let rows: Vec<Row> = stored.iter().filter_map(Row::from_stored).collect();
    Ok(Some(account(&rows, &meta)))
}

/// "measured 3 h ago" from a stored time and now.
pub fn age_text(then: u64, now: u64) -> String {
    let secs = now.saturating_sub(then);
    if secs < 90 {
        "just now".to_string()
    } else if secs < 90 * 60 {
        format!("{} min ago", secs / 60)
    } else if secs < 48 * 3600 {
        format!("{} h ago", secs / 3600)
    } else {
        format!("{} d ago", secs / 86400)
    }
}

/// The locations this observation already measured, for the ledger to
/// reuse: each project root's walked total, each external unit and each
/// agent tool's home. `observed_at` is the observation's own time.
pub fn accounted_locations(
    observation: &crate::report::ScopeObservation,
    scope: &crate::scope::EffectiveScope,
    observed_at: u64,
) -> Vec<Accounted> {
    let mut out: Vec<Accounted> = Vec::new();
    for c in &observation.coverage {
        if !c.status.was_observed() {
            continue;
        }
        let is_project_root = scope
            .roots
            .iter()
            .any(|r| r.path == c.path && r.is_project_root());
        if !is_project_root {
            continue;
        }
        out.push(Accounted {
            path: c.path.clone(),
            bytes: c.walked_total,
            category: Category::Declared,
            subset_of_enclosing: false,
            measured_at: observed_at,
            incomplete: matches!(c.status, crate::coverage::RegionStatus::Partial { .. }),
            note: None,
        });
    }
    for u in &observation.external_units {
        out.push(Accounted {
            path: u.path.clone(),
            bytes: u.bytes,
            category: Category::Catalog,
            subset_of_enclosing: false,
            measured_at: observed_at,
            incomplete: u.note.is_some(),
            note: u.note.clone(),
        });
    }
    let mut homes: std::collections::BTreeMap<PathBuf, (u64, bool)> =
        std::collections::BTreeMap::new();
    for u in &observation.agent_units {
        let e = homes.entry(u.tool_home.clone()).or_insert((0, false));
        e.0 += u.bytes;
        e.1 |= !u.complete;
    }
    for (path, (bytes, incomplete)) in homes {
        out.push(Accounted {
            path,
            bytes,
            category: Category::Catalog,
            subset_of_enclosing: true,
            measured_at: observed_at,
            incomplete,
            note: Some("agent sessions, caches and logs".to_string()),
        });
    }
    out
}

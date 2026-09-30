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

pub use view::{DEFAULT_JSON_ROWS, NOT_MEASURED_YET, disk_json, render_disk_view};

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
pub const RESIDUAL_NAME: &str = "unattributed: allocation not explained by any measured part";

/// The estimate for what could not be read, from the Data volume's own
/// consumed bytes minus everything measured.
pub const NOT_MEASURED_ESTIMATE_NAME: &str =
    "not measured (unreadable folders or not yet measured), estimated";

/// A row that says a location has not been measured yet this cycle.
pub const METHOD_PENDING: &str = "pending";
/// Audit rows are keyed `spot audit: <folder>`, so they can never be
/// mistaken for the folder's own row.
pub const AUDIT_PREFIX: &str = "spot audit: ";
/// A spot-audit row.
pub const METHOD_AUDIT: &str = "spot audit: naive lstat sum";
/// A folder that did not answer by the hard deadline.
pub const METHOD_STUCK: &str = "stuck";
/// A folder that was stuck three runs in a row: left until the next cycle.
pub const METHOD_SKIPPED: &str = "skipped";
/// Half a percent is not a tolerance: an audit difference is within
/// `max(1%, 4 MiB)`.
pub const AUDIT_TOLERANCE_BYTES: u64 = 4 << 20;

/// A lossless `path` for a row: the path as text when it is UTF-8, and
/// otherwise its lossy text plus the exact bytes in hex, so two names that
/// differ only in invalid UTF-8 are two rows, not one.
pub fn key_of(path: &std::path::Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    match path.to_str() {
        Some(s) => s.to_string(),
        None => {
            let hex: String = path
                .as_os_str()
                .as_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            format!("{}#bytes-{hex}", path.to_string_lossy())
        }
    }
}

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
    /// A location on another volume (a declared root on an external
    /// disk): real bytes, but not on this container.
    External,
    /// One folder of the spot audit (`bytes` = audited bytes, `entries` =
    /// the ledger's bytes for it).
    Audit,
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
            Category::External => "external",
            Category::Audit => "audit",
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
            "external" => Category::External,
            "audit" => Category::Audit,
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
        path: key_of(&m.path),
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
/// * a `subset_of_enclosing` location (an agent tool's sessions and
///   caches) is a view of a folder the walk or a catalog location already
///   measures whole: shown, never added, never pruned from the walk;
/// * mounted disk images inside a location (an observation that walked a
///   unit through its mount points counted their bytes) are taken back
///   out as overlap: the image files are counted where they are stored;
/// * a location on another volume (under a mount that is not this
///   container's own storage) is real, but not part of this container:
///   listed apart, never added.
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
        let mut category = a.category;
        let mut notes: Vec<String> = a.note.iter().cloned().collect();
        let other_volume = mounts
            .iter()
            .filter(|m| a.path.starts_with(&m.path) && m.kind != MountKind::SameContainer)
            .max_by_key(|m| m.path.components().count());
        if let Some(m) = other_volume {
            category = Category::External;
            notes.push(format!(
                "on {}: not part of this container",
                m.path.display()
            ));
        } else if a.subset_of_enclosing {
            overlap = a.bytes;
            notes.push(
                "a finer view of a folder measured as a whole elsewhere in this ledger; counted there, not added"
                    .to_string(),
            );
        } else {
            let views = mounts
                .iter()
                .filter(|m| m.kind == MountKind::OwnStorage && m.path.starts_with(&a.path))
                .filter_map(|m| m.used)
                .fold(0u64, u64::saturating_add)
                .min(a.bytes);
            if views > 0 {
                overlap = views;
                notes.push(format!(
                    "{} of it is mounted disk images, views of image files counted where they are stored",
                    crate::render::human_bytes_pub(views)
                ));
            }
        }
        rows.push(Row {
            path: key_of(&a.path),
            category,
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
/// is. The residual is NOT absorbed by anything: it is what no measured
/// part explains, it is signed, and it is flagged when it exceeds 1% of
/// the container's used bytes.
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
    pub audit: SpotAudit,
    /// Mounted disk images: their bytes live in the image files counted
    /// where they are stored, so these are never added.
    pub mounted_views: Vec<Row>,
    /// Locations on other volumes: not part of this container.
    pub external_volumes: Vec<Row>,
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
    /// Locations no run of the current cycle has measured yet (exact
    /// list, from the cursor).
    pub not_yet_measured: usize,
    pub not_yet_measured_names: Vec<String>,
    /// Data volume consumed bytes minus everything measured, counted only
    /// while something is unreadable or not yet measured. An ESTIMATE,
    /// shown apart from the measured parts; `None` when nothing is
    /// unreadable or pending (then the gap is the residual).
    pub estimate_bytes: Option<u64>,
    pub estimate_name: &'static str,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Residual {
    pub name: &'static str,
    /// Container used minus every part above, protected-folder estimate
    /// included; negative when the parts add up to more than the container
    /// holds (APFS clones and shared extents are counted once per file).
    /// `None` without a container figure.
    pub bytes: Option<i64>,
    pub percent_of_used: Option<f64>,
    /// BOOKKEEPING: the parts, with the estimate, add up to the container's
    /// used bytes within 1%. This is arithmetic, not evidence about the
    /// walk: the estimate is defined as a leftover, so it can balance
    /// anything. The walk is checked by the spot audit.
    pub bookkeeping_balanced: Option<bool>,
    /// `bookkeeping_balanced` is false (kept under this name for readers of
    /// the previous shape).
    pub residual_flag: bool,
    /// Container used minus the MEASURED parts only (no estimate): what
    /// the measurements do not explain, protected folders included. Not a
    /// pass/fail claim about the walk.
    pub unexplained_bytes: Option<i64>,
    /// `|unexplained|` is within 1% of used (an unusual thing on a Mac
    /// with protected folders; informational, never a claim about the
    /// walk).
    pub within_one_percent: Option<bool>,
}

/// One folder the pass measured twice: by the walk and by an independent
/// naive audit.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AuditFolder {
    pub path: String,
    pub ledger_bytes: u64,
    pub audited_bytes: u64,
    pub difference: i64,
    pub percent: f64,
    /// The difference is outside `max(1%, 4 MiB)`.
    pub outside_tolerance: bool,
}

/// The independent spot audit: the only check that can fail from the walk.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SpotAudit {
    pub folders: Vec<AuditFolder>,
    /// Why no folder was audited this pass (budget spent, nothing to
    /// audit); `None` when it ran.
    pub skipped: Option<String>,
    pub max_difference_percent: f64,
    pub audit_flag: bool,
}

fn sum_u64(values: impl Iterator<Item = u64>) -> u64 {
    values.fold(0u64, u64::saturating_add)
}

fn clamp_i64(v: i128) -> i64 {
    v.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

fn spot_audit(live: &[&Row]) -> SpotAudit {
    let mut folders = Vec::new();
    let mut skipped = None;
    for r in live.iter().filter(|r| r.category == Category::Audit) {
        match (r.bytes, r.entries) {
            (Some(audited), Some(ledger)) => {
                let diff = audited as i128 - ledger as i128;
                let pct = if ledger == 0 {
                    if audited == 0 { 0.0 } else { 100.0 }
                } else {
                    diff as f64 * 100.0 / ledger as f64
                };
                folders.push(AuditFolder {
                    path: r
                        .path
                        .strip_prefix(AUDIT_PREFIX)
                        .unwrap_or(&r.path)
                        .to_string(),
                    ledger_bytes: ledger,
                    audited_bytes: audited,
                    difference: clamp_i64(diff),
                    percent: pct,
                    outside_tolerance: r.exactness == Exactness::Estimated,
                });
            }
            _ => skipped = Some(r.note.clone().unwrap_or_else(|| "not run".to_string())),
        }
    }
    if folders.is_empty() && skipped.is_none() {
        skipped = Some("not run this pass".to_string());
    }
    let max = folders.iter().map(|f| f.percent.abs()).fold(0.0, f64::max);
    SpotAudit {
        audit_flag: folders.iter().any(|f| f.outside_tolerance),
        max_difference_percent: max,
        folders,
        skipped,
    }
}

/// Rows and meta into the reading. Pure: no disk, no clock. Every sum
/// saturates, so a corrupt or foreign ledger with huge values cannot
/// panic or wrap.
pub fn account(rows: &[Row], meta: &VolumeMetaRow) -> Account {
    let live: Vec<&Row> = rows.iter().filter(|r| !r.is_structure()).collect();
    let pending: Vec<&Row> = live
        .iter()
        .copied()
        .filter(|r| r.method == METHOD_PENDING)
        .collect();
    let accounted_rows: Vec<&Row> = live
        .iter()
        .copied()
        .filter(|r| matches!(r.category, Category::Catalog | Category::Declared))
        .collect();
    let accounted = Part {
        bytes: sum_u64(accounted_rows.iter().map(|r| r.additive())),
        locations: accounted_rows.len(),
    };
    let mut elsewhere_rows: Vec<&Row> = live
        .iter()
        .copied()
        .filter(|r| r.category == Category::Other && r.bytes.is_some())
        .collect();
    let else_bytes = sum_u64(elsewhere_rows.iter().map(|r| r.additive()));
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
    let system_bytes = sum_u64(volumes.iter().map(|r| r.additive()));
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
    let unreadable_count = sum_u64(unreadable.iter().map(|r| r.unreadable.max(1)));
    let mounted_views: Vec<Row> = live
        .iter()
        .copied()
        .filter(|r| r.category == Category::Mount)
        .cloned()
        .collect();
    let external_volumes: Vec<Row> = live
        .iter()
        .copied()
        .filter(|r| r.category == Category::External)
        .cloned()
        .collect();

    // The estimate exists only while something is unreadable or not yet
    // measured. With nothing to blame, the gap stays in the residual,
    // where it is checked.
    let measured_data_side = accounted.bytes.saturating_add(else_bytes);
    let estimate = match meta.data_volume_used {
        Some(used) if unreadable_count > 0 || !pending.is_empty() => {
            Some(used.saturating_sub(measured_data_side))
        }
        _ => None,
    };
    let parts_sum: u128 = [
        accounted.bytes,
        else_bytes,
        system_bytes,
        snapshot_bytes,
        estimate.unwrap_or(0),
    ]
    .iter()
    .map(|v| *v as u128)
    .sum();
    let measured_sum: u128 = [accounted.bytes, else_bytes, system_bytes, snapshot_bytes]
        .iter()
        .map(|v| *v as u128)
        .sum();
    let (residual_bytes, percent, balanced, unexplained, within_unexplained) =
        match meta.container_used {
            Some(used) => {
                let r: i128 = used as i128 - parts_sum as i128;
                let u: i128 = used as i128 - measured_sum as i128;
                let pct = if used == 0 {
                    0.0
                } else {
                    r as f64 * 100.0 / used as f64
                };
                (
                    Some(clamp_i64(r)),
                    Some(pct),
                    Some(r.unsigned_abs().saturating_mul(100) <= used as u128),
                    Some(clamp_i64(u)),
                    Some(u.unsigned_abs().saturating_mul(100) <= used as u128),
                )
            }
            None => (None, None, None, None, None),
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
    if unreadable_count > 0 {
        notes.push(FDA_NOTE.to_string());
    }
    if meta.container_used.is_none() {
        notes.push(
            "container size not available: the parts are listed but not reconciled".to_string(),
        );
    }
    if meta.data_volume_used.is_none() {
        notes.push(
            "the Data volume's own size is not known (diskutil), so what could not be read is not estimated; any gap is in the unattributed line".to_string(),
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
            count: unreadable_count.min(usize::MAX as u64) as usize,
            names: unreadable
                .iter()
                .take(MAX_NAMED_ROWS)
                .map(|r| r.path.clone())
                .collect(),
            not_yet_measured: pending.len(),
            not_yet_measured_names: pending
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
            bookkeeping_balanced: balanced,
            residual_flag: balanced == Some(false),
            unexplained_bytes: unexplained,
            within_one_percent: within_unexplained,
        },
        audit: spot_audit(&live),
        mounted_views,
        external_volumes,
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

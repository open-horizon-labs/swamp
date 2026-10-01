//! Last-used facts for storage units, with the source of each one.
//!
//! **Precedence** (maintainer rule, #176), highest first:
//!
//! 1. a **tool-native record** of use: the tool's own database or
//!    metadata, written for exactly this purpose, so a backup, an
//!    antivirus scan or an indexer cannot disturb it (Xcode DerivedData's
//!    `info.plist` `LastAccessedDate`, Cargo's `.global-cache`);
//! 2. the **file access time** of the unit's *key files* -- the regular
//!    files directly inside a `bin` directory, never a directory's own
//!    access time and never a symlink's -- only when no tool-native
//!    record exists;
//! 3. **no record**. Never a date derived from a modification time.
//!
//! When both a tool-native record and an access time exist and disagree,
//! the tool-native value is the one shown and the access time stays on
//! the fact ([`LastUsed::atime`]) so JSON keeps it.
//!
//! **What this never does.** Read a file's contents to learn an access
//! time (an `lstat` is enough, and a read would move the very timestamp
//! being measured), write anywhere, or call a value "unused". Access time
//! is touched by backup tools, antivirus and indexers, and a
//! `--version` counts as a read; `docs/usage.md` says so beside the table
//! of which unit kinds use which source.

use crate::locations::{CargoCacheTable, LastUseSource};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;

/// The label every rendered last-used fact starts with.
pub const LABEL: &str = "Last run or opened";

/// Tool-native source name for Xcode DerivedData's `info.plist`.
pub const XCODE_DERIVED_DATA: &str = "xcode-derived-data";
/// Tool-native source name for Cargo's `.global-cache`.
pub const CARGO_GLOBAL_CACHE: &str = "cargo-global-cache";

/// How many directories one unit's key-file scan may list. A unit whose
/// layout needs more is reported as `no record`, never as the newest of
/// a sample.
const MAX_LISTINGS: usize = 4096;
/// A tracker database larger than this is not opened.
const MAX_DATABASE_BYTES: u64 = 256 * 1024 * 1024;

/// Where a last-used value came from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LastUsedSource {
    /// The tool's own record, named (`xcode-derived-data`).
    ToolNative(String),
    /// Access time of the unit's key files.
    FileAtime,
    /// Nothing recorded the unit's use.
    #[default]
    None,
}

impl LastUsedSource {
    /// The stored and JSON spelling: `tool_native:<name>`, `file_atime`
    /// or `none`.
    pub fn label(&self) -> String {
        match self {
            Self::ToolNative(name) => format!("tool_native:{name}"),
            Self::FileAtime => "file_atime".to_string(),
            Self::None => "none".to_string(),
        }
    }

    pub fn from_label(label: &str) -> Self {
        match label {
            "file_atime" => Self::FileAtime,
            other => match other.strip_prefix("tool_native:") {
                Some(name) if !name.is_empty() => Self::ToolNative(name.to_string()),
                _ => Self::None,
            },
        }
    }

    /// The words a row puts in parentheses after the date.
    fn describe(&self) -> Option<String> {
        match self {
            Self::ToolNative(name) => Some(match name.as_str() {
                XCODE_DERIVED_DATA => "Xcode DerivedData record".to_string(),
                CARGO_GLOBAL_CACHE => "last used by cargo, from its global cache".to_string(),
                other => format!("{other} record"),
            }),
            Self::FileAtime => Some("file access time".to_string()),
            Self::None => None,
        }
    }
}

impl Serialize for LastUsedSource {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.label())
    }
}

impl<'de> Deserialize<'de> for LastUsedSource {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from_label(&String::deserialize(deserializer)?))
    }
}

/// One unit's last-used fact: the value, its source, and the access time
/// kept beside a tool-native value that won over it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LastUsed {
    /// Seconds since the epoch; `None` exactly when `source` is
    /// [`LastUsedSource::None`].
    pub at: Option<u64>,
    pub source: LastUsedSource,
    /// The key files' newest access time, whether or not it is the value
    /// shown. Present in JSON so the two can be compared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub atime: Option<u64>,
    /// Why there is no value when something was consulted and set aside:
    /// a date in the future, or a probe that hit its listing limit.
    /// `None` for a plain absence of signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why_none: Option<NoRecordWhy>,
}

/// Why a consulted source produced no value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoRecordWhy {
    /// The record was dated after now (a tracker written in milliseconds,
    /// a copied plist from a machine with a wrong clock): not a fact.
    FutureDate,
    /// The scan of the unit's `bin` directories hit its listing limit, so
    /// the newest access time seen would be that of a sample.
    ProbeLimit,
}

impl NoRecordWhy {
    fn label(self) -> &'static str {
        match self {
            Self::FutureDate => "future_date",
            Self::ProbeLimit => "probe_limit",
        }
    }
    fn from_label(s: &str) -> Option<Self> {
        Some(match s {
            "future_date" => Self::FutureDate,
            "probe_limit" => Self::ProbeLimit,
            _ => return None,
        })
    }
    fn describe(self) -> &'static str {
        match self {
            Self::FutureDate => "ignored: date in the future",
            Self::ProbeLimit => "probe limit reached",
        }
    }
}

/// A record dated after now (plus a day of clock slack) is not a fact: a
/// tracker written in milliseconds reads as the year 58,000, a plist
/// copied from a machine with a wrong clock as 2099. It is set aside with
/// its reason, never stated, and never "corrected" by guessing a unit.
fn plausible(ts: u64, now: u64) -> Result<u64, NoRecordWhy> {
    const SLACK: u64 = 86_400;
    if ts <= now.saturating_add(SLACK) {
        Ok(ts)
    } else {
        Err(NoRecordWhy::FutureDate)
    }
}

/// The precedence rule, in one place. A zero timestamp is "not
/// recorded", never the epoch; a date in the future is set aside
/// ([`NoRecordWhy::FutureDate`]), never stated as a fact.
pub fn resolve(tool_native: Option<(&str, u64)>, atime: Option<u64>) -> LastUsed {
    resolve_at(tool_native, atime, crate::entities::now())
}

pub(crate) fn resolve_at(
    tool_native: Option<(&str, u64)>,
    atime: Option<u64>,
    now: u64,
) -> LastUsed {
    let mut why = None;
    let atime = atime
        .filter(|t| *t > 0)
        .and_then(|t| match plausible(t, now) {
            Ok(t) => Some(t),
            Err(w) => {
                why = Some(w);
                None
            }
        });
    let native =
        tool_native
            .filter(|(_, t)| *t > 0)
            .and_then(|(name, t)| match plausible(t, now) {
                Ok(t) => Some((name, t)),
                Err(w) => {
                    why = Some(w);
                    None
                }
            });
    match native {
        Some((name, at)) => LastUsed {
            at: Some(at),
            source: LastUsedSource::ToolNative(name.to_string()),
            atime,
            why_none: None,
        },
        None => match atime {
            Some(at) => LastUsed {
                at: Some(at),
                source: LastUsedSource::FileAtime,
                atime,
                why_none: None,
            },
            None => LastUsed {
                why_none: why,
                ..LastUsed::default()
            },
        },
    }
}

impl LastUsed {
    /// Rebuilds a stored fact from its columns; an unknown source label
    /// or a missing value is `no record`. A `none:<why>` label keeps the
    /// reason a consulted source was set aside.
    pub fn from_columns(at: Option<u64>, source: Option<&str>, atime: Option<u64>) -> LastUsed {
        let label = source.unwrap_or("none");
        let why_none = label
            .strip_prefix("none:")
            .and_then(NoRecordWhy::from_label);
        let source = LastUsedSource::from_label(label);
        match (&source, at) {
            (LastUsedSource::None, _) | (_, None) => LastUsed {
                at: None,
                source: LastUsedSource::None,
                atime,
                why_none,
            },
            _ => LastUsed {
                at,
                source,
                atime,
                why_none: None,
            },
        }
    }

    /// The stored spelling of the source: [`LastUsedSource::label`], or
    /// `none:<why>` when a consulted source was set aside.
    pub fn source_column(&self) -> String {
        match (&self.source, self.why_none) {
            (LastUsedSource::None, Some(why)) => format!("none:{}", why.label()),
            (source, _) => source.label(),
        }
    }

    /// A probe that hit its listing limit, as a fact about this unit.
    pub(crate) fn probe_limit_reached() -> LastUsed {
        LastUsed {
            why_none: Some(NoRecordWhy::ProbeLimit),
            ..LastUsed::default()
        }
    }

    /// This fact with a tool-native record folded in under the same
    /// precedence rule (the key files' access time it already carries
    /// stays available).
    pub fn with_tool_native(&self, name: &str, at: u64) -> LastUsed {
        let mut merged = resolve(Some((name, at)), self.atime);
        if merged.at.is_none() {
            merged.why_none = merged.why_none.or(self.why_none);
        }
        merged
    }

    /// The row text, as a fact with its source:
    /// `Last run or opened: Jul 8 (file access time)`, or
    /// `Last run or opened: no record`.
    pub fn describe(&self, now: u64) -> String {
        format!("{LABEL}: {}", self.fact(now))
    }

    /// The value with its source, without the label: `Jul 8 (file access
    /// time)`, or `no record` (with the reason when a consulted source
    /// was set aside). A missing record is never a date.
    pub fn fact(&self, now: u64) -> String {
        match (self.at, self.source.describe()) {
            (Some(at), Some(source)) => format!("{} ({source})", format_day(at, now)),
            _ => match self.why_none {
                Some(why) => format!("no record ({})", why.describe()),
                None => "no record".to_string(),
            },
        }
    }
}

/// `Jul 8`, or `Jul 8, 2025` when the year is not the current one. UTC.
pub fn format_day(epoch: u64, now: u64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (year, month, day) = civil_from_days((epoch / 86_400) as i64);
    let (now_year, _, _) = civil_from_days((now / 86_400) as i64);
    let name = MONTHS[(month as usize).saturating_sub(1).min(11)];
    if year == now_year {
        format!("{name} {day}")
    } else {
        format!("{name} {day}, {year}")
    }
}

/// Days since 1970-01-01 to a civil (year, month, day), Howard Hinnant's
/// algorithm; no calendar dependency for one date label.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

// ---------------------------------------------------------------------
// Key-file access times
// ---------------------------------------------------------------------

/// What one bounded scan of a unit's `bin` directories found.
#[derive(Debug, Default)]
pub(crate) struct KeyFileScan {
    /// Newest access time over every key file found.
    pub(crate) newest: Option<u64>,
    /// The same, per first-level folder under the unit (a toolchain, a
    /// formula, a tool): what a depth-2 row shows for itself.
    pub(crate) by_child: BTreeMap<String, u64>,
    /// The listing budget ran out: the values above would be the newest
    /// of a sample, so the caller reports `no record` instead.
    pub(crate) truncated: bool,
}

/// Newest access time of the regular files directly inside each
/// directory named `bin` found at most `max_depth` levels below `unit`.
///
/// Listings go through the capped, counted [`crate::locations::shallow_list`],
/// which never follows or reports a symlink; each key file costs one
/// `lstat`. Nothing is opened, so nothing read here can move the access
/// time being measured.
pub(crate) fn scan_key_file_atimes(unit: &Path, max_depth: u8) -> KeyFileScan {
    use crate::fs_gate::MetadataExt;
    let mut scan = KeyFileScan::default();
    let mut listings = 0usize;
    let mut queue: VecDeque<(std::path::PathBuf, u8, Option<String>)> = VecDeque::new();
    queue.push_back((unit.to_path_buf(), 0, None));
    while let Some((dir, depth, top)) = queue.pop_front() {
        if listings >= MAX_LISTINGS {
            scan.truncated = true;
            break;
        }
        listings += 1;
        let listing = crate::locations::shallow_list(&dir);
        if matches!(
            listing.truncation,
            crate::locations::Truncation::Truncated { .. }
        ) {
            scan.truncated = true;
        }
        for entry in &listing.entries {
            if !entry.is_dir {
                continue;
            }
            let child_depth = depth + 1;
            let child_top = top.clone().unwrap_or_else(|| entry.name.clone());
            let child = dir.join(&entry.name);
            if entry.name == "bin" && child_depth <= max_depth {
                if listings >= MAX_LISTINGS {
                    scan.truncated = true;
                    continue;
                }
                listings += 1;
                let keys = crate::locations::shallow_list(&child);
                // A `bin` listed only in part (over the cap) or not at
                // all yields the newest of a sample, which is not the
                // unit's newest: the caller reports `no record`. Which
                // entries a capped listing keeps is the filesystem's
                // directory order, so a sample can even hold the real
                // newest on one filesystem and miss it on another.
                if !matches!(keys.truncation, crate::locations::Truncation::Complete) {
                    scan.truncated = true;
                }
                for key in &keys.entries {
                    if key.is_dir {
                        continue;
                    }
                    crate::work_counters::record_files_statted(1);
                    let Ok(meta) = crate::fs_gate::symlink_metadata(child.join(&key.name)) else {
                        continue;
                    };
                    if !meta.is_file() || meta.file_type().is_symlink() {
                        continue;
                    }
                    let atime = meta.atime();
                    if atime <= 0 {
                        continue;
                    }
                    let atime = atime as u64;
                    scan.newest = Some(scan.newest.map_or(atime, |n| n.max(atime)));
                    let slot = scan.by_child.entry(child_top.clone()).or_insert(0);
                    *slot = (*slot).max(atime);
                }
            } else if child_depth < max_depth {
                queue.push_back((child, child_depth, Some(child_top)));
            }
        }
    }
    scan
}

// ---------------------------------------------------------------------
// Cargo's tracker
// ---------------------------------------------------------------------

/// What reading Cargo's tracker gave.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CargoRead {
    Newest(u64),
    /// The table exists and has no rows.
    NoRows,
    /// Absent, locked, not SQLite, or a schema this reader does not know:
    /// the reason, for a caller that wants to say why there is no record.
    Unavailable(&'static str),
}

fn cargo_table_name(table: CargoCacheTable) -> &'static str {
    match table {
        CargoCacheTable::RegistryCrate => "registry_crate",
        CargoCacheTable::RegistrySrc => "registry_src",
        CargoCacheTable::GitDb => "git_db",
        CargoCacheTable::GitCheckout => "git_checkout",
    }
}

/// The newest `timestamp` in one table of `<cargo_home>/.global-cache`.
///
/// Read-only and bounded: the file must be a regular file under
/// [`MAX_DATABASE_BYTES`], the SQLite signature is checked before
/// SQLite opens it, one aggregate query runs, and a lock or a changed
/// schema is `Unavailable`, never an error and never a wait.
pub(crate) fn cargo_global_cache_newest(cargo_home: &Path, table: CargoCacheTable) -> CargoRead {
    let db = cargo_home.join(".global-cache");
    let Ok(meta) = crate::fs_gate::symlink_metadata(&db) else {
        return CargoRead::Unavailable("no .global-cache in this cargo home");
    };
    if !meta.is_file() || meta.file_type().is_symlink() {
        return CargoRead::Unavailable(".global-cache is not a regular file");
    }
    if meta.len() > MAX_DATABASE_BYTES {
        return CargoRead::Unavailable(".global-cache is larger than the read bound");
    }
    let Ok(header) = crate::fs_gate::read::bounded_read(
        &db,
        crate::fs_gate::read::BoundedCap::header_at_most(16),
    ) else {
        return CargoRead::Unavailable(".global-cache could not be read");
    };
    let Some(connection) = crate::sqlite_ro::open_immutable(&db, &header.bytes) else {
        return CargoRead::Unavailable(".global-cache is not an openable SQLite database");
    };
    let name = cargo_table_name(table);
    let Some(columns) = crate::sqlite_ro::table_columns(&connection, name) else {
        return CargoRead::Unavailable("the table this record lives in is not there");
    };
    if !columns.contains("timestamp") {
        return CargoRead::Unavailable("the table has no timestamp column");
    }
    match connection.query_row(&format!("SELECT MAX(timestamp) FROM {name}"), [], |row| {
        row.get::<_, Option<i64>>(0)
    }) {
        Ok(Some(t)) if t > 0 => CargoRead::Newest(t as u64),
        Ok(_) => CargoRead::NoRows,
        Err(_) => CargoRead::Unavailable(".global-cache is locked or unreadable right now"),
    }
}

// ---------------------------------------------------------------------
// One unit
// ---------------------------------------------------------------------

/// A unit's last-used fact, plus the per-child ones its key-file scan
/// produced.
#[derive(Debug, Default)]
pub(crate) struct UnitLastUse {
    pub(crate) last_used: LastUsed,
    pub(crate) children: BTreeMap<String, LastUsed>,
}

/// Evaluates every source a detector declared for the unit at `path`.
/// Sources that live outside the folded walk (Xcode's plist) contribute
/// nothing here; `consumer_wiring` folds them in where its plist reads
/// already happen.
pub(crate) fn probe(path: &Path, sources: &[LastUseSource], now: u64) -> UnitLastUse {
    let mut tool_native: Option<(&'static str, u64)> = None;
    let mut atime: Option<u64> = None;
    let mut children: BTreeMap<String, LastUsed> = BTreeMap::new();
    let mut limit_reached = false;
    for source in sources {
        match *source {
            LastUseSource::KeyFileAtime { max_depth } => {
                let scan = scan_key_file_atimes(path, max_depth);
                if scan.truncated {
                    limit_reached = true;
                    continue;
                }
                atime = atime.max(scan.newest);
                for (name, at) in scan.by_child {
                    children.insert(name, resolve_at(None, Some(at), now));
                }
            }
            LastUseSource::CargoGlobalCache { table, up } => {
                let Some(home) = path.ancestors().nth(up) else {
                    continue;
                };
                if let CargoRead::Newest(at) = cargo_global_cache_newest(home, table) {
                    tool_native = Some((CARGO_GLOBAL_CACHE, at));
                }
            }
            // Read where its plist reads already happen
            // (`consumer_wiring::attach_build_output_associations`), not here.
            LastUseSource::XcodeDerivedDataPlist => {}
            // Joined from the adapter's units after identification
            // (`build_adapters::model_stores::attach_last_read`).
            LastUseSource::AdapterStated => {}
        }
    }
    let mut last_used = resolve_at(tool_native, atime, now);
    if last_used.at.is_none() && limit_reached {
        last_used = LastUsed::probe_limit_reached();
    }
    UnitLastUse {
        last_used,
        children,
    }
}

/// Which last-use sources each measured location declared, as
/// `index into located -> sources`: each detector's declarations are
/// evaluated against the locations that detector itself published, with
/// the same anchor vocabulary the build-store join uses, so nothing here
/// names a detector or a path shape.
pub(crate) fn declared_sources(
    detectors: &crate::locations::Registry,
    located: &[crate::build_stores::Located],
) -> std::collections::HashMap<usize, Vec<LastUseSource>> {
    let mut out: std::collections::HashMap<usize, Vec<LastUseSource>> =
        std::collections::HashMap::new();
    for detector in detectors.detectors() {
        let mine: Vec<usize> = located
            .iter()
            .enumerate()
            .filter(|(_, l)| l.detector_id == detector.id())
            .map(|(i, _)| i)
            .collect();
        if mine.is_empty() {
            continue;
        }
        let published: Vec<(crate::locations::StorageCategory, &Path)> = mine
            .iter()
            .map(|&i| (located[i].category, located[i].path))
            .collect();
        for decl in detector.last_use_sources() {
            for j in decl.anchor.select(&published) {
                out.entry(mine[j]).or_default().push(decl.source);
            }
        }
    }
    out
}

/// Gives each drilldown entry the last-used fact its unit's scan found
/// for that folder (by name); an entry the scan did not reach keeps `no
/// record`, and the remainder and adjustment rows never carry one.
pub(crate) fn with_child_last_used(
    mut children: Vec<crate::drilldown::UnitChild>,
    found: &BTreeMap<String, LastUsed>,
) -> Vec<crate::drilldown::UnitChild> {
    for child in &mut children {
        if child.kind == crate::drilldown::ChildKind::Entry {
            child.last_used = found.get(&child.name).cloned().unwrap_or_default();
        }
    }
    children
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `bin` with more entries than the listing cap is a sample: its
    /// newest access time must never stand for the unit's. Which entries
    /// a capped listing keeps is the filesystem's directory order, so the
    /// test holds every file at the same time and asserts the verdict on
    /// the flag, not on which file was sampled.
    ///
    /// Tempting wrong patch this fails: using the capped `bin` listing's
    /// entries without looking at its truncation (what the scan did: the
    /// unit then reported a sampled file's time, or the real newest only
    /// where directory order happened to include it).
    #[test]
    fn a_bin_over_the_listing_cap_is_no_record_not_a_sampled_newest() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("tc/bin");
        std::fs::create_dir_all(&bin).unwrap();
        for i in 0..=crate::locations::SHALLOW_LIST_CAP {
            std::fs::write(bin.join(format!("f{i:05}")), b"").unwrap();
        }
        let scan = scan_key_file_atimes(tmp.path(), 2);
        assert!(scan.truncated, "a capped bin listing must mark the scan");
        let got = probe(
            tmp.path(),
            &[LastUseSource::KeyFileAtime { max_depth: 2 }],
            1_790_000_000,
        );
        assert_eq!(got.last_used.at, None, "{:?}", got.last_used);
    }

    #[test]
    fn a_tool_native_record_wins_over_a_newer_access_time_and_the_access_time_is_kept() {
        // The tempting wrong patch: `max(tool_native, atime)`, or
        // preferring the newer of the two, which shows today's access
        // time over the tool's own record.
        let today = 1_790_000_000;
        let sep6 = 1_788_708_000;
        let got = resolve(Some((XCODE_DERIVED_DATA, sep6)), Some(today));
        assert_eq!(got.at, Some(sep6));
        assert_eq!(
            got.source,
            LastUsedSource::ToolNative(XCODE_DERIVED_DATA.to_string())
        );
        assert_eq!(got.atime, Some(today), "the access time stays available");
    }

    #[test]
    fn an_access_time_is_used_only_when_no_tool_native_record_exists() {
        let got = resolve(None, Some(1_700_000_000));
        assert_eq!(got.source, LastUsedSource::FileAtime);
        assert_eq!(got.at, Some(1_700_000_000));
    }

    #[test]
    fn no_signal_is_no_record_never_a_date() {
        let got = resolve(None, None);
        assert_eq!(got, LastUsed::default());
        assert_eq!(got.at, None);
        assert!(got.describe(1_790_000_000).ends_with("no record"));
        // A zero timestamp is "not recorded", not 1970.
        assert_eq!(resolve(Some(("x", 0)), Some(0)), LastUsed::default());
    }

    #[test]
    fn a_date_after_now_is_set_aside_with_its_reason_never_a_fact() {
        let now = 1_790_000_000;
        // Milliseconds read as seconds, and a wrong-clock 2099.
        for bad in [1_788_000_000_000u64, 4_070_908_800] {
            let got = resolve_at(Some(("cargo-global-cache", bad)), None, now);
            assert_eq!(got.at, None);
            assert_eq!(got.why_none, Some(NoRecordWhy::FutureDate));
            assert_eq!(
                got.describe(now),
                "Last run or opened: no record (ignored: date in the future)"
            );
            // The reason survives the store.
            let back = LastUsed::from_columns(None, Some(&got.source_column()), None);
            assert_eq!(back.why_none, Some(NoRecordWhy::FutureDate));
        }
        // A valid access time is used, and a bad record does not hide it.
        let with_atime = resolve_at(Some(("x", 4_070_908_800)), Some(1_700_000_000), now);
        assert_eq!(with_atime.source, LastUsedSource::FileAtime);
        // A day of skew is tolerated.
        assert!(resolve_at(Some(("x", now + 3_600)), None, now).at.is_some());
        let limit = LastUsed::probe_limit_reached();
        assert_eq!(
            limit.describe(now),
            "Last run or opened: no record (probe limit reached)"
        );
    }

    #[test]
    fn the_row_names_the_value_and_its_source_and_never_says_unused() {
        let now = 1_790_000_000; // 2026-09-21 UTC
        let atime = resolve(None, Some(1_783_468_800)); // 2026-07-08
        assert_eq!(
            atime.describe(now),
            "Last run or opened: Jul 8 (file access time)"
        );
        let native = resolve(Some((XCODE_DERIVED_DATA, 1_788_652_800)), None); // 2026-09-06
        assert_eq!(
            native.describe(now),
            "Last run or opened: Sep 6 (Xcode DerivedData record)"
        );
        let old = resolve(None, Some(1_720_000_000)); // 2024
        assert!(old.describe(now).contains(", 2024)") || old.describe(now).contains("2024"));
        for text in [atime.describe(now), native.describe(now), old.describe(now)] {
            let lower = text.to_lowercase();
            // Built from parts: the source gate bans the verdict word as a
            // literal, and this test is what says the rows never use it.
            let verdict = ["un", "used"].concat();
            assert!(
                !lower.contains(&verdict) && !lower.contains("since"),
                "{text}"
            );
        }
    }

    #[test]
    fn source_labels_round_trip_and_an_unknown_one_is_no_record() {
        for s in [
            LastUsedSource::None,
            LastUsedSource::FileAtime,
            LastUsedSource::ToolNative("cargo-global-cache".into()),
        ] {
            assert_eq!(LastUsedSource::from_label(&s.label()), s);
        }
        assert_eq!(LastUsedSource::from_label("mtime"), LastUsedSource::None);
        assert_eq!(
            LastUsedSource::from_label("tool_native:"),
            LastUsedSource::None
        );
        // A stored value with a source that does not decode is no record.
        assert_eq!(
            LastUsed::from_columns(Some(5), Some("mtime"), None).at,
            None
        );
        assert_eq!(
            LastUsed::from_columns(None, Some("file_atime"), None).at,
            None
        );
    }

    fn cargo_home_with_tracker(rows: &[(&str, i64)]) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        let db = rusqlite::Connection::open(home.path().join(".global-cache")).unwrap();
        for table in ["registry_crate", "registry_src", "git_db", "git_checkout"] {
            db.execute(
                &format!("CREATE TABLE {table} (name TEXT NOT NULL, timestamp INTEGER NOT NULL)"),
                [],
            )
            .unwrap();
        }
        for (table, ts) in rows {
            db.execute(
                &format!("INSERT INTO {table} (name, timestamp) VALUES ('x', ?1)"),
                [ts],
            )
            .unwrap();
        }
        home
    }

    #[test]
    fn each_cargo_tracker_table_answers_for_its_own_subtree() {
        // The tempting wrong patch: one MAX over the whole database, so a
        // git checkout used today makes the crate download cache look
        // used today.
        let home = cargo_home_with_tracker(&[
            ("registry_crate", 1_000),
            ("registry_src", 2_000),
            ("git_db", 3_000),
            ("git_checkout", 4_000),
        ]);
        for (table, want) in [
            (CargoCacheTable::RegistryCrate, 1_000),
            (CargoCacheTable::RegistrySrc, 2_000),
            (CargoCacheTable::GitDb, 3_000),
            (CargoCacheTable::GitCheckout, 4_000),
        ] {
            assert_eq!(
                cargo_global_cache_newest(home.path(), table),
                CargoRead::Newest(want)
            );
        }
    }

    #[test]
    fn a_tool_native_record_older_than_the_key_file_access_time_still_wins() {
        // The tempting wrong patch: taking the newer of the two. The
        // fixture is the maintainer's: the record says Sep 6, the file
        // was read today.
        let sep6: u64 = 1_788_652_800;
        let home = cargo_home_with_tracker(&[("registry_src", sep6 as i64)]);
        let unit = home.path().join("registry/src");
        // A key file created just now: its access time is today.
        std::fs::create_dir_all(unit.join("index/pkg/bin")).unwrap();
        std::fs::write(unit.join("index/pkg/bin/tool"), b"x").unwrap();
        let found = probe(
            &unit,
            &[
                LastUseSource::KeyFileAtime { max_depth: 3 },
                LastUseSource::CargoGlobalCache {
                    table: CargoCacheTable::RegistrySrc,
                    up: 2,
                },
            ],
            crate::entities::now(),
        );
        assert_eq!(found.last_used.at, Some(sep6));
        assert_eq!(
            found.last_used.source,
            LastUsedSource::ToolNative(CARGO_GLOBAL_CACHE.to_string())
        );
        let atime = found.last_used.atime.expect("the access time is kept");
        assert!(atime > sep6, "the fixture's file was read after Sep 6");
    }

    #[test]
    fn an_absent_or_foreign_or_reshaped_tracker_is_unavailable_never_a_crash_or_a_date() {
        let table = CargoCacheTable::RegistrySrc;
        let empty = tempfile::tempdir().unwrap();
        assert!(matches!(
            cargo_global_cache_newest(empty.path(), table),
            CargoRead::Unavailable(_)
        ));
        // Not SQLite at all: refused before SQLite opens it.
        let text = tempfile::tempdir().unwrap();
        std::fs::write(text.path().join(".global-cache"), b"not a database").unwrap();
        assert!(matches!(
            cargo_global_cache_newest(text.path(), table),
            CargoRead::Unavailable(_)
        ));
        // SQLite, but a schema this reader does not know (a cargo that
        // renamed the column): a refusal, not a query error and not zero.
        let reshaped = tempfile::tempdir().unwrap();
        let db = rusqlite::Connection::open(reshaped.path().join(".global-cache")).unwrap();
        db.execute(
            "CREATE TABLE registry_src (name TEXT, last_used INTEGER)",
            [],
        )
        .unwrap();
        drop(db);
        assert!(matches!(
            cargo_global_cache_newest(reshaped.path(), table),
            CargoRead::Unavailable(_)
        ));
        // A known table with no rows has no record.
        let no_rows = cargo_home_with_tracker(&[]);
        assert_eq!(
            cargo_global_cache_newest(no_rows.path(), table),
            CargoRead::NoRows
        );
        // And none of that is a date: the unit falls to no record.
        let unit = empty.path().join("registry/src");
        std::fs::create_dir_all(&unit).unwrap();
        let found = probe(
            &unit,
            &[LastUseSource::CargoGlobalCache { table, up: 2 }],
            crate::entities::now(),
        );
        assert_eq!(found.last_used, LastUsed::default());
    }

    #[test]
    fn a_locked_tracker_is_read_without_waiting_or_locking_and_is_never_written() {
        let home = cargo_home_with_tracker(&[("registry_src", 5_000)]);
        let db_path = home.path().join(".global-cache");
        let before = std::fs::read(&db_path).unwrap();
        // Another process holds the write lock.
        let holder = rusqlite::Connection::open(&db_path).unwrap();
        holder.execute_batch("BEGIN EXCLUSIVE").unwrap();
        let started = std::time::Instant::now();
        let read = cargo_global_cache_newest(home.path(), CargoCacheTable::RegistrySrc);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "a lock is a refusal, not a wait"
        );
        // Immutable mode takes no lock, so a writer's lock is not even seen:
        // the committed value, or a refusal if the read was torn; never a wait.
        assert!(
            matches!(read, CargoRead::Unavailable(_) | CargoRead::Newest(5_000)),
            "{read:?}"
        );
        holder.execute_batch("ROLLBACK").unwrap();
        drop(holder);
        // Read-only: the file's bytes are identical and no journal or WAL
        // sidecar was left beside it.
        assert_eq!(std::fs::read(&db_path).unwrap(), before);
        assert_eq!(
            cargo_global_cache_newest(home.path(), CargoCacheTable::RegistrySrc),
            CargoRead::Newest(5_000)
        );
        assert_eq!(std::fs::read(&db_path).unwrap(), before);
        for sidecar in [
            ".global-cache-journal",
            ".global-cache-wal",
            ".global-cache-shm",
        ] {
            assert!(!home.path().join(sidecar).exists(), "{sidecar}");
        }
    }

    #[test]
    fn a_symlinked_or_oversized_tracker_is_not_opened() {
        let home = cargo_home_with_tracker(&[("registry_src", 5_000)]);
        let real = home.path().join(".global-cache");
        let link_home = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(&real, link_home.path().join(".global-cache")).unwrap();
        assert!(matches!(
            cargo_global_cache_newest(link_home.path(), CargoCacheTable::RegistrySrc),
            CargoRead::Unavailable(_)
        ));
        let big = tempfile::tempdir().unwrap();
        let f = std::fs::File::create(big.path().join(".global-cache")).unwrap();
        f.set_len(MAX_DATABASE_BYTES + 1).unwrap();
        assert!(matches!(
            cargo_global_cache_newest(big.path(), CargoCacheTable::RegistrySrc),
            CargoRead::Unavailable(_)
        ));
    }

    #[test]
    fn civil_dates_are_right_across_a_leap_day_and_a_year_boundary() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(20_453), (2025, 12, 31));
    }
}

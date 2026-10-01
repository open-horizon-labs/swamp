//! The developer-storage headline: the first thing a person sees in
//! `swamp report`, in the TUI and above the Reclaim view.
//!
//! ```text
//! Developer storage: 224.1GB across 40 projects and 77 tool locations (50.4% of used)
//! ```
//!
//! **What it counts** (documented in `docs/usage.md` and `DESIGN.md`):
//!
//! * projects: everything under the declared (or built-in default)
//!   source roots, less the standalone Cargo target directories found
//!   there, which have their own row;
//! * every catalog unit (toolchains and SDKs, caches, agent storage,
//!   container data, and the rest), each less the bytes that are mounted
//!   disk images (a view of image files stored, and counted, elsewhere);
//! * standalone Cargo target directories.
//!
//! **What it never counts:** system volumes, the measured "Everything
//! else" bucket, the remainder unit of a location (Homebrew's "other",
//! selected by [`crate::locations::Detector::remainder_of`], never by an
//! id), the unattributed residual, and estimates for what could not be
//! read.
//!
//! **A pure function of stored facts.** [`build`] lists nothing, stats
//! nothing and starts no process; the ledger comes in already read.
//! Every number here is allocated bytes at the time its source measured
//! it, and the headline says how old each source is.
//!
//! **The percent** is developer storage divided by the container's used
//! bytes from the disk ledger (never by the Data volume's: the container
//! is what `df` calls used), rounded DOWN to one decimal, so the figure is
//! never larger than the truth. With no ledger, an unreadable one, a
//! future-dated one, a zero or missing used figure, or developer storage
//! larger than used, there is no percent and the line says why.
//!
//! **Consistency.** On one store, with `walked` the project roots' walked
//! total and `standalone` the standalone Cargo targets:
//!
//! ```text
//! developer = (walked - standalone)                          projects
//!           + (reclaim totals: regenerable + held out
//!              + not regenerable + cost not established)     units and targets
//!           - reclaim totals: remainder units
//!           - mounted disk images
//! disk view accounted (declared + catalog, counted once)
//!           = developer + remainder units
//! ```
//!
//! `tests/g4b_headline.rs` pins both on a fixture whose parts are known.

use crate::external::ExternalUnit;
use crate::locations::{HeadlineGroup, Registry, StorageCategory};
use crate::render::human_bytes_pub as human;
use crate::report::{Report, UnownedReason};
use crate::volume_ledger::{Account, LedgerReading, age_text};
use serde::Serialize;
use serde_json::json;
use std::fmt::Write as _;

/// A ledger this much older than the observation beside it is said to be.
pub const LEDGER_MUCH_OLDER_SECS: u64 = 24 * 3600;
/// A ledger stamped this far ahead of now is dated in the future.
pub const FUTURE_SLACK_SECS: u64 = 300;

/// The rows of the breakdown, in the order they are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Projects,
    ToolchainsAndSdks,
    Caches,
    AgentStorage,
    ContainersAndVms,
    StandaloneCargoTargets,
    OtherDeveloperUnits,
}

impl Category {
    pub const ALL: [Category; 7] = [
        Category::Projects,
        Category::ToolchainsAndSdks,
        Category::Caches,
        Category::AgentStorage,
        Category::ContainersAndVms,
        Category::StandaloneCargoTargets,
        Category::OtherDeveloperUnits,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Projects => "projects",
            Category::ToolchainsAndSdks => "toolchains and SDKs",
            Category::Caches => "caches",
            Category::AgentStorage => "agent storage",
            Category::ContainersAndVms => "containers and VMs",
            Category::StandaloneCargoTargets => "standalone Cargo targets",
            Category::OtherDeveloperUnits => "other developer units",
        }
    }

    /// The label a narrow screen uses.
    pub fn short(self) -> &'static str {
        match self {
            Category::Projects => "projects",
            Category::ToolchainsAndSdks => "toolchains",
            Category::Caches => "caches",
            Category::AgentStorage => "agents",
            Category::ContainersAndVms => "VMs",
            Category::StandaloneCargoTargets => "cargo",
            Category::OtherDeveloperUnits => "other",
        }
    }
}

/// The breakdown row a unit belongs to. A detector's own group wins (an AI
/// tool's home is agent storage whatever its storage category); otherwise
/// the storage category decides. The match names every storage category:
/// a new one does not compile until it is placed here, and a unit whose
/// category says nothing lands in "other developer units", never nowhere.
pub fn category_of_unit(group: Option<HeadlineGroup>, category: StorageCategory) -> Category {
    match group {
        Some(HeadlineGroup::AgentStorage) => return Category::AgentStorage,
        Some(HeadlineGroup::Containers) => return Category::ContainersAndVms,
        None => {}
    }
    match category {
        StorageCategory::Installation | StorageCategory::Environments => {
            Category::ToolchainsAndSdks
        }
        StorageCategory::Downloads | StorageCategory::Cache | StorageCategory::BuildOutput => {
            Category::Caches
        }
        StorageCategory::LocalState | StorageCategory::Models | StorageCategory::Unclassified => {
            Category::OtherDeveloperUnits
        }
    }
}

/// Which scope the numbers cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    /// The configured scope: declared roots and the catalog.
    Current,
    /// The most recent observation of an earlier scope (roots were added
    /// or removed since): nothing was observed for the difference.
    Previous { roots: usize },
    /// One root named on the command line, which replaces the configured
    /// scope: not the machine's developer storage.
    ExplicitRoot,
}

/// Everything [`build`] reads.
pub struct Input<'a> {
    pub units: &'a [ExternalUnit],
    pub report: &'a Report,
    pub ledger: &'a LedgerReading,
    pub scope: ScopeKind,
    pub observed_at: u64,
    pub now: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CategoryRow {
    pub category: Category,
    pub label: &'static str,
    pub count: usize,
    pub bytes: u64,
}

/// A counted location whose storage category says it holds more than one
/// owner's files (`~/Library/Caches`): counted, and named as mixed.
#[derive(Debug, Clone, Serialize)]
pub struct MixedOwner {
    pub path: String,
    pub bytes: u64,
}

/// What the headline leaves out on purpose, with its bytes.
#[derive(Debug, Clone, Serialize)]
pub struct NotCounted {
    /// Remainder units (Homebrew's "other"): not developer tooling.
    pub remainder_bytes: u64,
    pub remainder_units: usize,
    pub remainder_names: Vec<String>,
    /// Bytes of units that are mounted disk images: counted where the
    /// image files are stored.
    pub mounted_image_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Piece {
    pub path: String,
    pub bytes: u64,
    pub exactness: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Elsewhere {
    pub bytes: u64,
    pub folders: usize,
    pub top: Vec<Piece>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemPart {
    pub bytes: u64,
    pub names: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NotMeasuredPart {
    /// Directories that could not be read (exact count).
    pub directories: usize,
    /// The first few of their names.
    pub names: Vec<String>,
    /// Locations no run of the current cycle has measured yet.
    pub not_yet_measured: usize,
    /// The unexplained part of the Data volume (its own size minus every
    /// measured part): the most the protected folders can hold. An
    /// estimate, and part of no check.
    pub estimate_bytes: Option<u64>,
}

/// The independent spot audit of the walk (the one check that can fail
/// from an undercounting walk), as the headline shows it.
#[derive(Debug, Clone, Serialize)]
pub struct AuditPart {
    /// Folders the pass audited.
    pub folders: usize,
    /// At least one folder's audit disagrees with the ledger beyond the
    /// tolerance.
    pub flag: bool,
    pub max_difference_percent: f64,
    /// Why nothing was audited, when nothing was.
    pub skipped: Option<String>,
}

/// The disk ledger's agreement with the headline, on one store.
#[derive(Debug, Clone, Serialize)]
pub struct AccountedCheck {
    /// The disk view's "accounted" bytes.
    pub disk_view_accounted: u64,
    /// The headline's developer bytes plus the remainder units.
    pub developer_plus_remainder: u64,
    /// `disk_view_accounted - developer_plus_remainder`.
    pub difference: i64,
    /// Bytes of units on another volume: the headline counts them, the
    /// ledger lists them apart and never adds them. A named part of the
    /// difference.
    pub other_volume_bytes: u64,
    /// When the ledger's accounted rows were measured, and the
    /// observation they are compared with (equal: the same observation).
    pub ledger_measured_at: u64,
    pub observed_at: u64,
    /// The two agree. They can differ when the ledger's accounted rows
    /// came from a different observation than the units read here.
    pub agrees: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Measured {
    pub measured_at: u64,
    pub container_used: Option<u64>,
    /// Developer storage as tenths of a percent of the container's used
    /// bytes, rounded down; `None` whenever no honest percent exists.
    pub percent_of_used_tenths: Option<u32>,
    /// Developer storage is more than the container's used bytes.
    pub exceeds_used: bool,
    /// The ledger is older than the observation by more than a day, by
    /// how much.
    pub older_than_observation_secs: Option<u64>,
    /// The observation is more than a day older than the ledger, by how
    /// much: developer storage from an old observation is divided by a
    /// newer disk reading.
    pub observation_older_than_ledger_secs: Option<u64>,
    pub everything_else: Elsewhere,
    pub system_volumes: SystemPart,
    pub not_measured: NotMeasuredPart,
    /// Bookkeeping only: the parts, with the estimate, add up to the
    /// container's used bytes within 1%. Arithmetic, not evidence about the
    /// walk (the estimate is a leftover); informational.
    pub bookkeeping_balanced: Option<bool>,
    /// Used bytes minus the measured parts only: what the measurements do
    /// not explain, protected folders included. Informational.
    pub unexplained_bytes: Option<i64>,
    pub audit: AuditPart,
    pub accounted_check: AccountedCheck,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Disk {
    NotMeasured,
    Unreadable { reason: String },
    Newer { unknown_rows: usize },
    FutureDated { measured_at: u64 },
    Measured(Box<Measured>),
}

#[derive(Debug, Clone, Serialize)]
pub struct Headline {
    pub developer_bytes: u64,
    pub locations: usize,
    /// Every category, in display order, zero rows included: they add up
    /// to `developer_bytes` exactly.
    pub categories: Vec<CategoryRow>,
    pub not_counted: NotCounted,
    /// Units whose own measurement is a lower bound (a coverage note).
    pub lower_bound_units: usize,
    /// Counted units the catalog could not attribute to a developer tool
    /// (category `unclassified`): part of "other developer units", named
    /// here because they hold other owners' files too.
    pub mixed_owners: Vec<MixedOwner>,
    pub scope: &'static str,
    pub previous_scope_roots: Option<usize>,
    pub observed_at: u64,
    pub disk: Disk,
    /// Data-quality flags: overflow, inconsistent inputs, developer storage
    /// larger than the disk's used bytes.
    pub flags: Vec<String>,
    /// A total did not fit in 64 bits and was capped: the rows do not add
    /// up to the headline.
    pub capped: bool,
    /// The first line, as text prints it.
    pub line: String,
}

fn add(acc: &mut u64, v: u64, overflow: &mut bool) {
    match acc.checked_add(v) {
        Some(x) => *acc = x,
        None => {
            *acc = u64::MAX;
            *overflow = true;
        }
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        one.to_string()
    } else {
        many.to_string()
    }
}

/// `57.4` from 574 tenths: integer arithmetic, no float rounding.
pub fn tenths_text(tenths: u32) -> String {
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// Developer bytes as tenths of a percent of `used`, rounded down.
/// `None` for zero used bytes. Exact for any pair of `u64`.
pub fn percent_tenths(developer: u64, used: u64) -> Option<u32> {
    if used == 0 {
        return None;
    }
    let t = (developer as u128).saturating_mul(1000) / used as u128;
    Some(t.min(u32::MAX as u128) as u32)
}

fn volume_name(path: &str) -> String {
    let rest = path.strip_prefix("APFS volume ").unwrap_or(path);
    match rest.rsplit_once(" (") {
        Some((name, dev)) if dev.ends_with(')') => name.to_string(),
        _ => rest.to_string(),
    }
}

/// Builds the headline from stored facts. Pure.
pub fn build(input: &Input<'_>) -> Headline {
    let registry = Registry::with_builtins();
    let detector_of = |id: &str| registry.detectors().iter().find(|d| d.id() == id);
    let overlaps = match input.ledger {
        LedgerReading::Measured(a) => a.mounted_image_overlaps(),
        _ => Default::default(),
    };
    let mut overflow = false;
    let mut flags: Vec<String> = Vec::new();
    let mut bytes = [0u64; 7];
    let mut counts = [0usize; 7];
    let idx = |c: Category| Category::ALL.iter().position(|x| *x == c).unwrap_or(6);
    let mut nc = NotCounted {
        remainder_bytes: 0,
        remainder_units: 0,
        remainder_names: Vec::new(),
        mounted_image_bytes: 0,
    };
    let mut lower_bound_units = 0usize;
    let mut mixed_owners: Vec<MixedOwner> = Vec::new();
    for u in input.units {
        let d = detector_of(&u.detector_id);
        if d.is_some_and(|d| d.remainder_of().is_some()) {
            add(&mut nc.remainder_bytes, u.bytes, &mut overflow);
            nc.remainder_units += 1;
            if !nc.remainder_names.contains(&u.detector_name) {
                nc.remainder_names.push(u.detector_name.clone());
            }
            continue;
        }
        let path = u.path.display().to_string();
        let mounted = overlaps
            .get(path.as_str())
            .copied()
            .unwrap_or(0)
            .min(u.bytes);
        add(&mut nc.mounted_image_bytes, mounted, &mut overflow);
        let counted = u.bytes - mounted;
        if counted == 0 {
            continue;
        }
        if u.note.is_some() {
            lower_bound_units += 1;
        }
        if u.category == StorageCategory::Unclassified {
            mixed_owners.push(MixedOwner {
                path: path.clone(),
                bytes: counted,
            });
        }
        let c = category_of_unit(d.and_then(|d| d.headline_group()), u.category);
        add(&mut bytes[idx(c)], counted, &mut overflow);
        counts[idx(c)] += 1;
    }
    // Standalone Cargo targets: their own row, and out of projects (they
    // sit inside a walked root, so the root's total already holds them).
    let mut standalone = 0u64;
    let mut standalone_n = 0usize;
    for r in &input.report.unowned {
        if r.reason == UnownedReason::StandaloneCargoTarget && r.bytes > 0 {
            add(&mut standalone, r.bytes, &mut overflow);
            standalone_n += 1;
        }
    }
    // The walked total of the source roots: the report's own figure, or,
    // when a store did not record it, what its rows add up to.
    let mut rows_sum = 0u64;
    for p in &input.report.projects {
        for w in &p.worktrees {
            for a in &w.artifacts {
                add(&mut rows_sum, a.bytes, &mut overflow);
            }
        }
    }
    for r in &input.report.unowned {
        if r.reason != UnownedReason::DockerNoJoin {
            add(&mut rows_sum, r.bytes, &mut overflow);
        }
    }
    let walked = input.report.reconciliation.walked_total.max(rows_sum);
    // `walked` is at least the rows it is made of, and the standalone
    // targets are among those rows, so this never goes below zero.
    let projects = walked.saturating_sub(standalone);
    bytes[idx(Category::Projects)] = projects;
    counts[idx(Category::Projects)] = if projects > 0 {
        input.report.projects.len().max(1)
    } else {
        0
    };
    bytes[idx(Category::StandaloneCargoTargets)] = standalone;
    counts[idx(Category::StandaloneCargoTargets)] = standalone_n;

    let mut developer = 0u64;
    let mut locations = 0usize;
    for c in Category::ALL {
        add(&mut developer, bytes[idx(c)], &mut overflow);
        locations += counts[idx(c)];
    }
    if overflow {
        flags.push(
            "a total does not fit in 64 bits, so a stored size is wrong; totals are capped"
                .to_string(),
        );
    }
    let categories: Vec<CategoryRow> = Category::ALL
        .iter()
        .map(|c| CategoryRow {
            category: *c,
            label: c.label(),
            count: counts[idx(*c)],
            bytes: bytes[idx(*c)],
        })
        .collect();

    let (scope, previous) = match input.scope {
        ScopeKind::Current => ("current", None),
        ScopeKind::Previous { roots } => ("previous", Some(roots)),
        ScopeKind::ExplicitRoot => ("explicit_root", None),
    };

    let disk = match input.ledger {
        LedgerReading::NotMeasured => Disk::NotMeasured,
        LedgerReading::Unreadable(r) => Disk::Unreadable { reason: r.clone() },
        LedgerReading::Newer { unknown_rows } => Disk::Newer {
            unknown_rows: *unknown_rows,
        },
        LedgerReading::Measured(a)
            if a.measured_at > input.now.saturating_add(FUTURE_SLACK_SECS) =>
        {
            Disk::FutureDated {
                measured_at: a.measured_at,
            }
        }
        LedgerReading::Measured(a) => Disk::Measured(Box::new(measured(
            a,
            developer,
            nc.remainder_bytes,
            input.observed_at,
            // A previous scope's developer storage divided by today's disk
            // mixes two scopes: no percent for it.
            input.scope == ScopeKind::Current,
        ))),
    };
    if let Disk::Measured(m) = &disk
        && m.exceeds_used
    {
        flags.push(format!(
            "developer storage ({}) is more than the disk's used bytes ({}); a measurement is wrong or counts shared bytes twice; no percent is shown",
            human(developer),
            human(m.container_used.unwrap_or(0))
        ));
    }

    let mut h = Headline {
        developer_bytes: developer,
        locations,
        categories,
        not_counted: nc,
        lower_bound_units,
        mixed_owners,
        scope,
        previous_scope_roots: previous,
        observed_at: input.observed_at,
        disk,
        flags,
        capped: overflow,
        line: String::new(),
    };
    h.line = h.first_line();
    h
}

fn measured(
    a: &Account,
    developer: u64,
    remainder: u64,
    observed_at: u64,
    whole_scope: bool,
) -> Measured {
    let used = a.container.used;
    let exceeds = used.is_some_and(|u| u > 0 && developer > u);
    let percent = match used {
        Some(u) if !exceeds && whole_scope => percent_tenths(developer, u),
        _ => None,
    };
    let older = (observed_at > 0
        && a.measured_at.saturating_add(LEDGER_MUCH_OLDER_SECS) < observed_at)
        .then(|| observed_at - a.measured_at);
    let observation_older = (observed_at > 0
        && observed_at.saturating_add(LEDGER_MUCH_OLDER_SECS) < a.measured_at)
        .then(|| a.measured_at - observed_at);
    let sum = developer.saturating_add(remainder);
    let diff =
        (a.accounted.bytes as i128 - sum as i128).clamp(i64::MIN as i128, i64::MAX as i128) as i64;
    Measured {
        measured_at: a.measured_at,
        container_used: used,
        percent_of_used_tenths: percent,
        exceeds_used: exceeds,
        older_than_observation_secs: older,
        observation_older_than_ledger_secs: observation_older,
        everything_else: Elsewhere {
            bytes: a.everything_else.bytes,
            folders: a.everything_else.folders,
            top: a
                .everything_else
                .top
                .iter()
                .map(|r| Piece {
                    path: r.path.clone(),
                    bytes: r.bytes.unwrap_or(0),
                    exactness: r.exactness.as_str(),
                })
                .collect(),
        },
        system_volumes: SystemPart {
            bytes: a.system_volumes.bytes,
            names: a
                .system_volumes
                .volumes
                .iter()
                .map(|r| volume_name(&r.path))
                .collect(),
        },
        not_measured: NotMeasuredPart {
            directories: a.not_measured.count,
            names: a.not_measured.names.iter().take(3).cloned().collect(),
            not_yet_measured: a.not_measured.not_yet_measured,
            estimate_bytes: a.not_measured.estimate_bytes,
        },
        bookkeeping_balanced: a.residual.bookkeeping_balanced,
        unexplained_bytes: a.residual.unexplained_bytes,
        audit: AuditPart {
            folders: a.audit.folders.len(),
            flag: a.audit.audit_flag,
            max_difference_percent: a.audit.max_difference_percent,
            skipped: a.audit.skipped.clone(),
        },
        accounted_check: AccountedCheck {
            disk_view_accounted: a.accounted.bytes,
            developer_plus_remainder: sum,
            difference: diff,
            other_volume_bytes: a
                .external_volumes
                .iter()
                .fold(0u64, |t, r| t.saturating_add(r.bytes.unwrap_or(0))),
            ledger_measured_at: a.measured_at,
            observed_at,
            agrees: diff == 0,
        },
    }
}

impl Headline {
    /// `Developer storage: 224.1GB across 40 projects and 77 tool locations (50.4% of used)`.
    fn first_line(&self) -> String {
        let projects = self
            .categories
            .iter()
            .find(|c| c.category == Category::Projects)
            .map_or(0, |c| c.count);
        let tools = self.locations.saturating_sub(projects);
        let across = match (projects, tools) {
            (0, t) => format!("{t} tool {}", plural(t, "location", "locations")),
            (p, 0) => format!("{p} {}", plural(p, "project", "projects")),
            (p, t) => format!(
                "{p} {} and {t} tool {}",
                plural(p, "project", "projects"),
                plural(t, "location", "locations")
            ),
        };
        let base = format!(
            "Developer storage: {} across {across}",
            human(self.developer_bytes)
        );
        if self.scope == "explicit_root" {
            return format!("{base} (only the root named on the command line, not the machine)");
        }
        match &self.disk {
            Disk::Measured(m) => match m.percent_of_used_tenths {
                Some(t) => format!("{base} ({}% of used)", tenths_text(t)),
                None => base,
            },
            _ => base,
        }
    }

    /// The shorter forms of the first line, longest first, for a narrow
    /// screen. The numbers never change; words drop away.
    pub fn line_tiers(&self) -> Vec<String> {
        let pct = match &self.disk {
            Disk::Measured(m) if self.scope != "explicit_root" => m
                .percent_of_used_tenths
                .map(|t| format!("{}%", tenths_text(t))),
            _ => None,
        };
        let suffix = if self.scope == "explicit_root" {
            " (one root only)"
        } else {
            ""
        };
        let mut v = vec![self.line.clone()];
        v.push(match &pct {
            Some(p) => format!(
                "Developer storage {} in {} {}, {p} of used{suffix}",
                human(self.developer_bytes),
                self.locations,
                plural(self.locations, "location", "locations")
            ),
            None => format!(
                "Developer storage {} in {} {}{suffix}",
                human(self.developer_bytes),
                self.locations,
                plural(self.locations, "location", "locations")
            ),
        });
        v.push(match &pct {
            Some(p) => format!(
                "Dev storage {} ({p} of used){suffix}",
                human(self.developer_bytes)
            ),
            None => format!("Dev storage {}{suffix}", human(self.developer_bytes)),
        });
        v.push(match &pct {
            Some(p) => format!("Dev {} ({p}){suffix}", human(self.developer_bytes)),
            None => format!("Dev {}{suffix}", human(self.developer_bytes)),
        });
        v
    }

    /// The scope sentence, when the numbers are not the current scope's.
    pub fn scope_sentence(&self) -> Option<String> {
        match (self.scope, self.previous_scope_roots) {
            ("previous", Some(n)) => Some(format!(
                "covers the previous scope ({n} {}), so no percent of used is shown; new roots are not observed yet, run swamp observe",
                plural(n, "root", "roots")
            )),
            ("explicit_root", _) => Some(
                "covers only the root named on the command line; the declared roots and the catalog were not used".to_string(),
            ),
            _ => None,
        }
    }

    /// `observed 4 min ago; disk ledger measured 3 h ago` and, when the
    /// ledger is much older than the observation, what that means.
    pub fn ages_sentence(&self, now: u64) -> String {
        let mut s = if self.observed_at > 0 {
            format!("observed {}", age_text(self.observed_at, now))
        } else {
            "observed at an unknown time".to_string()
        };
        match &self.disk {
            Disk::Measured(m) => {
                s.push_str(&format!(
                    "; disk ledger measured {}",
                    age_text(m.measured_at, now)
                ));
                if let Some(secs) = m.observation_older_than_ledger_secs {
                    s.push_str(&format!(
                        "; this observation is {} older than the ledger, so the percent divides older developer storage by a newer disk reading",
                        span_text(secs)
                    ));
                }
                if let Some(secs) = m.older_than_observation_secs {
                    s.push_str(&format!(
                        "; the ledger is {} older than this observation, so the percent divides newer developer storage by an older disk reading",
                        span_text(secs)
                    ));
                }
            }
            Disk::FutureDated { .. }
            | Disk::Unreadable { .. }
            | Disk::Newer { .. }
            | Disk::NotMeasured => {}
        }
        s
    }

    /// The line that says what the disk ledger is, when it is not a
    /// usable measurement, or what stops a percent.
    pub fn disk_state_sentence(&self) -> Option<String> {
        match &self.disk {
            Disk::NotMeasured => Some(
                "disk ledger: not measured yet; run swamp observe --volume".to_string(),
            ),
            Disk::Unreadable { reason } => Some(format!(
                "disk ledger: could not be read ({}); run swamp observe --volume to write a new one",
                first_line_of(reason)
            )),
            Disk::Newer { unknown_rows } => Some(format!(
                "disk ledger: written by a newer swamp ({unknown_rows} {} this version does not know); not used",
                plural(*unknown_rows, "row", "rows")
            )),
            Disk::FutureDated { .. } => Some(
                "disk ledger: dated in the future (the clock was set back, or the store was restored); not used; run swamp observe --volume".to_string(),
            ),
            Disk::Measured(m) => match (m.container_used, m.percent_of_used_tenths) {
                (None, _) => Some(
                    "disk ledger: the container's size is not available; no percent".to_string(),
                ),
                (Some(0), _) => Some(
                    "disk ledger: the container reports 0 bytes used; no percent".to_string(),
                ),
                // Developer storage above used is a FLAG in `flags`, once.
                (Some(_), None) => None,
                (Some(_), Some(_)) => None,
            },
        }
    }

    /// A plain line whenever the ledger's accounted part does not equal
    /// developer storage plus the remainder units. It names the parts it
    /// can compute (units on another volume, which the ledger lists apart
    /// and never adds), the unexplained rest, and the observation age only
    /// when the ledger really was measured at another time.
    pub fn accounted_sentence(&self) -> Option<String> {
        match &self.disk {
            Disk::Measured(m) if !m.accounted_check.agrees => {
                let c = &m.accounted_check;
                let signed = |d: i64| {
                    format!(
                        "{}{}",
                        if d < 0 { "-" } else { "+" },
                        human(d.unsigned_abs())
                    )
                };
                let mut why: Vec<String> = Vec::new();
                let mut rest = c.difference;
                if c.other_volume_bytes > 0 {
                    why.push(format!(
                        "{} is on another volume (the ledger lists it apart and never adds it)",
                        human(c.other_volume_bytes)
                    ));
                    rest = rest
                        .saturating_add(i64::try_from(c.other_volume_bytes).unwrap_or(i64::MAX));
                }
                why.push(format!(
                    "{} is not explained by a named part (for example worktrees outside the declared roots count under projects but not in the ledger's declared rows)",
                    signed(rest)
                ));
                if c.ledger_measured_at != c.observed_at {
                    why.push("the ledger's accounted rows were measured at a different time than this observation".to_string());
                }
                Some(format!(
                    "disk view check: the ledger's accounted bytes ({}) differ from developer storage plus the remainder units ({}) by {}: {}",
                    human(c.disk_view_accounted),
                    human(c.developer_plus_remainder),
                    signed(c.difference),
                    why.join("; "),
                ))
            }
            _ => None,
        }
    }

    /// The visible warning when the spot audit disagrees with the walk.
    pub fn audit_sentence(&self) -> Option<String> {
        match &self.disk {
            Disk::Measured(m) if m.audit.flag => {
                Some("FLAG: walk spot audit disagrees: see swamp report --view disk".to_string())
            }
            _ => None,
        }
    }

    /// `Everything else` and its largest pieces, `System volumes`,
    /// `Not measured`: the lines below the breakdown. Empty without a
    /// usable ledger.
    fn disk_lines(&self) -> Vec<String> {
        let Disk::Measured(m) = &self.disk else {
            return Vec::new();
        };
        let mut out = Vec::new();
        out.push(format!(
            "Everything else (measured, not developer storage): {} across {} {}",
            human(m.everything_else.bytes),
            m.everything_else.folders,
            plural(m.everything_else.folders, "folder", "folders")
        ));
        for p in &m.everything_else.top {
            out.push(format!(
                "  {:>10}  {}  ({})",
                human(p.bytes),
                p.path,
                p.exactness
            ));
        }
        out.push(format!(
            "System volumes: {}{} (separate volumes that share the container's free space)",
            human(m.system_volumes.bytes),
            if m.system_volumes.names.is_empty() {
                String::new()
            } else {
                format!(" ({})", m.system_volumes.names.join(", "))
            }
        ));
        let nm = &m.not_measured;
        let mut line = format!(
            "Not measured: {} {}",
            nm.directories,
            plural(nm.directories, "directory", "directories")
        );
        match nm.estimate_bytes {
            Some(est) => line.push_str(&format!(
                " (protected folders); the unexplained part of the Data volume, up to {}, may be inside them (an estimate, not part of any check)",
                human(est)
            )),
            None => line.push_str(" could not be read"),
        }
        out.push(line);
        for n in &nm.names {
            out.push(format!("  {n} (not measured)"));
        }
        if nm.not_yet_measured > 0 {
            out.push(format!(
                "  {} {} not measured yet in this pass; the next observe continues it",
                nm.not_yet_measured,
                plural(nm.not_yet_measured, "location", "locations")
            ));
        }
        match (&m.audit.skipped, m.audit.folders) {
            (Some(why), 0) => out.push(format!("Walk spot audit: not run ({why})")),
            (_, n) => out.push(format!(
                "Walk spot-audited: {n} {}, max difference {:.1}%",
                plural(n, "folder", "folders"),
                m.audit.max_difference_percent
            )),
        }
        out
    }

    /// The text `swamp report` prints first.
    pub fn render_text(&self, now: u64) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "{}", self.line);
        if let Some(s) = self.scope_sentence() {
            let _ = writeln!(out, "  {s}");
        }
        let _ = writeln!(out, "  {}", self.ages_sentence(now));
        if let Some(s) = self.disk_state_sentence() {
            let _ = writeln!(out, "  {s}");
        }
        if let Some(s) = self.audit_sentence() {
            let _ = writeln!(out, "  {s}");
        }
        if let Some(s) = self.accounted_sentence() {
            let _ = writeln!(out, "  {s}");
        }
        for f in &self.flags {
            let _ = writeln!(out, "  FLAG: {f}");
        }
        for c in self
            .categories
            .iter()
            .filter(|c| c.count > 0 || c.bytes > 0)
        {
            let unit = if c.category == Category::Projects {
                plural(c.count, "project", "projects")
            } else {
                plural(c.count, "location", "locations")
            };
            let _ = writeln!(
                out,
                "  {:<26} {:>10}  {:>4} {unit}",
                c.label,
                human(c.bytes),
                c.count
            );
        }
        if self.capped {
            let _ = writeln!(
                out,
                "  the rows cannot be added to a figure that fits in 64 bits; the total above is capped"
            );
        } else {
            let _ = writeln!(
                out,
                "  the rows add up to {} bytes, the figure above; each row is rounded on its own",
                group_digits(self.developer_bytes)
            );
        }
        if let Some(m) = self.mixed_owners.iter().max_by_key(|m| m.bytes) {
            let rest = self.mixed_owners.len() - 1;
            let _ = writeln!(
                out,
                "  other developer units include {} in {}: other, mixed owners (not only developer tools){}",
                human(m.bytes),
                m.path,
                if rest > 0 {
                    format!(
                        "; and {rest} smaller mixed {}",
                        plural(rest, "folder", "folders")
                    )
                } else {
                    String::new()
                }
            );
        }
        if self.lower_bound_units > 0 {
            let _ = writeln!(
                out,
                "  {} {} measured as a lower bound (see swamp report --view external)",
                self.lower_bound_units,
                plural(self.lower_bound_units, "unit is", "units are")
            );
        }
        if self.not_counted.remainder_bytes > 0 {
            let _ = writeln!(
                out,
                "  not counted: {} in {} (the rest of a location after its developer tooling)",
                human(self.not_counted.remainder_bytes),
                self.not_counted.remainder_names.join(", ")
            );
        }
        if self.not_counted.mounted_image_bytes > 0 {
            let _ = writeln!(
                out,
                "  not counted: {} of mounted disk images (views of image files; the disk cost is the image files themselves, listed under Everything else, not in developer storage)",
                human(self.not_counted.mounted_image_bytes)
            );
        }
        for l in self.disk_lines() {
            let _ = writeln!(out, "{l}");
        }
        out
    }

    /// The `headline` object of `report --json`: the numbers text prints.
    /// It carries the times its sources were measured, not ages: two
    /// reports of one store are identical however far apart they run
    /// (`ages_sentence` turns the times into "3 h ago" for text).
    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
        let ledger_at = match &self.disk {
            Disk::Measured(m) => Some(m.measured_at),
            Disk::FutureDated { measured_at } => Some(*measured_at),
            _ => None,
        };
        v["percent_of_used"] = match &self.disk {
            Disk::Measured(m) => m
                .percent_of_used_tenths
                .map(|t| json!(t as f64 / 10.0))
                .unwrap_or(serde_json::Value::Null),
            _ => serde_json::Value::Null,
        };
        v["measured_at"] = json!({
            "observed_at": self.observed_at,
            "ledger_measured_at": ledger_at,
        });
        v["disk_state"] = json!(self.disk_state_sentence());
        v["audit_warning"] = json!(self.audit_sentence());
        v["accounted_check_line"] = json!(self.accounted_sentence());
        v["scope_sentence"] = json!(self.scope_sentence());
        v
    }
}

fn first_line_of(s: &str) -> String {
    s.lines().next().unwrap_or("").to_string()
}

fn span_text(secs: u64) -> String {
    if secs < 48 * 3600 {
        format!("{} h", secs / 3600)
    } else {
        format!("{} d", secs / 86400)
    }
}

/// `224,105,331,712`: the exact byte count, once, at the end of the rows.
pub fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// How the Reclaim view's totals relate to the headline, on one store.
#[derive(Debug, Clone, Serialize)]
pub struct Relation {
    /// The view's own total: regenerable + held out + not regenerable +
    /// cost not established.
    pub units_total: u64,
    pub remainder_units: u64,
    pub mounted_images: u64,
    pub projects: u64,
    /// `projects + units_total - remainder_units - mounted_images`.
    pub developer_from_parts: u64,
    /// `developer_from_parts == the headline's developer bytes`.
    pub holds: bool,
}

/// The relation between the headline and a Reclaim view's totals.
pub fn relation(h: &Headline, t: &crate::reclaim::Totals) -> Relation {
    let projects = h
        .categories
        .iter()
        .find(|c| c.category == Category::Projects)
        .map_or(0, |c| c.bytes);
    let units_total = t
        .regenerable_bytes
        .saturating_add(t.held_bytes)
        .saturating_add(t.not_regenerable_bytes)
        .saturating_add(t.not_established_bytes);
    let from_parts = projects
        .saturating_add(units_total)
        .saturating_sub(t.remainder_bytes)
        .saturating_sub(h.not_counted.mounted_image_bytes);
    Relation {
        units_total,
        remainder_units: t.remainder_bytes,
        mounted_images: h.not_counted.mounted_image_bytes,
        projects,
        developer_from_parts: from_parts,
        holds: from_parts == h.developer_bytes,
    }
}

impl Relation {
    /// The sentence that says how the Reclaim totals add up to the
    /// headline.
    pub fn text(&self, developer: u64) -> String {
        let mut s = format!(
            "Developer storage {} = projects {} + this view's units {}",
            human(developer),
            human(self.projects),
            human(self.units_total)
        );
        if self.remainder_units > 0 {
            s.push_str(&format!(
                " - not developer tooling {}",
                human(self.remainder_units)
            ));
        }
        if self.mounted_images > 0 {
            s.push_str(&format!(
                " - mounted disk images {}",
                human(self.mounted_images)
            ));
        }
        if !self.holds {
            s.push_str(&format!(
                "; these do not add up: they come to {}",
                human(self.developer_from_parts)
            ));
        }
        s
    }
}

/// What `swamp report --view reclaim` prints before the view: the
/// headline, and how this view's totals add up to it.
pub fn render_reclaim_header(h: &Headline, t: &crate::reclaim::Totals, now: u64) -> String {
    format!(
        "{}  {}\n\n",
        h.render_text(now),
        relation(h, t).text(h.developer_bytes)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_is_rounded_down_in_integers() {
        // 99.96% must not read 100.0%.
        assert_eq!(percent_tenths(9996, 10_000), Some(999));
        assert_eq!(percent_tenths(1, 3), Some(333));
        assert_eq!(percent_tenths(10, 10), Some(1000));
        assert_eq!(percent_tenths(1, 0), None);
        assert_eq!(percent_tenths(u64::MAX, u64::MAX), Some(1000));
        assert_eq!(percent_tenths(u64::MAX, 1), Some(u32::MAX));
        assert_eq!(tenths_text(574), "57.4");
        assert_eq!(tenths_text(5), "0.5");
    }

    #[test]
    fn digits_are_grouped() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1000), "1,000");
        assert_eq!(group_digits(224_105_331_712), "224,105,331,712");
    }

    #[test]
    fn a_system_volume_is_named_without_its_device() {
        assert_eq!(volume_name("APFS volume Preboot (disk3s2)"), "Preboot");
        assert_eq!(volume_name("APFS volumes"), "APFS volumes");
    }
}

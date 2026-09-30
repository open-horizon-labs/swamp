//! The other volumes in the APFS container, purgeable space and local
//! snapshots (#170), from `diskutil` and `tmutil`.
//!
//! Every query is one allow-listed, read-only spawn (`fs_gate::spawn`:
//! fixed argv, killed on timeout, counted). Every answer is parsed
//! defensively: a missing program, a timeout, a permission error, a
//! non-zero exit or output that is not a property list each becomes a
//! not-measured row carrying the reason. A missing key is "not measured"
//! with a note, never a zero.

use super::{Category, Exactness, Row};
use crate::fs_gate::spawn::{self, Program};
use std::time::Duration;

/// How long one query may take before it is killed.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(20);

/// Refuse to parse an answer larger than this: a property list of a
/// dozen volumes is a few tens of kilobytes.
const MAX_PLIST_BYTES: usize = 4 * 1024 * 1024;

/// Why a query gave no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeError {
    /// The program is not on this machine.
    Missing(&'static str),
    /// Killed after the timeout.
    TimedOut(&'static str),
    /// Refused to start, exited non-zero, or otherwise failed.
    Failed(String),
    /// The platform has no such concept (no APFS): nothing to say.
    NotApplicable,
}

impl ProbeError {
    fn describe(&self) -> String {
        match self {
            ProbeError::Missing(p) => format!("`{p}` was not found on this machine"),
            ProbeError::TimedOut(p) => format!(
                "`{p}` did not answer within {} s and was stopped",
                QUERY_TIMEOUT.as_secs()
            ),
            ProbeError::Failed(why) => why.clone(),
            ProbeError::NotApplicable => "not applicable on this platform".to_string(),
        }
    }
}

/// The three queries. The real one spawns; a test answers from fixtures.
pub trait SystemProbe: Sync {
    /// `diskutil apfs list -plist`.
    fn apfs_list(&self) -> Result<String, ProbeError>;
    /// `diskutil info -plist /System/Volumes/Data`.
    fn info_data_volume(&self) -> Result<String, ProbeError>;
    /// `tmutil listlocalsnapshots /`.
    fn local_snapshots(&self) -> Result<String, ProbeError>;
}

/// The queries as spawns.
pub struct RealProbe;

fn one_line(bytes: &str) -> String {
    let line = bytes.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    line.trim().chars().take(160).collect()
}

fn run(program: Program, args: &[&str], name: &'static str) -> Result<String, ProbeError> {
    if !cfg!(target_os = "macos") {
        return Err(ProbeError::NotApplicable);
    }
    match spawn::run(program, args, QUERY_TIMEOUT) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ProbeError::Missing(name)),
        Err(e) => Err(ProbeError::Failed(format!("`{name}` did not run: {e}"))),
        Ok(out) if out.timed_out => Err(ProbeError::TimedOut(name)),
        Ok(out) if !out.success() => {
            let why = one_line(&out.stderr_lossy());
            Err(ProbeError::Failed(match out.code {
                Some(code) if why.is_empty() => format!("`{name}` exited with status {code}"),
                Some(code) => format!("`{name}` exited with status {code}: {why}"),
                None => format!("`{name}` was ended by a signal"),
            }))
        }
        Ok(out) => Ok(out.stdout_lossy()),
    }
}

impl SystemProbe for RealProbe {
    fn apfs_list(&self) -> Result<String, ProbeError> {
        run(Program::Diskutil, &["apfs", "list", "-plist"], "diskutil")
    }

    fn info_data_volume(&self) -> Result<String, ProbeError> {
        run(
            Program::Diskutil,
            &["info", "-plist", "/System/Volumes/Data"],
            "diskutil",
        )
    }

    fn local_snapshots(&self) -> Result<String, ProbeError> {
        run(Program::Tmutil, &["listlocalsnapshots", "/"], "tmutil")
    }
}

// ---- a small, defensive property-list reader ------------------------

#[derive(Debug, Clone, PartialEq)]
enum Plist {
    Dict(Vec<(String, Plist)>),
    Array(Vec<Plist>),
    Str(String),
    Int(i64),
    Other,
}

impl Plist {
    fn get(&self, key: &str) -> Option<&Plist> {
        match self {
            Plist::Dict(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_u64(&self) -> Option<u64> {
        match self {
            Plist::Int(n) => u64::try_from(*n).ok(),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Plist::Str(s) => Some(s),
            _ => None,
        }
    }
}

fn read_node(node: roxmltree::Node, depth: usize) -> Plist {
    if depth > 24 {
        return Plist::Other;
    }
    match node.tag_name().name() {
        "dict" => {
            let mut entries = Vec::new();
            let mut key: Option<String> = None;
            for child in node.children().filter(|c| c.is_element()) {
                if child.tag_name().name() == "key" {
                    key = Some(child.text().unwrap_or("").to_string());
                } else if let Some(k) = key.take() {
                    entries.push((k, read_node(child, depth + 1)));
                }
            }
            Plist::Dict(entries)
        }
        "array" => Plist::Array(
            node.children()
                .filter(|c| c.is_element())
                .map(|c| read_node(c, depth + 1))
                .collect(),
        ),
        "string" => Plist::Str(node.text().unwrap_or("").to_string()),
        "integer" => node
            .text()
            .and_then(|t| t.trim().parse::<i64>().ok())
            .map(Plist::Int)
            .unwrap_or(Plist::Other),
        _ => Plist::Other,
    }
}

fn parse_plist(xml: &str) -> Result<Plist, String> {
    if xml.len() > MAX_PLIST_BYTES {
        return Err("the answer is larger than a property list of volumes can be".to_string());
    }
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let doc = roxmltree::Document::parse_with_options(xml, options)
        .map_err(|_| "the answer is not a readable property list".to_string())?;
    let root = doc.root_element();
    if root.tag_name().name() != "plist" {
        return Err("the answer is not a property list".to_string());
    }
    let top = root
        .children()
        .find(|c| c.is_element())
        .ok_or_else(|| "the property list is empty".to_string())?;
    Ok(read_node(top, 0))
}

// ---- what the answers say -------------------------------------------

/// One volume of an APFS container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApfsVolume {
    pub name: String,
    pub roles: Vec<String>,
    pub device: String,
    pub in_use: Option<u64>,
}

/// The APFS container that holds the Data volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApfsContainer {
    pub reference: String,
    pub volumes: Vec<ApfsVolume>,
}

/// The container holding a `Data` volume from `diskutil apfs list
/// -plist`. Containers made of mounted disk images (simulator runtimes)
/// have no Data volume and are never chosen.
pub fn parse_apfs_list(xml: &str, prefer: Option<&str>) -> Result<ApfsContainer, String> {
    let plist = parse_plist(xml)?;
    let Some(Plist::Array(containers)) = plist.get("Containers") else {
        return Err("no Containers list in the answer".to_string());
    };
    let mut found: Vec<ApfsContainer> = Vec::new();
    for c in containers {
        let Some(reference) = c.get("ContainerReference").and_then(Plist::as_str) else {
            continue;
        };
        let Some(Plist::Array(vols)) = c.get("Volumes") else {
            continue;
        };
        let volumes: Vec<ApfsVolume> = vols
            .iter()
            .map(|v| ApfsVolume {
                name: v
                    .get("Name")
                    .and_then(Plist::as_str)
                    .unwrap_or("(unnamed)")
                    .to_string(),
                roles: match v.get("Roles") {
                    Some(Plist::Array(r)) => r
                        .iter()
                        .filter_map(|r| r.as_str().map(str::to_string))
                        .collect(),
                    _ => Vec::new(),
                },
                device: v
                    .get("DeviceIdentifier")
                    .and_then(Plist::as_str)
                    .unwrap_or("")
                    .to_string(),
                in_use: v.get("CapacityInUse").and_then(Plist::as_u64),
            })
            .collect();
        if volumes.iter().any(|v| v.roles.iter().any(|r| r == "Data")) {
            found.push(ApfsContainer {
                reference: reference.to_string(),
                volumes,
            });
        }
    }
    if let Some(p) = prefer
        && let Some(i) = found.iter().position(|c| c.reference == p)
    {
        return Ok(found.swap_remove(i));
    }
    found
        .into_iter()
        .next()
        .ok_or_else(|| "no APFS container with a Data volume in the answer".to_string())
}

/// What `diskutil info -plist` says about the Data volume.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DataVolumeInfo {
    pub container_reference: Option<String>,
    /// The volume's own device (`disk3s5`): how this machine's Data volume
    /// is told from another Data-role volume in the same container.
    pub device: Option<String>,
    pub in_use: Option<u64>,
    /// The first integer whose key names purgeable space
    /// (`...Purgeable...`); `None` when the answer has no such key.
    pub purgeable: Option<u64>,
}

fn find_purgeable(p: &Plist, depth: usize) -> Option<u64> {
    if depth > 6 {
        return None;
    }
    if let Plist::Dict(entries) = p {
        for (k, v) in entries {
            if k.to_ascii_lowercase().contains("purgeable")
                && let Some(n) = v.as_u64()
            {
                return Some(n);
            }
        }
        for (_, v) in entries {
            if let Some(n) = find_purgeable(v, depth + 1) {
                return Some(n);
            }
        }
    }
    None
}

pub fn parse_data_volume_info(xml: &str) -> Result<DataVolumeInfo, String> {
    let plist = parse_plist(xml)?;
    if !matches!(plist, Plist::Dict(_)) {
        return Err("the answer is not a dictionary".to_string());
    }
    Ok(DataVolumeInfo {
        container_reference: plist
            .get("APFSContainerReference")
            .and_then(Plist::as_str)
            .map(str::to_string),
        device: plist
            .get("DeviceIdentifier")
            .and_then(Plist::as_str)
            .map(str::to_string),
        in_use: plist.get("CapacityInUse").and_then(Plist::as_u64),
        purgeable: find_purgeable(&plist, 0),
    })
}

/// Snapshot names from `tmutil listlocalsnapshots /`: one token per line
/// (a header line ends in a colon and has spaces); anything else is not
/// taken for a snapshot.
pub fn parse_snapshots(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| {
            !l.is_empty()
                && l.contains('.')
                && l.len() <= 256
                && l.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        })
        .map(str::to_string)
        .collect()
}

/// What the three queries produced.
#[derive(Debug, Clone, Default)]
pub struct SystemFacts {
    pub rows: Vec<Row>,
    /// The Data volume's consumed bytes, when `diskutil` said.
    pub data_volume_used: Option<u64>,
}

fn not_measured(path: &str, category: Category, method: &str, now: u64, why: String) -> Row {
    Row {
        path: path.to_string(),
        category,
        bytes: None,
        overlap_bytes: 0,
        entries: None,
        unreadable: 0,
        measured_at: now,
        method: method.to_string(),
        exactness: Exactness::NotMeasured,
        note: Some(why),
    }
}

const M_APFS: &str = "diskutil apfs list -plist";
const M_INFO: &str = "diskutil info -plist /System/Volumes/Data";
const M_SNAP: &str = "tmutil listlocalsnapshots /";

/// Asks the three questions and turns each answer, or each failure, into
/// rows. On a platform without APFS nothing is asked and nothing is said.
pub fn collect(probe: &dyn SystemProbe, now: u64) -> SystemFacts {
    let mut facts = SystemFacts::default();

    let info = probe.info_data_volume();
    let info_parsed = match &info {
        Ok(xml) => Some(parse_data_volume_info(xml)),
        Err(_) => None,
    };
    let prefer = match &info_parsed {
        Some(Ok(i)) => i.container_reference.clone(),
        _ => None,
    };

    match probe.apfs_list() {
        Ok(xml) => match parse_apfs_list(&xml, prefer.as_deref()) {
            Ok(container) => {
                // This machine's Data volume is the one `diskutil info` says
                // is mounted at /System/Volumes/Data; without that answer, a
                // container with exactly one Data-role volume has it.
                let own_device: Option<String> = info_parsed
                    .as_ref()
                    .and_then(|r| r.as_ref().ok())
                    .and_then(|i| i.device.clone());
                let data_roles = container
                    .volumes
                    .iter()
                    .filter(|v| v.roles.iter().any(|r| r == "Data"))
                    .count();
                let is_own = |v: &ApfsVolume| {
                    v.roles.iter().any(|r| r == "Data")
                        && match &own_device {
                            Some(d) => v.device == *d,
                            None => data_roles == 1,
                        }
                };
                if data_roles > 1 && own_device.is_none() {
                    facts.rows.push(not_measured(
                        "APFS Data volume",
                        Category::System,
                        M_APFS,
                        now,
                        "this container has more than one Data volume and diskutil did not say which is mounted here".to_string(),
                    ));
                }
                for v in &container.volumes {
                    if is_own(v) {
                        facts.data_volume_used = v.in_use;
                        continue;
                    }
                    let role = if v.roles.is_empty() {
                        "no role".to_string()
                    } else {
                        v.roles.join(", ")
                    };
                    let path = format!("APFS volume {} ({})", v.name, v.device);
                    facts.rows.push(match v.in_use {
                        Some(n) => Row {
                            path,
                            category: Category::System,
                            bytes: Some(n),
                            overlap_bytes: 0,
                            entries: None,
                            unreadable: 0,
                            measured_at: now,
                            method: M_APFS.to_string(),
                            exactness: Exactness::Exact,
                            note: Some(format!(
                                "role: {role}; a separate volume sharing the container's free space"
                            )),
                        },
                        None => not_measured(
                            &path,
                            Category::System,
                            M_APFS,
                            now,
                            "diskutil gave no size for this volume".to_string(),
                        ),
                    });
                }
                if facts.data_volume_used.is_none() {
                    facts.data_volume_used = info_parsed
                        .as_ref()
                        .and_then(|r| r.as_ref().ok())
                        .and_then(|i| i.in_use);
                }
            }
            Err(why) => facts.rows.push(not_measured(
                "APFS volumes",
                Category::System,
                M_APFS,
                now,
                why,
            )),
        },
        Err(ProbeError::NotApplicable) => return facts,
        Err(e) => facts.rows.push(not_measured(
            "APFS volumes",
            Category::System,
            M_APFS,
            now,
            e.describe(),
        )),
    }

    // Purgeable space comes from the Data volume's own answer.
    facts.rows.push(match (&info, &info_parsed) {
        (Ok(_), Some(Ok(i))) => match i.purgeable {
            Some(n) => Row {
                path: "purgeable space".to_string(),
                category: Category::Purgeable,
                bytes: Some(n),
                overlap_bytes: 0,
                entries: None,
                unreadable: 0,
                measured_at: now,
                method: M_INFO.to_string(),
                exactness: Exactness::Exact,
                note: Some(
                    "already inside the folders above; macOS can give it back when it needs room"
                        .to_string(),
                ),
            },
            None => not_measured(
                "purgeable space",
                Category::Purgeable,
                M_INFO,
                now,
                "diskutil did not report purgeable space".to_string(),
            ),
        },
        (Ok(_), Some(Err(why))) => not_measured(
            "purgeable space",
            Category::Purgeable,
            M_INFO,
            now,
            why.clone(),
        ),
        (Err(e), _) => not_measured(
            "purgeable space",
            Category::Purgeable,
            M_INFO,
            now,
            e.describe(),
        ),
        (Ok(_), None) => not_measured(
            "purgeable space",
            Category::Purgeable,
            M_INFO,
            now,
            "diskutil gave no answer".to_string(),
        ),
    });

    facts.rows.push(match probe.local_snapshots() {
        Ok(text) => {
            let names = parse_snapshots(&text);
            // Nothing, or only tmutil's own header, is "none listed". Any
            // other output that yields no snapshot name is not understood,
            // and an answer that is not understood is not a zero.
            let says_none = text.trim().is_empty()
                || text
                    .lines()
                    .all(|l| l.trim().is_empty() || l.trim_start().starts_with("Snapshots for"));
            if names.is_empty() && !says_none {
                not_measured(
                    "local snapshots",
                    Category::Snapshot,
                    M_SNAP,
                    now,
                    "`tmutil` answered with something that is not a snapshot list".to_string(),
                )
            } else if names.is_empty() {
                Row {
                    path: "local snapshots".to_string(),
                    category: Category::Snapshot,
                    bytes: Some(0),
                    overlap_bytes: 0,
                    entries: Some(0),
                    unreadable: 0,
                    measured_at: now,
                    method: M_SNAP.to_string(),
                    exactness: Exactness::Exact,
                    note: Some("none listed".to_string()),
                }
            } else {
                let shown: Vec<&str> = names.iter().take(5).map(String::as_str).collect();
                Row {
                    path: "local snapshots".to_string(),
                    category: Category::Snapshot,
                    bytes: None,
                    overlap_bytes: 0,
                    entries: Some(names.len() as u64),
                    unreadable: 0,
                    measured_at: now,
                    method: M_SNAP.to_string(),
                    exactness: Exactness::NotMeasured,
                    note: Some(format!(
                        "{} listed, tmutil reports no sizes: {}{}",
                        names.len(),
                        shown.join(", "),
                        if names.len() > shown.len() {
                            ", ..."
                        } else {
                            ""
                        }
                    )),
                }
            }
        }
        Err(ProbeError::NotApplicable) => return facts,
        Err(e) => not_measured(
            "local snapshots",
            Category::Snapshot,
            M_SNAP,
            now,
            e.describe(),
        ),
    });
    facts
}

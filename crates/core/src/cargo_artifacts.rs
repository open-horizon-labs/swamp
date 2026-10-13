//! Read-only Cargo build-layout inspection.
//!
//! Cargo's build directory is intentionally an implementation detail.  This
//! adapter only makes claims that are supported by path layout, companion
//! metadata, and optionally existing `--message-format=json` records.  It
//! never invokes Cargo, reads a build script, or executes project code.

use crate::artifact::{
    ArtifactCoverage, ArtifactEvidence, ArtifactRole, ArtifactVariant, Membership, NestedArtifact,
    relative_path,
};
use crate::entities::Confidence;
use crate::fs_gate::MetadataExt;
use crate::report::{ArtifactKind, ProjectRow};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CargoInspection {
    pub target_dir: PathBuf,
    pub build_dir: Option<PathBuf>,
    pub total_bytes: u64,
    pub physical_bytes: u64,
    pub units: Vec<NestedArtifact>,
    pub coverage: ArtifactCoverage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CargoMessageEvidence {
    #[serde(default)]
    pub profile_test: bool,
    #[serde(default)]
    pub features: Option<Vec<String>>,
    pub target_name: Option<String>,
    pub target_kind: Vec<String>,
    pub package_id: Option<String>,
    pub filenames: Vec<PathBuf>,
}

/// Hard bounds for one explicit Cargo-profile inspection. Values supplied by
/// callers are clamped to these ceilings so an interactive caller cannot
/// accidentally turn an inspection into an unbounded walk.
#[derive(Debug, Clone)]
pub struct CargoProfileInspectionLimits {
    pub max_entries: usize,
    pub max_duration: Duration,
    pub max_metadata_bytes: usize,
}

impl Default for CargoProfileInspectionLimits {
    fn default() -> Self {
        Self {
            max_entries: 262_144,
            max_duration: Duration::from_secs(5),
            max_metadata_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CargoProfileInspection {
    pub profile_path: PathBuf,
    pub inspected_at: u64,
    pub elapsed_ms: u64,
    pub entries_examined: usize,
    pub groups: Vec<CargoDependencyGroup>,
    /// Sum of allocated bytes for every observed dependency-directory entry;
    /// hardlinks may be counted more than once.
    pub allocated_bytes: u64,
    /// Allocated bytes after de-duplicating inode identities across this
    /// profile's dependency entries.
    pub unique_allocated_bytes: u64,
    pub metadata_bytes_read: u64,
    pub coverage: ArtifactCoverage,
    pub accounting_note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CargoDependencyGroup {
    /// `None` for explicit unknown/residual groups. This name comes from a
    /// target-named fingerprint file, never from the fingerprint hash.
    pub target: Option<String>,
    pub target_kind: Option<String>,
    /// Cargo package identity is only set when metadata states it directly.
    pub package_id: Option<String>,
    pub variant: ArtifactVariant,
    pub allocated_bytes: u64,
    /// Inode-deduplicated within this group. A hardlink shared by groups may
    /// therefore occur in each group's value; use the profile total above for
    /// the cross-group unique total.
    pub unique_allocated_bytes: u64,
    pub logical_bytes: u64,
    pub entries: usize,
    pub hardlinked_entries: usize,
    pub fingerprint_paths: Vec<PathBuf>,
    pub residual_reason: Option<String>,
}

const MAX_PROFILE_INSPECTION_ENTRIES: usize = 262_144;
const MAX_PROFILE_INSPECTION_TIME: Duration = Duration::from_secs(30);
const MAX_PROFILE_METADATA_BYTES: usize = 64 * 1024 * 1024;
type ProfileFingerprintEvidence = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    PathBuf,
);

fn parse_fingerprint_target(stem: &str) -> Option<(&'static str, &str)> {
    [
        ("test-integration-test-", "test-integration-test"),
        ("integration-test-", "integration-test"),
        ("test-example-", "test-example"),
        ("test-proc-macro-", "test-proc-macro"),
        ("test-bin-", "test-bin"),
        ("test-lib-", "test-lib"),
        ("proc-macro-", "proc-macro"),
        ("example-", "example"),
        ("bin-", "bin"),
        ("lib-", "lib"),
    ]
    .into_iter()
    .find_map(|(prefix, kind)| stem.strip_prefix(prefix).map(|target| (kind, target)))
}

/// Inspect one already-selected Cargo profile without invoking Cargo or
/// consulting project configuration. Fingerprint JSON is attribution evidence
/// only; files that cannot be matched unambiguously remain residuals.
pub fn inspect_profile(
    profile_path: &Path,
    limits: CargoProfileInspectionLimits,
    cancelled: &AtomicBool,
) -> CargoProfileInspection {
    let started = Instant::now();
    let max_entries = limits.max_entries.clamp(1, MAX_PROFILE_INSPECTION_ENTRIES);
    let max_time = limits
        .max_duration
        .clamp(Duration::from_millis(1), MAX_PROFILE_INSPECTION_TIME);
    let max_metadata = limits
        .max_metadata_bytes
        .clamp(1, MAX_PROFILE_METADATA_BYTES);
    let mut coverage = ArtifactCoverage {
        supported: false,
        complete: true,
        limits: Vec::new(),
    };
    let mut groups = Vec::new();
    let mut entries_examined = 0;
    let mut metadata_bytes_read = 0u64;
    let mut unique = HashSet::new();
    let mut allocated_bytes = 0u64;
    let mut unique_allocated_bytes = 0u64;
    let add_limit = |coverage: &mut ArtifactCoverage, reason: String| {
        coverage.complete = false;
        if !coverage.limits.contains(&reason) {
            coverage.limits.push(reason);
        }
    };
    let check_budget = |coverage: &mut ArtifactCoverage, entries: usize, bytes: u64| -> bool {
        if cancelled.load(Ordering::Relaxed) {
            add_limit(coverage, "inspection cancelled; results are partial".into());
            return false;
        }
        if entries >= max_entries {
            add_limit(coverage, format!("entry limit reached ({max_entries})"));
            return false;
        }
        if started.elapsed() >= max_time {
            add_limit(
                coverage,
                format!("time limit reached ({} ms)", max_time.as_millis()),
            );
            return false;
        }
        if bytes >= max_metadata as u64 {
            add_limit(
                coverage,
                format!("fingerprint metadata byte limit reached ({max_metadata})"),
            );
            return false;
        }
        true
    };

    let deps = profile_path.join("deps");
    let fingerprints = profile_path.join(".fingerprint");
    let Ok(profile_meta) = crate::fs_gate::symlink_metadata(profile_path) else {
        add_limit(&mut coverage, "selected profile is unavailable".into());
        return profile_inspection_result(
            profile_path,
            started,
            entries_examined,
            groups,
            allocated_bytes,
            unique_allocated_bytes,
            metadata_bytes_read,
            coverage,
        );
    };
    if !profile_meta.is_dir() {
        add_limit(&mut coverage, "selected profile is not a directory".into());
        return profile_inspection_result(
            profile_path,
            started,
            entries_examined,
            groups,
            allocated_bytes,
            unique_allocated_bytes,
            metadata_bytes_read,
            coverage,
        );
    }
    coverage.supported = true;
    // Inspection is a non-atomic observation, not a cleanup authorization.
    // Do not mistake Cargo's persistent lock file for evidence of active use.
    let mut evidence: HashMap<String, Vec<ProfileFingerprintEvidence>> = HashMap::new();
    // Reserve room for dependency entries: a large fingerprint tree must not
    // consume the entire scan budget before any storage rows can be returned.
    let fingerprint_entry_limit = max_entries / 2;
    let fingerprint_dir_safe = match crate::fs_gate::symlink_metadata(&fingerprints) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => true,
        Ok(_) => {
            add_limit(
                &mut coverage,
                "profile .fingerprint is not a real directory; not followed".into(),
            );
            false
        }
        Err(_) => {
            add_limit(
                &mut coverage,
                "profile .fingerprint directory unavailable".into(),
            );
            false
        }
    };
    if fingerprint_dir_safe && let Ok(dirs) = crate::fs_gate::read_dir(&fingerprints) {
        'fingerprints: for dir in dirs {
            if entries_examined >= fingerprint_entry_limit {
                add_limit(
                    &mut coverage,
                    "fingerprint entry budget reached; remaining budget reserved for deps".into(),
                );
                break;
            }
            if !check_budget(&mut coverage, entries_examined, metadata_bytes_read) {
                break;
            }
            let Ok(dir) = dir else {
                add_limit(
                    &mut coverage,
                    "fingerprint directory entry could not be read".into(),
                );
                continue;
            };
            entries_examined += 1;
            let dir_path = dir.path();
            let Ok(dir_meta) = crate::fs_gate::symlink_metadata(&dir_path) else {
                add_limit(
                    &mut coverage,
                    "fingerprint entry became unavailable during inspection".into(),
                );
                continue;
            };
            if dir_meta.file_type().is_symlink() {
                add_limit(
                    &mut coverage,
                    "symlink in fingerprint directory was not followed".into(),
                );
                continue;
            }
            if !dir_meta.is_dir() {
                continue;
            }
            let Ok(files) = crate::fs_gate::read_dir(&dir_path) else {
                add_limit(
                    &mut coverage,
                    "fingerprint subdirectory could not be listed".into(),
                );
                continue;
            };
            for file in files {
                if entries_examined >= fingerprint_entry_limit {
                    add_limit(
                        &mut coverage,
                        "fingerprint entry budget reached; remaining budget reserved for deps"
                            .into(),
                    );
                    break 'fingerprints;
                }
                if !check_budget(&mut coverage, entries_examined, metadata_bytes_read) {
                    break 'fingerprints;
                }
                let Ok(file) = file else {
                    add_limit(
                        &mut coverage,
                        "fingerprint file entry could not be read".into(),
                    );
                    continue;
                };
                entries_examined += 1;
                let path = file.path();
                let name = file.file_name().to_string_lossy().into_owned();
                if !name.ends_with(".json") {
                    continue;
                }
                let Ok(meta) = crate::fs_gate::symlink_metadata(&path) else {
                    continue;
                };
                if meta.file_type().is_symlink() {
                    add_limit(
                        &mut coverage,
                        "symlink fingerprint metadata was not followed".into(),
                    );
                    continue;
                }
                if !meta.is_file() {
                    continue;
                }
                if metadata_bytes_read + meta.len() > max_metadata as u64
                    || meta.len() > crate::fs_gate::read::BoundedCap::BUILD_MANIFEST.bytes() as u64
                {
                    add_limit(
                        &mut coverage,
                        "fingerprint metadata byte/file-size limit reached; remaining records omitted".into(),
                    );
                    break 'fingerprints;
                }
                let Ok(text) = crate::fs_gate::read::bounded_string(
                    &path,
                    crate::fs_gate::read::BoundedCap::BUILD_MANIFEST,
                ) else {
                    continue;
                };
                metadata_bytes_read += text.len() as u64;
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                    continue;
                };
                let stem = name.trim_end_matches(".json");
                let Some((kind, target)) = parse_fingerprint_target(stem) else {
                    continue;
                };
                let target = target.to_string();
                let kind = kind.to_string();
                let features = value.get("features").and_then(|v| {
                    v.as_str().map(str::to_owned).or_else(|| {
                        v.as_array().map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str())
                                .collect::<Vec<_>>()
                                .join(",")
                        })
                    })
                });
                let package_id = value
                    .get("package_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned);
                let toolchain = value
                    .get("rustc")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned);
                let dirname = dir.file_name().to_string_lossy().into_owned();
                let hash = dirname
                    .rsplit_once('-')
                    .map(|(_, hash)| hash)
                    .unwrap_or(&dirname)
                    .to_string();
                evidence
                    .entry(hash)
                    .or_default()
                    .push((target, kind, features, package_id, toolchain, path));
            }
        }
    } else if fingerprint_dir_safe {
        add_limit(
            &mut coverage,
            "fingerprint metadata unavailable; dependency files remain residual".into(),
        );
    }
    let mut group_index: HashMap<String, usize> = HashMap::new();
    let mut group_unique: HashMap<usize, HashSet<(u64, u64)>> = HashMap::new();
    let deps_dir_safe = match crate::fs_gate::symlink_metadata(&deps) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => true,
        Ok(_) => {
            add_limit(
                &mut coverage,
                "profile deps is not a real directory; not followed".into(),
            );
            false
        }
        Err(_) => {
            add_limit(&mut coverage, "profile deps directory unavailable".into());
            false
        }
    };
    match if deps_dir_safe {
        Some(crate::fs_gate::read_dir(&deps))
    } else {
        None
    } {
        Some(Ok(files)) => {
            for file in files {
                if !check_budget(&mut coverage, entries_examined, metadata_bytes_read) {
                    break;
                }
                let Ok(file) = file else {
                    add_limit(
                        &mut coverage,
                        "dependency file entry could not be read".into(),
                    );
                    continue;
                };
                entries_examined += 1;
                let path = file.path();
                let Ok(meta) = crate::fs_gate::symlink_metadata(&path) else {
                    add_limit(
                        &mut coverage,
                        "dependency entry became unavailable during inspection".into(),
                    );
                    continue;
                };
                if meta.file_type().is_symlink() {
                    add_limit(
                        &mut coverage,
                        "symlink in dependency directory was not followed".into(),
                    );
                    continue;
                }
                if !meta.is_file() {
                    continue;
                }
                let filename = file.file_name().to_string_lossy().into_owned();
                let hash = filename
                    .rsplit_once('-')
                    .map(|(_, h)| h.split('.').next().unwrap_or(""));
                let artifact_stem = filename
                    .strip_prefix("lib")
                    .unwrap_or(&filename)
                    .split_once('-')
                    .map(|(name, _)| name)
                    .unwrap_or("");
                let match_data = hash.and_then(|h| evidence.get(h)).and_then(|records| {
                    let matching: Vec<_> = records
                        .iter()
                        .filter(|(target, _, _, _, _, _)| target == artifact_stem)
                        .collect();
                    (matching.len() == 1).then(|| matching[0])
                });
                let key =
                    if let Some((target, kind, features, package_id, toolchain, _)) = match_data {
                        format!("target:{target}:{kind}:{features:?}:{package_id:?}:{toolchain:?}")
                    } else {
                        "residual:missing-or-ambiguous-fingerprint".into()
                    };
                let idx = *group_index.entry(key.clone()).or_insert_with(|| {
                    let mut variant = ArtifactVariant {
                        profile: profile_path
                            .file_name()
                            .map(|v| v.to_string_lossy().into_owned()),
                        ..Default::default()
                    };
                    variant.unknowns.push("target triple/architecture".into());
                    let (target, target_kind, package_id, residual_reason) =
                        if let Some((target, kind, features, id, toolchain, _)) = match_data {
                            variant.target = Some(target.clone());
                            variant.features = features.clone();
                            variant.toolchain = toolchain.clone();
                            (Some(target.clone()), Some(kind.clone()), id.clone(), None)
                        } else {
                            variant.unknowns.push("target and package identity".into());
                            (
                                None,
                                None,
                                None,
                                Some(
                                    "no unique target-named fingerprint matched this artifact"
                                        .into(),
                                ),
                            )
                        };
                    groups.push(CargoDependencyGroup {
                        target,
                        target_kind,
                        package_id,
                        variant,
                        allocated_bytes: 0,
                        unique_allocated_bytes: 0,
                        logical_bytes: 0,
                        entries: 0,
                        hardlinked_entries: 0,
                        fingerprint_paths: Vec::new(),
                        residual_reason,
                    });
                    groups.len() - 1
                });
                if let Some((_, _, _, _, _, fingerprint_path)) = match_data
                    && !groups[idx].fingerprint_paths.contains(fingerprint_path)
                {
                    groups[idx].fingerprint_paths.push(fingerprint_path.clone());
                }
                let bytes = meta.blocks().saturating_mul(512);
                allocated_bytes = allocated_bytes.saturating_add(bytes);
                unique_allocated_bytes = unique_allocated_bytes.saturating_add(
                    if unique.insert((meta.dev(), meta.ino())) {
                        bytes
                    } else {
                        0
                    },
                );
                let g = &mut groups[idx];
                g.allocated_bytes = g.allocated_bytes.saturating_add(bytes);
                g.logical_bytes = g.logical_bytes.saturating_add(meta.len());
                g.entries += 1;
                if meta.nlink() > 1 {
                    g.hardlinked_entries += 1;
                }
                if group_unique
                    .entry(idx)
                    .or_default()
                    .insert((meta.dev(), meta.ino()))
                {
                    g.unique_allocated_bytes = g.unique_allocated_bytes.saturating_add(bytes);
                }
            }
        }
        Some(Err(_)) | None => add_limit(
            &mut coverage,
            "selected profile deps directory is unavailable".into(),
        ),
    }
    profile_inspection_result(
        profile_path,
        started,
        entries_examined,
        groups,
        allocated_bytes,
        unique_allocated_bytes,
        metadata_bytes_read,
        coverage,
    )
}

#[allow(clippy::too_many_arguments)]
fn profile_inspection_result(
    path: &Path,
    started: Instant,
    entries_examined: usize,
    groups: Vec<CargoDependencyGroup>,
    allocated_bytes: u64,
    unique_allocated_bytes: u64,
    metadata_bytes_read: u64,
    coverage: ArtifactCoverage,
) -> CargoProfileInspection {
    CargoProfileInspection { profile_path: path.to_path_buf(), inspected_at: crate::entities::now(), elapsed_ms: started.elapsed().as_millis() as u64, entries_examined, groups, allocated_bytes, unique_allocated_bytes, metadata_bytes_read, coverage, accounting_note: "This observation is not atomic; concurrent builds may change files. Allocated bytes count each dependency entry; profile unique bytes deduplicate filesystem inode identity. These are storage observations, not reclaimable bytes or proof of final-binary contribution.".into() }
}

/// The effective local Cargo directories that can be established without
/// running Cargo. Environment variables are process facts; config values are
/// read-only local metadata. Command-line overrides are necessarily unknown
/// to an observer and are called out in coverage.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CargoLayout {
    pub target_dir: Option<PathBuf>,
    pub build_dir: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
    pub notes: Vec<String>,
}

#[derive(Debug)]
struct ParsedCargoBuildPaths {
    /// `None`: absent; `Some(None)`: present but unresolved; `Some(Some(_))`:
    /// an observed literal path.
    target_dir: Option<Option<PathBuf>>,
    build_dir: Option<BuildDirValue>,
}

/// `build.build-dir` is the one Cargo path setting that supports templates
/// (Cargo Book, `build.build-dir`); `target-dir` does not, so a brace there
/// stays unresolved.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BuildDirValue {
    Literal(PathBuf),
    Template(String),
}

use serde::{Deserialize, Serialize};

pub fn layout_for(worktree: &Path) -> CargoLayout {
    layout_with_env(worktree, |k| std::env::var_os(k))
}

/// [`layout_for`] with the environment given (a test's is empty, so a
/// `CARGO_TARGET_DIR` in the shell running the tests cannot change it).
fn layout_with_env(
    worktree: &Path,
    env: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> CargoLayout {
    let mut layout = CargoLayout::default();
    let cargo_home = cargo_home(&env);
    let configs = config_files(worktree, cargo_home.as_deref());
    let config = configs.last().cloned();
    layout.config_path = config.clone();

    // Cargo merges per-directory config from the filesystem root toward the
    // project, with `$CARGO_HOME/config.toml` below all of them. A deeper
    // file overrides only keys it declares.
    for path in &configs {
        let values =
            crate::fs_gate::read::bounded_string(path, crate::fs_gate::read::BoundedCap::MANIFEST)
                .ok()
                .map(|text| parse_build_paths(&text));
        match values {
            Some(Some(parsed)) => {
                if let Some(target) = parsed.target_dir {
                    layout.target_dir = target.map(|value| resolve_config_path(Some(path), value));
                }
                if let Some(build) = parsed.build_dir {
                    layout.build_dir =
                        resolve_build_dir(&build, Some(path), worktree, cargo_home.as_deref());
                }
            }
            Some(None) | None => {
                // A config known to exist but not readable/parseable makes
                // inherited output values uncertain; do not carry them forward.
                layout.target_dir = None;
                layout.build_dir = None;
            }
        }
    }

    if let Some(p) = env("CARGO_TARGET_DIR").or_else(|| env("CARGO_BUILD_TARGET_DIR")) {
        layout.target_dir = resolve_env_path(p.into());
    }
    if let Some(p) = env("CARGO_BUILD_BUILD_DIR") {
        layout.build_dir = resolve_build_dir(
            &env_build_dir_value(p),
            None,
            worktree,
            cargo_home.as_deref(),
        );
    }
    if layout.target_dir.is_none() {
        layout.target_dir = Some(worktree.join("target"));
    }
    if layout.build_dir.is_none() {
        layout.build_dir = layout.target_dir.clone();
    }
    if layout.config_path.is_none() {
        layout
            .notes
            .push("no local Cargo config was observed".into());
    }
    layout
}

/// Cargo's home: `CARGO_HOME`, else `~/.cargo` (Cargo Book, "Cargo Home").
/// Not canonicalized, matching Cargo's `{cargo-cache-home}`.
fn cargo_home(env: &impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    env("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(|home| PathBuf::from(home).join(".cargo")))
}

/// The config files Cargo would read for `worktree`, lowest precedence first:
/// `$CARGO_HOME/config[.toml]` unless the ancestor walk already reaches it,
/// then each ancestor's `.cargo/config[.toml]` from the filesystem root down.
fn config_files(worktree: &Path, cargo_home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut cursor = Some(worktree);
    while let Some(dir) = cursor {
        dirs.push(dir.to_path_buf());
        cursor = dir.parent();
    }
    dirs.reverse();
    let config_in = |cargo_dir: &Path| {
        ["config", "config.toml"]
            .into_iter()
            .map(|name| cargo_dir.join(name))
            .find(|p| crate::fs_gate::is_file(p))
    };
    let ancestors: Vec<PathBuf> = dirs
        .into_iter()
        .filter_map(|dir| config_in(&dir.join(".cargo")))
        .collect();
    let home = cargo_home
        .and_then(config_in)
        .filter(|home| !ancestors.contains(home));
    home.into_iter().chain(ancestors).collect()
}

/// Static Cargo output declarations. Visible: every `.cargo/config[.toml]`
/// Cargo would read for this project (its ancestors and
/// `$CARGO_HOME/config.toml`) and the observer's Cargo target/build
/// environment. Command-line overrides are not inferred.
///
/// A templated `build.build-dir` is resolved for this project's own manifest
/// and declared only if that directory exists: a template in Cargo's home
/// config applies to every project, so a missing directory means "never
/// built here", not a coverage gap. A workspace member's template resolves
/// to a directory Cargo never uses (Cargo hashes the workspace root's
/// manifest) and is skipped the same way.
pub fn declared_build_paths(worktree: &Path) -> Vec<(PathBuf, String)> {
    declared_build_paths_with_env(worktree, |key| std::env::var_os(key))
}

fn declared_build_paths_with_env(
    worktree: &Path,
    env: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Vec<(PathBuf, String)> {
    let cargo_home = cargo_home(&env);
    let mut target = None;
    let mut build: Option<Option<(PathBuf, String)>> = None;
    for config in config_files(worktree, cargo_home.as_deref()) {
        let Some(text) = crate::fs_gate::read::bounded_string(
            &config,
            crate::fs_gate::read::BoundedCap::MANIFEST,
        )
        .ok() else {
            return Vec::new();
        };
        let Some(parsed) = parse_build_paths(&text) else {
            return Vec::new();
        };
        if let Some(value) = parsed.target_dir {
            target = value.map(|value| {
                (
                    resolve_config_path(Some(&config), value),
                    format!(
                        "Cargo .cargo config build.target-dir in {}",
                        config.display()
                    ),
                )
            });
        }
        if let Some(value) = parsed.build_dir {
            let evidence = match &value {
                BuildDirValue::Literal(_) => {
                    format!(
                        "Cargo .cargo config build.build-dir in {}",
                        config.display()
                    )
                }
                BuildDirValue::Template(template) => format!(
                    "Cargo .cargo config build.build-dir template `{template}` in {}, resolved for {}",
                    config.display(),
                    worktree.join("Cargo.toml").display()
                ),
            };
            let templated = matches!(value, BuildDirValue::Template(_));
            build = Some(
                resolve_build_dir(&value, Some(&config), worktree, cargo_home.as_deref())
                    .filter(|path| !templated || crate::fs_gate::is_dir(path))
                    .map(|path| (path, evidence)),
            );
        }
    }
    if let Some(value) = env("CARGO_TARGET_DIR") {
        target = resolve_env_path(value.into()).map(|path| {
            (
                path,
                "observer environment variable CARGO_TARGET_DIR".into(),
            )
        });
    } else if let Some(value) = env("CARGO_BUILD_TARGET_DIR") {
        target = resolve_env_path(value.into()).map(|path| {
            (
                path,
                "observer environment variable CARGO_BUILD_TARGET_DIR".into(),
            )
        });
    }
    if let Some(value) = env("CARGO_BUILD_BUILD_DIR") {
        let value = env_build_dir_value(value);
        let templated = matches!(value, BuildDirValue::Template(_));
        build = Some(
            resolve_build_dir(&value, None, worktree, cargo_home.as_deref())
                .filter(|path| !templated || crate::fs_gate::is_dir(path))
                .map(|path| {
                    (
                        path,
                        "observer environment variable CARGO_BUILD_BUILD_DIR".into(),
                    )
                }),
        );
    }
    target.into_iter().chain(build.flatten()).collect()
}

fn resolve_env_path(value: PathBuf) -> Option<PathBuf> {
    if value.is_absolute() {
        Some(value)
    } else {
        std::env::current_dir().ok().map(|cwd| cwd.join(value))
    }
}

fn env_build_dir_value(value: std::ffi::OsString) -> BuildDirValue {
    match value.to_str() {
        Some(text) if text.contains('{') || text.contains('}') => {
            BuildDirValue::Template(text.to_string())
        }
        _ => BuildDirValue::Literal(PathBuf::from(value)),
    }
}

/// A build-dir value as a path. Literal values resolve as before
/// (config-relative, or the observer's working directory for the
/// environment). Templates substitute Cargo's three variables for the
/// project at `worktree`; an unknown variable (which Cargo itself rejects)
/// leaves the value unresolved.
fn resolve_build_dir(
    value: &BuildDirValue,
    config: Option<&Path>,
    worktree: &Path,
    cargo_home: Option<&Path>,
) -> Option<PathBuf> {
    let template = match value {
        BuildDirValue::Literal(path) => {
            return match config {
                Some(_) => Some(resolve_config_path(config, path.clone())),
                None => resolve_env_path(path.clone()),
            };
        }
        BuildDirValue::Template(template) => template,
    };
    let manifest = worktree.join("Cargo.toml");
    let mut resolved = template.clone();
    if resolved.contains("{workspace-root}") {
        resolved = resolved.replace("{workspace-root}", worktree.to_str()?);
    }
    if resolved.contains("{cargo-cache-home}") {
        resolved = resolved.replace("{cargo-cache-home}", cargo_home?.to_str()?);
    }
    if resolved.contains("{workspace-path-hash}") {
        // Cargo hashes the manifest's real path, falling back to the path
        // as given when it cannot be canonicalized.
        let real = crate::fs_gate::canonicalize(&manifest).unwrap_or(manifest);
        resolved = resolved.replace("{workspace-path-hash}", &workspace_path_hash(&real));
    }
    if resolved.contains('{') || resolved.contains('}') {
        return None;
    }
    let path = PathBuf::from(resolved);
    Some(match config {
        Some(_) => resolve_config_path(config, path),
        None => resolve_env_path(path)?,
    })
}

/// Cargo's `{workspace-path-hash}`: `StableSipHasher128` over the manifest
/// path's `Hash`, little-endian hex, split after two characters
/// (`cargo::util::context::GlobalContext::custom_build_dir`). Cargo documents
/// this hasher as stable across releases and platforms.
pub(crate) fn workspace_path_hash(real_manifest: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = rustc_stable_hash::StableSipHasher128::new();
    real_manifest.hash(&mut hasher);
    let hex: String = Hasher::finish(&hasher)
        .to_le_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{}{}{}", &hex[..2], std::path::MAIN_SEPARATOR, &hex[2..])
}

fn parse_build_paths(text: &str) -> Option<ParsedCargoBuildPaths> {
    let value = text.parse::<toml::Value>().ok()?;
    let build = value.get("build");
    let target_dir = build.and_then(|b| b.get("target-dir")).map(|v| {
        v.as_str()
            .filter(|value| !value.contains('{') && !value.contains('}'))
            .map(PathBuf::from)
    });
    let build_dir = match build.and_then(|b| b.get("build-dir")) {
        None => None,
        Some(v) => match v.as_str() {
            Some(text) if text.contains('{') || text.contains('}') => {
                Some(BuildDirValue::Template(text.to_string()))
            }
            Some(text) => Some(BuildDirValue::Literal(PathBuf::from(text))),
            // Present but not a string: declared, value unknown.
            None => Some(BuildDirValue::Template(String::from("{unreadable}"))),
        },
    };
    Some(ParsedCargoBuildPaths {
        target_dir,
        build_dir,
    })
}

fn resolve_config_path(config: Option<&Path>, value: PathBuf) -> PathBuf {
    if value.is_absolute() {
        value
    } else {
        config
            .and_then(Path::parent)
            .and_then(Path::parent)
            .map(|p| p.join(&value))
            .unwrap_or(value)
    }
}

/// Inspect a target/build directory directly. This is useful when the build
/// root is shared or custom and is not beneath the checkout being reported.
pub fn inspect_target(target_dir: &Path, workspace_root: Option<&Path>) -> CargoInspection {
    inspect_target_incremental(target_dir, workspace_root, &[], None)
}

/// Replay-driven refresh. Only directories named by a complete event batch
/// (and their ancestors) are listed again; unaffected subtrees use saved facts.
pub fn inspect_target_incremental(
    target_dir: &Path,
    workspace_root: Option<&Path>,
    cached: &[NestedArtifact],
    changed: Option<&[PathBuf]>,
) -> CargoInspection {
    let scope = NestedArtifact::storage_id(target_dir, "");
    let mut units = Vec::new();
    let mut limits = Vec::new();
    let old: HashMap<PathBuf, &NestedArtifact> =
        cached.iter().map(|u| (u.path.clone(), u)).collect();
    let mut children: HashMap<PathBuf, Vec<&NestedArtifact>> = HashMap::new();
    for u in cached {
        if u.path != target_dir
            && let Some(p) = u.path.parent()
        {
            children.entry(p.to_path_buf()).or_default().push(u);
        }
    }
    fn copy_tree(
        path: &Path,
        old: &HashMap<PathBuf, &NestedArtifact>,
        children: &HashMap<PathBuf, Vec<&NestedArtifact>>,
        out: &mut Vec<NestedArtifact>,
    ) {
        if let Some(u) = old.get(path) {
            let mut u = (*u).clone();
            if u.is_dir {
                u.bytes = 0;
                u.logical_bytes = 0;
            }
            u.physical_bytes = 0;
            out.push(u);
            if let Some(kids) = children.get(path) {
                for k in kids {
                    copy_tree(&k.path, old, children, out);
                }
            }
        }
    }
    // Existing recursive scanner helper keeps traversal state explicit across calls.
    #[allow(clippy::too_many_arguments)]
    fn visit(
        path: &Path,
        root: &Path,
        scope: &str,
        old: &HashMap<PathBuf, &NestedArtifact>,
        children: &HashMap<PathBuf, Vec<&NestedArtifact>>,
        changed: Option<&[PathBuf]>,
        out: &mut Vec<NestedArtifact>,
        limits: &mut Vec<String>,
    ) {
        if path.to_str().is_none() {
            limits.push("non-UTF8 Cargo path unsupported; observation incomplete".into());
            return;
        }
        if let Some(changed) = changed
            && old.contains_key(path)
            && !changed
                .iter()
                .any(|c| c.starts_with(path) || c == path.parent().unwrap_or(path))
        {
            copy_tree(path, old, children, out);
            return;
        }
        let meta = match crate::fs_gate::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) => {
                limits.push(format!("{}: {e}", path.display()));
                return;
            }
        };
        let rel = relative_path(root, path);
        let (role, variant) = crate::build_adapters::cargo::classify_path(&rel, meta.is_dir());
        let id = NestedArtifact::within(scope, &rel);
        let parent = if path == root {
            None
        } else {
            Some(NestedArtifact::within(
                scope,
                &relative_path(root, path.parent().unwrap()),
            ))
        };
        let mut u = node(
            path,
            root,
            Some(id),
            parent,
            role,
            if meta.is_file() {
                meta.blocks() * 512
            } else {
                0
            },
            0,
            variant,
            vec![evidence(
                "cargo-layout",
                "observed layout; not evidence of last use or obsolescence",
                Confidence::Medium,
            )],
            vec![],
            ArtifactCoverage {
                supported: true,
                complete: true,
                limits: vec!["Cargo intermediate layout is version-dependent".into()],
            },
            None,
            true,
        );
        u.is_dir = meta.is_dir();
        u.mtime_max = meta.mtime().max(0) as u64;
        u.device = meta.dev();
        u.inode = meta.ino();
        if meta.file_type().is_symlink() {
            u.role = ArtifactRole::Unknown;
            u.coverage.limits.push("symlink not followed".into());
        }
        u.logical_bytes = if meta.is_file() { meta.len() } else { 0 };
        u.membership = if meta.nlink() > 1 && meta.is_file() {
            Membership::SharedHardlink
        } else {
            Membership::Exclusive
        };
        if path != root {
            u.container_id = Some(scope.to_string());
        }
        out.push(u);
        if meta.is_dir() {
            match crate::fs_gate::read_dir(path) {
                Err(e) => limits.push(format!("{}: {e}", path.display())),
                Ok(entries) => {
                    let mut paths = Vec::new();
                    for entry in entries {
                        match entry {
                            Ok(e) => paths.push(e.path()),
                            Err(e) => limits.push(format!("{}: {e}", path.display())),
                        }
                    }
                    paths.sort();
                    for p in paths {
                        visit(&p, root, scope, old, children, changed, out, limits);
                    }
                }
            }
        } else if path == root {
            limits.push("build root is not a directory".into());
        }
    }
    visit(
        target_dir,
        target_dir,
        &scope,
        &old,
        &children,
        changed,
        &mut units,
        &mut limits,
    );
    let complete = limits.is_empty();
    if workspace_root.is_none() {
        limits.push("Cargo.toml was not established; ownership unknown".into());
    }
    let mut seen = HashSet::new();
    for u in &mut units {
        if !u.is_dir && seen.insert((u.device, u.inode)) {
            u.physical_bytes = u.bytes;
        }
        u.coverage.complete = complete;
        u.coverage.supported = workspace_root.is_some() && complete;
        u.coverage.limits.extend(limits.iter().cloned());
    }
    aggregate_units(&mut units);
    charge_physical(&mut units);
    enrich_fingerprints(target_dir, &mut units);
    let physical_bytes = units.iter().map(|u| u.physical_bytes).sum();
    let total_bytes = units.first().map_or(0, |u| u.bytes);
    CargoInspection {
        target_dir: target_dir.into(),
        build_dir: None,
        total_bytes,
        physical_bytes,
        units,
        coverage: ArtifactCoverage {
            supported: workspace_root.is_some() && complete,
            complete,
            limits,
        },
    }
}

/// A single directory/file node in the shape the legacy direct
/// inspection produced, kept for the reviewed-role recheck below.
///
/// The folded identification that used to live here moved to
/// `crate::build_adapters::cargo` when the adapters gained a trait and a
/// registry; what stays is the direct `inspect_target` API (used by the
/// TUI's own fixtures and by the cleanup rechecks), which walks a target
/// directory on purpose rather than reading folded rows.
fn folded_node(root: &Path, scope: &str, path: &Path, is_dir: bool, bytes: u64) -> NestedArtifact {
    let rel = relative_path(root, path);
    let (role, variant) = crate::build_adapters::cargo::classify_path(&rel, is_dir);
    let mut u = node(
        path,
        root,
        Some(NestedArtifact::within(scope, &rel)),
        (path != root)
            .then(|| NestedArtifact::within(scope, &relative_path(root, path.parent().unwrap()))),
        role,
        bytes,
        0,
        variant,
        vec![evidence(
            "cargo-folded-v2",
            "directory aggregates; internal files are not retained",
            Confidence::High,
        )],
        vec![],
        ArtifactCoverage {
            supported: true,
            complete: true,
            limits: vec![
                "internal file history and subgroup hardlink attribution are not retained".into(),
            ],
        },
        None,
        true,
    );
    u.container_id = Some(scope.into());
    u.is_dir = is_dir;
    u
}

/// Recheck only the selected unit and its recorded producer evidence.
pub(crate) fn reviewed_role(
    root: &Path,
    selected: &Path,
    fingerprints: &[PathBuf],
) -> anyhow::Result<(ArtifactRole, bool)> {
    let meta = crate::fs_gate::symlink_metadata(selected)?;
    let scope = NestedArtifact::storage_id(root, "");
    let mut units = vec![folded_node(root, &scope, selected, meta.is_dir(), 0)];
    for path in fingerprints {
        units.push(folded_node(root, &scope, path, false, 0));
    }
    enrich_fingerprints(root, &mut units);
    Ok((units[0].role.clone(), meta.is_dir()))
}

/// Fingerprints provide a baseline test/executable distinction without running
/// Cargo. They describe an observed build variant, never current project intent.
fn enrich_fingerprints(root: &Path, units: &mut [NestedArtifact]) {
    let mut facts = HashMap::new();
    for u in units.iter().filter(|u| {
        !u.is_dir
            && u.role == ArtifactRole::CompanionMetadata
            && u.relative_path.contains("/.fingerprint/")
            && u.path.extension().is_some_and(|e| e == "json")
    }) {
        let name = u.path.file_name().unwrap().to_string_lossy();
        if !name.starts_with("test-") {
            continue;
        }
        let Some(dir) = u.path.parent() else { continue };
        let Some(crate_hash) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some((_, hash)) = crate_hash.rsplit_once('-') else {
            continue;
        };
        let stem = name.trim_end_matches(".json");
        let target = stem
            .strip_prefix("test-lib-")
            .or_else(|| stem.strip_prefix("test-bin-"))
            .or_else(|| stem.strip_prefix("test-integration-test-"));
        let Some(target) = target else { continue };
        let Some(profile) = dir.parent().and_then(Path::parent) else {
            continue;
        };
        let Ok(text) = crate::fs_gate::read::bounded_string(
            &u.path,
            crate::fs_gate::read::BoundedCap::MANIFEST,
        ) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        facts.insert(
            profile.join("deps").join(format!("{target}-{hash}")),
            (target.to_string(), json, u.path.clone()),
        );
    }
    for u in units.iter_mut() {
        if u.role == ArtifactRole::TestExecutable {
            // Cached enrichment is current evidence, not permanent identity.
            u.role = ArtifactRole::Dependency;
            u.variant.target = None;
            u.variant.features = None;
            u.variant.toolchain = None;
            u.action_group = None;
            u.producer_evidence
                .retain(|e| e.source != "cargo-fingerprint");
        }
        if let Some((target, json, path)) = facts.get(&u.path) {
            u.role = ArtifactRole::TestExecutable;
            u.variant.target = Some(target.clone());
            u.variant.features = json.get("features").map(|v| v.to_string());
            u.variant.toolchain = json.get("rustc").map(|v| format!("fingerprint:{v}"));
            u.producer_evidence.push(evidence(
                "cargo-fingerprint",
                path.display().to_string(),
                Confidence::Medium,
            ));
            u.action_group = Some(NestedArtifact::storage_id(
                root,
                &format!("action:{}", u.relative_path),
            ));
        }
    }
}

// Existing node-construction helper centralizes the full artifact record shape.
#[allow(clippy::too_many_arguments)]
fn node(
    path: &Path,
    root: &Path,
    id: Option<String>,
    parent_id: Option<String>,
    role: ArtifactRole,
    bytes: u64,
    physical_bytes: u64,
    variant: ArtifactVariant,
    producer_evidence: Vec<ArtifactEvidence>,
    consumer_evidence: Vec<ArtifactEvidence>,
    coverage: ArtifactCoverage,
    action_group: Option<String>,
    present: bool,
) -> NestedArtifact {
    let relative_path = relative_path(root, path);
    let id = id.unwrap_or_else(|| NestedArtifact::stable_id(&relative_path, &role));
    let mtime_max = 0;
    NestedArtifact {
        physical_total: 0,
        is_dir: false,
        device: 0,
        inode: 0,
        logical_bytes: 0,
        id: id.clone(),
        path: path.to_path_buf(),
        relative_path,
        parent_id,
        container_id: None,
        role,
        membership: if physical_bytes == 0 {
            Membership::Unknown
        } else {
            Membership::Exclusive
        },
        bytes,
        physical_bytes,
        mtime_max,
        variant,
        producer_evidence,
        consumer_evidence,
        coverage,
        action_group,
        present,
        growth_bytes: None,
        regrowth_count: 0,
        decision_evidence: Vec::new(),
        adapter: Some("cargo".into()),
        basis: crate::artifact::AccountingBasis::Allocated,
        time_source: crate::artifact::TimeSource::FileModification,
        action: crate::artifact::NestedActionCapability::InspectionOnly,
        consequence: None,
        reported_by: None,
        writer_lock: None,
        guidance: crate::cargo_cleanup::Guidance::default(),
    }
}

fn evidence(source: &str, detail: impl Into<String>, confidence: Confidence) -> ArtifactEvidence {
    ArtifactEvidence {
        source: source.into(),
        detail: detail.into(),
        confidence,
    }
}

/// Every directory is emitted before its descendants. Fold child logical
/// totals and newest mtimes upward once, after the single layout traversal.
/// This keeps a 250k-file target from being recursively sized once per
/// container/profile in addition to enumerating it.
fn aggregate_units(units: &mut [NestedArtifact]) {
    let indexes: HashMap<String, usize> = units
        .iter()
        .enumerate()
        .map(|(index, unit)| (unit.id.clone(), index))
        .collect();
    for child_index in (0..units.len()).rev() {
        let Some(parent_id) = units[child_index].parent_id.clone() else {
            continue;
        };
        let Some(&parent_index) = indexes.get(&parent_id) else {
            continue;
        };
        if parent_index == child_index {
            continue;
        }
        let child_bytes = units[child_index].bytes;
        let child_logical = units[child_index].logical_bytes;
        let child_mtime = units[child_index].mtime_max;
        units[parent_index].bytes += child_bytes;
        units[parent_index].logical_bytes += child_logical;
        units[parent_index].mtime_max = units[parent_index].mtime_max.max(child_mtime);
    }
}

/// One deterministic charge per inode across all observed Cargo roots.
pub fn charge_physical(units: &mut [NestedArtifact]) {
    let mut seen = HashSet::new();
    for u in units.iter_mut() {
        u.physical_bytes = if !u.is_dir && seen.insert((u.device, u.inode)) {
            u.bytes
        } else {
            0
        };
        u.physical_total = u.physical_bytes;
    }
    let indexes: HashMap<_, _> = units
        .iter()
        .enumerate()
        .map(|(i, u)| (u.id.clone(), i))
        .collect();
    for i in (0..units.len()).rev() {
        if let Some(parent) = units[i]
            .parent_id
            .as_ref()
            .and_then(|p| indexes.get(p))
            .copied()
            && parent != i
        {
            units[parent].physical_total += units[i].physical_total;
        }
    }
}

/// Apply optional existing Cargo JSON messages to an inspection. A malformed
/// or stale record is retained as a limitation; it never upgrades a path-only
/// guess into an ownership verdict.
pub fn apply_message_evidence(inspection: &mut CargoInspection, messages: &[CargoMessageEvidence]) {
    for message in messages {
        for filename in &message.filenames {
            let Some(unit) = inspection.units.iter_mut().find(|u| u.path == *filename) else {
                continue;
            };
            if let Some(name) = &message.target_name {
                unit.variant.target = Some(name.clone());
            }
            if let Some(package_id) = &message.package_id {
                unit.variant.package = Some(package_id.clone());
            }
            if message.profile_test || message.target_kind.iter().any(|k| k == "test") {
                unit.role = ArtifactRole::TestExecutable;
            } else if message.target_kind.iter().any(|k| k == "example") {
                unit.role = ArtifactRole::Example;
            }
            unit.producer_evidence.push(evidence(
                "cargo-json",
                "caller-supplied historical compiler-artifact message; path match does not prove freshness or execution",
                Confidence::Low,
            ));
            if let Some(features) = &message.features {
                unit.variant.features = Some(features.join(","));
            }
            unit.coverage.limits.push("JSON build record is not bound to current file content; cleanup requires fresh native evidence".into());
            unit.variant
                .unknowns
                .retain(|u| u != "package (unless JSON build evidence is supplied)");
        }
    }
}

/// Parse existing JSON lines without executing Cargo. Unknown message kinds
/// are ignored, as Cargo may add message variants over time.
pub fn parse_json_messages(text: &str) -> Vec<CargoMessageEvidence> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|v| v.get("reason").and_then(|r| r.as_str()) == Some("compiler-artifact"))
        .map(|v| CargoMessageEvidence {
            profile_test: v
                .get("profile")
                .and_then(|p| p.get("test"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            features: v.get("features").and_then(|v| v.as_array()).map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            }),
            target_name: v
                .get("target")
                .and_then(|t| t.get("name"))
                .and_then(|n| n.as_str())
                .map(str::to_string),
            target_kind: v
                .get("target")
                .and_then(|t| t.get("kind"))
                .and_then(|k| k.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            package_id: v
                .get("package_id")
                .and_then(|p| p.as_str())
                .map(str::to_string),
            filenames: v
                .get("filenames")
                .and_then(|f| f.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(PathBuf::from))
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
}

/// Already observed Cargo build boundaries only, deduplicated across owners.
pub fn project_roots(projects: &[ProjectRow]) -> Vec<(PathBuf, PathBuf)> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for project in projects {
        for wt in &project.worktrees {
            let layout = layout_for(&wt.path);
            let cargo_present = crate::fs_gate::is_file(wt.path.join("Cargo.toml"));
            if !cargo_present && !project.ecosystems.iter().any(|t| t == "rs") {
                continue;
            }
            for row in &wt.artifacts {
                if row.kind != ArtifactKind::BuildOutput || !crate::fs_gate::is_dir(&row.path) {
                    continue;
                }
                let matches_layout = layout.target_dir.as_ref().is_some_and(|p| p == &row.path)
                    || layout.build_dir.as_ref().is_some_and(|p| p == &row.path)
                    || row.path.file_name().and_then(|n| n.to_str()) == Some("target");
                if !matches_layout {
                    continue;
                }
                let canonical =
                    crate::fs_gate::canonicalize(&row.path).unwrap_or_else(|_| row.path.clone());
                if seen.insert(canonical) {
                    out.push((row.path.clone(), wt.path.clone()));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn identifies_profiles_roles_companions_incremental_and_hardlinks() {
        let tmp = tempdir().unwrap();
        let target = tmp.path().join("target");
        fs::create_dir_all(target.join("debug/deps")).unwrap();
        fs::create_dir_all(target.join("debug/examples")).unwrap();
        fs::create_dir_all(target.join("debug/incremental/unit")).unwrap();
        fs::create_dir_all(target.join("debug/build/pkg-hash/out")).unwrap();
        fs::write(target.join("debug/deps/libfoo-abc123.rlib"), b"dep").unwrap();
        fs::write(target.join("debug/deps/libfoo-abc123.d"), b"dep").unwrap();
        fs::hard_link(
            target.join("debug/deps/libfoo-abc123.rlib"),
            target.join("debug/examples/shared-output"),
        )
        .unwrap();
        fs::write(target.join("debug/examples/demo"), b"example").unwrap();
        fs::write(target.join("debug/incremental/unit/cache"), b"incremental").unwrap();
        fs::write(
            target.join("debug/build/pkg-hash/out/generated.rs"),
            b"generated",
        )
        .unwrap();
        let report = inspect_target(&target, Some(tmp.path()));
        assert!(
            report
                .units
                .iter()
                .any(|u| u.role == ArtifactRole::Dependency)
        );
        assert!(
            report
                .units
                .iter()
                .any(|u| u.role == ArtifactRole::CompanionMetadata)
        );
        assert!(report.units.iter().any(|u| u.role == ArtifactRole::Example));
        assert!(
            report
                .units
                .iter()
                .any(|u| u.role == ArtifactRole::Incremental)
        );
        assert!(
            report
                .units
                .iter()
                .any(|u| u.role == ArtifactRole::BuildScriptOutput)
        );
        assert!(report.physical_bytes <= report.total_bytes);
        assert!(
            report
                .units
                .iter()
                .any(|u| u.membership == Membership::SharedHardlink)
        );
        assert!(report.units.iter().any(|u| u.parent_id.is_some()));
    }

    #[test]
    fn bounded_profile_inspection_returns_explicit_partial_residuals() {
        let tmp = tempdir().unwrap();
        let profile = tmp.path().join("debug");
        fs::create_dir_all(profile.join("deps")).unwrap();
        fs::create_dir_all(profile.join(".fingerprint")).unwrap();
        fs::write(profile.join("deps/librenamed-abc123.rlib"), b"artifact").unwrap();
        fs::write(
            profile.join("deps/libsecond-def456.rlib"),
            b"second artifact",
        )
        .unwrap();
        let cancelled = AtomicBool::new(true);
        let result = inspect_profile(
            &profile,
            CargoProfileInspectionLimits::default(),
            &cancelled,
        );
        assert!(!result.coverage.complete);
        assert!(
            result
                .coverage
                .limits
                .iter()
                .any(|s| s.contains("cancelled"))
        );
        assert_eq!(result.allocated_bytes, 0);

        let result = inspect_profile(
            &profile,
            CargoProfileInspectionLimits {
                max_entries: 1,
                ..Default::default()
            },
            &AtomicBool::new(false),
        );
        assert!(!result.coverage.complete);
        assert!(result.entries_examined <= 1);
        assert!(result.allocated_bytes == 0 || !result.groups.is_empty());
        assert!(result.accounting_note.contains("not reclaimable"));
    }

    #[test]
    fn profile_inspection_counts_hardlinks_in_allocated_but_not_unique_total() {
        let tmp = tempdir().unwrap();
        let profile = tmp.path().join("debug");
        fs::create_dir_all(profile.join("deps")).unwrap();
        fs::create_dir_all(profile.join(".fingerprint")).unwrap();
        fs::write(profile.join("deps/liba-abc.rlib"), vec![7u8; 4096]).unwrap();
        fs::hard_link(
            profile.join("deps/liba-abc.rlib"),
            profile.join("deps/libb-def.rlib"),
        )
        .unwrap();
        let result = inspect_profile(
            &profile,
            CargoProfileInspectionLimits::default(),
            &AtomicBool::new(false),
        );
        assert_eq!(
            result.allocated_bytes,
            result.unique_allocated_bytes.saturating_mul(2)
        );
        assert_eq!(result.groups.iter().map(|g| g.entries).sum::<usize>(), 2);
        assert!(result.groups.iter().all(|g| g.residual_reason.is_some()));
    }

    #[cfg(unix)]
    #[test]
    fn profile_inspection_does_not_gate_on_cargo_build_lock() {
        let tmp = tempdir().unwrap();
        let profile = tmp.path().join("debug");
        fs::create_dir_all(profile.join("deps")).unwrap();
        fs::create_dir_all(profile.join(".fingerprint")).unwrap();
        fs::write(profile.join("deps/libbusy-abc.rlib"), b"artifact").unwrap();
        let build_lock = profile.join(".cargo-lock");
        fs::File::create(&build_lock).unwrap();
        let lock = crate::fs_gate::sys::RegularFile::open_nofollow(&build_lock).unwrap();
        lock.try_lock().unwrap();
        let result = inspect_profile(
            &profile,
            CargoProfileInspectionLimits::default(),
            &AtomicBool::new(false),
        );
        assert!(result.coverage.complete);
        assert!(result.allocated_bytes > 0);
        assert!(result.accounting_note.contains("not atomic"));
        lock.unlock().unwrap();
        let after = inspect_profile(
            &profile,
            CargoProfileInspectionLimits::default(),
            &AtomicBool::new(false),
        );
        assert!(
            after.coverage.complete,
            "an idle lock file is not an active build: {:?}",
            after.coverage.limits
        );
    }

    #[cfg(unix)]
    #[test]
    fn profile_inspection_never_follows_deps_or_fingerprint_directory_symlinks() {
        use std::os::unix::fs::symlink;
        let tmp = tempdir().unwrap();
        let profile = tmp.path().join("debug");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&profile).unwrap();
        fs::create_dir_all(outside.join("deps")).unwrap();
        fs::create_dir_all(outside.join(".fingerprint/opaque-abc")).unwrap();
        fs::write(outside.join("deps/libsecret-abc.rlib"), vec![1u8; 4096]).unwrap();
        fs::write(
            outside.join(".fingerprint/opaque-abc/lib-secret.json"),
            "{}",
        )
        .unwrap();
        symlink(outside.join("deps"), profile.join("deps")).unwrap();
        symlink(outside.join(".fingerprint"), profile.join(".fingerprint")).unwrap();
        let result = inspect_profile(
            &profile,
            CargoProfileInspectionLimits::default(),
            &AtomicBool::new(false),
        );
        assert_eq!(result.allocated_bytes, 0);
        assert_eq!(result.metadata_bytes_read, 0);
        assert!(
            result
                .coverage
                .limits
                .iter()
                .any(|s| s.contains("not a real directory"))
        );
    }

    #[test]
    fn fingerprint_target_parser_prefers_test_target_prefixes() {
        assert_eq!(
            parse_fingerprint_target("test-lib-renamed-with-dash"),
            Some(("test-lib", "renamed-with-dash"))
        );
        assert_eq!(
            parse_fingerprint_target("test-integration-test-cli"),
            Some(("test-integration-test", "cli"))
        );
    }

    #[test]
    fn profile_inspection_uses_target_named_fingerprint_not_package_hash_as_identity() {
        let tmp = tempdir().unwrap();
        let profile = tmp.path().join("debug");
        fs::create_dir_all(profile.join("deps")).unwrap();
        fs::create_dir_all(profile.join(".fingerprint/opaque-abc")).unwrap();
        fs::write(profile.join("deps/librenamed-abc.rlib"), b"artifact").unwrap();
        fs::write(
            profile.join(".fingerprint/opaque-abc/lib-renamed.json"),
            r#"{"features":["fast"],"rustc":"rustc-test"}"#,
        )
        .unwrap();
        fs::create_dir_all(profile.join(".fingerprint/other-def")).unwrap();
        fs::write(profile.join("deps/librenamed-def.rlib"), b"another variant").unwrap();
        fs::write(
            profile.join(".fingerprint/other-def/lib-renamed.json"),
            r#"{"features":["slow"],"rustc":"rustc-other"}"#,
        )
        .unwrap();
        fs::write(profile.join("deps/libstale-zzz.rlib"), b"stale fingerprint").unwrap();
        let result = inspect_profile(
            &profile,
            CargoProfileInspectionLimits::default(),
            &AtomicBool::new(false),
        );
        let fast = result
            .groups
            .iter()
            .find(|g| g.variant.features.as_deref() == Some("fast"))
            .unwrap();
        let slow = result
            .groups
            .iter()
            .find(|g| g.variant.features.as_deref() == Some("slow"))
            .unwrap();
        let residual = result
            .groups
            .iter()
            .find(|g| g.residual_reason.is_some())
            .unwrap();
        assert_eq!(fast.target.as_deref(), Some("renamed"));
        assert_eq!(fast.variant.toolchain.as_deref(), Some("rustc-test"));
        assert_eq!(fast.package_id, None);
        assert_ne!(fast.variant.features, slow.variant.features);
        assert_eq!(residual.target, None);
    }

    #[test]
    fn config_paths_are_read_without_running_cargo() {
        let tmp = tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".cargo")).unwrap();
        let mut f = fs::File::create(tmp.path().join(".cargo/config.toml")).unwrap();
        writeln!(
            f,
            "[build]\ntarget-dir = \"../shared-target\"\nbuild-dir = \"../shared-build\""
        )
        .unwrap();
        let layout = layout_with_env(tmp.path(), |_| None);
        assert_eq!(layout.target_dir, Some(tmp.path().join("../shared-target")));
        assert_eq!(layout.build_dir, Some(tmp.path().join("../shared-build")));
    }

    #[test]
    fn configured_paths_follow_effective_precedence_and_reject_unknown_overrides() {
        let tmp = tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(tmp.path().join(".cargo")).unwrap();
        fs::create_dir_all(repo.join(".cargo")).unwrap();
        fs::write(
            tmp.path().join(".cargo/config.toml"),
            "[build]\ntarget-dir='../ancestor-target'\nbuild-dir='../shared-build'\n",
        )
        .unwrap();
        let child_config = repo.join(".cargo/config.toml");
        fs::write(&child_config, "[build]\ntarget-dir='../local-target'\n").unwrap();
        let local_target = repo.join("../local-target");
        let ancestor_target = tmp.path().join("../ancestor-target");
        let shared_build = tmp.path().join("../shared-build");

        let no_env = |_: &str| None;
        let declarations = declared_build_paths_with_env(&repo, no_env);
        assert_eq!(declarations.len(), 2);
        assert!(declarations.iter().any(|(path, _)| path == &local_target));
        assert!(declarations.iter().any(|(path, _)| path == &shared_build));
        assert!(
            declarations
                .iter()
                .all(|(path, _)| path != &ancestor_target)
        );

        let env_target = tmp.path().join("env-target");
        let declarations = declared_build_paths_with_env(&repo, |key| {
            (key == "CARGO_TARGET_DIR").then(|| env_target.clone().into_os_string())
        });
        assert!(
            declarations.iter().any(|(path, source)| {
                path == &env_target && source.contains("CARGO_TARGET_DIR")
            })
        );
        assert!(declarations.iter().all(|(path, _)| path != &local_target));
        let layout = layout_with_env(&repo, |key| {
            (key == "CARGO_TARGET_DIR").then(|| env_target.clone().into_os_string())
        });
        assert_eq!(layout.target_dir, Some(env_target.clone()));
        assert_eq!(layout.build_dir, Some(shared_build));

        fs::write(
            &child_config,
            "[build]\ntarget-dir='{workspace-root}/unknown-cache'\n",
        )
        .unwrap();
        let declarations = declared_build_paths_with_env(&repo, no_env);
        assert!(
            declarations
                .iter()
                .all(|(path, _)| path != &ancestor_target)
        );
        assert!(declarations.iter().all(|(path, _)| path != &local_target));
    }

    /// The value Cargo 1.98.1 itself created on macOS for this manifest
    /// (`~/.cargo/build/b5/39d5cda724e19e` after `cargo build` with
    /// `build-dir = "{cargo-cache-home}/build/{workspace-path-hash}"`).
    #[cfg(unix)]
    #[test]
    fn workspace_path_hash_matches_cargo() {
        assert_eq!(
            workspace_path_hash(Path::new("/private/tmp/bdx.cjHC/demo/Cargo.toml")),
            "b5/39d5cda724e19e"
        );
    }

    /// A fixture: `$CARGO_HOME/config.toml` with `home_config`, and a
    /// package at `<tmp>/src/<name>` outside the home's ancestry (as a
    /// shared `/Volumes/src` checkout is).
    fn cargo_home_fixture(home_config: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempdir().unwrap();
        let cargo_home = tmp.path().join("home/.cargo");
        fs::create_dir_all(&cargo_home).unwrap();
        fs::write(cargo_home.join("config.toml"), home_config).unwrap();
        let project = tmp.path().join("src/app");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("Cargo.toml"), "[package]\nname='app'\n").unwrap();
        (tmp, cargo_home, project)
    }

    fn built_dir(cargo_home: &Path, project: &Path) -> PathBuf {
        let manifest = fs::canonicalize(project.join("Cargo.toml")).unwrap();
        let dir = cargo_home
            .join("build")
            .join(workspace_path_hash(&manifest));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn cargo_home_build_dir_template_links_the_directory_cargo_built() {
        let (_tmp, cargo_home, project) = cargo_home_fixture(
            "[build]\nbuild-dir = \"{cargo-cache-home}/build/{workspace-path-hash}\"\n",
        );
        let env = |key: &str| (key == "CARGO_HOME").then(|| cargo_home.clone().into_os_string());

        // Never built here: the template resolves, but nothing is declared.
        assert!(declared_build_paths_with_env(&project, env).is_empty());

        let dir = built_dir(&cargo_home, &project);
        let declarations = declared_build_paths_with_env(&project, env);
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0].0, dir);
        assert!(declarations[0].1.contains("{workspace-path-hash}"));
        assert!(declarations[0].1.contains("resolved for"));

        let layout = layout_with_env(&project, env);
        assert_eq!(layout.build_dir, Some(dir));
        assert_eq!(layout.target_dir, Some(project.join("target")));
    }

    #[test]
    fn cargo_home_defaults_to_dot_cargo_under_home() {
        let (tmp, _cargo_home, project) = cargo_home_fixture(
            "[build]\nbuild-dir = \"{cargo-cache-home}/build/{workspace-path-hash}\"\n",
        );
        let home = tmp.path().join("home");
        let dir = built_dir(&home.join(".cargo"), &project);
        let declarations = declared_build_paths_with_env(&project, |key| {
            (key == "HOME").then(|| home.clone().into_os_string())
        });
        assert_eq!(
            declarations.iter().map(|(p, _)| p).collect::<Vec<_>>(),
            vec![&dir]
        );
    }

    #[test]
    fn project_config_overrides_cargo_home_config() {
        let (_tmp, cargo_home, project) = cargo_home_fixture(
            "[build]\nbuild-dir = \"{cargo-cache-home}/build/{workspace-path-hash}\"\n",
        );
        built_dir(&cargo_home, &project);
        fs::create_dir_all(project.join(".cargo")).unwrap();
        fs::write(
            project.join(".cargo/config.toml"),
            "[build]\nbuild-dir = 'local-build'\n",
        )
        .unwrap();
        let env = |key: &str| (key == "CARGO_HOME").then(|| cargo_home.clone().into_os_string());
        let declarations = declared_build_paths_with_env(&project, env);
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0].0, project.join("local-build"));
    }

    #[test]
    fn workspace_member_template_is_not_declared() {
        // Cargo hashes the workspace root's manifest, so a member's own
        // resolution names a directory Cargo never creates.
        let (_tmp, cargo_home, root) = cargo_home_fixture(
            "[build]\nbuild-dir = \"{cargo-cache-home}/build/{workspace-path-hash}\"\n",
        );
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers=['member']\n").unwrap();
        let member = root.join("member");
        fs::create_dir_all(&member).unwrap();
        fs::write(member.join("Cargo.toml"), "[package]\nname='member'\n").unwrap();
        let root_dir = built_dir(&cargo_home, &root);
        let env = |key: &str| (key == "CARGO_HOME").then(|| cargo_home.clone().into_os_string());
        assert!(declared_build_paths_with_env(&member, env).is_empty());
        assert_eq!(declared_build_paths_with_env(&root, env)[0].0, root_dir);
    }

    #[test]
    fn unknown_template_variables_stay_unresolved() {
        let (_tmp, cargo_home, project) =
            cargo_home_fixture("[build]\nbuild-dir = \"{cargo-cache-home}/{not-a-variable}\"\n");
        fs::create_dir_all(cargo_home.join("{not-a-variable}")).unwrap();
        let env = |key: &str| (key == "CARGO_HOME").then(|| cargo_home.clone().into_os_string());
        assert!(declared_build_paths_with_env(&project, env).is_empty());
        assert_eq!(
            layout_with_env(&project, env).build_dir,
            Some(project.join("target"))
        );
    }

    #[test]
    fn environment_build_dir_template_resolves_like_config() {
        let (_tmp, cargo_home, project) = cargo_home_fixture("");
        let dir = built_dir(&cargo_home, &project);
        let env = |key: &str| match key {
            "CARGO_HOME" => Some(cargo_home.clone().into_os_string()),
            "CARGO_BUILD_BUILD_DIR" => {
                Some("{cargo-cache-home}/build/{workspace-path-hash}".into())
            }
            _ => None,
        };
        let declarations = declared_build_paths_with_env(&project, env);
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0].0, dir);
        assert!(declarations[0].1.contains("CARGO_BUILD_BUILD_DIR"));
    }

    #[test]
    fn json_evidence_distinguishes_test_and_renamed_package_without_guessing() {
        let text = r#"{"reason":"compiler-artifact","package_id":"pkg 1.0.0","target":{"name":"renamed-test","kind":["test"]},"filenames":["/tmp/target/debug/deps/renamed-test"]}"#;
        let messages = parse_json_messages(text);
        assert_eq!(messages[0].target_name.as_deref(), Some("renamed-test"));
        assert_eq!(messages[0].target_kind, vec!["test"]);

        let tmp = tempdir().unwrap();
        let target = tmp.path().join("target");
        fs::create_dir_all(target.join("debug/deps")).unwrap();
        let output = target.join("debug/deps/renamed-test");
        fs::write(&output, b"test").unwrap();
        let mut inspection = inspect_target(&target, Some(tmp.path()));
        apply_message_evidence(
            &mut inspection,
            &[CargoMessageEvidence {
                profile_test: true,
                features: None,
                target_name: Some("renamed-test".into()),
                target_kind: vec!["test".into()],
                package_id: Some("pkg 1.0.0".into()),
                filenames: vec![output.clone()],
            }],
        );
        let unit = inspection.units.iter().find(|u| u.path == output).unwrap();
        assert_eq!(unit.role, ArtifactRole::TestExecutable);
        assert_eq!(unit.variant.package.as_deref(), Some("pkg 1.0.0"));
        assert!(
            unit.producer_evidence
                .iter()
                .any(|e| e.source == "cargo-json")
        );
    }

    #[test]
    fn absent_cargo_keeps_layout_visible_but_coverage_unknown() {
        let tmp = tempdir().unwrap();
        let target = tmp.path().join("target");
        fs::create_dir_all(target.join("debug")).unwrap();
        fs::write(target.join("debug/app"), b"output").unwrap();
        let inspection = inspect_target(&target, None);
        assert!(!inspection.coverage.supported);
        assert!(
            inspection
                .coverage
                .limits
                .iter()
                .any(|limit| limit.contains("Cargo.toml"))
        );
        assert!(
            inspection
                .units
                .iter()
                .any(|u| u.role == ArtifactRole::FinalOutput)
        );
    }

    #[test]
    fn identity_survives_role_reclassification() {
        assert_eq!(
            NestedArtifact::stable_id("debug/deps/renamed-test", &ArtifactRole::Dependency),
            NestedArtifact::stable_id("debug/deps/renamed-test", &ArtifactRole::TestExecutable)
        );
    }
}

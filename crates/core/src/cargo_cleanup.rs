//! Exact, reviewed Cargo groups. No age-based eligibility or blanket prune.
//! Supports the tested legacy profile layout only. Locks are advisory: manual
//! writers that ignore Cargo's locks must be stopped by the user.
use crate::artifact::{ArtifactRole, NestedArtifact};
use crate::fs_gate::{self as fs, MetadataExt, sys::RegularFile};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Member {
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
    #[serde(default)]
    pub nlink: u64,
    #[serde(default)]
    pub hardlink_members: u64,
    pub bytes: u64,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CargoGroup {
    pub container: PathBuf,
    pub selected: PathBuf,
    pub profile: PathBuf,
    pub role: ArtifactRole,
    pub members: Vec<Member>,
    pub lock_paths: Vec<PathBuf>,
    pub evidence: Vec<Member>,
    /// Bytes allocated by the selected directory entries. This is not a
    /// promise about bytes reclaimed: an inode may still have aliases.
    #[serde(default)]
    pub allocated_bytes: u64,
    /// Reserved for a conservative allocation-based potential estimate.
    /// Currently always `None`: APFS clones, snapshots, and same-device
    /// Trash make any reclaimed-space value unknowable, so this never becomes
    /// an immediate-free-space promise or an execution gate.
    #[serde(default)]
    pub reclaimable_bytes: Option<u64>,
    /// Compact per-plan evidence for the action/UI layers. We deliberately do
    /// not retain or build a filesystem-wide alias index.
    #[serde(default)]
    pub hardlink_members: u64,
    #[serde(default)]
    pub shared_storage: bool,
}

/// Derived from existing facts only. Never performs I/O or implies authorization.
///
/// R18a: computed exactly once per observe pass, from the report's own
/// fixed `observed_at` (`report::attach_cargo_guidance`), and stored as
/// typed columns on `nested_artifacts.parquet`
/// (`NestedArtifact::guidance`) -- never recomputed from a live clock at
/// serialization time. Before this, `Report.nested_artifacts`'s
/// `#[serde(serialize_with = "serialize_units")]` hook called
/// `guidance(unit)` (i.e. `guidance_at(unit, entities::now())`) at
/// serialize time, so serializing the same `Report` twice a wall-clock
/// second apart produced two different `modified_age_secs` values --
/// the flaky
/// `project_worktree_tables::report_reads_projects_worktrees_and_artifact_facts_from_the_tables_not_the_snapshot_json`
/// (a `swamp report --json` run had the identical non-reproducibility
/// bug). `guidance`/`guidance_at` below are still used for on-demand,
/// intentionally-live computations (`swamp cargo check`'s human-
/// initiated review, the TUI's live rendering) -- only the report JSON
/// serialization path stopped calling them at read time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct Guidance {
    pub recommendation: String,
    pub modified_age_secs: Option<u64>,
    /// Owned (not `&'static str`, unlike [`recommendation`]'s own return
    /// type) so a rebuilt `Guidance` -- read back from
    /// `nested_artifacts.parquet`'s typed `guidance_*` columns rather
    /// than recomputed from a live clock -- can be constructed from a
    /// stored `String` without leaking memory or guessing which static
    /// constant it came from (R18a).
    pub consequence: String,
    pub scope: String,
    pub check_status: String,
    pub reason_code: String,
    pub message: String,
    pub next_action: String,
}

pub fn guidance(unit: &NestedArtifact) -> Guidance {
    guidance_at(unit, crate::entities::now())
}

pub fn guidance_at(unit: &NestedArtifact, now: u64) -> Guidance {
    let (scope, status, code, message, next) =
        if !unit.coverage.complete || !unit.coverage.supported {
            (
                "unknown",
                "blocked",
                "coverage_limited",
                "Coverage is incomplete; refresh before reviewing cleanup.",
                "refresh",
            )
        } else if candidate(unit) {
            (
                "group",
                "unchecked",
                "checks_not_run",
                "Cleanup candidate; review rebuilding cost and run exact-selection checks.",
                "review_cleanup",
            )
        } else if unit.is_dir {
            (
                "summary",
                "not_applicable",
                "summary_row",
                "Category total, not a selective cleanup unit. Inspect individual groups.",
                "inspect_groups",
            )
        } else {
            (
                "output",
                "blocked",
                "unsupported_role",
                "This output is inspection-only; selective cleanup is not supported.",
                "inspect",
            )
        };
    let age = modified_age_secs(unit, now);
    let advice = if candidate(unit) {
        let modified = match age {
            Some(seconds) if seconds >= 86400 => format!("{}d ago", seconds / 86400),
            Some(seconds) if seconds >= 3600 => format!("{}h ago", seconds / 3600),
            Some(seconds) if seconds >= 60 => format!("{}m ago", seconds / 60),
            Some(_) => "just now".into(),
            None => "unknown".into(),
        };
        format!("Cleanup candidate · last modified {modified}")
    } else {
        recommendation(unit).0.into()
    };
    Guidance {
        recommendation: advice,
        modified_age_secs: age,
        consequence: recommendation(unit).1.to_string(),
        scope: scope.to_string(),
        check_status: status.to_string(),
        reason_code: code.to_string(),
        message: message.to_string(),
        next_action: next.to_string(),
    }
}

/// Modification age is a useful heuristic, not evidence of last execution.
/// Missing and future timestamps must not look like extremely old artifacts.
pub fn modified_age_secs(unit: &NestedArtifact, now: u64) -> Option<u64> {
    (unit.mtime_max > 0 && unit.mtime_max <= now).then(|| now - unit.mtime_max)
}

/// Oldest known candidates first; size breaks ties, unknown ages come last.
pub fn cleanup_order(a: &NestedArtifact, b: &NestedArtifact, now: u64) -> std::cmp::Ordering {
    modified_age_secs(b, now)
        .cmp(&modified_age_secs(a, now))
        .then_with(|| b.bytes.cmp(&a.bytes))
        .then_with(|| a.path.cmp(&b.path))
}

/// Decision support, not eligibility or authorization. No age-based disuse claim.
pub fn recommendation(unit: &NestedArtifact) -> (&'static str, &'static str) {
    if !unit.coverage.complete || !unit.coverage.supported {
        return (
            "Inspect coverage",
            "Incomplete evidence; refresh before cleanup",
        );
    }
    match unit.role {
        ArtifactRole::Incremental => (
            "Start here: compiler cache",
            "Remove to trade cached compilation work for space; next build may be slower",
        ),
        ArtifactRole::TestExecutable | ArtifactRole::Example => (
            "Review compiled executable",
            "Remove if no longer needed; rerunning requires rebuilding with source and toolchain",
        ),
        ArtifactRole::BuildScriptOutput => (
            "Review generated output",
            "Rebuild reruns build scripts and may need external tools or network access",
        ),
        ArtifactRole::Dependency => (
            "Compiled dependencies",
            "Folded aggregate; selective dependency cleanup is not implemented",
        ),
        ArtifactRole::Container | ArtifactRole::Profile => (
            "Inspect build groups",
            "Includes children; sizes are allocation, not guaranteed free space",
        ),
        _ => (
            "Inspect purpose",
            "No supported cleanup recommendation for this output",
        ),
    }
}

#[derive(Debug, Serialize)]
pub struct CheckResult {
    pub recommendation: String,
    pub consequence: String,
    pub modified_age_secs: Option<u64>,
    pub path: PathBuf,
    pub allocated_bytes: u64,
    pub check_status: String,
    pub reason_code: String,
    pub message: String,
    pub next_action: String,
    pub plan_id: Option<String>,
    /// Argument vector, never shell-interpolated. Caller must retain its store.
    pub next_command: Vec<String>,
    pub members: Vec<PathBuf>,
    /// Human-facing plan warnings, including shared-storage uncertainty.
    pub warnings: Vec<String>,
    pub recovery: Option<String>,
    pub checked_at: u64,
    pub elapsed_ms: u128,
}

/// Explicit bounded review; no approval, execution, or automatic scope expansion.
pub fn check(
    report: &crate::report::Report,
    _store: &Path,
    paths: &[PathBuf],
    role: Option<&str>,
    limit: usize,
) -> Result<Vec<CheckResult>> {
    anyhow::ensure!((1..=20).contains(&limit), "limit must be between 1 and 20");
    let mut selected: Vec<_> = report
        .nested_artifacts
        .iter()
        .filter(|u| {
            (if paths.is_empty() {
                candidate(u)
            } else {
                paths.contains(&u.path)
            }) && role.is_none_or(|r| u.role.label() == r)
        })
        .collect();
    if !paths.is_empty() {
        for path in paths {
            anyhow::ensure!(
                selected.iter().any(|u| &u.path == path),
                "No matching Cargo row for {}. Use an exact path from report JSON; selection was not widened.",
                path.display()
            );
        }
        anyhow::ensure!(
            selected.len() <= limit,
            "Selection exceeds limit; increase --limit (maximum 20) or select fewer paths"
        );
    }
    selected.sort_by(|a, b| cleanup_order(a, b, report.observed_at));
    selected.truncate(limit);
    let mut results = Vec::new();
    for unit in selected {
        let start = std::time::Instant::now();
        let checked_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        let g = guidance(unit);
        let check_status_is_unchecked = g.check_status == "unchecked";
        let mut result = CheckResult {
            recommendation: g.recommendation,
            consequence: g.consequence,
            modified_age_secs: g.modified_age_secs,
            path: unit.path.clone(),
            allocated_bytes: unit.bytes,
            check_status: g.check_status,
            reason_code: g.reason_code,
            message: g.message,
            next_action: g.next_action,
            plan_id: None,
            next_command: vec![
                "swamp".into(),
                "report".into(),
                report.root.display().to_string(),
                "--view".into(),
                "rust".into(),
                "--all".into(),
            ],
            members: Vec::new(),
            warnings: Vec::new(),
            recovery: None,
            checked_at,
            elapsed_ms: 0,
        };
        if check_status_is_unchecked {
            match crate::actions::propose(
                report,
                None,
                std::slice::from_ref(&unit.path),
                "cleanup-check",
            ) {
                Ok(units) => {
                    result.members = units
                        .iter()
                        .filter_map(|u| u.cargo_group())
                        .flat_map(|g| g.members.iter().map(|m| m.path.clone()))
                        .collect();
                    result.recovery = units.first().map(|u| u.recovery().to_string());
                    result.warnings = units
                        .iter()
                        .flat_map(|u| u.warnings().iter().cloned())
                        .collect();
                    result.check_status = "ready_for_review".to_string();
                    result.reason_code = "checks_passed".to_string();
                    result.message = "Checked layout, Cargo lock, member contents and fingerprint evidence. Nothing here establishes that nothing needs it. Review exact members and rebuilding consequences before deleting; swamp does not delete anything itself.".into();
                    result.next_action = "review_members".to_string();
                    result.allocated_bytes = units.iter().map(|u| u.bytes()).sum();
                }
                Err(error) => {
                    result.check_status = "blocked".to_string();
                    let message = error.to_string();
                    result.reason_code =
                        if message.contains("Cargo build busy or lock unavailable") {
                            "lock_unavailable"
                        } else if message.contains("no established Cargo build lock") {
                            "missing_lock"
                        } else {
                            "review_refused"
                        }
                        .to_string();
                    result.message = message;
                    if result.reason_code == "lock_unavailable" {
                        result.message.push_str(" A build may hold the lock, or locking may be unavailable. Wait for builds to finish, then retry this exact selection. Nothing changed.");
                        result.next_action = "retry_after_builds".to_string();
                        result.next_command = vec![
                            "swamp".into(),
                            "cleanup-check".into(),
                            report.root.display().to_string(),
                            "--path".into(),
                            unit.path.display().to_string(),
                        ];
                    } else {
                        result.next_action = "inspect".to_string();
                    }
                }
            }
        }
        result.elapsed_ms = start.elapsed().as_millis();
        results.push(result);
    }
    Ok(results)
}

/// Whether this module's guidance and cleanup groups speak for a unit
/// in `role` -- the Cargo layout vocabulary this module was written
/// against (`Profile`, `Incremental`, `TestExecutable`, ...).
///
/// Views decide how to present a container by the *roles* its units
/// carry, never by comparing an adapter id: a container whose units use
/// the ecosystem-neutral roles (`Output`, `InstalledDependencies`,
/// `SharedStoreEntry`, ...) is presented as neutral family groups, and
/// this module's vocabulary ("rebuild before rerunning") is not applied
/// to it.
pub fn speaks_for(role: &ArtifactRole) -> bool {
    matches!(
        role,
        ArtifactRole::Profile
            | ArtifactRole::Dependency
            | ArtifactRole::TestExecutable
            | ArtifactRole::Example
            | ArtifactRole::BuildScriptOutput
            | ArtifactRole::Incremental
            | ArtifactRole::FinalOutput
            | ArtifactRole::CompanionMetadata
    )
}

pub fn candidate(unit: &NestedArtifact) -> bool {
    if !unit.coverage.complete || !unit.coverage.supported {
        return false;
    }
    let parent = unit
        .path
        .parent()
        .and_then(Path::file_name)
        .and_then(|n| n.to_str());
    if unit.is_dir {
        matches!(
            (&unit.role, parent),
            (ArtifactRole::Incremental, Some("incremental"))
                | (ArtifactRole::BuildScriptOutput, Some("build"))
        )
    } else {
        matches!(
            unit.role,
            ArtifactRole::TestExecutable | ArtifactRole::Example
        )
    }
}

fn regular(path: &Path) -> Result<RegularFile> {
    Ok(RegularFile::open_nofollow(path)?)
}

fn snapshot(path: &Path) -> Result<Member> {
    snapshot_at(path, 0, &mut 0)
}

/// Authorization identity for a selected path. Link counts and allocation
/// accounting are observations, not safety facts: aliases outside the
/// selection may change without changing the selected content or membership.
fn snapshot_at(path: &Path, depth: usize, visited: &mut usize) -> Result<Member> {
    *visited += 1;
    if depth > 128 || *visited > 100_000 {
        bail!("cleanup review exceeds depth/member limit; inspection-only");
    }
    let canonical = fs::canonicalize(path)?;
    if canonical != path {
        bail!("symlink or noncanonical member: {}", path.display());
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        let mut paths = fs::read_dir(path)?
            .map(|e| e.map(|e| e.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        paths.sort();
        let mut hash = blake3::Hasher::new();
        let mut bytes = 0;
        let mut hardlink_members = 0;
        for p in paths {
            let m = snapshot_at(&p, depth + 1, visited)?;
            bytes += m.bytes;
            hardlink_members += m.hardlink_members;
            // The safety identity deliberately excludes allocation/link
            // accounting. External aliases may appear or disappear without
            // changing this selected group's membership or content.
            hash.update(&serde_json::to_vec(&(
                &m.path, m.device, m.inode, &m.digest,
            ))?);
        }
        return Ok(Member {
            path: path.into(),
            device: metadata.dev(),
            inode: metadata.ino(),
            // Directory link counts describe `.`/`..`, not aliases of the
            // directory's contents. Child file counts are aggregated above.
            nlink: 1,
            hardlink_members,
            bytes,
            digest: hash.finalize().to_hex().to_string(),
        });
    }
    let mut file = regular(path)?;
    let before = file.metadata()?;
    let digest = file.digest()?;
    let after = file.metadata()?;
    if (
        before.len(),
        before.mtime(),
        before.mtime_nsec(),
        before.ctime(),
        before.ctime_nsec(),
    ) != (
        after.len(),
        after.mtime(),
        after.mtime_nsec(),
        after.ctime(),
        after.ctime_nsec(),
    ) {
        bail!("member changed during review");
    }
    Ok(Member {
        path: path.into(),
        device: before.dev(),
        inode: before.ino(),
        nlink: before.nlink(),
        hardlink_members: u64::from(before.nlink() > 1),
        bytes: before.blocks() * 512,
        digest,
    })
}

fn local_filesystem(path: &Path) -> Result<()> {
    if !fs::sys::volume_info(path)?.is_local() {
        bail!(
            "Cargo cleanup requires a supported local filesystem; network/unknown mounts are inspection-only"
        );
    }
    Ok(())
}

fn locks(profile: &Path) -> Result<Vec<PathBuf>> {
    local_filesystem(profile)?;
    let mut paths = Vec::new();
    for name in [".cargo-lock", ".cargo-build-lock"] {
        let p = profile.join(name);
        match fs::symlink_metadata(&p) {
            Ok(m) if m.is_file() && !m.file_type().is_symlink() => paths.push(p),
            Ok(_) => bail!("invalid Cargo lock: {}", p.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    if paths.is_empty() {
        bail!("no established Cargo build lock; inspection-only");
    }
    Ok(paths)
}

struct HeldLocks(Vec<RegularFile>);

impl Drop for HeldLocks {
    fn drop(&mut self) {
        // Closing alone leaves flock held when another thread's fork temporarily
        // inherits the open file description. End our critical section explicitly.
        for file in &self.0 {
            let _ = file.unlock();
        }
    }
}

fn acquire(paths: &[PathBuf]) -> Result<HeldLocks> {
    let mut held = HeldLocks(Vec::new());
    for p in paths {
        let file = regular(p)?;
        file.try_lock().map_err(|e| {
            anyhow::anyhow!(
                "Cargo build busy or lock unavailable at {}: {e}",
                p.display()
            )
        })?;
        held.0.push(file);
        let file = held.0.last().unwrap();
        let fd = file.metadata()?;
        let path = fs::symlink_metadata(p)?;
        if (fd.dev(), fd.ino()) != (path.dev(), path.ino()) {
            bail!("Cargo lock replaced");
        }
    }
    Ok(held)
}

#[test]
fn explicit_unlock_releases_even_with_a_duplicated_description() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(".cargo-lock");
    std::fs::write(&path, b"").unwrap();
    let held = acquire(std::slice::from_ref(&path)).unwrap();
    let duplicate = held.0[0].try_clone().unwrap();
    assert!(
        acquire(std::slice::from_ref(&path)).is_err(),
        "live guard must exclude another holder"
    );
    drop(held);
    let reacquired = acquire(&[path]).expect("duplicate must not extend the critical section");
    drop(reacquired);
    drop(duplicate);
}

/// Select an evidenced test/example executable with its companions, or one
/// incremental/build-script directory. Shared dependencies, whole profiles, unknown layouts,
/// and files without a trustworthy role are intentionally not action groups.
pub fn propose(units: &[NestedArtifact], selected: &Path, container: &Path) -> Result<CargoGroup> {
    let unit = units
        .iter()
        .find(|u| u.path == selected)
        .context("no nested artifact at selection")?;
    if !unit.coverage.complete || !unit.coverage.supported {
        bail!("incomplete/unsupported Cargo coverage");
    }
    if !candidate(unit) {
        bail!(
            "{} Nothing changed. Run `swamp ui` to inspect cleanup groups and review supported actions.",
            guidance(unit).message
        );
    }
    let directory_group = unit.is_dir
        && matches!(
            unit.role,
            ArtifactRole::Incremental | ArtifactRole::BuildScriptOutput
        );
    if !directory_group
        && (!matches!(
            unit.role,
            ArtifactRole::TestExecutable | ArtifactRole::Example
        ) || unit.is_dir)
    {
        bail!("this Cargo role is inspection-only; select a test or example executable");
    }
    let relative = crate::scope::relative_to(selected, container)
        .context("selection is not inside its container")?;
    let canonical_container = fs::canonicalize(container)?;
    let canonical_selected = fs::canonicalize(selected)?;
    if canonical_container.join(relative) != canonical_selected
        || fs::symlink_metadata(selected)?.file_type().is_symlink()
    {
        bail!("selection crosses a symlink; propose again");
    }
    let container = canonical_container;
    let selected = canonical_selected;
    let rel = crate::scope::relative_to(&selected, &container)
        .context("selection is not inside its container")?;
    let count = rel.components().count();
    if !(count == 3 || count == 4) {
        bail!("unsupported Cargo executable layout");
    }
    let dir = selected.parent().context("missing executable parent")?;
    let dirname = dir.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if !(matches!(dirname, "deps" | "examples") && !directory_group
        || matches!(dirname, "incremental" | "build") && directory_group)
    {
        bail!("unsupported Cargo executable directory");
    }
    let meta = fs::symlink_metadata(&selected)?;
    if !directory_group && meta.mode() & 0o111 == 0 {
        bail!("not an executable");
    }
    let profile = dir.parent().context("missing profile")?.to_path_buf();
    let lock_paths = locks(&profile)?;
    let _held = acquire(&lock_paths)?;
    let evidence: Vec<_> = unit
        .producer_evidence
        .iter()
        .filter(|e| e.source == "cargo-fingerprint")
        .map(|e| snapshot(Path::new(&e.detail)))
        .collect::<Result<_>>()?;
    let evidence_paths: Vec<_> = evidence.iter().map(|e| e.path.clone()).collect();
    if crate::cargo_artifacts::reviewed_role(&container, &selected, &evidence_paths)?.0 != unit.role
    {
        bail!("Cargo role/evidence changed; refresh before proposing");
    }
    let mut paths = vec![selected.clone()];
    let dep = selected.with_extension("d");
    if !directory_group && dep != selected && fs::exists(&dep) {
        paths.push(dep);
    }
    let dsym = selected.with_extension("dSYM");
    if !directory_group && fs::exists(&dsym) {
        paths.push(dsym);
    }
    let members: Vec<_> = paths.iter().map(|p| snapshot(p)).collect::<Result<_>>()?;
    let allocated_bytes = members.iter().map(|m| m.bytes).sum();
    let hardlink_members = members.iter().map(|m| m.hardlink_members).sum();
    Ok(CargoGroup {
        container,
        selected,
        profile,
        role: unit.role.clone(),
        members,
        lock_paths,
        evidence,
        allocated_bytes,
        reclaimable_bytes: None,
        hardlink_members,
        shared_storage: hardlink_members > 0,
    })
}

/// Moves a Cargo group's exact member list into one Trash envelope. Holds
/// the group's advisory Cargo lock for the duration of the move so a
/// concurrent `cargo build` does not write into a directory mid-rename;
/// this is a mutual-exclusion measure, not a "did anything change"
/// refusal -- swamp reports the group, the human decided to trash it, and
/// the only way this refuses is an OS-level rename failure (a member
/// already gone, a name collision). Failure rolls back completed moves
/// where possible, with the recovery envelope retained on disk.
pub(crate) fn move_group(group: &CargoGroup, trash: &Path) -> Result<PathBuf> {
    let _held = acquire(&group.lock_paths)?;
    let mut envelope = fs::destroy::Envelope::open(
        &group.selected,
        trash,
        &format!("swamp-cargo-{}", crate::entities::new_id()),
        Some(group.members[0].device),
    )?;
    envelope.write_manifest(group)?;
    for (i, member) in group.members.iter().enumerate() {
        let name = format!(
            "{i}-{}",
            member
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "member".to_string())
        );
        if let Err(e) = envelope.move_member(&member.path, &name) {
            let failures = envelope.roll_back();
            bail!(
                "Cargo move failed: {e}; recovery manifest {}; rollback errors: {:?}",
                envelope.path().display(),
                failures
            );
        }
    }
    let dest = envelope.path().to_path_buf();
    Ok(dest)
}

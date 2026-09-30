//! Writes to swamp's **own** files. The history tables are Parquet
//! ([`super::columns`]); everything else swamp persists is one of the
//! variants below, and there is no other way to put bytes on disk.
//!
//! That makes two guardrails types rather than audits:
//!
//! * `.oh/guardrails/json-persistence-is-allowlisted.md`: JSON reaches
//!   disk only through [`write_json`], and [`JsonFile`] *is* the
//!   allow-list -- a new control file is a new variant here, reviewed
//!   where every other one is. `serde_json::to_string(..)` followed by a
//!   raw `std::fs::write` is a gate violation (audit + clippy), and there
//!   is no `write_bytes(path, ..)` to launder it through.
//! * `.oh/guardrails/store-data-is-parquet-not-json-sidecars.md`: there
//!   is no per-unit, per-path or free-named JSON file; every variant's
//!   file name is fixed (or, for plans and cached reports, built from a
//!   validated id under a fixed prefix).
//!
//! Every write is atomic (sibling temp file + `fsync` + rename + parent
//! directory `fsync`) except [`append_line`], the ledger's append-only
//! discipline.
//!
//! **Locations are types too** (re-review 5, finding 4). A file here is a
//! fixed name inside a [`StoreDir`] -- never a caller's path -- and the
//! three files that live outside a store (the ledger override, the
//! observation log, the LaunchAgent plist) are resolved *here*, from the
//! environment, or validated by name. There is no `create_dir_all(path)`,
//! `list(path)` or `remove(path)` a caller can point anywhere.

use serde::Serialize;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// A swamp state directory: every [`JsonFile`], [`TextFile`] and lock
/// lives at a fixed name inside one.
///
/// Built from the resolved swamp dir ([`StoreDir::resolved`]: what the
/// CLI and TUI use), or -- for the core entry points that take a store
/// `&Path`, which tests hand a temp dir -- [`StoreDir::at`], which the
/// gate audit allows only in the store modules. `at` refuses a relative
/// path, a symlink, and anything that exists and is not a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreDir(PathBuf);

impl StoreDir {
    /// The store-format generation. Bumped only for a change v0.7.x could
    /// not read around: an installed older swamp resets the store on any
    /// marker it does not know, so two installs sharing one store (a
    /// scheduled observe on one, the terminal on the other) would reset it
    /// on every alternation. v0.8.0's additions are therefore new sibling
    /// tables (`unit_meta`, `unit_children`) that older versions ignore,
    /// and the marker stays 2.
    const FORMAT: &'static str = "2\n";
    /// `$SWAMP_DIR`, else `$HOME/.local/share/swamp`: the one resolver.
    pub fn resolved() -> StoreDir {
        if let Some(dir) = std::env::var_os("SWAMP_DIR") {
            return StoreDir(PathBuf::from(dir));
        }
        let home = std::env::var_os("HOME").unwrap_or_else(|| ".".into());
        StoreDir(PathBuf::from(home).join(".local/share/swamp"))
    }

    /// A store at `dir`. Refused unless `dir` is absolute and, when
    /// something is there, a real directory (not a symlink to one).
    pub fn at(dir: &Path) -> io::Result<StoreDir> {
        if !dir.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not an absolute store directory", dir.display()),
            ));
        }
        match std::fs::symlink_metadata(dir) {
            Ok(m) if m.is_dir() => Ok(StoreDir(dir.to_path_buf())),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{} is not a directory swamp can keep state in",
                    dir.display()
                ),
            )),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(StoreDir(dir.to_path_buf())),
            Err(e) => Err(e),
        }
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Creates the directory (and its parents) if it is missing.
    pub fn create(&self) -> io::Result<()> {
        std::fs::create_dir_all(&self.0)
    }

    /// Serializes observation writers that may update the same store.
    /// The lock is advisory and held for the full observing pipeline.
    pub fn lock_observation_writes(&self) -> io::Result<super::continuity::FileLock> {
        self.create()?;
        let path = self.0.join("store-write.lock");
        loop {
            if let Some(lock) = super::continuity::try_lock(&path, true)? {
                return Ok(lock);
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }

    /// Serializes read-modify-write edits of `config.toml` (`swamp config
    /// add-root` / `remove-root`, the first-run answer) so two of them
    /// racing never lose one's change. Waits up to ten seconds; the lock is
    /// advisory and held only for the edit, never for an observation.
    pub fn lock_config_edits(&self) -> io::Result<super::continuity::FileLock> {
        self.create()?;
        let path = self.0.join("config.lock");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(lock) = super::continuity::try_lock(&path, true)? {
                return Ok(lock);
            }
            if std::time::Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "config.lock is held by another swamp process (waited 10 s): {}",
                        path.display()
                    ),
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn marker_state(&self) -> io::Result<MarkerState> {
        let marker = read_housekeeping_marker(&self.0.join("housekeeping.version"))?;
        Ok(match marker {
            None => MarkerState::Absent,
            Some(m) if m == Self::FORMAT => MarkerState::Current,
            Some(m) => match m.trim().parse::<u32>() {
                Ok(n) if n > Self::FORMAT.trim().parse::<u32>().unwrap_or(0) => MarkerState::Newer,
                // Older, or a marker no swamp wrote: nothing to read.
                _ => MarkerState::Older,
            },
        })
    }

    /// Whether derived tables can be read: the current generation, or a
    /// newer one (read-only: a report may read what it can, and never
    /// modifies it). The marker read is bounded and never follows links or
    /// blocks on a FIFO.
    pub fn has_current_format(&self) -> io::Result<bool> {
        Ok(matches!(
            self.marker_state()?,
            MarkerState::Current | MarkerState::Newer
        ))
    }

    /// True for an explicit marker of an OLDER (or unrecognized)
    /// generation. A missing marker is the ordinary first-use case once
    /// recognized caches reset; a newer marker is not incompatible, it is
    /// somebody else's and is left alone.
    pub fn has_incompatible_marker(&self) -> io::Result<bool> {
        Ok(self.marker_state()? == MarkerState::Older)
    }

    /// True when a newer swamp wrote this store. This swamp reads what it
    /// can and does not modify it: no reset, no derived-table write.
    pub fn is_newer_generation(&self) -> io::Result<bool> {
        Ok(self.marker_state()? == MarkerState::Newer)
    }

    /// Resets recognized Swamp-derived state for an absent/older generation.
    /// Call under the observation writer lock, before reading any derived
    /// tables. Durable human intent, action history, live enrichment, and
    /// unknown user files are deliberately outside this ownership set.
    pub fn reset_incompatible_format(&self) -> io::Result<bool> {
        // Only an older (or absent, or unrecognized) generation is reset. A
        // newer one is never touched.
        if matches!(
            self.marker_state()?,
            MarkerState::Current | MarkerState::Newer
        ) {
            return Ok(false);
        }
        self.create()?;
        let entries = std::fs::read_dir(&self.0)?;
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let path = entry.path();
            let ty = entry.file_type()?;
            if ty.is_dir() && is_numeric(&name) {
                clean_derived_directory(&path)?;
                remove_dir_if_empty(&path)?;
            } else if ty.is_dir() && name == "external" {
                clean_external_generation(&path)?;
                remove_dir_if_empty(&path)?;
            } else if ty.is_dir() && name == "associations" {
                clean_association_generation(&path)?;
                remove_dir_if_empty(&path)?;
            } else if is_reset_owned_file(&name) && (ty.is_file() || ty.is_symlink()) {
                std::fs::remove_file(path)?;
            } else if name == "plans" && ty.is_dir() {
                clean_legacy_plans_dir(&path)?;
                remove_dir_if_empty(&path)?;
            }
        }
        Ok(true)
    }

    /// Publish the generation only after the complete observation pipeline
    /// has persisted successfully. Keep this separate from reset so errors
    /// are retryable and cannot bless a partial scan.
    pub fn mark_current_format(&self) -> io::Result<()> {
        if self.marker_state()? == MarkerState::Newer {
            return Ok(());
        }
        write_atomic(
            &self.0.join("housekeeping.version"),
            Self::FORMAT.as_bytes(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkerState {
    Absent,
    Older,
    Current,
    Newer,
}

/// The volume pass's own lock and the ledger's quarantine: additive, so
/// they sit apart from the format and reset logic above.
impl StoreDir {
    /// Takes `volume-pass.lock` without blocking: `Ok(None)` when another
    /// volume pass holds it. Separate from the observation locks, so a
    /// stuck pass can never make an observe report "another observation is
    /// running". Transient: it is a lock file, never data.
    pub fn try_lock_volume_pass(&self) -> io::Result<Option<super::continuity::FileLock>> {
        self.create()?;
        super::continuity::try_lock(&self.0.join("volume-pass.lock"), true)
    }

    /// Removes `volume_ledger*.parquet.corrupt-<stamp>` files whose stamp is
    /// older than `max_age_secs` before `now`. Only those names.
    pub fn remove_stale_quarantine(&self, now: u64, max_age_secs: u64) -> io::Result<usize> {
        let mut removed = 0;
        for entry in std::fs::read_dir(&self.0)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some((base, stamp)) = name.rsplit_once(".corrupt-") else {
                continue;
            };
            if !matches!(base, "volume_ledger.parquet" | "volume_ledger_meta.parquet") {
                continue;
            }
            let Ok(stamp) = stamp.parse::<u64>() else {
                continue;
            };
            if now.saturating_sub(stamp) > max_age_secs && entry.file_type()?.is_file() {
                std::fs::remove_file(entry.path())?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Moves a file swamp wrote inside this store aside as
    /// `<name>.corrupt-<stamp>` (never over an existing file), so a
    /// ledger that cannot be read stops blocking the next pass and is
    /// still there to look at. `name` is a plain file name, never a path.
    pub fn quarantine_file(&self, name: &str, stamp: u64) -> io::Result<()> {
        if name.contains('/') || name.contains("..") || name.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a plain file name",
            ));
        }
        let from = self.0.join(name);
        let to = self.0.join(format!("{name}.corrupt-{stamp}"));
        if !to.exists() && from.exists() {
            std::fs::rename(from, to)?;
        }
        Ok(())
    }
}

fn read_housekeeping_marker(path: &Path) -> io::Result<Option<String>> {
    let Some(bytes) = read_small_regular_file(path, 64)? else {
        return Ok(None);
    };
    Ok(String::from_utf8(bytes).ok())
}

fn read_small_regular_file(path: &Path, limit: usize) -> io::Result<Option<Vec<u8>>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) if is_symlink_open_error(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() {
        return Ok(None);
    }
    let mut bytes = Vec::with_capacity(limit.min(256));
    file.take((limit as u64) + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Ok(None);
    }
    Ok(Some(bytes))
}

#[cfg(unix)]
fn is_symlink_open_error(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::ELOOP)
}

#[cfg(not(unix))]
fn is_symlink_open_error(_error: &io::Error) -> bool {
    false
}

fn is_numeric(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit())
}

fn is_reset_owned_file(name: &str) -> bool {
    is_derived_table(name)
        || matches!(
            name,
            "unowned.json"
                | "fsevents.json"
                | "topology.json"
                | "docker_facts.json"
                | "last_run.json"
                | "grants.json"
                | "ledger.jsonl"
                | "scope.json"
                | "external_consumers.json"
                | "toolchain_declarations_cache.json"
                | "dependency_identities_cache.json"
        )
        || is_legacy_report(name)
}

fn is_derived_table(name: &str) -> bool {
    matches!(
        name.strip_suffix(".parquet"),
        Some(
            "cursors" | "current" | "dirs" | "files" | "unowned" | "unowned_lists"
        | "unowned_evidence" | "docker_unowned" | "docker_unowned_lists"
        | "docker_unowned_evidence" | "dir_tracks" | "topology" | "volume_stamps"
        | "build_stores" | "xcode_derived_data" | "declarations" | "dependency_identities"
        | "git_signals" | "git_signals_values" | "cargo_replay_cache"
        | "cargo_replay_cache_lists" | "cargo_replay_cache_evidence" | "cargo_replay_cache_meta"
        | "scope" | "scope_values" | "scope_roots" | "scope_root_reasons" | "docker_meta"
        | "docker_images" | "docker_build_cache" | "docker_volumes" | "docker_builders"
        | "docker_values" | "docker_containers" | "runs" | "coverage" | "projects"
        | "worktrees" | "worktree_facts" | "artifact_shape" | "artifact_shape_lists"
        | "external_units" | "unit_children" | "unit_meta" | "manager_facts" | "agent_units"
        | "agent_unit_members" | "unit_consumers"
        | "agent_identifications" | "agent_containers" | "nested_artifacts"
        | "nested_artifact_lists" | "nested_artifact_evidence" | "evidence"
        // Historical rendered caches and removed derived views.
        | "report_rows" | "summary" | "series" | "unowned_summary" | "worktree_entries"
        | "github_enrichment"
        )
    )
}

fn is_legacy_report(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("last_report-") else {
        return false;
    };
    [".json", ".json.zst"].iter().any(|suffix| {
        rest.strip_suffix(suffix)
            .is_some_and(|key| key.len() == 16 && key.bytes().all(|b| b.is_ascii_hexdigit()))
    })
}

fn clean_derived_directory(dir: &Path) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let ty = entry.file_type()?;
        let path = entry.path();
        if (ty.is_file() || ty.is_symlink())
            && (is_reset_owned_file(&name) || is_derived_table(&name))
        {
            std::fs::remove_file(path)?;
        } else if ty.is_dir()
            && matches!(
                name.as_ref(),
                "deltas" | "dirs_deltas" | "files_deltas" | "external"
            )
        {
            if name == "external" {
                clean_external_generation(&path)?;
            } else {
                clean_delta_generation(&path)?;
            }
        }
    }
    Ok(())
}

fn clean_delta_generation(dir: &Path) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let ty = entry.file_type()?;
        if (ty.is_file() || ty.is_symlink()) && is_delta_cache_name(&name) {
            std::fs::remove_file(entry.path())?;
        }
    }
    remove_dir_if_empty(dir)?;
    Ok(())
}

fn clean_external_generation(dir: &Path) -> io::Result<()> {
    remove_known_file(&dir.join("current.parquet"))?;
    let volume_stamps = dir.join("volume_stamps.parquet");
    remove_known_file(&volume_stamps)?;
    remove_known_file(&dir.join("overlap_marks.parquet"))?;
    let deltas = dir.join("deltas");
    if std::fs::symlink_metadata(&deltas).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink()) {
        clean_delta_generation(&deltas)?;
    }
    // R18's pre-split folded cache was a single external/folded.parquet.
    remove_known_file(&dir.join("folded.parquet"))?;
    let folded = dir.join("folded");
    if std::fs::symlink_metadata(&folded).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink()) {
        for entry in std::fs::read_dir(&folded)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let ty = entry.file_type()?;
            if (ty.is_file() || ty.is_symlink()) && is_folded_cache_name(&name) {
                std::fs::remove_file(entry.path())?;
            }
        }
        remove_dir_if_empty(&folded)?;
    }
    remove_dir_if_empty(dir)?;
    Ok(())
}

fn clean_association_generation(dir: &Path) -> io::Result<()> {
    for name in [
        "declarations.parquet",
        "dependency_identities.parquet",
        "xcode_derived_data.parquet",
        "agent_identifications.parquet",
        "agent_containers.parquet",
        "build_stores.parquet",
    ] {
        remove_known_file(&dir.join(name))?;
    }
    // external_consumers.parquet is a user-declared association (see
    // ConsumerTable), so unlike the fingerprinted caches it survives reset.
    remove_dir_if_empty(dir)
}

fn is_folded_cache_name(name: &str) -> bool {
    let Some(id) = name.strip_suffix(".parquet") else {
        return false;
    };
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_delta_cache_name(name: &str) -> bool {
    let Some(sequence) = name
        .strip_prefix("delta-")
        .and_then(|name| name.strip_suffix(".parquet"))
    else {
        return false;
    };
    sequence.len() == 12 && sequence.bytes().all(|b| b.is_ascii_digit())
}

fn remove_known_file(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() || meta.file_type().is_symlink() => std::fs::remove_file(path),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remove_dir_if_empty(path: &Path) -> io::Result<()> {
    match std::fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        // Windows reports PermissionDenied for a non-empty directory.
        #[cfg(windows)]
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Ok(()),
        Err(error) => Err(error),
    }
}

fn clean_legacy_plans_dir(dir: &Path) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let ty = entry.file_type()?;
        if (ty.is_file() || ty.is_symlink()) && is_uuid_json(&name) {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn is_uuid_json(name: &str) -> bool {
    let Some(id) = name.strip_suffix(".json") else {
        return false;
    };
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

/// The one JSON file swamp persists: the TUI's remembered filter and
/// view, `<store>/ui_state.json` (tiny; `store-data-is-parquet-not-json-
/// sidecars`). Every other store file is a Parquet table, `config.toml`,
/// a lock, or the scheduled-observation log.
#[derive(Debug, Clone, Copy)]
pub enum JsonFile<'a> {
    UiState { store: &'a StoreDir },
}

impl JsonFile<'_> {
    /// Where the file lives.
    pub fn path(&self) -> io::Result<PathBuf> {
        Ok(match *self {
            JsonFile::UiState { store } => store.0.join("ui_state.json"),
        })
    }
}

#[cfg(test)]
mod housekeeping_tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStrExt;
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    use tempfile::tempdir;

    #[test]
    fn one_time_cleanup_removes_only_retired_owned_payloads() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let store = StoreDir::at(root).unwrap();
        let volume = root.join("12345");
        fs::create_dir(&volume).unwrap();
        let plans = root.join("plans");
        fs::create_dir(&plans).unwrap();
        for name in [
            "unowned.json",
            "fsevents.json",
            "topology.json",
            "docker_facts.json",
            "last_run.json",
            "grants.json",
            "ledger.jsonl",
            "last_report-0123456789abcdef.json",
            "last_report-0123456789abcdef.json.zst",
        ] {
            fs::write(root.join(name), b"legacy").unwrap();
            fs::write(volume.join(name), b"legacy").unwrap();
        }
        fs::write(
            plans.join("01234567-89ab-cdef-0123-456789abcdef.json"),
            b"plan",
        )
        .unwrap();
        fs::write(plans.join("leave-me.txt"), b"user file").unwrap();

        for name in [
            "config.toml",
            "protect.parquet",
            "notes.parquet",
            "enrich.parquet",
            "agent_protect.json",
        ] {
            fs::write(root.join(name), b"current data").unwrap();
        }
        fs::write(volume.join("current.parquet"), b"compatible history").unwrap();
        fs::create_dir(volume.join("deltas")).unwrap();
        fs::write(volume.join("deltas/delta-000000000001.parquet"), b"history").unwrap();
        let empty_generation = root.join("67890");
        fs::create_dir(&empty_generation).unwrap();
        fs::write(empty_generation.join("current.parquet"), b"retired").unwrap();
        fs::write(root.join("last_report-not-a-key.json"), b"unrecognized").unwrap();

        let outside = tmp.path().join("outside-sentinel");
        fs::write(&outside, b"must survive").unwrap();
        #[cfg(unix)]
        fs::remove_file(volume.join("unowned.json")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, volume.join("unowned.json")).unwrap();

        assert!(store.reset_incompatible_format().unwrap());
        for name in [
            "unowned.json",
            "fsevents.json",
            "topology.json",
            "docker_facts.json",
            "last_run.json",
            "grants.json",
            "ledger.jsonl",
            "last_report-0123456789abcdef.json",
            "last_report-0123456789abcdef.json.zst",
        ] {
            assert!(!root.join(name).exists());
            assert!(fs::symlink_metadata(volume.join(name)).is_err());
        }
        assert!(
            !plans
                .join("01234567-89ab-cdef-0123-456789abcdef.json")
                .exists()
        );
        assert!(plans.join("leave-me.txt").exists());
        for name in [
            "config.toml",
            "protect.parquet",
            "notes.parquet",
            "enrich.parquet",
            "agent_protect.json",
        ] {
            assert!(root.join(name).exists(), "removed retained file {name}");
        }
        assert!(!volume.exists(), "empty numeric generation remains");
        assert!(
            !empty_generation.exists(),
            "empty numeric generation remains"
        );
        assert!(root.join("last_report-not-a-key.json").exists());
        assert_eq!(fs::read(outside).unwrap(), b"must survive");

        fs::write(root.join("unowned.json"), b"later file").unwrap();
        store.mark_current_format().unwrap();
        assert!(!store.reset_incompatible_format().unwrap());
        assert!(root.join("unowned.json").exists());
        fs::write(root.join("housekeeping.version"), b"0\n").unwrap();
        assert!(store.reset_incompatible_format().unwrap());
        assert!(!root.join("unowned.json").exists());
        assert_eq!(
            fs::read_to_string(root.join("housekeeping.version")).unwrap(),
            "0\n"
        );

        #[cfg(unix)]
        {
            let outside_marker = root.join("outside-marker");
            fs::write(&outside_marker, "1\n").unwrap();
            fs::remove_file(root.join("housekeeping.version")).unwrap();
            std::os::unix::fs::symlink(&outside_marker, root.join("housekeeping.version")).unwrap();
            fs::write(root.join("unowned.json"), b"legacy").unwrap();
            assert!(store.reset_incompatible_format().unwrap());
            assert!(!root.join("unowned.json").exists());
            assert_eq!(fs::read_to_string(outside_marker).unwrap(), "1\n");
        }
    }

    #[test]
    fn missing_generation_resets_known_old_parquet_but_preserves_durable_and_unknown_files() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let store = StoreDir::at(root).unwrap();
        let volume = root.join("12345");
        fs::create_dir(&volume).unwrap();
        for path in [
            root.join("report_rows.parquet"),
            root.join("summary.parquet"),
            volume.join("report_rows.parquet"),
            volume.join("current.parquet"),
            volume.join("scope.json"),
            volume.join("external_consumers.json"),
        ] {
            fs::write(path, b"old generation").unwrap();
        }
        let external = root.join("external");
        fs::create_dir_all(external.join("folded")).unwrap();
        fs::create_dir_all(external.join("deltas")).unwrap();
        fs::write(external.join("current.parquet"), b"old").unwrap();
        fs::write(external.join("volume_stamps.parquet"), b"old").unwrap();
        fs::write(external.join("deltas/delta-000000000001.parquet"), b"old").unwrap();
        fs::write(external.join("folded.parquet"), b"old").unwrap();
        fs::write(
            external
                .join("folded")
                .join(format!("{}.parquet", "a".repeat(64))),
            b"old",
        )
        .unwrap();
        fs::write(
            external.join("folded").join("user-data.parquet"),
            b"unknown",
        )
        .unwrap();
        let associations = root.join("associations");
        fs::create_dir(&associations).unwrap();
        for name in [
            "declarations.parquet",
            "dependency_identities.parquet",
            "xcode_derived_data.parquet",
            "agent_identifications.parquet",
            "agent_containers.parquet",
            "build_stores.parquet",
            "external_consumers.parquet",
        ] {
            fs::write(associations.join(name), b"old").unwrap();
        }
        for path in [
            root.join("protect.parquet"),
            root.join("notes.parquet"),
            root.join("ledger.parquet"),
            root.join("external_consumers.parquet"),
            root.join("config.toml"),
            root.join("unrelated.bin"),
            volume.join("enrich.parquet"),
            external.join("folded/user-data.parquet"),
        ] {
            fs::write(path, b"preserve").unwrap();
        }
        let lock = store.lock_observation_writes().unwrap();
        let lock_inode = fs::metadata(root.join("store-write.lock")).unwrap();
        assert!(store.reset_incompatible_format().unwrap());
        assert!(!root.join("report_rows.parquet").exists());
        assert!(!root.join("summary.parquet").exists());
        assert!(!volume.join("report_rows.parquet").exists());
        assert!(!volume.join("current.parquet").exists());
        assert!(!volume.join("scope.json").exists());
        assert!(!volume.join("external_consumers.json").exists());
        assert!(!external.join("current.parquet").exists());
        assert!(!external.join("volume_stamps.parquet").exists());
        assert!(!external.join("deltas/delta-000000000001.parquet").exists());
        assert!(!external.join("folded.parquet").exists());
        assert!(
            !external
                .join("folded")
                .join(format!("{}.parquet", "a".repeat(64)))
                .exists()
        );
        assert!(!associations.join("declarations.parquet").exists());
        assert!(!associations.join("dependency_identities.parquet").exists());
        for name in [
            "xcode_derived_data.parquet",
            "agent_identifications.parquet",
            "agent_containers.parquet",
            "build_stores.parquet",
        ] {
            assert!(
                !associations.join(name).exists(),
                "old association table survived: {name}"
            );
        }
        assert_eq!(
            fs::read(associations.join("external_consumers.parquet")).unwrap(),
            b"old",
            "user-declared consumer intent must survive"
        );
        for path in [
            root.join("protect.parquet"),
            root.join("notes.parquet"),
            root.join("ledger.parquet"),
            root.join("external_consumers.parquet"),
            root.join("config.toml"),
            root.join("unrelated.bin"),
            volume.join("enrich.parquet"),
        ] {
            assert_eq!(fs::read(path).unwrap(), b"preserve");
        }
        assert_eq!(
            fs::read(external.join("folded/user-data.parquet")).unwrap(),
            b"preserve"
        );
        assert_eq!(
            fs::metadata(root.join("store-write.lock")).unwrap().ino(),
            lock_inode.ino()
        );
        drop(lock);
        assert!(!store.has_current_format().unwrap());
    }

    #[test]
    fn observation_writer_lock_serializes_holders() {
        let tmp = tempdir().unwrap();
        let store = StoreDir::at(tmp.path()).unwrap();
        let first = store.lock_observation_writes().unwrap();
        let other_store = store.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let join = std::thread::spawn(move || {
            let _second = other_store.lock_observation_writes().unwrap();
            tx.send(()).unwrap();
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(100))
                .is_err()
        );
        drop(first);
        rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        join.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn observation_writer_lock_refuses_a_symlinked_lock_file() {
        let tmp = tempdir().unwrap();
        let store = StoreDir::at(tmp.path()).unwrap();
        let outside = tmp.path().join("outside");
        fs::write(&outside, b"sentinel").unwrap();
        std::os::unix::fs::symlink(&outside, tmp.path().join("store-write.lock")).unwrap();
        assert!(store.lock_observation_writes().is_err());
        assert_eq!(fs::read(outside).unwrap(), b"sentinel");
    }

    #[cfg(unix)]
    #[test]
    fn marker_fifo_is_nonblocking_and_replaced_only_when_generation_commits() {
        let tmp = tempdir().unwrap();
        let store = StoreDir::at(tmp.path()).unwrap();
        let marker = tmp.path().join("housekeeping.version");
        let path = std::ffi::CString::new(marker.as_os_str().as_bytes()).unwrap();
        // SAFETY: path is a valid NUL-terminated pathname; mkfifo does not
        // retain the pointer after returning.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        assert!(store.reset_incompatible_format().unwrap());
        assert!(!store.has_current_format().unwrap());
        store.mark_current_format().unwrap();
        assert!(store.has_current_format().unwrap());
        assert!(fs::symlink_metadata(marker).unwrap().is_file());
    }

    #[cfg(unix)]
    #[test]
    fn reset_never_descends_through_numeric_or_plans_symlink_directories() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let outside = root.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("current.parquet"), b"sentinel").unwrap();
        fs::write(
            outside.join("01234567-89ab-cdef-0123-456789abcdef.json"),
            b"sentinel",
        )
        .unwrap();
        std::os::unix::fs::symlink(&outside, root.join("12345")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("plans")).unwrap();
        StoreDir::at(root)
            .unwrap()
            .reset_incompatible_format()
            .unwrap();
        assert_eq!(
            fs::read(outside.join("current.parquet")).unwrap(),
            b"sentinel"
        );
        assert_eq!(
            fs::read(outside.join("01234567-89ab-cdef-0123-456789abcdef.json")).unwrap(),
            b"sentinel"
        );
    }
}

/// Serializes `value` into `file`, atomically.
pub fn write_json<T: Serialize + ?Sized>(file: JsonFile<'_>, value: &T) -> io::Result<()> {
    let path = file.path()?;
    let bytes = match file {
        JsonFile::UiState { .. } => serde_json::to_vec(value)?,
    };
    write_atomic(&path, &bytes)
}

/// Reads `file` back: `Ok(None)` when it does not exist. The bytes are
/// the JSON text.
pub fn read_json_bytes(file: JsonFile<'_>) -> io::Result<Option<Vec<u8>>> {
    let path = file.path()?;
    match std::fs::read(&path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Every non-JSON text file swamp writes.
#[derive(Debug, Clone, Copy)]
pub enum TextFile<'a> {
    /// `<store>/config.toml`, written by `swamp config init`, and edited in
    /// place (under [`StoreDir::lock_config_edits`]) by `swamp config
    /// add-root` / `remove-root` and the first-run answer.
    Config { store: &'a StoreDir },
    /// The scheduled refresh's LaunchAgent plist, where
    /// [`launch_agent_plist`] resolves it.
    LaunchAgent,
    /// `<store>/first-run-asked`: the record that the first-run question
    /// was asked or that the store already held an observation. Not a
    /// derived table, so a store reset leaves it alone (like
    /// `ui_state.json`).
    Onboarded { store: &'a StoreDir },
}

impl TextFile<'_> {
    pub fn path(&self) -> io::Result<PathBuf> {
        match *self {
            TextFile::Config { store } => Ok(store.0.join("config.toml")),
            TextFile::LaunchAgent => launch_agent_plist(),
            TextFile::Onboarded { store } => Ok(store.0.join("first-run-asked")),
        }
    }
}

/// `<LaunchAgents>/<label>.plist`: `$SWAMP_LAUNCH_AGENTS_DIR` (tests), else
/// `~/Library/LaunchAgents`. The only plist swamp writes, removes or hands
/// to `launchctl`.
pub fn launch_agent_plist() -> io::Result<PathBuf> {
    let dir = match std::env::var_os("SWAMP_LAUNCH_AGENTS_DIR") {
        Some(v) => PathBuf::from(v),
        None => PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into()))
            .join("Library/LaunchAgents"),
    };
    Ok(dir.join(format!("{}.plist", crate::schedule::LABEL)))
}

/// Writes `text` into `file`, atomically.
pub fn write_text(file: TextFile<'_>, text: &str) -> io::Result<()> {
    write_atomic(&file.path()?, text.as_bytes())
}

/// Whether the `Onboarded` record exists.
pub fn onboarded_recorded(store: &StoreDir) -> bool {
    TextFile::Onboarded { store }
        .path()
        .map(|p| p.exists())
        .unwrap_or(false)
}

/// Rewrites the user's `config.toml` in place: a symlinked config is
/// followed and its target replaced (the link stays), the file's mode is
/// kept, and the temporary file is created next to the target, so the
/// rename never crosses a filesystem. Fails, leaving the old file, when
/// that temporary file cannot be created.
pub fn write_config_in_place(store: &StoreDir, text: &str) -> io::Result<()> {
    let path = TextFile::Config { store }.path()?;
    let target = match std::fs::symlink_metadata(&path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(&path)?,
        _ => path,
    };
    let perms = std::fs::metadata(&target).ok().map(|m| m.permissions());
    write_atomic_with(&target, text.as_bytes(), perms)
}

/// Removes a file swamp wrote (the LaunchAgent plist on `schedule
/// --off`). Absent is not an error.
pub fn remove_text(file: TextFile<'_>) -> io::Result<()> {
    remove_owned_file(&file.path()?)
}

/// Every append-only log swamp keeps.
/// The action ledger's table: a store's `ledger.parquet`, or where
/// `$SWAMP_LEDGER_PATH` puts it (read here, not handed in). A caller
/// never names the file.
#[derive(Debug, Clone, Copy)]
pub enum LedgerFile<'a> {
    Store(&'a StoreDir),
    Resolved(&'a StoreDir),
}

impl LedgerFile<'_> {
    pub fn path(&self) -> io::Result<PathBuf> {
        Ok(match *self {
            LedgerFile::Store(store) => store.0.join("ledger.parquet"),
            LedgerFile::Resolved(store) => match std::env::var_os("SWAMP_LEDGER_PATH") {
                Some(p) => PathBuf::from(p),
                None => store.0.join("ledger.parquet"),
            },
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub enum LogFile<'a> {
    /// The scheduled observation log: a file named `observe.log`
    /// (`schedule::log_file`), refused under any other name.
    Observations(&'a Path),
}

impl LogFile<'_> {
    /// Where the log lives.
    pub fn path(&self) -> io::Result<PathBuf> {
        Ok(match *self {
            LogFile::Observations(path) => {
                if path.file_name().is_none_or(|n| n != "observe.log") || !path.is_absolute() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{} is not swamp's observation log", path.display()),
                    ));
                }
                path.to_path_buf()
            }
        })
    }
}

/// Appends one line and syncs it.
pub fn append_line(file: LogFile<'_>, line: &str) -> io::Result<()> {
    let path = file.path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(f, "{line}")?;
    f.sync_data()
}

/// The single-flight observation lock, `<store>/observe.lock`.
#[derive(Debug, Clone, Copy)]
pub struct ObserveLock<'a> {
    pub store: &'a StoreDir,
}

impl ObserveLock<'_> {
    pub fn path(&self) -> PathBuf {
        self.store.0.join("observe.lock")
    }

    /// Creates the lock only if nothing is there (`O_EXCL`), holding
    /// `contents` (`pid<TAB>started_at`).
    pub fn create(&self, contents: &str) -> io::Result<()> {
        self.store.create()?;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.path())?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()
    }

    /// Releases (or reclaims a stale) lock.
    pub fn remove(&self) -> io::Result<()> {
        remove_owned_file(&self.path())
    }
}

/// Atomic write: sibling temp file + `fsync` + rename + parent `fsync`.
/// Private: callers name a [`JsonFile`] or [`TextFile`].
///
/// Durability of the *rename*, not only of the bytes: without an fsync
/// on the parent directory a crash can lose the directory entry the
/// rename created, leaving the old contents (or nothing) where
/// protection state should be (the 2026-09-22 re-review's P3).
/// Best-effort: a filesystem that refuses to sync a directory handle
/// must not fail the write that already succeeded.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with(path, bytes, None)
}

fn write_atomic_with(
    path: &Path,
    bytes: &[u8],
    perms: Option<std::fs::Permissions>,
) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}-{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("control"),
        std::process::id(),
        crate::entities::new_id()
    ));
    std::fs::write(&tmp, bytes)?;
    if let Some(perms) = perms
        && let Err(e) = std::fs::set_permissions(&tmp, perms)
    {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Ok(f) = std::fs::File::open(&tmp) {
        let _ = f.sync_all();
    }
    match std::fs::rename(&tmp, path) {
        Ok(()) => {
            if let Ok(d) = std::fs::File::open(dir) {
                let _ = d.sync_all();
            }
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Removes a regular file swamp wrote; absent is fine.
pub(super) fn remove_owned_file(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_file() => std::fs::remove_file(path),
        Ok(_) => Err(io::Error::other(format!(
            "{} is not a regular file swamp wrote",
            path.display()
        ))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

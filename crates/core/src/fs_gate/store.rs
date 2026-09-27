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

    /// Removes retired Swamp store formats once, after a successful
    /// observation. Paths and basenames are a fixed ownership allow-list;
    /// directory traversal is limited to numeric volume directories and
    /// UUID-named legacy plan payloads. Symlink entries are unlinked as
    /// links and are never descended into.
    pub fn clean_retired_store_state(&self) -> io::Result<bool> {
        const FORMAT: &str = "1\n";
        let marker = self.0.join("housekeeping.version");
        if read_housekeeping_marker(&marker)?.as_deref() == Some(FORMAT) {
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
            if ty.is_dir() && name.bytes().all(|b| b.is_ascii_digit()) && !name.is_empty() {
                clean_legacy_volume_dir(&path)?;
            } else if is_legacy_store_file(&name) && (ty.is_file() || ty.is_symlink()) {
                std::fs::remove_file(path)?;
            } else if name == "plans" && ty.is_dir() {
                clean_legacy_plans_dir(&path)?;
            }
        }
        // `rename` replaces an existing marker entry itself, including a
        // symlink, without opening or following its target.
        write_atomic(&marker, FORMAT.as_bytes())?;
        Ok(true)
    }
}

fn read_housekeeping_marker(path: &Path) -> io::Result<Option<String>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) if is_symlink_open_error(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut marker = String::new();
    file.read_to_string(&mut marker)?;
    Ok(Some(marker))
}

#[cfg(unix)]
fn is_symlink_open_error(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::ELOOP)
}

#[cfg(not(unix))]
fn is_symlink_open_error(_error: &io::Error) -> bool {
    false
}

fn is_legacy_store_file(name: &str) -> bool {
    matches!(
        name,
        "unowned.json"
            | "fsevents.json"
            | "topology.json"
            | "docker_facts.json"
            | "last_run.json"
            | "grants.json"
            | "ledger.jsonl"
    ) || is_legacy_report(name)
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

fn clean_legacy_volume_dir(dir: &Path) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if (ty.is_file() || ty.is_symlink())
            && is_legacy_store_file(&entry.file_name().to_string_lossy())
        {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
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
        ] {
            fs::write(root.join(name), b"current data").unwrap();
        }
        fs::write(volume.join("current.parquet"), b"compatible history").unwrap();
        fs::create_dir(volume.join("deltas")).unwrap();
        fs::write(volume.join("deltas/delta-1.parquet"), b"history").unwrap();
        fs::write(root.join("last_report-not-a-key.json"), b"unrecognized").unwrap();

        let outside = tmp.path().join("outside-sentinel");
        fs::write(&outside, b"must survive").unwrap();
        #[cfg(unix)]
        fs::remove_file(volume.join("unowned.json")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, volume.join("unowned.json")).unwrap();

        assert!(store.clean_retired_store_state().unwrap());
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
        ] {
            assert!(root.join(name).exists(), "removed retained file {name}");
        }
        assert_eq!(
            fs::read(volume.join("current.parquet")).unwrap(),
            b"compatible history"
        );
        assert!(root.join("last_report-not-a-key.json").exists());
        assert_eq!(fs::read(outside).unwrap(), b"must survive");

        fs::write(root.join("unowned.json"), b"later file").unwrap();
        assert!(!store.clean_retired_store_state().unwrap());
        assert!(root.join("unowned.json").exists());
        fs::write(root.join("housekeeping.version"), b"0\n").unwrap();
        assert!(store.clean_retired_store_state().unwrap());
        assert!(!root.join("unowned.json").exists());
        assert_eq!(
            fs::read_to_string(root.join("housekeeping.version")).unwrap(),
            "1\n"
        );

        #[cfg(unix)]
        {
            let outside_marker = root.join("outside-marker");
            fs::write(&outside_marker, "1\n").unwrap();
            fs::remove_file(root.join("housekeeping.version")).unwrap();
            std::os::unix::fs::symlink(&outside_marker, root.join("housekeeping.version")).unwrap();
            fs::write(root.join("unowned.json"), b"legacy").unwrap();
            assert!(store.clean_retired_store_state().unwrap());
            assert!(!root.join("unowned.json").exists());
            assert_eq!(fs::read_to_string(outside_marker).unwrap(), "1\n");
        }
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
    /// `<store>/config.toml`, written only by `swamp config init`.
    Config { store: &'a StoreDir },
    /// The scheduled refresh's LaunchAgent plist, where
    /// [`launch_agent_plist`] resolves it.
    LaunchAgent,
}

impl TextFile<'_> {
    pub fn path(&self) -> io::Result<PathBuf> {
        match *self {
            TextFile::Config { store } => Ok(store.0.join("config.toml")),
            TextFile::LaunchAgent => launch_agent_plist(),
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

//! Read-only access to Codex's session index.
//!
//! This is the single exception to the transcript-header-only content
//! rule: SQLite is opened read-only, and the only selected columns are
//! `threads.rollout_path` and `threads.cwd`. No rollout JSONL contents,
//! titles, previews, or message fields are read. A missing, ambiguous, or
//! incompatible index fails closed; it never falls back to transcript
//! parsing.

use super::IdentifyCtx;
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

const CONFIG_READ_BYTES: usize = 64 * 1024;

#[derive(Default)]
pub(super) struct SessionIndex {
    available: bool,
    by_rollout_path: HashMap<PathBuf, Option<PathBuf>>,
}

pub(super) enum CwdLookup<'a> {
    Declared(&'a Path),
    NoIndex,
    NoRow,
    NoUsableCwd,
}

impl SessionIndex {
    pub(super) fn read_from_home(codex_home: &Path, ctx: &IdentifyCtx) -> Self {
        let Some(sqlite_home) = resolve_sqlite_home(codex_home, ctx) else {
            return Self::default();
        };

        let Some(db_path) = newest_state_database(&sqlite_home, ctx) else {
            return Self::default();
        };

        let Some(by_rollout_path) = read_session_rows(&db_path, ctx) else {
            return Self::default();
        };

        Self {
            available: true,
            by_rollout_path,
        }
    }

    pub(super) fn cwd_for(&self, rollout_path: &Path) -> CwdLookup<'_> {
        if !self.available {
            return CwdLookup::NoIndex;
        }
        match self.by_rollout_path.get(rollout_path) {
            Some(Some(cwd)) => CwdLookup::Declared(cwd),
            Some(None) => CwdLookup::NoUsableCwd,
            None => CwdLookup::NoRow,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct CodexConfig {
    sqlite_home: Option<String>,
}

enum ConfigHome {
    Missing,
    Path(PathBuf),
    Invalid,
}

fn config_sqlite_home(codex_home: &Path, ctx: &IdentifyCtx) -> ConfigHome {
    let path = codex_home.join("config.toml");
    let metadata = match ctx.stat(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return ConfigHome::Missing,
        Err(_) => return ConfigHome::Invalid,
    };
    if !metadata.is_file() || metadata.len() > CONFIG_READ_BYTES as u64 {
        return ConfigHome::Invalid;
    }
    let Some(text) = ctx.read_header(&path, CONFIG_READ_BYTES) else {
        return ConfigHome::Invalid;
    };
    let Ok(config) = toml::from_str::<CodexConfig>(&text) else {
        return ConfigHome::Invalid;
    };
    match config.sqlite_home {
        Some(value) => {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                ConfigHome::Path(path)
            } else {
                ConfigHome::Invalid
            }
        }
        None => ConfigHome::Missing,
    }
}

fn resolve_sqlite_home(codex_home: &Path, ctx: &IdentifyCtx) -> Option<PathBuf> {
    let environment = ctx.codex_sqlite_home_override().map(Path::to_path_buf);
    choose_sqlite_home(codex_home, config_sqlite_home(codex_home, ctx), environment)
}

fn choose_sqlite_home(
    codex_home: &Path,
    config: ConfigHome,
    environment: Option<PathBuf>,
) -> Option<PathBuf> {
    match config {
        ConfigHome::Path(path) => Some(path),
        // An explicit but unreadable/malformed setting makes the active
        // DB home unknowable. Do not guess at CODEX_HOME and risk using a
        // stale duplicate database.
        ConfigHome::Invalid => None,
        ConfigHome::Missing => match environment {
            Some(path) if path.is_absolute() => Some(path),
            Some(_) => None,
            None => Some(codex_home.to_path_buf()),
        },
    }
}

pub(super) fn state_database_version(name: &str) -> Option<u32> {
    name.strip_prefix("state_")?
        .strip_suffix(".sqlite")?
        .parse()
        .ok()
}

fn newest_state_database(sqlite_home: &Path, ctx: &IdentifyCtx) -> Option<PathBuf> {
    let mut candidates: Vec<(u32, PathBuf)> = ctx
        .list(sqlite_home)
        .into_iter()
        .filter(|entry| !entry.is_dir)
        .filter_map(|entry| {
            let version = state_database_version(&entry.name)?;
            let path = sqlite_home.join(entry.name);
            let metadata = ctx.stat(&path).ok()?;
            metadata.is_file().then_some((version, path))
        })
        .collect();
    candidates.sort_by_key(|(version, _)| *version);
    candidates.pop().map(|(_, path)| path)
}

fn read_session_rows(
    db_path: &Path,
    ctx: &IdentifyCtx,
) -> Option<HashMap<PathBuf, Option<PathBuf>>> {
    // Reject non-SQLite files before SQLite gets a chance to initialize or
    // resize a WAL shared-memory sidecar beside a damaged/renamed file.
    // This is the 16-byte SQLite file signature, not transcript content.
    let signature = ctx.read_header(db_path, 16)?;
    if signature.as_bytes() != b"SQLite format 3\0" {
        return None;
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection = Connection::open_with_flags(db_path, flags).ok()?;
    connection.busy_timeout(Duration::from_millis(25)).ok()?;

    let has_threads_table: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'threads')",
            [],
            |row| row.get(0),
        )
        .ok()?;
    if !has_threads_table {
        return None;
    }

    let mut columns = HashSet::new();
    let mut table_info = connection.prepare("PRAGMA table_info(threads)").ok()?;
    let names = table_info
        .query_map([], |row| row.get::<_, String>(1))
        .ok()?;
    for name in names {
        columns.insert(name.ok()?);
    }
    drop(table_info);
    if !columns.contains("rollout_path") || !columns.contains("cwd") {
        return None;
    }

    // Deliberately select only the two allowlisted metadata columns.
    // In particular, never select title, preview, or first_user_message.
    let mut statement = connection
        .prepare("SELECT rollout_path, cwd FROM threads")
        .ok()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        })
        .ok()?;

    let mut index: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
    for row in rows {
        let (Some(rollout), cwd) = row.ok()? else {
            continue;
        };
        let rollout = PathBuf::from(rollout);
        let cwd = cwd
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());
        match index.entry(rollout) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(cwd);
            }
            std::collections::hash_map::Entry::Occupied(mut entry) if entry.get() != &cwd => {
                entry.insert(None);
            }
            std::collections::hash_map::Entry::Occupied(_) => {}
        }
    }
    Some(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn read_rows(db: &Path) -> Option<HashMap<PathBuf, Option<PathBuf>>> {
        let cache = super::super::IdentificationCache::disabled();
        let ctx = IdentifyCtx::new(1, &cache);
        read_session_rows(db, &ctx)
    }

    #[test]
    fn config_home_precedes_environment_and_default() {
        let home = Path::new("/codex-home");
        let configured = ConfigHome::Path(PathBuf::from("/configured/sqlite"));
        assert_eq!(
            choose_sqlite_home(home, configured, Some(PathBuf::from("/env/sqlite"))),
            Some(PathBuf::from("/configured/sqlite"))
        );
        assert_eq!(
            choose_sqlite_home(
                home,
                ConfigHome::Missing,
                Some(PathBuf::from("/env/sqlite"))
            ),
            Some(PathBuf::from("/env/sqlite"))
        );
        assert_eq!(
            choose_sqlite_home(home, ConfigHome::Missing, None),
            Some(home.to_path_buf())
        );
        assert_eq!(
            choose_sqlite_home(
                home,
                ConfigHome::Invalid,
                Some(PathBuf::from("/env/sqlite"))
            ),
            None
        );
        assert_eq!(
            choose_sqlite_home(
                home,
                ConfigHome::Missing,
                Some(PathBuf::from("relative/sqlite"))
            ),
            None
        );
    }

    #[test]
    fn accepts_only_versioned_state_database_names() {
        assert_eq!(state_database_version("state_5.sqlite"), Some(5));
        assert_eq!(state_database_version("state_12.sqlite"), Some(12));
        for name in [
            "state.sqlite",
            "state_latest.sqlite",
            "state_5.sqlite-wal",
            "other_5.sqlite",
            "state_-1.sqlite",
        ] {
            assert_eq!(state_database_version(name), None, "{name}");
        }
    }

    #[test]
    fn reads_only_exact_rollout_path_and_cwd_columns_and_marks_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state_5.sqlite");
        let rollout = dir.path().join("rollout.jsonl");
        let cwd = dir.path().join("repo");
        fs::create_dir_all(&cwd).unwrap();
        let connection = Connection::open(&db).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE threads (rollout_path TEXT NOT NULL, cwd TEXT NOT NULL, title TEXT, preview TEXT, first_user_message TEXT);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads (rollout_path,cwd,title,preview,first_user_message) VALUES (?1,?2,'PRIVATE TITLE','PRIVATE PREVIEW','PRIVATE MESSAGE')",
                rusqlite::params![rollout.display().to_string(), cwd.display().to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads (rollout_path,cwd) VALUES (?1,?2)",
                rusqlite::params![
                    dir.path().join("conflict.jsonl").display().to_string(),
                    cwd.display().to_string()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads (rollout_path,cwd) VALUES (?1,?2)",
                rusqlite::params![
                    dir.path().join("conflict.jsonl").display().to_string(),
                    dir.path().display().to_string()
                ],
            )
            .unwrap();
        drop(connection);

        let rows = read_rows(&db).expect("valid read-only index");
        assert_eq!(rows.get(&rollout), Some(&Some(cwd)));
        assert_eq!(rows.get(&dir.path().join("conflict.jsonl")), Some(&None));
        let debug = format!("{rows:?}");
        assert!(!debug.contains("PRIVATE TITLE"));
        assert!(!debug.contains("PRIVATE PREVIEW"));
        assert!(!debug.contains("PRIVATE MESSAGE"));
    }

    #[test]
    fn incompatible_or_non_database_files_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let invalid = dir.path().join("not-a-database.sqlite");
        fs::write(&invalid, b"not sqlite").unwrap();
        assert!(read_rows(&invalid).is_none());

        let wrong_schema = dir.path().join("wrong-schema.sqlite");
        let connection = Connection::open(&wrong_schema).unwrap();
        connection
            .execute_batch("CREATE TABLE threads (id TEXT, message TEXT);")
            .unwrap();
        drop(connection);
        assert!(read_rows(&wrong_schema).is_none());
    }
}

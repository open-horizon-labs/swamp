//! The one way swamp opens another tool's SQLite database: read-only,
//! after the 16-byte file signature says it is one, with a short busy
//! timeout. Shared by the Codex thread index (`agents::codex_state`) and
//! Cargo's last-use tracker (`crate::last_used`), so a second reader
//! cannot drift from the first one's rules.
//!
//! Nothing here writes. `SQLITE_OPEN_READ_ONLY` refuses every statement
//! that would, and the signature check runs *before* SQLite is asked to
//! open the file, so a renamed or damaged file never gets a WAL
//! shared-memory sidecar created beside it.

use rusqlite::{Connection, OpenFlags};
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

/// The first 16 bytes of every SQLite 3 database file.
const SIGNATURE: &[u8; 16] = b"SQLite format 3\0";

/// How long a read waits for another process's lock before giving up.
/// A locked database is "not read this pass", never a wait.
const BUSY_TIMEOUT: Duration = Duration::from_millis(25);

/// Opens `db_path` read-only, or `None` when `header` (the file's first
/// 16 bytes, read by the caller through its own counted gate) is not the
/// SQLite signature or the open fails.
pub(crate) fn open_read_only(db_path: &Path, header: &[u8]) -> Option<Connection> {
    if header != SIGNATURE {
        return None;
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection = Connection::open_with_flags(db_path, flags).ok()?;
    connection.busy_timeout(BUSY_TIMEOUT).ok()?;
    Some(connection)
}

/// The column names of `table`, or `None` when it does not exist or
/// cannot be described. A reader checks the columns it selects before it
/// selects them, so a tool's schema change is a refusal, not an error
/// string from inside a query.
pub(crate) fn table_columns(connection: &Connection, table: &str) -> Option<HashSet<String>> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [table],
            |row| row.get(0),
        )
        .ok()?;
    if !exists {
        return None;
    }
    // `table` is a caller-chosen constant, never user input; PRAGMA takes
    // no bound parameter.
    let mut info = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .ok()?;
    let names = info.query_map([], |row| row.get::<_, String>(1)).ok()?;
    let mut columns = HashSet::new();
    for name in names {
        columns.insert(name.ok()?);
    }
    Some(columns)
}

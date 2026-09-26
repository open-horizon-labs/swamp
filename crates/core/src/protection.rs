//! Human keep/protect intent (#100's `swamp protect add/list/remove`):
//! a small typed table in the store (`protect.parquet`), deliberately
//! decoupled from the growth store. Survives refresh; blocks actions;
//! never inferred from observation
//! (`.oh/guardrails/protection-fails-closed.md`).
//!
//! [`ProtectList`] is opaque. Its one query is [`ProtectList::conflict`],
//! the both-directions containment test; there is no accessor to the
//! protected paths, no iterator, no `Deref`, no `Clone` of the inner
//! list. So a second protection predicate -- the one-directional
//! `starts_with` that re-review 1 found and re-review 4's sweep wrote
//! again as `strip_prefix(k).is_ok()` -- has nothing to run over: it can
//! be written, but it cannot be written *about protection*
//! (`crates/core/tests/compile_fail/protect_list_*.rs`). That replaces
//! the `protection_fails_closed` call-graph audit.
//!
//! This module never names Arrow directly (`gate_paths_only_inside_gates`
//! confines table schemas to `growth::columns`/`assoc_store`/`store`/
//! `github`/`fs_gate`): the Parquet schema and (de)serialization live in
//! `crate::growth::columns::{StoredProtectRow, read_protect_rows,
//! write_protect_rows}`, and this module owns only the semantics.

use anyhow::Result;
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::growth::columns::{StoredProtectRow, read_protect_rows, write_protect_rows};

/// One row of the human keep list: an absolute path and when it was
/// added (epoch seconds), typed -- not a JSON control file
/// (`.oh/guardrails/store-data-is-parquet-not-json-sidecars.md`).
type ProtectRow = StoredProtectRow;

/// The table the protect list lives in.
pub fn protect_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("protect.parquet")
}

/// The human keep list, loaded. Opaque: see the module docs.
#[derive(Debug)]
pub struct ProtectList {
    paths: Vec<PathBuf>,
}

impl ProtectList {
    /// Nothing protected: what a store without a protect file means.
    pub fn empty() -> ProtectList {
        ProtectList { paths: Vec::new() }
    }

    /// This list plus `extra` (a caller's own keep entries, e.g. a
    /// proposal's convenience list). The union only ever protects more.
    pub fn including(mut self, extra: &[PathBuf]) -> ProtectList {
        for p in extra {
            if !self.paths.contains(p) {
                self.paths.push(p.clone());
            }
        }
        self
    }

    /// Whether nothing at all is protected (a shortcut, not a query about
    /// any path).
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// The reason human keep/protect intent blocks `candidate`, or `None`.
    ///
    /// **The only** protection predicate in the crate. There used to be a
    /// second, `is_human_protected`, which returned a bare `bool` by
    /// delegating here -- and that was how a one-directional mutation
    /// survived: the audit inspected this function, while
    /// `actions::propose_checking_protection` called the boolean wrapper,
    /// and no test proposed an ordinary directory *containing* a protected
    /// descendant. One predicate, everywhere, so there is nothing to
    /// inspect the wrong one of
    /// (`.oh/guardrails/protection-fails-closed.md`).
    ///
    /// It covers `candidate` in **both** directions:
    ///
    /// * `candidate` is the protected path or lies beneath it -- the
    ///   original, obvious direction; and
    /// * a protected path lies beneath `candidate` -- the direction the
    ///   2026-09-21 review's `protected_descendant_must_prevent_parent_cache_proposal`
    ///   counterexample falsified. Protecting `debug/log.txt` and then
    ///   removing `debug/` destroys exactly what the human asked to keep, so
    ///   a unit *containing* a protected path is protected too.
    ///
    /// The returned string carries which direction matched, so a refusal can
    /// say *why*. It is the only query this type has, so there is no
    /// second (one-directional) predicate to reach for.
    pub fn conflict(&self, candidate: &Path) -> Option<String> {
        // One spelling for both sides. `external::discover_and_measure`
        // canonicalizes every candidate and `agents::discover_and_measure`
        // does not, so the same home comes back as `/var/folders/.../claude`
        // from one pass and `/private/var/folders/.../claude` from the
        // other. Compared literally, a single `protect` entry covered one
        // family and not the other (the 2026-09-22 re-review's P2); through
        // `scope::comparable` it covers both.
        let cand = crate::scope::comparable(candidate);
        for p in &self.paths {
            let prot = crate::scope::comparable(p);
            if cand == prot {
                return Some(format!("{} is kept by `swamp protect`", p.display()));
            }
            if cand.starts_with(&prot) {
                return Some(format!(
                    "{} is beneath the human-protected path {}",
                    candidate.display(),
                    p.display()
                ));
            }
            if prot.starts_with(&cand) {
                return Some(format!("contains human-protected path {}", p.display()));
            }
        }
        None
    }
}

/// What `swamp protect list` prints: the entries as text, one per line
/// (`Display`), or as a JSON array (`Serialize`). Nothing else: it is a
/// rendering, not a second copy of the list to test paths against --
/// there is no iterator, no accessor to an entry and no containment
/// query, so a caller cannot grow a second protection predicate over it
/// (the production listing; the raw `protect_list` exists only under the
/// `testing` feature).
#[derive(Debug, Serialize)]
#[serde(transparent)]
pub struct ProtectListing(Vec<String>);

impl ProtectListing {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl std::fmt::Display for ProtectListing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, p) in self.0.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{p}")?;
        }
        Ok(())
    }
}

fn load_rows(swamp_dir: &Path) -> Result<Vec<ProtectRow>> {
    let path = protect_path(swamp_dir);
    match read_protect_rows(&path) {
        Ok(rows) => {
            for r in &rows {
                if r.path.trim().is_empty() {
                    anyhow::bail!(
                        "protection state unknown: {} contains an empty path entry. Every action \
                         is refused until it is repaired or removed.",
                        path.display()
                    );
                }
            }
            Ok(rows)
        }
        Err(e) => Err(anyhow::anyhow!(
            "protection state unknown: {} could not be read ({e}). Every action is refused \
             until it is repaired or removed; `swamp protect list` shows this same error.",
            path.display()
        )),
    }
}

fn load_paths(swamp_dir: &Path) -> Result<Vec<PathBuf>> {
    Ok(load_rows(swamp_dir)?
        .into_iter()
        .map(|r| PathBuf::from(r.path))
        .collect())
}

/// The single entry point for protection state
/// (`.oh/guardrails/protection-fails-closed.md`). An absent file is an
/// empty keep list -- the ordinary "nothing protected yet" case. A file
/// that exists but cannot be read or parsed is **not**: protection state
/// is then *unknown*, and every caller must fail closed rather than
/// proceed as if nothing were protected. Returning `Result` -- with no
/// `Default` for `ProtectList`, so `.unwrap_or_default()` does not
/// compile -- is what makes that structural instead of a convention.
pub fn load_protect(swamp_dir: &Path) -> Result<ProtectList> {
    Ok(ProtectList {
        paths: load_paths(swamp_dir)?,
    })
}

fn save_rows(swamp_dir: &Path, rows: &[ProtectRow]) -> Result<()> {
    crate::fs_gate::store::StoreDir::at(swamp_dir)?.create()?;
    write_protect_rows(&protect_path(swamp_dir), rows)
}

/// Adds `path` to the human keep list. Stored verbatim, and **refused
/// unless it is absolute**.
///
/// Verbatim, because `AgentUnit.path`/`AgentMember.path` are built as
/// `home.join(relative)` and both sides of every comparison are brought
/// into one spelling by `scope::comparable` at comparison time, not by
/// rewriting what the human typed.
///
/// Absolute, because a relative entry protects nothing. The 2026-09-22
/// re-review's CE5: `swamp protect add debug` returned `Ok`, `swamp
/// protect list` showed `debug`, and the very next
/// propose/approve/execute moved `<home>/debug`.
/// `protection_conflict` compares against absolute unit paths in both
/// directions and a relative entry matches neither, so the protection
/// layer -- which refuses every action on a corrupt protect file --
/// accepted, confirmed, and then did not protect. The existing "used
/// verbatim, never canonicalized" rationale is about *symlink*
/// mismatch; it never justified accepting a path that cannot be
/// enforced.
///
/// Idempotent; a not-yet-observed path can still be protected in
/// advance.
///
/// **Human-only by convention, not by a technical wall**: this is
/// `swamp protect add`'s handler, and the TUI never calls it -- it only
/// reads the list this writes.
pub fn protect_add(swamp_dir: &Path, path: &Path) -> Result<()> {
    protect_add_unchecked(swamp_dir, path)
}

fn protect_add_unchecked(swamp_dir: &Path, path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        anyhow::bail!("refused: an empty path protects nothing");
    }
    if !path.is_absolute() {
        anyhow::bail!(
            "refused: `{}` is not an absolute path, and a relative entry protects nothing \
             (protection is compared against absolute unit paths in both directions). Pass the \
             full path, e.g. `$PWD/{}`.",
            path.display(),
            path.display()
        );
    }
    let mut rows = load_rows(swamp_dir)?;
    let candidate = path.display().to_string();
    if !rows.iter().any(|r| r.path == candidate) {
        rows.push(ProtectRow {
            path: candidate,
            added_at: crate::entities::now(),
        });
        save_rows(swamp_dir, &rows)?;
    }
    Ok(())
}

/// Removes `path` from the human keep list. `swamp protect remove`'s
/// handler.
pub fn protect_remove(swamp_dir: &Path, path: &Path) -> Result<()> {
    protect_remove_unchecked(swamp_dir, path)
}

fn protect_remove_unchecked(swamp_dir: &Path, path: &Path) -> Result<()> {
    let mut rows = load_rows(swamp_dir)?;
    let target = path.display().to_string();
    let before = rows.len();
    rows.retain(|r| r.path != target);
    if rows.len() != before {
        save_rows(swamp_dir, &rows)?;
    }
    Ok(())
}

/// The keep list's raw entries, for integration tests only (`testing`
/// feature; no production build has it). The reviewers' counterexample
/// files read the list through this name.
#[cfg(feature = "testing")]
pub fn protect_list(swamp_dir: &Path) -> Result<Vec<PathBuf>> {
    load_paths(swamp_dir)
}

/// The keep list as `swamp protect list` shows it; errors exactly as
/// [`load_protect`] does.
pub fn protect_listing(swamp_dir: &Path) -> Result<ProtectListing> {
    Ok(ProtectListing(
        load_paths(swamp_dir)?
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of R14 item A for this file: `protect.parquet` is
    /// the table, not a JSON sidecar -- and it round-trips through the
    /// public API exactly as `agent_protect.json` used to, including the
    /// `added_at` column the old file never had.
    #[test]
    fn protect_round_trips_through_parquet_with_added_at() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        protect_add(dir.path(), &a).unwrap();
        protect_add(dir.path(), &b).unwrap();

        let table = protect_path(dir.path());
        assert!(table.exists(), "protect.parquet must exist after an add");
        assert!(
            !dir.path().join("agent_protect.json").exists(),
            "no JSON control file for the protect list"
        );

        let rows = read_protect_rows(&table).unwrap();
        assert_eq!(rows.len(), 2);
        for r in &rows {
            assert!(r.added_at > 0, "added_at must be stamped, not left at 0");
        }

        let list = load_protect(dir.path()).unwrap();
        assert!(list.conflict(&a).is_some());
        assert!(list.conflict(&b).is_some());

        protect_remove(dir.path(), &a).unwrap();
        let rows = read_protect_rows(&table).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, b.display().to_string());
    }

    /// Adding twice does not duplicate the row or reset `added_at`.
    #[test]
    fn protect_add_is_idempotent_and_keeps_the_original_added_at() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("kept");
        protect_add(dir.path(), &p).unwrap();
        let first = read_protect_rows(&protect_path(dir.path())).unwrap();
        assert_eq!(first.len(), 1);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        protect_add(dir.path(), &p).unwrap();
        let second = read_protect_rows(&protect_path(dir.path())).unwrap();
        assert_eq!(second.len(), 1, "re-adding must not duplicate the row");
        assert_eq!(
            second[0].added_at, first[0].added_at,
            "re-adding an already-protected path must not reset when it was added"
        );
    }

    /// Fails closed: a `protect.parquet` that exists but cannot be read
    /// as Parquet must refuse every caller, not silently mean "nothing
    /// protected" (`.oh/guardrails/protection-fails-closed.md`).
    #[test]
    fn a_corrupt_protect_table_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(protect_path(dir.path()), b"not a parquet file").unwrap();
        let err = load_protect(dir.path()).unwrap_err();
        assert!(
            err.to_string().contains("protection state unknown"),
            "got: {err}"
        );
    }

    /// A missing table is the ordinary "nothing protected yet" case, not
    /// an error.
    #[test]
    fn a_missing_protect_table_is_an_empty_list_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let list = load_protect(dir.path()).unwrap();
        assert!(list.is_empty());
    }
}

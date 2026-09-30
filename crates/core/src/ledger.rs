use crate::fs_gate::store::{LedgerFile, StoreDir};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Verb {
    Delete,
    Archive,
    RemoveWorktree,
    /// Removed through the manager's own command (mise, simctl), with no
    /// Trash (#177). Additive: a binary that predates it reads the label
    /// as `Delete`, an ordinary removal, and keeps the row's text as is.
    ToolRemove,
}

impl Verb {
    fn label(&self) -> &'static str {
        match self {
            Verb::Delete => "delete",
            Verb::Archive => "archive",
            Verb::RemoveWorktree => "remove-worktree",
            Verb::ToolRemove => "tool-remove",
        }
    }
    fn from_label(s: &str) -> Self {
        match s {
            "archive" => Verb::Archive,
            "remove-worktree" => Verb::RemoveWorktree,
            "tool-remove" => Verb::ToolRemove,
            _ => Verb::Delete,
        }
    }
}

pub const NO_GRANT: &str = "human-marked";

/// How an append that did write its record, but only into a new ledger
/// (the old one could not be read and was kept aside), begins its error.
pub const KEPT_ASIDE: &str = "the previous ledger could not be read";

/// Whether an append error is the one that still wrote the record (into
/// a new ledger, the unreadable old one kept aside): the action may go on.
pub fn wrote_into_new_ledger(e: &anyhow::Error) -> bool {
    e.to_string().starts_with(KEPT_ASIDE)
}

/// One fact the human saw before the action ran, as a key and its
/// rendered value (`bytes`, `label`, `observed_at`, ...). Typed rows in
/// `ledger_facts.parquet`, never a JSON blob.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LedgerFact {
    pub key: String,
    pub value: String,
}

impl LedgerFact {
    pub fn new(key: impl Into<String>, value: impl ToString) -> Self {
        Self {
            key: key.into(),
            value: value.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRecord {
    pub id: String,
    pub verb: Verb,
    pub entity_id: String,
    pub evidence: Vec<LedgerFact>,
    pub grant_id: String,
    pub actor: String,
    pub outcome: String,
    pub recovery_location: Option<PathBuf>,
    pub measured_free_space_delta: Option<i64>,
    pub observed_path_state: Option<String>,
    pub recorded_at: u64,
}

/// The action ledger: `<store>/ledger.parquet` (+ `ledger_facts.parquet`
/// beside it), "where did it go". An append reads the rows back and
/// rewrites both tables -- a ledger holds a human's few actions, not a
/// log.
#[derive(Debug, Clone)]
pub struct Ledger {
    store: StoreDir,
    resolved: bool,
}

impl Ledger {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if path.file_name().is_none_or(|n| n != "ledger.parquet") {
            anyhow::bail!(
                "{} is not a swamp ledger (a ledger is a store's `ledger.parquet`)",
                path.display()
            );
        }
        let dir = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("{} has no store directory", path.display()))?;
        Ok(Self {
            store: StoreDir::at(dir)?,
            resolved: false,
        })
    }
    /// The ledger for `store`, honouring `$SWAMP_LEDGER_PATH` (the TUI).
    pub fn resolved(store: &StoreDir) -> Self {
        Self {
            store: store.clone(),
            resolved: true,
        }
    }
    fn file(&self) -> LedgerFile<'_> {
        if self.resolved {
            LedgerFile::Resolved(&self.store)
        } else {
            LedgerFile::Store(&self.store)
        }
    }
    fn facts_path(&self) -> PathBuf {
        self.path().with_file_name("ledger_facts.parquet")
    }
    /// Adds one record. A ledger that exists but cannot be read (a
    /// truncated or corrupt file) is never overwritten: it is kept beside
    /// itself as `ledger.parquet.corrupt-<ts>` (and its facts table the
    /// same way), the record starts a new ledger, and the call returns an
    /// error that says where the old one went.
    pub fn append(&self, r: &ActionRecord) -> Result<()> {
        self.write_record(r, false)
    }

    /// Replaces the record with `r.id` (a `started` row's final outcome),
    /// or adds it when there is none.
    pub fn replace(&self, r: &ActionRecord) -> Result<()> {
        self.write_record(r, true)
    }

    fn write_record(&self, r: &ActionRecord, replace: bool) -> Result<()> {
        use crate::growth::columns as c;
        let path = self.path();
        let facts_path = self.facts_path();
        self.store.create()?;
        // One writer at a time across processes: the table is read, a row
        // added and the table rewritten, and two writers would lose rows.
        let _lock = StoreDir::lock_ledger_writes(&path).map_err(|e| {
            anyhow::anyhow!("swamp's ledger could not be locked ({e}), so nothing was written")
        })?;
        let mut kept: Vec<String> = Vec::new();
        let mut rows = if crate::fs_gate::exists(&path) {
            match c::read_ledger_rows(&path) {
                Ok(rows) => rows,
                Err(e) => {
                    let aside = crate::fs_gate::store::keep_aside(&path, crate::entities::now())
                        .with_context(|| format!("keep {} aside", path.display()))?;
                    kept.push(format!("{} ({e})", aside.display()));
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        let mut facts = if crate::fs_gate::exists(&facts_path) {
            match c::read_ledger_fact_rows(&facts_path) {
                Ok(f) if kept.is_empty() => f,
                other => {
                    let aside =
                        crate::fs_gate::store::keep_aside(&facts_path, crate::entities::now())
                            .with_context(|| format!("keep {} aside", facts_path.display()))?;
                    if let Err(e) = other {
                        kept.push(format!("{} ({e})", aside.display()));
                    }
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        if replace {
            rows.retain(|row| row.id != r.id);
            facts.retain(|f| f.record_id != r.id);
        }
        rows.push(c::StoredLedgerRow {
            id: r.id.clone(),
            verb: r.verb.label().to_string(),
            entity_id: r.entity_id.clone(),
            grant_id: r.grant_id.clone(),
            actor: r.actor.clone(),
            outcome: r.outcome.clone(),
            recovery_location: r
                .recovery_location
                .as_ref()
                .map(|p| p.display().to_string()),
            measured_free_space_delta: r.measured_free_space_delta,
            observed_path_state: r.observed_path_state.clone(),
            recorded_at: r.recorded_at,
        });
        for (seq, f) in r.evidence.iter().enumerate() {
            facts.push(c::StoredLedgerFactRow {
                record_id: r.id.clone(),
                seq: seq as u32,
                key: f.key.clone(),
                value: f.value.clone(),
            });
        }
        c::write_ledger_rows(&path, &rows).with_context(|| format!("write {}", path.display()))?;
        c::write_ledger_fact_rows(&facts_path, &facts)
            .with_context(|| format!("write {}", facts_path.display()))?;
        if !kept.is_empty() {
            anyhow::bail!(
                "{KEPT_ASIDE}; it was kept as {}, and this record starts a new ledger",
                kept.join(", ")
            );
        }
        Ok(())
    }
    pub fn all(&self) -> Result<Vec<ActionRecord>> {
        use crate::growth::columns as c;
        let rows = c::read_ledger_rows(&self.path())?;
        let mut facts = c::read_ledger_fact_rows(&self.facts_path()).unwrap_or_default();
        facts.sort_by_key(|f| f.seq);
        Ok(rows
            .into_iter()
            .map(|r| ActionRecord {
                evidence: facts
                    .iter()
                    .filter(|f| f.record_id == r.id)
                    .map(|f| LedgerFact {
                        key: f.key.clone(),
                        value: f.value.clone(),
                    })
                    .collect(),
                id: r.id,
                verb: Verb::from_label(&r.verb),
                entity_id: r.entity_id,
                grant_id: r.grant_id,
                actor: r.actor,
                outcome: r.outcome,
                recovery_location: r.recovery_location.map(PathBuf::from),
                measured_free_space_delta: r.measured_free_space_delta,
                observed_path_state: r.observed_path_state,
                recorded_at: r.recorded_at,
            })
            .collect())
    }
    pub fn path(&self) -> PathBuf {
        self.file()
            .path()
            .unwrap_or_else(|_| self.store.path().join("ledger.parquet"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::now;
    use tempfile::tempdir;
    #[test]
    fn ledger_survives_index_removal() {
        let d = tempdir().unwrap();
        let l = Ledger::open(d.path().join("ledger.parquet")).unwrap();
        let r = ActionRecord {
            id: "1".into(),
            verb: Verb::Delete,
            entity_id: "e".into(),
            evidence: vec![LedgerFact::new("bytes", 3)],
            grant_id: "g".into(),
            actor: "human".into(),
            outcome: "completed".into(),
            recovery_location: None,
            measured_free_space_delta: Some(2),
            observed_path_state: Some("gone".into()),
            recorded_at: now(),
        };
        l.append(&r).unwrap();
        let all = l.all().unwrap();
        assert_eq!(all[0].entity_id, "e");
        assert_eq!(all[0].evidence, vec![LedgerFact::new("bytes", 3)]);
        assert_eq!(all[0].measured_free_space_delta, Some(2));
    }
}

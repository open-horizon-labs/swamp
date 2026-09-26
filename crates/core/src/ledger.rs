use crate::fs_gate::store::{LedgerFile, StoreDir};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Verb {
    Delete,
    Archive,
    RemoveWorktree,
}

impl Verb {
    fn label(&self) -> &'static str {
        match self {
            Verb::Delete => "delete",
            Verb::Archive => "archive",
            Verb::RemoveWorktree => "remove-worktree",
        }
    }
    fn from_label(s: &str) -> Self {
        match s {
            "archive" => Verb::Archive,
            "remove-worktree" => Verb::RemoveWorktree,
            _ => Verb::Delete,
        }
    }
}

pub const NO_GRANT: &str = "human-marked";

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
    pub fn append(&self, r: &ActionRecord) -> Result<()> {
        use crate::growth::columns as c;
        let path = self.path();
        self.store.create()?;
        let mut rows = c::read_ledger_rows(&path).unwrap_or_default();
        let mut facts = c::read_ledger_fact_rows(&self.facts_path()).unwrap_or_default();
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
        c::write_ledger_fact_rows(&self.facts_path(), &facts)
            .with_context(|| format!("write {}", self.facts_path().display()))?;
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

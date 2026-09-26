//! Current-state Parquet tables for the association layer's caches:
//! toolchain declarations, dependency identities, and the Xcode
//! DerivedData -> workspace join.
//!
//! These were three JSON sidecars under the store
//! (`toolchain_declarations_cache.json`,
//! `dependency_identities_cache.json`, and no cache at all for the Xcode
//! join). Per-worktree, per-unit, per-row data rewritten whole on every
//! change is a parallel database with a JSON syntax, which the handoff
//! rules out and `.oh/guardrails/store-data-is-parquet-not-json-sidecars.md`
//! now enforces. They live here instead, as properly columnar
//! current-state tables written through the store's own atomic
//! zstd-Parquet writer.
//!
//! Every table is keyed by an identity plus a *source fingerprint* --
//! the `(name, size, mtime)` of the files the cached value was derived
//! from -- so invalidation is a comparison, never a timestamp guess. An
//! unchanged worktree is a table lookup; an unchanged DerivedData folder
//! costs no `plutil` subprocess at all, which is what the PR #123
//! review's `a_second_unchanged_pass_must_not_respawn_plutil_for_every_derived_data_folder`
//! counterexample demanded.
//!
//! There is deliberately no migration from the JSON files: this is a
//! brand-new project, and a stale cache is re-derived on first use.

use anyhow::{Context, Result};
use arrow_array::{ArrayRef, RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const ZSTD_LEVEL: i32 = 9;

fn dir(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("associations")
}

/// A source fingerprint, rendered as one stable string so it is a single
/// comparable column rather than a nested list.
pub fn fingerprint_string(parts: &[(String, u64)]) -> String {
    let mut parts: Vec<String> = parts
        .iter()
        .map(|(name, mtime)| format!("{name}@{mtime}"))
        .collect();
    parts.sort();
    parts.join("\u{1}")
}

/// One stored row: key, fingerprint, the observation that wrote it, and
/// the value columns.
type StoredRow = (String, String, u64, Vec<String>);

/// One `(key, fingerprint) -> values` table. Three columns, so a reader
/// can scan one without materializing the others, and one row per cached
/// value rather than one blob per key.
struct KeyedTable {
    path: PathBuf,
    value_columns: &'static [&'static str],
}

impl KeyedTable {
    fn schema(&self) -> Arc<Schema> {
        let mut fields = vec![
            Field::new("key", DataType::Utf8, false),
            Field::new("fingerprint", DataType::Utf8, false),
            Field::new("observed_at", DataType::UInt64, false),
        ];
        for c in self.value_columns {
            fields.push(Field::new(*c, DataType::Utf8, false));
        }
        Arc::new(Schema::new(fields))
    }

    /// Every row as `(key, fingerprint, observed_at, values)`.
    fn read(&self) -> Result<Vec<StoredRow>> {
        let Some(reader) =
            crate::fs_gate::columns::open_parquet(&self.path).with_context(|| {
                format!(
                    "read {} (delete it to re-derive this cache)",
                    self.path.display()
                )
            })?
        else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for batch in reader {
            let batch = batch?;
            let col = |name: &str| -> Result<&StringArray> {
                batch
                    .column_by_name(name)
                    .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                    .with_context(|| format!("column {name} is not Utf8"))
            };
            let keys = col("key")?;
            let fps = col("fingerprint")?;
            // `observed_at` predates every reader of it, so a table
            // written before this column was consumed reads as 0 --
            // "older than any window", which refuses reuse rather than
            // inventing freshness.
            let times = batch
                .column_by_name("observed_at")
                .and_then(|c| c.as_any().downcast_ref::<UInt64Array>());
            let value_cols: Vec<&StringArray> = self
                .value_columns
                .iter()
                .map(|c| col(c))
                .collect::<Result<_>>()?;
            for i in 0..batch.num_rows() {
                out.push((
                    keys.value(i).to_string(),
                    fps.value(i).to_string(),
                    times.map(|t| t.value(i)).unwrap_or(0),
                    value_cols.iter().map(|c| c.value(i).to_string()).collect(),
                ));
            }
        }
        Ok(out)
    }

    fn write(&self, rows: &[StoredRow]) -> Result<()> {
        let schema = self.schema();
        let keys: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        let fps: Vec<&str> = rows.iter().map(|r| r.1.as_str()).collect();
        let times: Vec<u64> = rows.iter().map(|r| r.2).collect();
        let mut columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from(keys)) as ArrayRef,
            Arc::new(StringArray::from(fps)),
            Arc::new(UInt64Array::from(times)),
        ];
        for (n, _) in self.value_columns.iter().enumerate() {
            let vals: Vec<&str> = rows
                .iter()
                .map(|r| r.3.get(n).map(String::as_str).unwrap_or_default())
                .collect();
            columns.push(Arc::new(StringArray::from(vals)));
        }
        let batch = RecordBatch::try_new(schema.clone(), columns)?;
        crate::fs_gate::columns::write_parquet_atomic(
            &self.path,
            schema,
            std::iter::once(Ok(batch)),
            ZSTD_LEVEL,
        )
    }
}

/// The cached values for one key, with the fingerprint they were derived
/// under. A caller compares the fingerprint before trusting the values.
pub struct CachedRows {
    pub fingerprint: String,
    /// The observation that last *verified* these values, carried
    /// per entry rather than stamped uniformly at save time.
    ///
    /// `crate::fs_events::EventCoverage::unchanged_since` needs to know
    /// whether a replay window opened before or after these rows were
    /// written, and a uniform save-time stamp would answer "just now"
    /// for an entry that was merely loaded and carried forward
    /// unverified -- which is precisely the entry whose age matters.
    /// A caller that has no freshness claim to make leaves it `0`, and
    /// the table stamps those rows with the save time.
    pub observed_at: u64,
    pub rows: Vec<Vec<String>>,
}

/// A whole table as `key -> CachedRows`. Keys with rows under more than
/// one fingerprint keep the newest read.
fn load(table: &KeyedTable) -> HashMap<String, CachedRows> {
    let mut out: HashMap<String, CachedRows> = HashMap::new();
    // A corrupt cache is re-derived, never fatal: this is derived data,
    // not history. The error is swallowed here and only here.
    let Ok(rows) = table.read() else {
        return out;
    };
    for (key, fingerprint, observed_at, values) in rows {
        let entry = out.entry(key).or_insert_with(|| CachedRows {
            fingerprint: fingerprint.clone(),
            observed_at,
            rows: Vec::new(),
        });
        if entry.fingerprint != fingerprint {
            entry.fingerprint = fingerprint;
            entry.observed_at = observed_at;
            entry.rows.clear();
        }
        entry.observed_at = entry.observed_at.min(observed_at);
        // A fingerprint-only row (every value column empty) records
        // "this key genuinely has nothing", which is a cached answer,
        // not a cached value.
        if values.iter().all(String::is_empty) {
            continue;
        }
        entry.rows.push(values);
    }
    out
}

fn store(table: &KeyedTable, cache: &HashMap<String, CachedRows>, observed_at: u64) -> Result<()> {
    let mut rows: Vec<StoredRow> = Vec::new();
    let mut keys: Vec<&String> = cache.keys().collect();
    keys.sort();
    for key in keys {
        let entry = &cache[key];
        // An entry that made no freshness claim of its own is stamped
        // with this save; one that did keeps it, so carrying an
        // unverified entry forward cannot make it look re-verified.
        let stamped_at = if entry.observed_at == 0 {
            observed_at
        } else {
            entry.observed_at
        };
        if entry.rows.is_empty() {
            // A key with no values still needs its fingerprint recorded,
            // or "this worktree genuinely declares nothing" would be
            // re-derived on every pass.
            rows.push((
                key.clone(),
                entry.fingerprint.clone(),
                stamped_at,
                vec![String::new(); table.value_columns.len()],
            ));
            continue;
        }
        for values in &entry.rows {
            rows.push((
                key.clone(),
                entry.fingerprint.clone(),
                stamped_at,
                values.clone(),
            ));
        }
    }
    table.write(&rows)
}

// ---------------------------------------------------------------------
// The three tables
// ---------------------------------------------------------------------

/// `worktree -> (tool, version, source_file, role)`.
pub struct DeclarationTable(KeyedTable);
/// `worktree -> (ecosystem, name, version)` plus `(ecosystem, message)`
/// gap rows, distinguished by a leading marker column.
pub struct IdentityTable(KeyedTable);
/// `DerivedData subfolder -> workspace path` (or the read error), keyed
/// by the `info.plist`'s own `(size, mtime)`.
pub struct XcodeJoinTable(KeyedTable);

/// `adapter\u{1}kind\u{1}unit path -> one derived string`: the agent
/// adapters' identification cache.
///
/// An adapter derives a small fact from a session's *header* -- the
/// declared working directory, a workspace path, a format marker. Before
/// this table each of those facts was re-derived on every observation,
/// so an unchanged home with 5,000 sessions re-read 5,000 headers
/// (`.oh/sessions/2026-09-21-foundation-repairs.md` recorded the
/// measured 740 KB). The fingerprint is the source file's own
/// `(size, mtime)` plus the adapter version, so an unchanged session is
/// a table lookup and a changed or appended one is read exactly once.
pub struct IdentificationTable(KeyedTable);

/// `adapter\u{1}container directory -> the units that directory
/// produced`, keyed by the stamps of every directory the identification
/// of that container actually listed: the agent family's container-level
/// reuse.
///
/// [`IdentificationTable`] removes the header *reads* from an unchanged
/// pass but not the `stat`s: its validity key is each session file's own
/// `(len, mtime_ns, ctime_ns, inode)`, so knowing a session is unchanged
/// costs one `stat` per session, and a 5,000-session home costs 5,000 of
/// them plus a listing per container. That is the "scales with all
/// files" shape the handoff forbids, and the 2026-09-22 cost measurement
/// recorded it as 10,580 stats over 46 listings.
///
/// This table is the other half, and it is the same bargain
/// `external/folded.parquet` already strikes for external units: one row
/// per **directory** the container's identification listed (never one
/// per file -- the handoff forbids a per-file persistent inventory),
/// plus the units that identification produced. An unchanged container
/// costs one `stat` per recorded directory and no listing at all; a
/// changed container is re-identified file by file, with the per-file
/// identification cache still keeping its header reads at zero.
pub struct ContainerTable(KeyedTable);

/// `store container -> the units a build adapter identified inside one
/// machine-wide build store`: the store family's container-level reuse,
/// the same bargain [`ContainerTable`] strikes for agent homes.
///
/// A store's interior is identified from the folded walk's directory
/// rows, which are only produced when the store is actually measured. An
/// unchanged store is not measured -- its folded total is replayed under
/// the event window -- so its units have to be replayed too, from here,
/// under the same window. One row per *(unit, field)*, never per file
/// inside the store: the unit set is already the adapter's own bounded
/// grouping (a module version, a DerivedData project, a cache category),
/// and the columns stay columns rather than a serialized blob
/// (`.oh/guardrails/store-data-is-parquet-not-json-sidecars.md`). The
/// fingerprint is the swamp version: a new adapter's identification is
/// never answered by an old one's rows.
pub struct BuildStoreTable(KeyedTable);

/// `external unit key -> (consumer label, note)`: the declared-consumer
/// sidecar. Human intent rather than derived data, so it is *not*
/// fingerprint-invalidated -- the fingerprint column is a constant --
/// but it is still per-unit data and so still a table, not a JSON file.
pub struct ConsumerTable(KeyedTable);

/// The fingerprint used for tables whose rows are declarations rather
/// than derivations: nothing invalidates them but an explicit change.
pub const DECLARED_BY_HAND: &str = "declared";

impl DeclarationTable {
    pub fn open(swamp_dir: &Path) -> Self {
        Self(KeyedTable {
            path: dir(swamp_dir).join("declarations.parquet"),
            value_columns: &["tool", "version", "source_file", "role"],
        })
    }
    pub fn load(&self) -> HashMap<String, CachedRows> {
        load(&self.0)
    }
    pub fn save(&self, cache: &HashMap<String, CachedRows>, observed_at: u64) -> Result<()> {
        store(&self.0, cache, observed_at)
    }
}

impl IdentityTable {
    pub fn open(swamp_dir: &Path) -> Self {
        Self(KeyedTable {
            path: dir(swamp_dir).join("dependency_identities.parquet"),
            value_columns: &["row_kind", "ecosystem", "name", "version"],
        })
    }
    pub fn load(&self) -> HashMap<String, CachedRows> {
        load(&self.0)
    }
    pub fn save(&self, cache: &HashMap<String, CachedRows>, observed_at: u64) -> Result<()> {
        store(&self.0, cache, observed_at)
    }
}

impl XcodeJoinTable {
    pub fn open(swamp_dir: &Path) -> Self {
        Self(KeyedTable {
            path: dir(swamp_dir).join("xcode_derived_data.parquet"),
            value_columns: &["outcome", "detail"],
        })
    }
    pub fn load(&self) -> HashMap<String, CachedRows> {
        load(&self.0)
    }
    pub fn save(&self, cache: &HashMap<String, CachedRows>, observed_at: u64) -> Result<()> {
        store(&self.0, cache, observed_at)
    }
}

impl IdentificationTable {
    pub fn open(swamp_dir: &Path) -> Self {
        Self(KeyedTable {
            path: dir(swamp_dir).join("agent_identifications.parquet"),
            value_columns: &["value"],
        })
    }
    pub fn load(&self) -> HashMap<String, CachedRows> {
        load(&self.0)
    }
    pub fn save(&self, cache: &HashMap<String, CachedRows>, observed_at: u64) -> Result<()> {
        store(&self.0, cache, observed_at)
    }
}

impl ContainerTable {
    /// The value columns, in order. `row_kind` distinguishes the three
    /// row shapes this table holds: `dir` (a directory whose stamp the
    /// fingerprint covers), `unit` (an identified unit) and `member` (a
    /// member of the unit above it). Columns, not a JSON blob in a
    /// Parquet cell: `.oh/guardrails/store-data-is-parquet-not-json-sidecars.md`
    /// is about the shape of the data, not only the file extension.
    pub const COLUMNS: &'static [&'static str] = &[
        "row_kind",
        "path",
        "category",
        "rel_path",
        "bytes",
        "mtime_max",
        "action",
        "note",
        "protected",
        "protect_reason",
        "link_kind",
        "link_declared",
        "link_reason",
    ];

    pub fn open(swamp_dir: &Path) -> Self {
        Self(KeyedTable {
            path: dir(swamp_dir).join("agent_containers.parquet"),
            value_columns: Self::COLUMNS,
        })
    }
    pub fn load(&self) -> HashMap<String, CachedRows> {
        load(&self.0)
    }
    pub fn save(&self, cache: &HashMap<String, CachedRows>, observed_at: u64) -> Result<()> {
        store(&self.0, cache, observed_at)
    }
}

impl BuildStoreTable {
    pub const COLUMNS: &'static [&'static str] = &["unit", "field", "value"];

    pub fn open(swamp_dir: &Path) -> Self {
        Self(KeyedTable {
            path: dir(swamp_dir).join("build_stores.parquet"),
            value_columns: Self::COLUMNS,
        })
    }
    pub fn load(&self) -> HashMap<String, CachedRows> {
        load(&self.0)
    }
    pub fn save(&self, cache: &HashMap<String, CachedRows>, observed_at: u64) -> Result<()> {
        store(&self.0, cache, observed_at)
    }
}

impl ConsumerTable {
    pub fn open(swamp_dir: &Path) -> Self {
        Self(KeyedTable {
            path: dir(swamp_dir).join("external_consumers.parquet"),
            value_columns: &["label", "note"],
        })
    }
    pub fn load(&self) -> HashMap<String, CachedRows> {
        load(&self.0)
    }
    pub fn save(&self, cache: &HashMap<String, CachedRows>, observed_at: u64) -> Result<()> {
        store(&self.0, cache, observed_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_round_trips_rows_and_fingerprints() {
        let tmp = tempfile::tempdir().unwrap();
        let table = DeclarationTable::open(tmp.path());
        let mut cache = HashMap::new();
        cache.insert(
            "/proj/a".to_string(),
            CachedRows {
                fingerprint: "tool-versions@100".into(),
                observed_at: 0,
                rows: vec![vec![
                    "nodejs".into(),
                    "20.11.1".into(),
                    ".tool-versions".into(),
                    "project".into(),
                ]],
            },
        );
        table.save(&cache, 1_000).unwrap();
        let back = table.load();
        assert_eq!(back.len(), 1);
        let entry = &back["/proj/a"];
        assert_eq!(entry.fingerprint, "tool-versions@100");
        assert_eq!(entry.rows[0][1], "20.11.1");
    }

    #[test]
    fn a_key_with_no_values_still_records_its_fingerprint() {
        // Otherwise "this worktree declares nothing" is re-derived on
        // every pass, which is exactly the cost the cache exists to
        // avoid.
        let tmp = tempfile::tempdir().unwrap();
        let table = IdentityTable::open(tmp.path());
        let mut cache = HashMap::new();
        cache.insert(
            "/proj/empty".to_string(),
            CachedRows {
                fingerprint: "none".into(),
                observed_at: 0,
                rows: Vec::new(),
            },
        );
        table.save(&cache, 1_000).unwrap();
        let back = table.load();
        assert_eq!(back["/proj/empty"].fingerprint, "none");
        assert!(back["/proj/empty"].rows.is_empty());
    }

    #[test]
    fn the_store_holds_parquet_not_json() {
        let tmp = tempfile::tempdir().unwrap();
        let table = XcodeJoinTable::open(tmp.path());
        table.save(&HashMap::new(), 1).unwrap();
        let files: Vec<String> = std::fs::read_dir(tmp.path().join("associations"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            files.iter().all(|f| f.ends_with(".parquet")),
            "the association store must be columnar: {files:?}"
        );
    }

    #[test]
    fn a_corrupt_cache_is_re_derived_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        let table = DeclarationTable::open(tmp.path());
        std::fs::create_dir_all(tmp.path().join("associations")).unwrap();
        std::fs::write(
            tmp.path().join("associations/declarations.parquet"),
            b"not parquet",
        )
        .unwrap();
        assert!(table.load().is_empty());
    }
}

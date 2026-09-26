use crate::entities::*;
use anyhow::Result;
use arrow_array::{ArrayRef, RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use std::{path::PathBuf, sync::Arc};

#[derive(Debug, Clone)]
pub struct Store {
    pub root: PathBuf,
}

impl Store {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        crate::fs_gate::store::StoreDir::at(&root)?.create()?;
        Ok(Self { root })
    }
    fn path(&self, volume: u64) -> PathBuf {
        self.root.join(format!("volume-{volume}.parquet"))
    }
    pub fn write(&self, observation: &Observation) -> Result<()> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("path", DataType::Utf8, false),
            Field::new("bytes", DataType::UInt64, false),
            Field::new("kind", DataType::Utf8, false),
            Field::new("project_id", DataType::Utf8, true),
            Field::new("observed_at", DataType::UInt64, false),
        ]));
        let paths: Vec<String> = observation
            .artifacts
            .iter()
            .map(|a| a.path.display().to_string())
            .collect();
        let bytes: Vec<u64> = observation.artifacts.iter().map(|a| a.bytes).collect();
        let kinds: Vec<String> = observation
            .artifacts
            .iter()
            .map(|a| format!("{:?}", a.kind))
            .collect();
        let projects: Vec<Option<String>> = observation
            .artifacts
            .iter()
            .map(|a| a.project_id.clone())
            .collect();
        let observed = vec![observation.observed_at; observation.artifacts.len()];
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(paths)) as ArrayRef,
                Arc::new(UInt64Array::from(bytes)),
                Arc::new(StringArray::from(kinds)),
                Arc::new(StringArray::from(projects)),
                Arc::new(UInt64Array::from(observed)),
            ],
        )?;
        crate::fs_gate::columns::write_parquet_atomic(
            &self.path(observation.volume_id),
            schema,
            std::iter::once(Ok(batch)),
            crate::fs_gate::columns::DEFAULT_ZSTD_LEVEL,
        )
    }
    pub fn volume_path(&self, volume: u64) -> PathBuf {
        self.path(volume)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{ScanOptions, observation};
    use std::fs;
    use tempfile::tempdir;
    #[test]
    fn writes_real_parquet() {
        let d = tempdir().unwrap();
        let obs = observation(&ScanOptions {
            roots: vec![d.path().to_path_buf()],
            cross_device: false,
            max_depth: Some(1),
        })
        .unwrap();
        let store = Store::open(d.path().join("state")).unwrap();
        store.write(&obs).unwrap();
        let data = fs::read(store.volume_path(obs.volume_id)).unwrap();
        assert_eq!(&data[data.len() - 4..], b"PAR1");
    }
}

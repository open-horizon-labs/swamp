//! The one Parquet writer and the one Parquet reader
//! (`.oh/guardrails/column-store-parquet-zstd.md`).
//!
//! A caller chooses a zstd *level*, never a codec: there is no
//! uncompressed or snappy writer to reach for, because `parquet` is named
//! nowhere else. Writes are atomic -- rows go to a sibling temp file,
//! which is renamed over the target only after the writer closed cleanly
//! and the bytes were synced. A process killed mid-write (this happened:
//! a SIGKILL during `observe` left a `current.parquet` whose footer never
//! landed, and every later run died reading it) loses the new
//! observation, never the store.

use anyhow::{Context, Result};
use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::{WriterProperties, WriterVersion};
use std::path::Path;

/// A reader over one Parquet file's record batches.
pub use parquet::arrow::arrow_reader::ParquetRecordBatchReader;

fn zstd_properties(level: i32) -> WriterProperties {
    let level = ZstdLevel::try_new(level).unwrap_or_default();
    WriterProperties::builder()
        .set_compression(Compression::ZSTD(level))
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .build()
}

/// The default zstd level (the codec's own default).
pub const DEFAULT_ZSTD_LEVEL: i32 = 3;

/// Writes `batches` as one zstd-compressed Parquet file at `path`,
/// atomically. Creates the parent directory.
pub fn write_parquet_atomic(
    path: &Path,
    schema: SchemaRef,
    batches: impl IntoIterator<Item = Result<RecordBatch>>,
    zstd_level: i32,
) -> Result<()> {
    write_parquet_atomic_with_dictionary_disabled(path, schema, batches, zstd_level, &[])
}

/// Writes one zstd Parquet file atomically, optionally disabling dictionary
/// encoding for named UTF-8 columns whose measured contents compress better
/// without a dictionary. The selected columns must exist and be UTF-8; this
/// keeps caller mistakes visible while leaving every other column on Parquet's
/// standard defaults.
pub fn write_parquet_atomic_with_dictionary_disabled(
    path: &Path,
    schema: SchemaRef,
    batches: impl IntoIterator<Item = Result<RecordBatch>>,
    zstd_level: i32,
    columns: &[&str],
) -> Result<()> {
    let mut properties = zstd_properties(zstd_level).into_builder();
    for column in columns {
        let field = schema
            .field_with_name(column)
            .with_context(|| format!("dictionary override names missing column {column}"))?;
        if field.data_type() != &arrow_schema::DataType::Utf8 {
            anyhow::bail!("dictionary override column {column} is not UTF-8");
        }
        properties = properties.set_column_dictionary_enabled(
            parquet::schema::types::ColumnPath::from(*column),
            false,
        );
    }
    let properties = properties.build();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Unique even for concurrent writes within the same process/second.
    // RAII removes a failed partial stream; readers retain the prior file.
    let tmp = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
    {
        let file = tmp.reopen()?;
        let mut writer = ArrowWriter::try_new(file, schema, Some(properties))?;
        for batch in batches {
            writer.write(&batch?)?;
            writer.flush()?;
        }
        // `close` writes the footer and the trailing magic; until it
        // returns the file is not a Parquet file at all.
        writer.close()?;
    }
    tmp.as_file().sync_all()?;
    tmp.persist(path)
        .map_err(|e| e.error)
        .with_context(|| format!("publish {}", path.display()))?;
    Ok(())
}

/// Opens `path` for reading. `Ok(None)` when there is no file (an
/// ordinary "never written" store); a file that exists but is not
/// readable Parquet is an error naming the file, so a caller can say
/// "delete it to rebuild".
pub fn open_parquet(path: &Path) -> Result<Option<ParquetRecordBatchReader>> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("open {}", path.display())),
    };
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .and_then(|b| b.build())
        .with_context(|| format!("read {}", path.display()))?;
    Ok(Some(reader))
}

/// Whether `path` ends in the Parquet magic (`PAR1`): the cheap
/// "is this a Parquet file at all" check, reading only the last four
/// bytes.
pub fn has_parquet_footer(path: &Path) -> Result<bool> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).with_context(|| format!("read {}", path.display()))?;
    let len = f.metadata()?.len();
    if len < 4 {
        return Ok(false);
    }
    f.seek(SeekFrom::End(-4))?;
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic)?;
    Ok(&magic == b"PAR1")
}

/// Retires a superseded history file (a merged or expired delta). Only
/// `.parquet` files swamp wrote.
pub fn retire(path: &Path) -> Result<()> {
    if path.extension().is_none_or(|e| e != "parquet") {
        anyhow::bail!("{} is not a history table file", path.display());
    }
    super::store::remove_owned_file(path).with_context(|| format!("remove {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Array, ArrayRef, Int32Array, RecordBatchReader, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use parquet::file::reader::{FileReader, SerializedFileReader};
    use std::sync::Arc;
    use std::time::Instant;

    /// `.oh/guardrails/column-store-parquet-zstd.md`, at run time: every
    /// column chunk the one writer produces is zstd, whatever level the
    /// caller asked for (an out-of-range level falls back to the default
    /// level, never to another codec).
    #[test]
    fn every_column_chunk_written_is_zstd() {
        let dir = tempfile::tempdir().unwrap();
        let schema = Arc::new(Schema::new(vec![
            Field::new("key", DataType::Utf8, false),
            Field::new("mod_time_min", DataType::Int32, false),
        ]));
        for level in [1, DEFAULT_ZSTD_LEVEL, 9, 999] {
            let path = dir.path().join(format!("t{level}.parquet"));
            let batch = RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(StringArray::from(vec!["a", "b"])) as ArrayRef,
                    Arc::new(Int32Array::from(vec![1, 2])) as ArrayRef,
                ],
            )
            .unwrap();
            write_parquet_atomic(&path, schema.clone(), [Ok(batch)], level).unwrap();
            assert!(has_parquet_footer(&path).unwrap());
            let reader = SerializedFileReader::new(std::fs::File::open(&path).unwrap()).unwrap();
            let meta = reader.metadata();
            assert!(meta.num_row_groups() > 0);
            for rg in meta.row_groups() {
                for col in rg.columns() {
                    assert!(
                        matches!(col.compression(), Compression::ZSTD(_)),
                        "level {level}: column {} is {:?}",
                        col.column_path(),
                        col.compression()
                    );
                }
            }
        }
    }

    fn normalized_rows(batches: &[RecordBatch]) -> Vec<(Option<String>, String, i32)> {
        batches
            .iter()
            .flat_map(|batch| {
                let key = batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap();
                let group = batch
                    .column(1)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap();
                let value = batch
                    .column(2)
                    .as_any()
                    .downcast_ref::<Int32Array>()
                    .unwrap();
                (0..batch.num_rows())
                    .map(|row| {
                        (
                            key.is_valid(row).then(|| key.value(row).to_owned()),
                            group.value(row).to_owned(),
                            value.value(row),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn normalized_digest(batches: &[RecordBatch]) -> blake3::Hash {
        let mut hash = blake3::Hasher::new();
        for batch in batches {
            for row in 0..batch.num_rows() {
                for column in batch.columns() {
                    hash.update(format!("{:?}", column.slice(row, 1)).as_bytes());
                    hash.update(&[0]);
                }
                hash.update(&[1]);
            }
        }
        hash.finalize()
    }

    #[test]
    fn dictionary_override_is_scoped_nullable_roundtrip_and_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let schema = Arc::new(Schema::new(vec![
            Field::new("path", DataType::Utf8, true),
            Field::new("kind", DataType::Utf8, false),
            Field::new("value", DataType::Int32, false),
        ]));
        let batch = |paths: Vec<Option<String>>, kinds: Vec<&str>, values: Vec<i32>| {
            RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(StringArray::from(paths)) as ArrayRef,
                    Arc::new(StringArray::from(kinds)),
                    Arc::new(Int32Array::from(values)),
                ],
            )
            .unwrap()
        };
        let first = batch(
            vec![Some("root/a".into()), None, Some("root/b".into())],
            vec!["source", "source", "source"],
            vec![1, 2, 3],
        );
        let empty = RecordBatch::new_empty(schema.clone());
        let last = batch(vec![Some("root/c".into())], vec!["source"], vec![4]);
        let path = dir.path().join("scoped.parquet");
        write_parquet_atomic_with_dictionary_disabled(
            &path,
            schema.clone(),
            [Ok(first.clone()), Ok(empty), Ok(last.clone())],
            3,
            &["path"],
        )
        .unwrap();

        let reader = SerializedFileReader::new(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(reader.metadata().num_row_groups(), 2);
        let columns = reader.metadata().row_group(0).columns();
        assert_eq!(columns[0].dictionary_page_offset(), None);
        assert!(columns[1].dictionary_page_offset().is_some());
        assert!(
            columns
                .iter()
                .all(|c| matches!(c.compression(), Compression::ZSTD(_)))
        );
        let decoded: Vec<_> = open_parquet(&path)
            .unwrap()
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            normalized_rows(&decoded),
            normalized_rows(&[first.clone(), last.clone()])
        );

        let before = std::fs::read(&path).unwrap();
        let failed = [
            Ok(first.clone()),
            Err(anyhow::anyhow!("injected iterator failure")),
        ];
        assert!(
            write_parquet_atomic_with_dictionary_disabled(
                &path,
                schema.clone(),
                failed,
                3,
                &["path"],
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(
            write_parquet_atomic_with_dictionary_disabled(
                &path,
                schema.clone(),
                [Ok(first.clone())],
                3,
                &["missing"],
            )
            .is_err()
        );
        assert!(
            write_parquet_atomic_with_dictionary_disabled(
                &path,
                schema,
                [Ok(first)],
                3,
                &["value"],
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    /// Repeats the three measured dictionary overrides against a disposable
    /// seed, comparing decoded rows and reporting only bytes and write time.
    #[test]
    #[ignore = "manual storage benchmark; requires SWAMP_COLUMN_BENCH_SEED_DIR"]
    fn measured_dictionary_overrides_real_store_benchmark() {
        let root = std::path::PathBuf::from(
            std::env::var_os("SWAMP_COLUMN_BENCH_SEED_DIR").expect("seed directory path"),
        );
        let mut targets = Vec::new();
        let agent_members = root.join("agent_unit_members.parquet");
        if agent_members.is_file() {
            targets.push(("agent_unit_members.parquet".to_owned(), "path", 9));
        }
        for volume in std::fs::read_dir(&root).unwrap().flatten() {
            let volume_name = volume.file_name();
            if !volume_name
                .to_string_lossy()
                .bytes()
                .all(|byte| byte.is_ascii_digit())
            {
                continue;
            }
            for (base_name, base_column) in
                [("current.parquet", "kind"), ("dirs.parquet", "rel_path")]
            {
                let base_path = volume.path().join(base_name);
                if base_path.is_file() {
                    targets.push((
                        base_path
                            .strip_prefix(&root)
                            .unwrap()
                            .to_string_lossy()
                            .into_owned(),
                        base_column,
                        9,
                    ));
                }
            }
            for (directory, column, level) in
                [("deltas", "kind", 9), ("dirs_deltas", "rel_path", 3)]
            {
                let delta_dir = volume.path().join(directory);
                let Ok(entries) = std::fs::read_dir(&delta_dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path
                        .extension()
                        .is_some_and(|extension| extension == "parquet")
                    {
                        targets.push((
                            path.strip_prefix(&root)
                                .unwrap()
                                .to_string_lossy()
                                .into_owned(),
                            column,
                            level,
                        ));
                    }
                }
            }
        }
        let output = tempfile::tempdir().unwrap();
        let mut total_default_bytes = 0u64;
        let mut total_hinted_bytes = 0u64;
        for (index, (relative, column, level)) in targets.into_iter().enumerate() {
            let input = ParquetRecordBatchReaderBuilder::try_new(
                std::fs::File::open(root.join(&relative)).unwrap(),
            )
            .unwrap()
            .with_batch_size(usize::MAX)
            .build()
            .unwrap();
            let schema = input.schema();
            let batches: Vec<_> = input.map(Result::unwrap).collect();
            let expected = normalized_digest(&batches);
            let paths = [
                output.path().join(format!("{index}-default.parquet")),
                output.path().join(format!("{index}-hinted.parquet")),
            ];
            let mut times = [Vec::new(), Vec::new()];
            for repetition in 0..3 {
                let order = if repetition % 2 == 0 {
                    [0usize, 1]
                } else {
                    [1usize, 0]
                };
                for variant in order {
                    let started = Instant::now();
                    if variant == 0 {
                        write_parquet_atomic(
                            &paths[variant],
                            schema.clone(),
                            batches.iter().cloned().map(Ok),
                            level,
                        )
                        .unwrap();
                    } else {
                        write_parquet_atomic_with_dictionary_disabled(
                            &paths[variant],
                            schema.clone(),
                            batches.iter().cloned().map(Ok),
                            level,
                            &[column],
                        )
                        .unwrap();
                    }
                    times[variant].push(started.elapsed().as_millis());
                }
            }
            for path in &paths {
                let decoded: Vec<_> = open_parquet(path)
                    .unwrap()
                    .unwrap()
                    .map(Result::unwrap)
                    .collect();
                assert_eq!(normalized_digest(&decoded), expected);
            }
            let median = |values: &mut Vec<u128>| {
                values.sort_unstable();
                values[values.len() / 2]
            };
            let default_bytes = std::fs::metadata(&paths[0]).unwrap().len();
            let hinted_bytes = std::fs::metadata(&paths[1]).unwrap().len();
            total_default_bytes += default_bytes;
            total_hinted_bytes += hinted_bytes;
            println!(
                "table={relative} column={column} level={level} rows={} default_bytes={default_bytes} hinted_bytes={hinted_bytes} default_median_ms={} hinted_median_ms={} repetitions=3",
                batches.iter().map(RecordBatch::num_rows).sum::<usize>(),
                median(&mut times[0]),
                median(&mut times[1]),
            );
        }
        println!(
            "selected_tables_and_deltas default_total_bytes={total_default_bytes} hinted_total_bytes={total_hinted_bytes}"
        );
    }
}

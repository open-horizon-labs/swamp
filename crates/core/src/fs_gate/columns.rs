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
    let properties = zstd_properties(zstd_level);
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
    use arrow_array::{ArrayRef, Int32Array, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use parquet::file::reader::{FileReader, SerializedFileReader};
    use std::sync::Arc;

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
}

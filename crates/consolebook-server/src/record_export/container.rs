//! Deterministic ZIP container writer and naming helpers shared across
//! record exports, trainee packets, and installation exports (ADR 0014; #47).

use std::io::{self, Cursor, Seek, SeekFrom, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use time::OffsetDateTime;
use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

use super::{
    RECORD_FILE, Scope, UNIT_FORMAT, UNIT_FORMAT_VERSION, UNIT_MANIFEST_FILE, UnitEntry,
    UnitManifest, VersionRow,
};
use crate::canonical;

/// The container writer every export shares: stored entries, the export
/// instant as each entry's modification time, `0644`, entries in the
/// order they are added. The sink need only be seekable; a streamed
/// export passes [`crate::export_stream::EntryBuffer`], which holds one
/// local header at a time and yields the same bytes (#47).
///
/// A container is finished only by [`Self::finish`] or
/// [`Self::into_sink`]. One dropped before that is a failed export: the
/// ZIP writer would otherwise finalize itself on drop and append a central
/// directory to whatever was already written, so a transfer that failed part
/// way could reach the client as a readable archive. Dropping seals the sink
/// first, and nothing more reaches it (#215).
pub(crate) struct ArchiveWriter<W: Write + Seek> {
    writer: Option<ZipWriter<Sealable<W>>>,
    sealed: Arc<AtomicBool>,
    options: SimpleFileOptions,
}

impl<W: Write + Seek> Drop for ArchiveWriter<W> {
    fn drop(&mut self) {
        if self.writer.is_some() {
            self.sealed.store(true, Ordering::Release);
        }
    }
}

/// The sink as the ZIP writer sees it: transparent until sealed, then a
/// sink that discards everything while keeping a consistent position (the
/// writer seeks and checks offsets as it finalizes), so the writer's own
/// finalize-on-drop writes nothing and has nothing to report.
struct Sealable<W> {
    inner: W,
    sealed: Arc<AtomicBool>,
    /// The position and end of the discarding sink, from the moment it
    /// was sealed.
    discarded: Option<(u64, u64)>,
}

impl<W: Seek> Sealable<W> {
    /// The discarding sink's position and end, if sealed: it starts where
    /// the real sink stood.
    fn discarding(&mut self) -> io::Result<Option<&mut (u64, u64)>> {
        if self.discarded.is_none() && self.sealed.load(Ordering::Acquire) {
            let at = self.inner.stream_position()?;
            self.discarded = Some((at, at));
        }
        Ok(self.discarded.as_mut())
    }
}

impl<W: Write + Seek> Write for Sealable<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some((pos, end)) = self.discarding()? {
            *pos += buf.len() as u64;
            *end = (*end).max(*pos);
            return Ok(buf.len());
        }
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.discarding()?.is_some() {
            return Ok(());
        }
        self.inner.flush()
    }
}

impl<W: Seek> Seek for Sealable<W> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        if let Some((pos, end)) = self.discarding()? {
            let target = match to {
                SeekFrom::Start(at) => Some(at),
                SeekFrom::Current(by) => pos.checked_add_signed(by),
                SeekFrom::End(by) => end.checked_add_signed(by),
            };
            *pos = target.ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
            return Ok(*pos);
        }
        self.inner.seek(to)
    }
}

impl ArchiveWriter<Cursor<Vec<u8>>> {
    /// The in-memory writer the packet and the compatibility tests use.
    pub(crate) fn in_memory(exported_at: i64) -> Result<Self> {
        Self::new(Cursor::new(Vec::new()), exported_at)
    }

    pub(crate) fn finish(self) -> Result<Vec<u8>> {
        Ok(self.into_sink()?.into_inner())
    }
}

impl<W: Write + Seek> ArchiveWriter<W> {
    pub(crate) fn new(sink: W, exported_at: i64) -> Result<Self> {
        let sealed = Arc::new(AtomicBool::new(false));
        Ok(Self {
            writer: Some(ZipWriter::new(Sealable {
                inner: sink,
                sealed: Arc::clone(&sealed),
                discarded: None,
            })),
            sealed,
            options: SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored)
                .last_modified_time(dos_time(exported_at)?)
                .unix_permissions(0o644),
        })
    }

    /// Finishes the container, returning the sink it wrote into.
    pub(crate) fn into_sink(mut self) -> Result<W> {
        let writer = self.writer.take().context("archive already finished")?;
        // Taken out first, so the drop below sees a finished container and
        // leaves the sink unsealed.
        let sink = writer.finish().context("finishing the export archive")?;
        Ok(sink.inner)
    }

    fn zip(&mut self) -> Result<&mut ZipWriter<Sealable<W>>> {
        self.writer.as_mut().context("archive already finished")
    }

    pub(crate) fn add(&mut self, name: &str, bytes: &[u8]) -> Result<()> {
        let options = self.options;
        let writer = self.zip()?;
        writer
            .start_file(name, options)
            .with_context(|| format!("starting archive entry {name}"))?;
        writer
            .write_all(bytes)
            .with_context(|| format!("writing archive entry {name}"))?;
        Ok(())
    }

    /// Adds every unit — `record.json` then `manifest.json` — consuming
    /// the rows so each version's bytes are released once written.
    pub(crate) fn add_units(
        &mut self,
        installation_id: &str,
        exported_at: i64,
        rows: Vec<VersionRow>,
        units: &[UnitEntry],
    ) -> Result<()> {
        for (row, entry) in rows.into_iter().zip(units) {
            self.add(&format!("{}/{RECORD_FILE}", entry.path), &row.bytes)?;
            let unit = UnitManifest {
                format: UNIT_FORMAT.to_owned(),
                format_version: UNIT_FORMAT_VERSION,
                installation_id: installation_id.to_owned(),
                exported_at,
                record_id: entry.record_id,
                version_number: entry.version_number,
                record_schema: entry.record_schema,
                content_hash: entry.content_hash.clone(),
                chain_hash: entry.chain_hash.clone(),
                predecessor_content_hash: entry.predecessor_content_hash.clone(),
            };
            self.add(
                &format!("{}/{UNIT_MANIFEST_FILE}", entry.path),
                &canonical_json(&unit)?,
            )?;
        }
        Ok(())
    }
}

/// Manifests are canonical JSON under the record format's subset, so
/// the archive is deterministic and a manifest is itself checkable.
pub(crate) fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let value = serde_json::to_value(value).context("serializing manifest")?;
    canonical::canonical_bytes(&value)
}

fn dos_time(exported_at: i64) -> Result<zip::DateTime> {
    let at = OffsetDateTime::from_unix_timestamp(exported_at).context("export instant")?;
    zip::DateTime::from_date_and_time(
        u16::try_from(at.year()).context("export year")?,
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second(),
    )
    .map_err(|_| anyhow!("export instant {exported_at} is outside the ZIP date range"))
}

/// The download-name stamp for an export instant: `YYYYMMDDTHHMMSSZ`.
pub(crate) fn stamp(exported_at: i64) -> Result<String> {
    OffsetDateTime::from_unix_timestamp(exported_at)
        .context("export instant")?
        .format(&time::macros::format_description!(
            "[year][month][day]T[hour][minute][second]Z"
        ))
        .context("formatting export instant")
}

pub(crate) fn file_name(scope: Scope, exported_at: i64) -> Result<String> {
    let stamp = stamp(exported_at)?;
    let scope_part = match scope {
        Scope::Version {
            record_id,
            version_number,
        } => format!("record-{record_id}-v{version_number}"),
        Scope::Record { record_id } => format!("record-{record_id}"),
        Scope::Enrollment { enrollment_id } => format!("enrollment-{enrollment_id}"),
        Scope::Installation => "installation".to_owned(),
    };
    Ok(format!("consolebook-{scope_part}-{stamp}.zip"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPORTED_AT: i64 = 1_756_753_200;
    /// The end-of-central-directory signature a readable ZIP ends with.
    const EOCD: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];

    /// A sink that keeps what it was given after the writer is gone.
    #[derive(Clone, Default)]
    struct Shared(Arc<std::sync::Mutex<Cursor<Vec<u8>>>>);

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().expect("sink").write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Seek for Shared {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.0.lock().expect("sink").seek(pos)
        }
    }

    fn has_central_directory(bytes: &[u8]) -> bool {
        bytes.windows(EOCD.len()).any(|w| w == EOCD)
    }

    #[test]
    fn a_dropped_writer_leaves_no_readable_archive() {
        let sink = Shared::default();
        let mut writer = ArchiveWriter::new(sink.clone(), EXPORTED_AT).expect("writer");
        writer.add("manifest.json", b"{}").expect("entry");
        drop(writer);
        let bytes = sink.0.lock().expect("sink").get_ref().clone();
        assert!(
            !bytes.is_empty(),
            "the entry written before the failure stays"
        );
        assert!(
            !has_central_directory(&bytes),
            "a failed export must not be finalized into a readable archive"
        );
        assert!(zip::ZipArchive::new(Cursor::new(bytes)).is_err());
    }

    #[test]
    fn a_finished_writer_is_a_readable_archive() {
        let sink = Shared::default();
        let mut writer = ArchiveWriter::new(sink.clone(), EXPORTED_AT).expect("writer");
        writer.add("manifest.json", b"{}").expect("entry");
        writer.into_sink().expect("finish");
        let bytes = sink.0.lock().expect("sink").get_ref().clone();
        assert!(has_central_directory(&bytes));
        assert_eq!(
            zip::ZipArchive::new(Cursor::new(bytes)).expect("zip").len(),
            1
        );
    }
}

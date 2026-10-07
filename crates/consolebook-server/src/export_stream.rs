//! A `Write + Seek` sink that lets the export archive stream to a
//! non-seekable destination while staying byte-identical to the buffered
//! archive (#47).
//!
//! `docs/formats/record-export.md` fixes the container's bytes: stored
//! entries, a fixed entry order, and every entry's modification time and
//! permissions. A `zip` writer over a non-seekable sink would not produce
//! those bytes — it cannot go back and fill in an entry's CRC-32 and
//! sizes, so it writes zeros in the local header, sets the data-descriptor
//! flag, and appends a 16-byte descriptor after the payload
//! (`zip::write::ZipWriter::new_stream`). Verification would still accept
//! such an archive, but byte compatibility with the shipped exporter would
//! be gone, and issue #47 requires both.
//!
//! Writing one entry with `ZipWriter::new` produces this sequence:
//!
//! ```text
//! seek(stream_position)          the entry's header_start
//! write(local header)            magic, fields, name, extra field
//! write(payload)                 the record's stored bytes
//! seek(header_start + 14)        back to the CRC-32 field
//! write(crc32) write(sizes)      the patch, three short writes
//! seek(file_end)                 forward again, to the archive's end
//! ```
//!
//! A patch reaches into bytes the destination has already been given, so
//! the entry it belongs to cannot be handed on before the patch is known.
//! This sink therefore holds the entry being written — its local header
//! and its payload — and applies a patch to it in place:
//!
//! - an entry begins when a write starts with the ZIP local-header magic;
//!   the previous entry is complete by then and is released;
//! - a seek that lands **inside the entry being held** is a patch, in
//!   whichever direction it came from: the CRC-32 and size patch seeks
//!   backwards to the local header's fixed fields, and a ZIP64 entry's
//!   extra-field update seeks forwards to it. Both write into the held
//!   bytes at the position asked for;
//! - a seek to or past the entry's end closes it and hands it to the
//!   destination as one append.
//!
//! Peak memory is one entry — a record's stored bytes and its local
//! header — plus, while the container finalizes, the central directory.
//! Both terms are named in ADR 0014's costs; neither is the corpus, and
//! the entry term is bounded by the largest stored record rather than by
//! the history's size.

use std::io::{self, Seek, SeekFrom, Write};

/// The first bytes of a ZIP local file header (APPNOTE 6.3), which mark
/// where one entry's bytes end and the next begins.
const LOCAL_HEADER_MAGIC: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];

/// A seekable view of an append-only `Write` sink: one entry of memory,
/// real positions, and no rewinding.
pub struct EntryBuffer<W: Write + Seek> {
    sink: W,
    /// The entry being written: its local header, then its payload.
    entry: Vec<u8>,
    /// Where the entry being written began.
    entry_at: u64,
    /// Whether an entry is open.
    open: bool,
    /// Where the writer's next write logically lands.
    cursor: u64,
    /// The destination's end: everything already sent.
    delivered: u64,
    /// Whether the writer is writing into the open entry rather than
    /// appending to it: the local-header or ZIP64-extra-field patch.
    patching: bool,
    /// The most bytes one entry ever held.
    peak_entry: usize,
}

impl<W: Write + Seek> EntryBuffer<W> {
    /// Wraps `sink`.
    #[must_use]
    pub fn new(sink: W) -> Self {
        Self {
            sink,
            entry: Vec::new(),
            entry_at: 0,
            open: false,
            cursor: 0,
            delivered: 0,
            patching: false,
            peak_entry: 0,
        }
    }

    /// The most bytes held for one entry: its local header and payload,
    /// and at finalization the central directory that follows the last
    /// entry. `0` before anything is written.
    #[must_use]
    pub fn peak_entry_bytes(&self) -> usize {
        self.peak_entry
    }

    fn remember_peak(&mut self) {
        self.peak_entry = self.peak_entry.max(self.entry.len());
    }

    /// Hands the open entry to the destination and closes it.
    fn release_entry(&mut self) -> io::Result<()> {
        if !self.open {
            return Ok(());
        }
        self.remember_peak();
        let entry = std::mem::take(&mut self.entry);
        // The destination only ever appends; a completed entry is written
        // where its stream has reached, which is where it began.
        self.sink.seek(SeekFrom::Start(self.delivered))?;
        self.sink.write_all(&entry)?;
        self.delivered = self.entry_at + entry.len() as u64;
        self.patching = false;
        self.open = false;
        Ok(())
    }

    /// The underlying sink, once the container is complete. An entry left
    /// open by an unfinished container is released first, so no buffered
    /// byte is dropped silently.
    pub fn into_inner(mut self) -> io::Result<W> {
        self.release_entry()?;
        Ok(self.sink)
    }
}

impl<W: Write + Seek> Write for EntryBuffer<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.patching {
            // The writer sought into the entry it is still writing; its
            // bytes belong at the position it asked for, which the entry
            // already reaches or is grown to reach.
            let offset =
                usize::try_from(self.cursor.saturating_sub(self.entry_at)).unwrap_or(usize::MAX);
            let end = offset.checked_add(buf.len()).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "entry position overflow")
            })?;
            if end > self.entry.len() {
                self.entry.resize(end, 0);
            }
            self.entry[offset..end].copy_from_slice(buf);
            self.cursor += buf.len() as u64;
            self.remember_peak();
            return Ok(buf.len());
        }
        if !self.open || (self.cursor >= self.delivered && buf.starts_with(&LOCAL_HEADER_MAGIC)) {
            // A local file header begins an entry; the previous one is
            // complete now and goes out.
            self.release_entry()?;
            self.open = true;
            self.entry_at = self.cursor;
        }
        self.entry.extend_from_slice(buf);
        self.cursor += buf.len() as u64;
        self.remember_peak();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.sink.flush()
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        let mut total = 0;
        for buf in bufs {
            total += self.write(buf)?;
        }
        Ok(total)
    }
}

impl<W: Write + Seek> Seek for EntryBuffer<W> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match pos {
            SeekFrom::Start(at) => {
                let entry_end = self.entry_at.saturating_add(self.entry.len() as u64);
                // Inside the open entry is where the container's patches
                // land; anywhere else is an ordinary move.
                self.patching = self.open && at >= self.entry_at && at < entry_end;
                self.cursor = at;
                Ok(at)
            }
            SeekFrom::Current(by) => {
                let at = i64::try_from(self.cursor)
                    .ok()
                    .and_then(|base| base.checked_add(by))
                    .filter(|at| *at >= 0)
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, "negative position")
                    })?;
                self.seek(SeekFrom::Start(at.cast_unsigned()))
            }
            SeekFrom::End(by) => {
                if by != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "seeking backwards from the end of a streamed archive",
                    ));
                }
                // The container is complete: the last entry — which by now
                // carries the central directory and the end record — goes
                // out, and the stream ends where the archive does.
                self.release_entry()?;
                self.cursor = self.delivered;
                Ok(self.delivered)
            }
        }
    }
}

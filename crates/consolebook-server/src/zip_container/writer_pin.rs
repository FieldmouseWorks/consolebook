//! The export container writer's bytes, pinned across builds (#136).
//!
//! Every export, packet, and installation export is written by
//! `record_export::ArchiveWriter`, and `docs/formats/record-export.md`
//! (Container, Determinism) promises the same inputs at the same instant
//! give the same bytes — from any build, not just this one. A `zip` crate
//! upgrade that adds an extra field, sets a header flag, or reorders a
//! record would pass every round-trip test while breaking that promise,
//! so the digest below pins the container production rules. If it
//! changes, the container changed: update the format document and say
//! so; never just re-record the digest.

use sha2::{Digest, Sha256};

use super::central_directory_entries;
use crate::record_export::ArchiveWriter;

/// An odd second, so the DOS time's two-second rounding is exercised:
/// 2026-09-21T14:13:21Z.
const EXPORTED_AT: i64 = 1_790_000_001;
/// 14:13:20 — DOS time keeps even seconds: hour << 11, minute << 5,
/// second / 2.
const DOS_TIME: u16 = 14 * 2048 + 13 * 32 + 20 / 2;
/// 2026-09-21: (year - 1980) << 9, month << 5, day.
const DOS_DATE: u16 = (2026 - 1980) * 512 + 9 * 32 + 21;
/// A regular file with permissions `0644`, in the high half.
const EXTERNAL_ATTRIBUTES: u32 = 0o100_644 << 16;

/// SHA-256 of [`archive`]'s bytes, as zip 8.6.0 and 9.0.3 both write them.
const PINNED_SHA256: &str = "3e90eec8af74485f517cb1cb7cf3eb393112bc28632006495faf771f99e06f55";

const RECORD_BYTES: usize = 300 * 1024;

/// Fixed invented entries: a small manifest, a record large enough for
/// any size-dependent choice in the writer, and its unit manifest.
fn archive() -> Vec<u8> {
    let mut state: u32 = 0x1357_9bdf;
    let record: Vec<u8> = (0..RECORD_BYTES)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state.to_be_bytes()[0]
        })
        .collect();
    let mut writer = ArchiveWriter::in_memory(EXPORTED_AT).expect("writer");
    writer
        .add("manifest.json", br#"{"format":"invented"}"#)
        .expect("manifest");
    writer
        .add("records/1/v1/record.json", &record)
        .expect("record");
    writer
        .add("records/1/v1/manifest.json", br#"{"unit":"invented"}"#)
        .expect("unit manifest");
    writer.finish().expect("finish")
}

#[test]
fn the_container_writer_produces_the_pinned_bytes() {
    let bytes = archive();
    assert_eq!(
        crate::canonical::to_hex(&Sha256::digest(&bytes)),
        PINNED_SHA256,
        "the export container's bytes changed; see this module's documentation"
    );
}

/// The rules the digest stands for, readable on their own: stored,
/// `0644`, the export instant, and nothing extra in either header.
#[test]
fn every_entry_carries_the_specified_metadata() {
    let bytes = archive();
    let entries = central_directory_entries(&bytes).expect("central directory");
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "manifest.json",
            "records/1/v1/record.json",
            "records/1/v1/manifest.json"
        ]
    );
    for entry in &entries {
        assert_eq!(entry.method, 0, "stored: {entry:?}");
        assert_eq!(entry.flags, 0, "no flags: {entry:?}");
        assert_eq!(entry.external_attributes, EXTERNAL_ATTRIBUTES, "{entry:?}");
        assert_eq!((entry.dos_time, entry.dos_date), (DOS_TIME, DOS_DATE));
        assert_eq!((entry.extra_len, entry.comment_len), (0, 0), "{entry:?}");
        let local = usize::try_from(entry.local_header_offset).expect("offset");
        let local_extra = u16::from_le_bytes([bytes[local + 28], bytes[local + 29]]);
        assert_eq!(local_extra, 0, "local header extra field: {entry:?}");
    }
}

//! Structured record exports: the format vocabulary and the producing
//! side (ADR 0014; docs/formats/record-export.md; #45).
//!
//! An export unit is one finalized version's stored canonical bytes,
//! copied verbatim, beside a canonical-JSON unit manifest; an archive
//! is a ZIP container with stored entries in a fixed order holding an
//! archive manifest and its units. The archive is a pure function of
//! the scope's stored rows and the export instant, so the same scope
//! exported at the same instant is byte-identical. Export follows the
//! read rules that already exist — a unit contains exactly what its
//! reader may already read — and the installation scope takes
//! `export_records`. Verification from the archive alone is
//! `export_verify`'s, which reads the manifests defined here.

use std::fmt;
use std::io::{Cursor, Seek, Write};

use anyhow::{Context, Result, anyhow};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection, SqlitePool};
use time::OffsetDateTime;
use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

use crate::audit::{self, EventKind, Subject};
use crate::canonical;
use crate::capabilities::{self, Capability};
use crate::evaluation_drafts;
use crate::lifecycle;
use crate::storage;

/// Archive-manifest discriminator; never changes.
pub const ARCHIVE_FORMAT: &str = "consolebook-record-export";
/// Unit-manifest discriminator; never changes.
pub const UNIT_FORMAT: &str = "consolebook-record-unit";
/// Shared by both manifests; bumped by any change to either shape.
pub const FORMAT_VERSION: i64 = 1;
/// The archive manifest's entry name.
pub const ARCHIVE_MANIFEST_PATH: &str = "manifest.json";
/// The canonical record bytes within a unit directory.
pub const RECORD_FILE: &str = "record.json";
/// The unit manifest within a unit directory.
pub const UNIT_MANIFEST_FILE: &str = "manifest.json";

/// What an archive claims to contain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scope {
    /// Exactly one finalized version.
    Version { record_id: i64, version_number: i64 },
    /// Every retained version of one record, superseded originals
    /// included.
    Record { record_id: i64 },
    /// Every finalized version of every record of one enrollment.
    Enrollment { enrollment_id: i64 },
    /// Every finalized version the installation holds.
    Installation,
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version {
                record_id,
                version_number,
            } => write!(f, "record {record_id}, version {version_number}"),
            Self::Record { record_id } => {
                write!(f, "record {record_id}, every retained version")
            }
            Self::Enrollment { enrollment_id } => write!(f, "enrollment {enrollment_id}"),
            Self::Installation => f.write_str("the whole installation"),
        }
    }
}

/// Typed refusals for the export act.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportRefusal {
    NoSuchRecord,
    NoSuchVersion,
    NoSuchEnrollment,
    CapabilityRequired,
    /// The scope exists but holds no finalized version; an empty
    /// archive is never presented as a complete export.
    NothingToExport,
    /// Producing or delivering the archive failed after the scope was
    /// known to hold versions. This is not an empty scope, and it is
    /// never presented as one.
    ExportFailed,
}

/// One unit as the archive manifest lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitEntry {
    pub path: String,
    pub record_id: i64,
    pub version_number: i64,
    pub record_schema: i64,
    pub content_hash: String,
    pub chain_hash: String,
    pub predecessor_content_hash: Option<String>,
}

/// The archive manifest (`manifest.json` at the root).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveManifest {
    pub format: String,
    pub format_version: i64,
    pub installation_id: String,
    pub exported_at: i64,
    pub scope: Scope,
    pub units: Vec<UnitEntry>,
}

/// The unit manifest beside each unit's record bytes. It repeats what
/// the archive manifest says so a unit directory stands on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitManifest {
    pub format: String,
    pub format_version: i64,
    pub installation_id: String,
    pub exported_at: i64,
    pub record_id: i64,
    pub version_number: i64,
    pub record_schema: i64,
    pub content_hash: String,
    pub chain_hash: String,
    pub predecessor_content_hash: Option<String>,
}

/// A produced archive, ready to deliver.
#[derive(Debug)]
pub struct Export {
    /// The documented download name, `consolebook-<scope>-<stamp>.zip`.
    pub file_name: String,
    pub bytes: Vec<u8>,
    pub exported_at: i64,
    pub unit_count: usize,
}

/// What a produced archive states about itself. The bytes are not here:
/// a streamed export writes them as it produces them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveProduced {
    /// The documented download name, `consolebook-<scope>-<stamp>.zip`.
    pub file_name: String,
    pub exported_at: i64,
    pub unit_count: usize,
}

/// The unit directory for one version: `records/{record_id}/v{n}`.
#[must_use]
pub fn unit_path(record_id: i64, version_number: i64) -> String {
    format!("records/{record_id}/v{version_number}")
}

// ------------------------------------------------------------ producing

/// Exports `scope` now, for an actor the scope's read rule admits.
pub async fn export(
    pool: &SqlitePool,
    actor_user_id: i64,
    scope: Scope,
) -> Result<std::result::Result<Export, ExportRefusal>> {
    export_at(
        pool,
        actor_user_id,
        scope,
        OffsetDateTime::now_utc().unix_timestamp(),
    )
    .await
}

/// Exports `scope` stamped with `exported_at` (UTC unix seconds),
/// writing the container into `sink` as it is produced (#47).
///
/// The archive is a pure function of the scope's rows and this instant,
/// and its bytes are the same whichever sink receives them. Two passes
/// over one read transaction produce it: the first reads every unit's
/// stored fingerprints — not its bytes — so the archive manifest, the
/// container's first entry, can be written before any record byte is
/// read; the second streams each unit's stored bytes and its unit
/// manifest straight into the sink. What is held is the entry metadata
/// (one small record per unit, O(units), and the serialized manifest that
/// lists it), one unit's bytes at a time, and the container's central
/// directory — never the corpus of payloads.
///
/// The export never holds more than one pooled connection at a time: the
/// audit is written in its own short write transaction, which commits
/// before the read snapshot is taken, and that snapshot is a reader from
/// beginning to end. An export therefore reserves no writer while it
/// streams (ADR 0019), and exports sharing a pool never wait for a
/// connection another export holds.
///
/// The audit precedes both content passes, so a scope with nothing to
/// export is refused before anything is recorded, and a typed refusal
/// still reaches the caller as one. A recorded `record_exported` attests
/// that the installation produced this export from the state at this
/// instant; it does not attest that the operator received or saved the
/// file, and a delivery that fails part way leaves the record it already
/// wrote — while the client sees an incomplete transfer rather than a
/// complete export.
///
/// `ready` is called with what the archive states about itself once the
/// scope is authorized, the rows are read, and the export is audited: a
/// streamed delivery starts its response there and reports a failure after
/// that point as an incomplete transfer, never as a complete export.
pub async fn export_to<W: Write + Seek, F: FnOnce(ArchiveProduced)>(
    pool: &SqlitePool,
    actor_user_id: i64,
    scope: Scope,
    exported_at: i64,
    sink: W,
    ready: F,
) -> Result<(std::result::Result<ArchiveProduced, ExportRefusal>, W)> {
    let audited = match authorize(pool, actor_user_id, scope).await? {
        Ok(audited) => audited,
        // The sink comes back with the refusal so the caller can flush and
        // close whatever destination it handed over.
        Err(refusal) => return Ok((Err(refusal), sink)),
    };
    // The audit is written first, by itself, on one connection: it must be
    // committed before the first byte, it must never be written for a scope
    // that holds nothing to export, and it must not be part of the
    // transaction the payloads stream from. `storage::write_tx` reserves
    // the writer for the length of two short statements (ADR 0019).
    {
        let mut tx = storage::write_tx(pool)
            .await
            .context("beginning audit write")?;
        if !scope_has_units(&mut tx, scope).await? {
            // Nothing to export is never audited, and never exported empty.
            tx.rollback().await.context("rolling back empty export")?;
            return Ok((Err(nothing_to_export(scope)), sink));
        }
        audit_export(&mut *tx, actor_user_id, &audited).await?;
        tx.commit().await.context("committing audit write")?;
    }
    // One read transaction for both content passes: the manifest and the
    // payloads describe the same committed state and the same export
    // instant. It is a reader from beginning to end — the audit is already
    // committed — so a download, however slow, reserves no writer, and the
    // export holds exactly one connection throughout.
    let mut tx = pool.begin().await.context("beginning export read")?;
    let installation_id = storage::installation_id(&mut *tx).await?;
    let read = async {
        let units = collect_meta(&mut tx, scope).await?;
        if units.is_empty() {
            // The scope held units when it was audited, and a finalized
            // version is immutable and removed only by an authorized
            // disposition, which this installation does not yet perform.
            // Rather than export emptiness the audit does not describe,
            // the export fails: the caller may retry, and the record
            // stands for the attempt.
            return Err(anyhow!(
                "the audited scope no longer holds a finalized version to export"
            ));
        }
        let produced = ArchiveProduced {
            file_name: file_name(scope, exported_at)?,
            exported_at,
            unit_count: units.len(),
        };
        // The scope is authorized, the rows are read, and the export is
        // recorded: the delivery may start now.
        ready(produced.clone());
        let manifest = archive_manifest(&installation_id, exported_at, scope, &units);
        let mut writer = ArchiveWriter::new(sink, exported_at)?;
        writer.add(ARCHIVE_MANIFEST_PATH, &canonical_json(&manifest)?)?;
        write_units_streaming(&mut tx, &mut writer, scope, &installation_id, exported_at).await?;
        let sink = writer.into_sink()?;
        Ok((produced, sink))
    }
    .await;
    let (produced, sink) = match read {
        Ok((produced, sink)) => (produced, sink),
        Err(err) => return Err(err),
    };
    tx.commit().await.context("ending export read")?;
    Ok((Ok(produced), sink))
}

/// The refusal an empty scope earns: a version scope that names a missing
/// version, anything else with no finalized version at all.
fn nothing_to_export(scope: Scope) -> ExportRefusal {
    match scope {
        Scope::Version { .. } => ExportRefusal::NoSuchVersion,
        Scope::Record { .. } | Scope::Enrollment { .. } | Scope::Installation => {
            ExportRefusal::NothingToExport
        }
    }
}

/// Whether the scope holds at least one finalized version, read in the
/// caller's transaction so the answer and the audit below it describe one
/// state.
async fn scope_has_units(conn: &mut SqliteConnection, scope: Scope) -> Result<bool> {
    let found: Option<i64> = match scope {
        Scope::Version {
            record_id,
            version_number,
        } => sqlx::query_scalar(
            "SELECT v.id FROM evaluation_version v
             WHERE v.evaluation_record_id = ?1 AND v.version_number = ?2 LIMIT 1",
        )
        .bind(record_id)
        .bind(version_number)
        .fetch_optional(&mut *conn)
        .await
        .context("checking the version scope")?,
        Scope::Record { record_id } => sqlx::query_scalar(
            "SELECT v.id FROM evaluation_version v
             WHERE v.evaluation_record_id = ?1 LIMIT 1",
        )
        .bind(record_id)
        .fetch_optional(&mut *conn)
        .await
        .context("checking the record scope")?,
        Scope::Enrollment { enrollment_id } => sqlx::query_scalar(
            "SELECT v.id FROM evaluation_version v
             JOIN evaluation_record r ON r.id = v.evaluation_record_id
             WHERE r.enrollment_id = ?1 LIMIT 1",
        )
        .bind(enrollment_id)
        .fetch_optional(&mut *conn)
        .await
        .context("checking the enrollment scope")?,
        Scope::Installation => sqlx::query_scalar("SELECT v.id FROM evaluation_version v LIMIT 1")
            .fetch_optional(&mut *conn)
            .await
            .context("checking the installation scope")?,
    };
    Ok(found.is_some())
}

/// Exports `scope` stamped with `exported_at` (UTC unix seconds). The
/// archive is a pure function of the scope's rows and this instant.
pub async fn export_at(
    pool: &SqlitePool,
    actor_user_id: i64,
    scope: Scope,
    exported_at: i64,
) -> Result<std::result::Result<Export, ExportRefusal>> {
    let (produced, bytes) = export_to(
        pool,
        actor_user_id,
        scope,
        exported_at,
        Cursor::new(Vec::new()),
        |_| {},
    )
    .await?;
    let produced = match produced {
        Ok(produced) => produced,
        Err(refusal) => return Ok(Err(refusal)),
    };
    Ok(Ok(Export {
        file_name: produced.file_name,
        bytes: bytes.into_inner(),
        exported_at: produced.exported_at,
        unit_count: produced.unit_count,
    }))
}

/// Counts for the installation-export interface, for `export_records`
/// holders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportSummary {
    pub installation_id: String,
    pub record_count: i64,
    pub version_count: i64,
}

/// How many finalized records and versions an installation export
/// would carry.
pub async fn summary(
    pool: &SqlitePool,
    actor_user_id: i64,
) -> Result<std::result::Result<ExportSummary, ExportRefusal>> {
    if !capabilities::user_has(pool, actor_user_id, Capability::ExportRecords).await? {
        return Ok(Err(ExportRefusal::CapabilityRequired));
    }
    let (record_count, version_count): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(DISTINCT evaluation_record_id), COUNT(*) FROM evaluation_version",
    )
    .fetch_one(pool)
    .await
    .context("counting finalized versions")?;
    Ok(Ok(ExportSummary {
        installation_id: storage::installation_id(pool).await?,
        record_count,
        version_count,
    }))
}

/// What the audit event names once the export exists.
struct Audited {
    subject: Option<Subject>,
    trainee: Option<i64>,
}

/// Records one export in the caller's transaction.
///
/// Deliberately not the transaction the payloads stream from: an audit row
/// committed with the payload pass would hold a write reservation for the
/// whole download (ADR 0019). The caller runs this in its own short write
/// transaction, before the read snapshot is taken, so the record is
/// committed before the first byte — and a scope that holds nothing to
/// export never reaches it.
async fn audit_export<'e>(
    executor: impl sqlx::Executor<'e, Database = sqlx::Sqlite>,
    actor_user_id: i64,
    audited: &Audited,
) -> Result<()> {
    match audited.subject {
        Some(subject) => {
            audit::record_for_subject(
                executor,
                EventKind::RecordExported,
                Some(actor_user_id),
                audited.trainee,
                subject,
            )
            .await?;
        }
        None => {
            audit::record(
                executor,
                EventKind::RecordExported,
                Some(actor_user_id),
                None,
            )
            .await?;
        }
    }
    Ok(())
}

/// The scope's read rule, as the typed contract it already is elsewhere
/// (ADR 0010): the record read rule for a version or record, the
/// training-history read rule for an enrollment, and the explicit
/// `export_records` authority for the installation.
async fn authorize(
    pool: &SqlitePool,
    actor_user_id: i64,
    scope: Scope,
) -> Result<std::result::Result<Audited, ExportRefusal>> {
    match scope {
        Scope::Version { record_id, .. } | Scope::Record { record_id } => {
            let mut conn = pool.acquire().await.context("acquiring connection")?;
            let Some(record) = evaluation_drafts::load_record(&mut conn, record_id).await? else {
                return Ok(Err(ExportRefusal::NoSuchRecord));
            };
            drop(conn);
            if !crate::draft_access::may_read(pool, actor_user_id, &record).await? {
                return Ok(Err(ExportRefusal::CapabilityRequired));
            }
            let trainee: i64 = sqlx::query_scalar("SELECT user_id FROM enrollment WHERE id = ?1")
                .bind(record.enrollment_id)
                .fetch_one(pool)
                .await
                .context("reading enrollment")?;
            Ok(Ok(Audited {
                subject: Some(Subject::Record(record_id)),
                trainee: Some(trainee),
            }))
        }
        Scope::Enrollment { enrollment_id } => {
            let trainee: Option<i64> =
                sqlx::query_scalar("SELECT user_id FROM enrollment WHERE id = ?1")
                    .bind(enrollment_id)
                    .fetch_optional(pool)
                    .await
                    .context("reading enrollment")?;
            let Some(trainee) = trainee else {
                return Ok(Err(ExportRefusal::NoSuchEnrollment));
            };
            if !lifecycle::may_read(pool, actor_user_id, enrollment_id).await? {
                return Ok(Err(ExportRefusal::CapabilityRequired));
            }
            Ok(Ok(Audited {
                subject: Some(Subject::Enrollment(enrollment_id)),
                trainee: Some(trainee),
            }))
        }
        Scope::Installation => {
            if !capabilities::user_has(pool, actor_user_id, Capability::ExportRecords).await? {
                return Ok(Err(ExportRefusal::CapabilityRequired));
            }
            Ok(Ok(Audited {
                subject: None,
                trainee: None,
            }))
        }
    }
}

/// One stored version, exactly as the archive carries it.
pub(crate) struct VersionRow {
    pub(crate) record_id: i64,
    pub(crate) version_number: i64,
    pub(crate) record_schema: i64,
    pub(crate) bytes: Vec<u8>,
    pub(crate) content_hash: String,
    pub(crate) chain_hash: String,
    pub(crate) predecessor_content_hash: Option<String>,
}

/// The stored rows of a scope in archive order: ascending record id,
/// then version number. The predecessor's content hash is read from
/// its own row, so the manifest states what the chain hash was
/// computed over (ADR 0011).
macro_rules! unit_query {
    ($where:literal) => {
        concat!(
            "SELECT v.evaluation_record_id AS record_id, v.version_number,
                    v.record_schema, v.canonical_bytes, v.content_hash,
                    v.chain_hash, p.content_hash AS predecessor_content_hash
             FROM evaluation_version v
             LEFT JOIN evaluation_version p ON p.id = v.predecessor_id
             JOIN evaluation_record r ON r.id = v.evaluation_record_id ",
            $where,
            " ORDER BY v.evaluation_record_id, v.version_number"
        )
    };
}

/// The same rows and order, without the stored bytes: what the archive
/// manifest needs and nothing that would make the metadata pass read the
/// corpus.
macro_rules! unit_meta_query {
    ($where:literal) => {
        concat!(
            "SELECT v.evaluation_record_id AS record_id, v.version_number,
                    v.record_schema, v.content_hash,
                    v.chain_hash, p.content_hash AS predecessor_content_hash
             FROM evaluation_version v
             LEFT JOIN evaluation_version p ON p.id = v.predecessor_id
             JOIN evaluation_record r ON r.id = v.evaluation_record_id ",
            $where,
            " ORDER BY v.evaluation_record_id, v.version_number"
        )
    };
}

pub(crate) async fn collect(conn: &mut SqliteConnection, scope: Scope) -> Result<Vec<VersionRow>> {
    let rows = match scope {
        Scope::Version {
            record_id,
            version_number,
        } => {
            sqlx::query(unit_query!(
                "WHERE v.evaluation_record_id = ?1 AND v.version_number = ?2"
            ))
            .bind(record_id)
            .bind(version_number)
            .fetch_all(&mut *conn)
            .await
        }
        Scope::Record { record_id } => {
            sqlx::query(unit_query!("WHERE v.evaluation_record_id = ?1"))
                .bind(record_id)
                .fetch_all(&mut *conn)
                .await
        }
        Scope::Enrollment { enrollment_id } => {
            sqlx::query(unit_query!("WHERE r.enrollment_id = ?1"))
                .bind(enrollment_id)
                .fetch_all(&mut *conn)
                .await
        }
        Scope::Installation => sqlx::query(unit_query!("")).fetch_all(&mut *conn).await,
    }
    .context("reading finalized versions")?;
    Ok(rows
        .iter()
        .map(|row| VersionRow {
            record_id: row.get("record_id"),
            version_number: row.get("version_number"),
            record_schema: row.get("record_schema"),
            bytes: row.get("canonical_bytes"),
            content_hash: row.get("content_hash"),
            chain_hash: row.get("chain_hash"),
            predecessor_content_hash: row.get("predecessor_content_hash"),
        })
        .collect())
}

/// The unit list for `rows`, in archive order.
pub(crate) fn unit_entries(rows: &[VersionRow]) -> Vec<UnitEntry> {
    rows.iter()
        .map(|row| UnitEntry {
            path: unit_path(row.record_id, row.version_number),
            record_id: row.record_id,
            version_number: row.version_number,
            record_schema: row.record_schema,
            content_hash: row.content_hash.clone(),
            chain_hash: row.chain_hash.clone(),
            predecessor_content_hash: row.predecessor_content_hash.clone(),
        })
        .collect()
}

/// One stored version's metadata, without its bytes: everything both
/// manifests need. The export reads this first so the archive manifest —
/// which must be the container's first entry — can be written before any
/// record byte is read, while the payloads themselves stay out of memory
/// (#47).
type UnitMeta = UnitEntry;

/// The metadata pass: every unit of the scope in archive order, with the
/// stored fingerprints but not the stored bytes. The metadata columns are
/// selected on their own — a `canonical_bytes` read here would double the
/// corpus I/O and hold a blob at a time for a length and a CRC-32 the
/// container writer computes for itself. Rows are consumed one at a time,
/// so what this pass holds is the one small record per unit that the
/// archive manifest has to carry.
pub(crate) async fn collect_meta(
    conn: &mut SqliteConnection,
    scope: Scope,
) -> Result<Vec<UnitMeta>> {
    let mut rows = match scope {
        Scope::Version {
            record_id,
            version_number,
        } => sqlx::query(unit_meta_query!(
            "WHERE v.evaluation_record_id = ?1 AND v.version_number = ?2"
        ))
        .bind(record_id)
        .bind(version_number)
        .fetch(&mut *conn),
        Scope::Record { record_id } => {
            sqlx::query(unit_meta_query!("WHERE v.evaluation_record_id = ?1"))
                .bind(record_id)
                .fetch(&mut *conn)
        }
        Scope::Enrollment { enrollment_id } => {
            sqlx::query(unit_meta_query!("WHERE r.enrollment_id = ?1"))
                .bind(enrollment_id)
                .fetch(&mut *conn)
        }
        Scope::Installation => sqlx::query(unit_meta_query!("")).fetch(&mut *conn),
    };
    let mut units = Vec::new();
    while let Some(row) = rows
        .try_next()
        .await
        .context("reading finalized versions")?
    {
        units.push(UnitMeta {
            path: unit_path(row.get("record_id"), row.get("version_number")),
            record_id: row.get("record_id"),
            version_number: row.get("version_number"),
            record_schema: row.get("record_schema"),
            content_hash: row.get("content_hash"),
            chain_hash: row.get("chain_hash"),
            predecessor_content_hash: row.get("predecessor_content_hash"),
        });
    }
    Ok(units)
}

/// The payload pass: streams each unit's stored bytes and its unit
/// manifest into an already-started archive, in archive order. Peak
/// memory is one version's bytes at a time.
pub(crate) async fn write_units_streaming<W: Write + Seek>(
    conn: &mut SqliteConnection,
    writer: &mut ArchiveWriter<W>,
    scope: Scope,
    installation_id: &str,
    exported_at: i64,
) -> Result<()> {
    let mut rows = match scope {
        Scope::Version {
            record_id,
            version_number,
        } => sqlx::query(unit_query!(
            "WHERE v.evaluation_record_id = ?1 AND v.version_number = ?2"
        ))
        .bind(record_id)
        .bind(version_number)
        .fetch(&mut *conn),
        Scope::Record { record_id } => {
            sqlx::query(unit_query!("WHERE v.evaluation_record_id = ?1"))
                .bind(record_id)
                .fetch(&mut *conn)
        }
        Scope::Enrollment { enrollment_id } => {
            sqlx::query(unit_query!("WHERE r.enrollment_id = ?1"))
                .bind(enrollment_id)
                .fetch(&mut *conn)
        }
        Scope::Installation => sqlx::query(unit_query!("")).fetch(&mut *conn),
    };
    while let Some(row) = rows
        .try_next()
        .await
        .context("reading finalized versions")?
    {
        let record_id: i64 = row.get("record_id");
        let version_number: i64 = row.get("version_number");
        let bytes: Vec<u8> = row.get("canonical_bytes");
        let path = unit_path(record_id, version_number);
        writer.add(&format!("{path}/{RECORD_FILE}"), &bytes)?;
        drop(bytes);
        let unit = UnitManifest {
            format: UNIT_FORMAT.to_owned(),
            format_version: FORMAT_VERSION,
            installation_id: installation_id.to_owned(),
            exported_at,
            record_id,
            version_number,
            record_schema: row.get("record_schema"),
            content_hash: row.get("content_hash"),
            chain_hash: row.get("chain_hash"),
            predecessor_content_hash: row.get("predecessor_content_hash"),
        };
        writer.add(
            &format!("{path}/{UNIT_MANIFEST_FILE}"),
            &canonical_json(&unit)?,
        )?;
    }
    Ok(())
}

/// The archive manifest for `units`, in the container's entry order.
pub(crate) fn archive_manifest(
    installation_id: &str,
    exported_at: i64,
    scope: Scope,
    units: &[UnitEntry],
) -> ArchiveManifest {
    ArchiveManifest {
        format: ARCHIVE_FORMAT.to_owned(),
        format_version: FORMAT_VERSION,
        installation_id: installation_id.to_owned(),
        exported_at,
        scope,
        units: units.to_vec(),
    }
}

/// The container writer every export shares: stored entries, the export
/// instant as each entry's modification time, `0644`, entries in the
/// order they are added. The sink need only be seekable; a streamed
/// export passes [`crate::export_stream::EntryBuffer`], which holds one
/// local header at a time and yields the same bytes (#47).
pub(crate) struct ArchiveWriter<W: Write + Seek> {
    writer: ZipWriter<W>,
    options: SimpleFileOptions,
}

impl ArchiveWriter<Cursor<Vec<u8>>> {
    /// The in-memory writer the packet and the compatibility tests use.
    pub(crate) fn in_memory(exported_at: i64) -> Result<Self> {
        Self::new(Cursor::new(Vec::new()), exported_at)
    }

    pub(crate) fn finish(self) -> Result<Vec<u8>> {
        let cursor = self
            .writer
            .finish()
            .context("finishing the export archive")?;
        Ok(cursor.into_inner())
    }
}

impl<W: Write + Seek> ArchiveWriter<W> {
    pub(crate) fn new(sink: W, exported_at: i64) -> Result<Self> {
        Ok(Self {
            writer: ZipWriter::new(sink),
            options: SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored)
                .last_modified_time(dos_time(exported_at)?)
                .unix_permissions(0o644),
        })
    }

    /// Finishes the container, returning the sink it wrote into.
    pub(crate) fn into_sink(self) -> Result<W> {
        self.writer.finish().context("finishing the export archive")
    }

    pub(crate) fn add(&mut self, name: &str, bytes: &[u8]) -> Result<()> {
        self.writer
            .start_file(name, self.options)
            .with_context(|| format!("starting archive entry {name}"))?;
        self.writer
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
                format_version: FORMAT_VERSION,
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

fn file_name(scope: Scope, exported_at: i64) -> Result<String> {
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

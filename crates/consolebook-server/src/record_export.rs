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

mod container;
mod disposition;

use std::fmt;
use std::io::{Cursor, Seek, Write};

use anyhow::{Context, Result, anyhow};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection, SqlitePool};
use time::OffsetDateTime;

pub(crate) use container::{ArchiveWriter, canonical_json, file_name, stamp};
pub use disposition::{DISPOSITION_EVIDENCE_PATH, DispositionEvidenceRef};

use crate::audit::{self, EventKind, Subject};
use crate::capabilities::{self, Capability};
use crate::evaluation_drafts;
use crate::lifecycle;
use crate::storage;

/// Archive-manifest discriminator; never changes.
pub const ARCHIVE_FORMAT: &str = "consolebook-record-export";
/// Unit-manifest discriminator; never changes.
pub const UNIT_FORMAT: &str = "consolebook-record-unit";
/// Current archive-manifest format version (`2` adds `disposition_evidence`
/// and fully-disposed / boundary-closed scope support; ADR 0030 §7; #215).
pub const FORMAT_VERSION: i64 = 2;
/// Legacy archive-manifest format version verified for compatibility.
pub const LEGACY_FORMAT_VERSION: i64 = 1;
/// Unit-manifest format version (shared unchanged across record exports,
/// trainee packets, and installation exports).
pub const UNIT_FORMAT_VERSION: i64 = 1;
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

/// The archive manifest (`manifest.json` at the root) for format version 2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveManifest {
    pub format: String,
    pub format_version: i64,
    pub installation_id: String,
    pub exported_at: i64,
    pub scope: Scope,
    pub units: Vec<UnitEntry>,
    pub disposition_evidence: Option<DispositionEvidenceRef>,
}

/// Legacy format-version 1 archive manifest (retained for compatibility
/// verification under ADR 0030).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveManifestV1 {
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
    pub disposed_record_count: usize,
    pub boundary_checkpoint_count: usize,
}

/// What a produced archive states about itself. The bytes are not here:
/// a streamed export writes them as it produces them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveProduced {
    /// The documented download name, `consolebook-<scope>-<stamp>.zip`.
    pub file_name: String,
    pub exported_at: i64,
    pub unit_count: usize,
    pub disposed_record_count: usize,
    pub boundary_checkpoint_count: usize,
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
/// produce it. The first, in one short read transaction that commits
/// before anything is written, reads every unit's stored fingerprints —
/// not its bytes — so the archive manifest, the container's first entry,
/// can be written before any record byte is read. The second streams each
/// unit's stored bytes and its unit manifest into the sink, reading each
/// version by its key and checking it against the fingerprints the
/// manifest already states: finalized versions are immutable and
/// undeletable (migration `0010`), so payloads and manifest describe one
/// state because that is checked, not because a snapshot is held. What is
/// held is the entry metadata (one small record per unit, O(units), and
/// the serialized manifest that lists it), one unit's bytes at a time,
/// and the container's central directory — never the corpus of payloads.
///
/// No read transaction outlives the metadata pass, and no pooled connection
/// is held while the sink waits on a client: each version is read by one
/// autocommit statement on a connection acquired for that read and returned
/// to the pool before the bytes are written. A download, however slow,
/// therefore reserves no writer (ADR 0019), pins no WAL reader that would
/// hold back a checkpoint, and holds no lease that ordinary work or another
/// export could be starved of; the export holds at most one pooled
/// connection at any moment, and none between reads. The price is that the
/// point reads pass through whichever connections the pool hands out, so
/// their SQLite page caches — at most the pool's size times the per-
/// connection cache limit, a constant the corpus does not grow — count
/// toward the export's memory beside the terms above.
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
/// `cancelled` reports whether the requester has already gone. It is
/// asked before anything is read, before the audit, before the metadata
/// pass, for every metadata row, and before `ready`: an abandoned request stops there with a
/// [`std::io::ErrorKind::BrokenPipe`] error rather than auditing and
/// reading a scope nobody will receive. After `ready`, the sink reports a
/// departed client itself.
///
/// `ready` is called with what the archive states about itself once the
/// scope is authorized, the export is audited, and the metadata is read: a
/// streamed delivery starts its response there and reports a failure after
/// that point as an incomplete transfer, never as a complete export.
pub async fn export_to<W: Write + Seek, F: FnOnce(ArchiveProduced)>(
    pool: &SqlitePool,
    actor_user_id: i64,
    scope: Scope,
    exported_at: i64,
    sink: W,
    cancelled: &(dyn Fn() -> bool + Send + Sync),
    ready: F,
) -> Result<(std::result::Result<ArchiveProduced, ExportRefusal>, W)> {
    if cancelled() {
        return Err(client_left());
    }
    let audited = match authorize(pool, actor_user_id, scope).await? {
        Ok(audited) => audited,
        // The sink comes back with the refusal so the caller can flush and
        // close whatever destination it handed over.
        Err(refusal) => return Ok((Err(refusal), sink)),
    };
    if cancelled() {
        return Err(client_left());
    }
    // The audit is written first, by itself, on one connection: it must be
    // committed before the first byte, it must never be written for a scope
    // that holds nothing to export, and it must not be part of any read the
    // export makes afterwards. `storage::write_tx` reserves the writer for
    // the length of two short statements (ADR 0019).
    {
        let mut tx = storage::write_tx(pool)
            .await
            .context("beginning audit write")?;
        if !scope_has_exportable_state(&mut tx, scope).await? {
            // Nothing to export is never audited, and never exported empty.
            tx.rollback().await.context("rolling back empty export")?;
            return Ok((Err(nothing_to_export(scope)), sink));
        }
        audit_export(&mut *tx, actor_user_id, &audited).await?;
        tx.commit().await.context("committing audit write")?;
    }
    if cancelled() {
        return Err(client_left());
    }
    // One short read transaction for the installation's identity, the
    // retained unit list, and any disposition evidence / boundary checkpoints
    // so manifest.json and disposition/evidence.json describe one coherent
    // committed state. It commits here, before the delivery starts: nothing
    // the client does can hold it open.
    let (installation_id, units, disposition_doc) = {
        let mut tx = pool.begin().await.context("beginning export read")?;
        let installation_id = storage::installation_id(&mut *tx).await?;
        let units = collect_meta(&mut tx, scope, cancelled).await?;
        let disposition_doc = disposition::collect_disposition_evidence(
            &mut tx,
            &installation_id,
            exported_at,
            scope,
        )
        .await?;
        tx.commit().await.context("ending export read")?;
        (installation_id, units, disposition_doc)
    };
    if units.is_empty() && disposition_doc.is_none() {
        // The scope held exportable state when audited, but neither retained
        // versions nor disposition evidence remain now. Rather than export
        // emptiness the audit does not describe, fail visibly so the caller
        // may retry.
        return Err(anyhow!(
            "the audited scope no longer holds finalized versions or disposition evidence to export"
        ));
    }
    if cancelled() {
        return Err(client_left());
    }
    let produced = ArchiveProduced {
        file_name: file_name(scope, exported_at)?,
        exported_at,
        unit_count: units.len(),
        disposed_record_count: disposition_doc
            .as_ref()
            .map_or(0, |d| d.disposed_record_count),
        boundary_checkpoint_count: disposition_doc
            .as_ref()
            .map_or(0, |d| d.boundary_checkpoint_count),
    };
    // The scope is authorized, the export is recorded, and the rows are
    // read: the delivery may start now.
    ready(produced.clone());
    let manifest = archive_manifest(
        &installation_id,
        exported_at,
        scope,
        &units,
        disposition_doc.as_ref().map(|d| d.manifest_ref.clone()),
    );
    let mut writer = ArchiveWriter::new(sink, exported_at)?;
    writer.add(ARCHIVE_MANIFEST_PATH, &canonical_json(&manifest)?)?;
    write_units_streaming(pool, &mut writer, &units, &installation_id, exported_at).await?;
    if let Some(ev) = disposition_doc {
        writer.add(&ev.manifest_ref.path, &ev.bytes)?;
    }
    let sink = writer.into_sink()?;
    Ok((Ok(produced), sink))
}

/// The error an abandoned request ends with: a departed client, which the
/// delivery recognizes and neither logs nor signals.
pub(crate) fn client_left() -> anyhow::Error {
    anyhow::Error::from(std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "the client left before the export started",
    ))
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

/// Whether `scope` holds at least one retained finalized version or
/// applicable disposition evidence / policy boundary checkpoint.
async fn scope_has_exportable_state(conn: &mut SqliteConnection, scope: Scope) -> Result<bool> {
    if scope_has_units(conn, scope).await? {
        return Ok(true);
    }
    disposition::scope_has_disposition_evidence(conn, scope).await
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
        &|| false,
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
        disposed_record_count: produced.disposed_record_count,
        boundary_checkpoint_count: produced.boundary_checkpoint_count,
    }))
}

/// Counts for the installation-export interface, for `export_records`
/// holders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportSummary {
    pub installation_id: String,
    pub record_count: i64,
    pub version_count: i64,
    pub disposed_record_count: i64,
    pub boundary_checkpoint_count: i64,
}

/// How many finalized records, versions, disposed records, and policy
/// boundary checkpoints an installation export would carry.
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
    let disposed_record_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM disposition_event")
        .fetch_one(pool)
        .await
        .context("counting disposition events")?;
    let boundary_checkpoint_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM disposition_boundary_checkpoint")
            .fetch_one(pool)
            .await
            .context("counting disposition boundary checkpoints")?;
    Ok(Ok(ExportSummary {
        installation_id: storage::installation_id(pool).await?,
        record_count,
        version_count,
        disposed_record_count,
        boundary_checkpoint_count,
    }))
}

/// What the audit event names once the export exists.
struct Audited {
    subject: Option<Subject>,
    trainee: Option<i64>,
}

/// Records one export in the caller's transaction.
///
/// Deliberately not part of any read the export makes: an audit row
/// committed with the payload pass would hold a write reservation for the
/// whole download (ADR 0019). The caller runs this in its own short write
/// transaction, before the metadata is read, so the record is committed
/// before the first byte — and a scope that holds nothing to export never
/// reaches it.
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
            if let Some(record) = evaluation_drafts::load_record(&mut conn, record_id).await? {
                drop(conn);
                if !crate::draft_access::may_read(pool, actor_user_id, &record).await? {
                    return Ok(Err(ExportRefusal::CapabilityRequired));
                }
                let trainee: i64 =
                    sqlx::query_scalar("SELECT user_id FROM enrollment WHERE id = ?1")
                        .bind(record.enrollment_id)
                        .fetch_one(pool)
                        .await
                        .context("reading enrollment")?;
                return Ok(Ok(Audited {
                    subject: Some(Subject::Record(record_id)),
                    trainee: Some(trainee),
                }));
            }
            let disposed = disposition::lookup_disposed_record(&mut conn, record_id).await?;
            drop(conn);
            let Some(disposed) = disposed else {
                return Ok(Err(ExportRefusal::NoSuchRecord));
            };
            if !crate::draft_access::may_read_disposed_record(
                pool,
                actor_user_id,
                disposed.enrollment_id,
            )
            .await?
            {
                return Ok(Err(ExportRefusal::CapabilityRequired));
            }
            if let Scope::Version { version_number, .. } = scope
                && (version_number < 1 || version_number > disposed.version_count)
            {
                return Ok(Err(ExportRefusal::NoSuchVersion));
            }
            let trainee: i64 = sqlx::query_scalar("SELECT user_id FROM enrollment WHERE id = ?1")
                .bind(disposed.enrollment_id)
                .fetch_one(pool)
                .await
                .context("reading enrollment for disposed record")?;
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
/// archive manifest has to carry. `cancelled` is asked for every row, so an
/// abandoned request stops reading a large scope part way.
pub(crate) async fn collect_meta(
    conn: &mut SqliteConnection,
    scope: Scope,
    cancelled: &(dyn Fn() -> bool + Send + Sync),
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
        if cancelled() {
            return Err(client_left());
        }
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
/// manifest into an already-started archive, in the manifest's order. Peak
/// memory is one version's bytes at a time.
///
/// Each version is a point read by its key: one autocommit statement on a
/// pooled connection acquired for that read alone, back in the pool before
/// the bytes reach the writer — which may wait on the client — so a stalled
/// download holds no lease and no transaction. The stored fingerprints must
/// be the ones the manifest already states: a version that is gone or
/// differs fails the export rather than shipping a payload its manifest
/// does not describe.
pub(crate) async fn write_units_streaming<W: Write + Seek>(
    pool: &SqlitePool,
    writer: &mut ArchiveWriter<W>,
    units: &[UnitMeta],
    installation_id: &str,
    exported_at: i64,
) -> Result<()> {
    for entry in units {
        let stored: Option<(i64, Vec<u8>, String, String)> = sqlx::query_as(
            "SELECT record_schema, canonical_bytes, content_hash, chain_hash
             FROM evaluation_version
             WHERE evaluation_record_id = ?1 AND version_number = ?2",
        )
        .bind(entry.record_id)
        .bind(entry.version_number)
        .fetch_optional(pool)
        .await
        .with_context(|| format!("reading finalized version {}", entry.path))?;
        let Some((record_schema, bytes, content_hash, chain_hash)) = stored else {
            return Err(anyhow!(
                "a finalized version the manifest lists is no longer stored: {}",
                entry.path
            ));
        };
        if record_schema != entry.record_schema
            || content_hash != entry.content_hash
            || chain_hash != entry.chain_hash
        {
            return Err(anyhow!(
                "finalized version {} no longer matches the fingerprints its manifest states",
                entry.path
            ));
        }
        writer.add(&format!("{}/{RECORD_FILE}", entry.path), &bytes)?;
        drop(bytes);
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
        writer.add(
            &format!("{}/{UNIT_MANIFEST_FILE}", entry.path),
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
    disposition_evidence: Option<DispositionEvidenceRef>,
) -> ArchiveManifest {
    ArchiveManifest {
        format: ARCHIVE_FORMAT.to_owned(),
        format_version: FORMAT_VERSION,
        installation_id: installation_id.to_owned(),
        exported_at,
        scope,
        units: units.to_vec(),
        disposition_evidence,
    }
}

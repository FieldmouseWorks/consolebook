//! Producing the installation export (ADR 0028;
//! docs/formats/installation-export.md, Determinism and consistency).
//!
//! Authorization and the audit come first, as for the record export.
//! Then one read transaction reads the unit list, every carried table in
//! its specified columns and key order, and the installation's identity,
//! and commits before anything is written: every document and count
//! describes one committed state. The archive is then written with the
//! record export's container writer: the manifest, the units streamed and
//! checked against the stored hashes the manifest states, and the table
//! documents in name order. No database transaction is held across the
//! transfer.

use std::io::{Cursor, Seek, Write};

use anyhow::{Context, Result, bail};
use futures_util::TryStreamExt;
use serde_json::{Map, Value};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteConnection, SqlitePool};
use time::OffsetDateTime;

use super::spec::{self, Column, ColumnType, Table};
use super::{
    INSTALLATION_FORMAT, INSTALLATION_FORMAT_VERSION, InstallationManifest, TableEntry,
    excluded_entries,
};
use crate::audit::{self, EventKind};
use crate::canonical;
use crate::capabilities::{self, Capability};
use crate::installation_verify::row_findings;
use crate::record_export::{
    ARCHIVE_MANIFEST_PATH, ArchiveProduced, ArchiveWriter, ExportRefusal, Scope, UnitEntry,
    canonical_json, client_left, collect_meta, stamp, write_units_streaming,
};
use crate::storage;

/// A produced installation export, held in memory.
#[derive(Debug)]
pub struct InstallationExport {
    /// `consolebook-installation-export-<stamp>.zip`.
    pub file_name: String,
    pub bytes: Vec<u8>,
    pub exported_at: i64,
    pub unit_count: usize,
}

/// One table document, serialized and fingerprinted.
struct Document {
    entry: TableEntry,
    bytes: Vec<u8>,
}

/// Exports the installation now.
pub async fn export(
    pool: &SqlitePool,
    actor_user_id: i64,
) -> Result<std::result::Result<InstallationExport, ExportRefusal>> {
    export_at(
        pool,
        actor_user_id,
        OffsetDateTime::now_utc().unix_timestamp(),
    )
    .await
}

/// Exports the installation stamped with `exported_at` (UTC unix
/// seconds), in memory.
pub async fn export_at(
    pool: &SqlitePool,
    actor_user_id: i64,
    exported_at: i64,
) -> Result<std::result::Result<InstallationExport, ExportRefusal>> {
    let (produced, sink) = export_to(
        pool,
        actor_user_id,
        exported_at,
        Cursor::new(Vec::new()),
        &|| false,
        |_| {},
    )
    .await?;
    Ok(produced.map(|produced| InstallationExport {
        file_name: produced.file_name,
        bytes: sink.into_inner(),
        exported_at: produced.exported_at,
        unit_count: produced.unit_count,
    }))
}

/// Exports the installation stamped with `exported_at`, writing the
/// container into `sink` as it is produced.
///
/// Only `export_records` holders may export; anyone else is refused
/// before anything is recorded or read. The export is audited as
/// `installation_exported`, with the actor and no content, in its own
/// short write transaction committed before the snapshot is read, so the
/// archive's audit trail includes the export that produced it. A stored
/// value the build does not understand — an unknown vocabulary value, a
/// flag that is neither `0` nor `1`, an integer outside the canonical
/// range — fails the export before `ready`, never passing through.
///
/// `cancelled` and `ready` mean what they mean for
/// [`crate::record_export::export_to`].
pub async fn export_to<W: Write + Seek, F: FnOnce(ArchiveProduced)>(
    pool: &SqlitePool,
    actor_user_id: i64,
    exported_at: i64,
    sink: W,
    cancelled: &(dyn Fn() -> bool + Send + Sync),
    ready: F,
) -> Result<(std::result::Result<ArchiveProduced, ExportRefusal>, W)> {
    if cancelled() {
        return Err(client_left());
    }
    if !capabilities::user_has(pool, actor_user_id, Capability::ExportRecords).await? {
        return Ok((Err(ExportRefusal::CapabilityRequired), sink));
    }
    if cancelled() {
        return Err(client_left());
    }
    {
        let mut tx = storage::write_tx(pool)
            .await
            .context("beginning audit write")?;
        audit::record(
            &mut *tx,
            EventKind::InstallationExported,
            Some(actor_user_id),
            None,
        )
        .await?;
        tx.commit().await.context("committing audit write")?;
    }
    if cancelled() {
        return Err(client_left());
    }
    // One read transaction for the identity, the unit list, and every
    // table: the archive describes one committed state. It commits before
    // the delivery starts, so nothing the client does can hold it open.
    let (installation_id, units, documents) = {
        let mut tx = pool.begin().await.context("beginning export read")?;
        let installation_id = storage::installation_id(&mut *tx).await?;
        let units = collect_meta(&mut tx, Scope::Installation, cancelled).await?;
        let mut documents = Vec::with_capacity(spec::TABLES.len());
        for table in &spec::TABLES {
            if cancelled() {
                return Err(client_left());
            }
            documents.push(read_document(&mut tx, table, cancelled).await?);
        }
        tx.commit().await.context("ending export read")?;
        (installation_id, units, documents)
    };
    if cancelled() {
        return Err(client_left());
    }
    let produced = ArchiveProduced {
        file_name: file_name(exported_at)?,
        exported_at,
        unit_count: units.len(),
    };
    ready(produced.clone());
    let manifest = manifest(&installation_id, exported_at, units, &documents);
    let mut writer = ArchiveWriter::new(sink, exported_at)?;
    writer.add(ARCHIVE_MANIFEST_PATH, &canonical_json(&manifest)?)?;
    write_units_streaming(
        pool,
        &mut writer,
        &manifest.units,
        &installation_id,
        exported_at,
    )
    .await?;
    for document in documents {
        writer.add(&document.entry.path, &document.bytes)?;
    }
    let sink = writer.into_sink()?;
    Ok((Ok(produced), sink))
}

/// The download name: `consolebook-installation-export-<stamp>.zip`.
fn file_name(exported_at: i64) -> Result<String> {
    Ok(format!(
        "consolebook-installation-export-{}.zip",
        stamp(exported_at)?
    ))
}

fn manifest(
    installation_id: &str,
    exported_at: i64,
    units: Vec<UnitEntry>,
    documents: &[Document],
) -> InstallationManifest {
    InstallationManifest {
        format: INSTALLATION_FORMAT.to_owned(),
        format_version: INSTALLATION_FORMAT_VERSION,
        installation_id: installation_id.to_owned(),
        exported_at,
        units,
        tables: documents
            .iter()
            .map(|document| document.entry.clone())
            .collect(),
        excluded: excluded_entries(),
    }
}

/// `"name"`, quoted as an SQL identifier. Names come from the
/// specification, never from input.
fn quoted(name: &str) -> String {
    format!("\"{name}\"")
}

/// One carried table read in its specified columns and key order, mapped
/// by the specification, and serialized as canonical JSON. `cancelled` is
/// asked for every row, as the record export's metadata pass asks it for
/// every unit, so an abandoned export stops part way through a large table
/// rather than holding its read transaction and export slot to the end.
async fn read_document(
    conn: &mut SqliteConnection,
    table: &Table,
    cancelled: &(dyn Fn() -> bool + Send + Sync),
) -> Result<Document> {
    let columns: Vec<String> = table.columns.iter().map(|c| quoted(c.name)).collect();
    let key: Vec<String> = table.key.iter().map(|name| quoted(name)).collect();
    let sql = format!(
        "SELECT {} FROM {} ORDER BY {}",
        columns.join(", "),
        quoted(table.name),
        key.join(", ")
    );
    let mut rows = sqlx::query(&sql).fetch(&mut *conn);
    let mut out = Vec::new();
    while let Some(row) = rows
        .try_next()
        .await
        .with_context(|| format!("reading table {}", table.name))?
    {
        if cancelled() {
            return Err(client_left());
        }
        let index = out.len();
        let object = map_row(table, &row)
            .with_context(|| format!("exporting row {index} of table {}", table.name))?;
        // The verifier's own row rules, so nothing leaves that a verifier
        // would refuse: an archive is never produced "complete" only to
        // fail verification.
        let findings = row_findings(table, &table.path(), index, &object);
        if let Some(finding) = findings.first() {
            bail!(
                "table {} row with key ({}) is not what the format carries: {finding}",
                table.name,
                key_text(table, &object)
            );
        }
        out.push(Value::Object(object));
    }
    let count = out.len();
    let bytes = canonical::canonical_bytes(&Value::Array(out))
        .with_context(|| format!("serializing table {}", table.name))?;
    Ok(Document {
        entry: TableEntry {
            name: table.name.to_owned(),
            path: table.path(),
            rows: i64::try_from(count).context("row count")?,
            sha256: canonical::content_hash_hex(&bytes),
        },
        bytes,
    })
}

/// A row's key as `column=value, ...`, for a refusal that names it.
fn key_text(table: &Table, object: &Map<String, Value>) -> String {
    table
        .key
        .iter()
        .map(|name| format!("{name}={}", object.get(*name).unwrap_or(&Value::Null)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One stored row as its document object: every specified column, typed.
/// Mapping only converts storage to JSON; whether the row is one the
/// format carries is [`row_findings`]' answer, shared with the verifier.
fn map_row(table: &Table, row: &SqliteRow) -> Result<Map<String, Value>> {
    let mut object = Map::new();
    for (index, column) in table.columns.iter().enumerate() {
        let value =
            map_value(column, row, index).with_context(|| format!("column {}", column.name))?;
        object.insert(column.name.to_owned(), value);
    }
    Ok(object)
}

fn map_value(column: &Column, row: &SqliteRow, index: usize) -> Result<Value> {
    let value = match column.ty {
        ColumnType::Integer => row
            .try_get::<Option<i64>, _>(index)?
            .map(|value| Value::Number(value.into())),
        ColumnType::Flag => match row.try_get::<Option<i64>, _>(index)? {
            None => None,
            Some(0) => Some(Value::Bool(false)),
            Some(1) => Some(Value::Bool(true)),
            Some(other) => bail!("a flag is stored as 0 or 1, not {other}"),
        },
        ColumnType::Text => row.try_get::<Option<String>, _>(index)?.map(Value::String),
    };
    Ok(value.unwrap_or(Value::Null))
}

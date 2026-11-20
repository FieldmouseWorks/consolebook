//! Restore from a validated snapshot.
//!
//! Restore is a product workflow, not file surgery: it validates the
//! snapshot as its own database, refuses while a server holds the
//! installation, sets the current database aside as a pre-restore snapshot,
//! moves the validated snapshot into place, and proves the result (connection
//! invariants, integrity, instance identity) before reporting success. The
//! restore is recorded as an audit event in the restored database, which is
//! then backed up: that backup is the newest, so copies off the server
//! resume from the restored installation (#115).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use time::OffsetDateTime;
use time::macros::format_description;

use crate::audit::{self, EventKind};
use crate::data_dir::DataDir;
use crate::disposition_recovery::{self, RestoreLedgerReport};
use crate::serve_lock::ServeLock;
use crate::{backup, storage};

/// Optional recovery-ledger sources for disposition-aware restore (ADR 0030).
#[derive(Debug, Clone, Copy, Default)]
pub struct RestoreOptions<'a> {
    /// Configured off-server copy directory to consult for a mirrored
    /// disposition recovery ledger (ADR 0024, ADR 0030).
    pub backup_copy_dir: Option<&'a Path>,
    /// Explicit operator-supplied disposition recovery ledger file, for
    /// clean-installation recovery or offline recovery media.
    pub disposition_ledger: Option<&'a Path>,
}

/// Outcome of a completed restore.
#[derive(Debug)]
pub struct RestoreReport {
    pub restored_from: PathBuf,
    pub pre_restore_snapshot: Option<PathBuf>,
    pub installation_id: String,
    /// The backup of the restored database, unless taking it failed. The
    /// restore stands either way; the next scheduled backup takes one.
    pub backup: Option<PathBuf>,
    /// Which disposition recovery ledger sources were consulted, or an
    /// explicit warning when no recovery ledger was available to check.
    pub ledger_check: RestoreLedgerReport,
    /// Local disposition recovery ledger persisted after restore, if any
    /// disposition evidence exists for this installation.
    pub local_ledger: Option<PathBuf>,
}

/// Restores `snapshot` as the installation's database using default options
/// (consulting local installation evidence only).
pub async fn run(data_dir: &DataDir, snapshot: &Path) -> Result<RestoreReport> {
    run_with_options(data_dir, snapshot, &RestoreOptions::default()).await
}

/// Restores `snapshot` as the installation's database with explicit
/// disposition recovery ledger options (ADR 0030 §6).
pub async fn run_with_options(
    data_dir: &DataDir,
    snapshot: &Path,
    options: &RestoreOptions<'_>,
) -> Result<RestoreReport> {
    if !snapshot.exists() {
        bail!("snapshot {} does not exist", snapshot.display());
    }
    // Validate the incoming snapshot before touching anything. Per ADR 0027,
    // Consolebook copies are plain SQLite files on agency-protected storage;
    // externally encrypted blobs fail closed here until decrypted.
    backup::validate_snapshot(snapshot).await.with_context(|| {
        format!(
            "snapshot {} failed validation (if this copy is wrapped in external encryption, \
             decrypt it at the storage/operator layer before running `consolebook restore`; \
             ADR 0027)",
            snapshot.display()
        )
    })?;
    let snapshot_installation_id = backup::snapshot_installation(snapshot)
        .await
        .with_context(|| format!("reading installation identity of {}", snapshot.display()))?;

    // Refuse while a server holds the installation; hold the lock ourselves
    // for the duration so a server starting mid-restore is refused instead.
    let _lock = ServeLock::acquire(data_dir)
        .context("cannot restore while the installation is in use (stop the server first)")?;

    // Consult all available disposition recovery ledgers BEFORE moving aside
    // or altering the live database so pre-disposition snapshots and missing,
    // stale, or corrupt recovery evidence fail closed with zero side effects.
    let evidence = disposition_recovery::verify_snapshot_for_restore(
        data_dir,
        snapshot,
        &snapshot_installation_id,
        options.backup_copy_dir,
        options.disposition_ledger,
    )
    .await?;
    let ledger_check = evidence.to_report();

    let database = data_dir.database();
    let pre_restore_snapshot = set_aside_current_database(data_dir, &database).await?;

    // Persist any collected recovery ledger BEFORE promoting the snapshot into
    // place so a crash or I/O failure between copying the database and writing
    // the ledger can never leave a clean host with a promoted database and no
    // local resurrection protection (ADR 0030 §6).
    if let Some(ledger) = &evidence.effective_ledger {
        disposition_recovery::write_ledger_atomic(&data_dir.disposition_ledger(), ledger)
            .context("persisting disposition recovery ledger before promoting restored database")?;
    }

    // WAL sidecar files belong to the old database; a stale pair must not
    // be replayed into the restored one.
    for sidecar in ["-wal", "-shm"] {
        let mut path = database.as_os_str().to_owned();
        path.push(sidecar);
        let path = PathBuf::from(path);
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("removing stale {}", path.display()))?;
        }
    }
    std::fs::copy(snapshot, &database).with_context(|| {
        format!(
            "copying {} into place as {}",
            snapshot.display(),
            database.display()
        )
    })?;
    backup::persist(&database)?;

    // Prove the result the same way startup would, and record the restore
    // in the restored database's own audit record.
    let pool = storage::open(&database).await.context(
        "restored database failed startup verification; the pre-restore snapshot is untouched",
    )?;
    let verdict = storage::integrity_check(&pool).await?;
    if verdict != ["ok"] {
        bail!(
            "restored database failed integrity check: {}",
            verdict.join("; ")
        );
    }
    let installation_id = storage::installation_id(&pool).await?;
    audit::record(&pool, EventKind::RestoreCompleted, None, None).await?;
    let local_ledger =
        persist_restored_ledger(data_dir, &pool, &installation_id, &evidence, options).await?;
    pool.close().await;

    let backup = match backup::run_after_restore(data_dir).await {
        Ok(report) => Some(report.snapshot),
        Err(err) => {
            tracing::warn!("could not back up the restored database: {err:#}");
            None
        }
    };

    Ok(RestoreReport {
        restored_from: snapshot.to_path_buf(),
        pre_restore_snapshot,
        installation_id,
        backup,
        ledger_check,
        local_ledger,
    })
}

/// Preserves and merges disposition recovery evidence so it survives restore
/// onto an older host or a clean installation.
async fn persist_restored_ledger(
    data_dir: &DataDir,
    pool: &sqlx::SqlitePool,
    installation_id: &str,
    evidence: &disposition_recovery::CollectedRestoreEvidence,
    options: &RestoreOptions<'_>,
) -> Result<Option<PathBuf>> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let restored_db_ledger = disposition_recovery::build_ledger_from_db(pool, now).await?;
    let mut to_merge = Vec::new();
    if let Some(existing) = &evidence.effective_ledger {
        to_merge.push(existing);
    }
    if !restored_db_ledger.entries.is_empty() {
        to_merge.push(&restored_db_ledger);
    }
    if to_merge.is_empty() && !evidence.had_ledger_file {
        return Ok(None);
    }
    let mut merged = disposition_recovery::merge_ledgers(installation_id, &to_merge)?;
    merged.updated_at = merged.updated_at.max(now);
    disposition_recovery::advance_record_id_floor(pool, &merged).await?;
    let local_path = data_dir.disposition_ledger();
    disposition_recovery::write_ledger_atomic(&local_path, &merged)?;
    if let Some(copy_dir) = options.backup_copy_dir
        && !merged.entries.is_empty()
        && let Err(err) =
            disposition_recovery::mirror_ledger_to_copy_dir(data_dir, copy_dir, &merged).await
    {
        tracing::warn!("could not mirror disposition recovery ledger off the server: {err:#}");
    }
    Ok(Some(local_path))
}

fn timestamp() -> Result<String> {
    OffsetDateTime::now_utc()
        .format(format_description!(
            "[year][month][day]T[hour][minute][second]Z"
        ))
        .context("formatting pre-restore timestamp")
}

/// Sets the current database aside so a mistaken restore is itself recoverable.
/// A healthy database becomes a validated pre-restore snapshot; one too damaged
/// to snapshot is moved aside raw rather than blocking its own replacement.
async fn set_aside_current_database(
    data_dir: &DataDir,
    database: &Path,
) -> Result<Option<PathBuf>> {
    if !database.exists() {
        return Ok(None);
    }
    match backup::set_aside_before_restore(data_dir).await {
        Ok(snapshot) => Ok(Some(snapshot)),
        Err(err) => {
            let stamp = timestamp()?;
            let aside = data_dir
                .backups()
                .join(format!("consolebook-prerestore-{stamp}.damaged"));
            std::fs::rename(database, &aside).with_context(|| {
                format!(
                    "current database cannot be snapshotted ({err:#}) and \
                     cannot be moved aside to {}",
                    aside.display()
                )
            })?;
            backup::persist(&aside)?;
            tracing::warn!(
                moved_to = %aside.display(),
                "current database could not be snapshotted ({err:#}); raw bytes moved aside"
            );
            Ok(Some(aside))
        }
    }
}

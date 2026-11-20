//! Post-commit residual-byte reclamation, recovery ledger durability, clean
//! snapshot replacement, and application-controlled copy cleanup for
//! `disposition_execute` (ADR 0030 §5; #214).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sqlx::SqlitePool;

use super::{DispositionExecutionFault, DispositionExecutionReport};
use crate::audit::{self, EventKind, Subject};
use crate::backup;
use crate::backup_copy::{self, BackupSettings};
use crate::disposition_evidence::{DispositionErrorCode, DispositionRunState};
use crate::disposition_recovery::{
    self, CopyArtifactsInventory, DispositionLedger, EXTERNAL_COPIES_NON_GUARANTEE,
    progress::find_pre_disposition_snapshots,
};

/// Drives a committed `disposition_run` from its current persisted state
/// (`db_committed_vacuum_pending` or `copy_cleanup_pending`) through `VACUUM`,
/// recovery ledger sync, clean-snapshot replacement, local artifact cleanup,
/// optional off-server copy cleanup, and `completed` transition.
///
/// Any failure records its closed [`DispositionErrorCode`] on `disposition_run`
/// and returns an explicit incomplete report — never `Completed` — so an
/// authorized operator can idempotently retry until every controlled location
/// is verified clean.
#[allow(clippy::too_many_lines)]
pub(super) async fn drive_post_commit_steps(
    pool: &SqlitePool,
    actor: i64,
    run_id: i64,
    disposed_record_ids: Vec<i64>,
    backups: &BackupSettings,
    now: i64,
    fault: DispositionExecutionFault,
) -> Result<DispositionExecutionReport> {
    let mut state = load_run_state(pool, run_id).await?;
    let mut report = DispositionExecutionReport {
        run_id,
        state,
        disposed_record_ids,
        last_error_code: None,
        vacuum_completed: state != DispositionRunState::DbCommittedVacuumPending,
        local_ledger_updated: false,
        clean_snapshot: None,
        removed_local_files: Vec::new(),
        removed_copy_files: Vec::new(),
        copy_cleanup_detail: None,
        external_copies_note: EXTERNAL_COPIES_NON_GUARANTEE,
    };

    // Phase 2: Residual-byte reclamation (wal_checkpoint -> VACUUM -> wal_checkpoint).
    if state == DispositionRunState::DbCommittedVacuumPending {
        if let Err(err) = reclaim_residual_bytes(pool, fault).await {
            let code = DispositionErrorCode::VacuumFailed;
            disposition_recovery::record_step_failure(pool, run_id, code).await?;
            report.last_error_code = Some(code);
            report.copy_cleanup_detail = Some(format!("{err:#}"));
            return Ok(report);
        }
        disposition_recovery::advance_to_copy_cleanup_pending(pool, run_id, None).await?;
        state = DispositionRunState::CopyCleanupPending;
        report.state = state;
        report.vacuum_completed = true;
    }

    // Phase 3A: Durable local recovery ledger update FIRST.
    let ledger = match sync_local_ledger(pool, backups, now, fault).await {
        Ok(ledger) => {
            report.local_ledger_updated = true;
            ledger
        }
        Err(err) => {
            return record_cleanup_failure(
                pool,
                run_id,
                DispositionErrorCode::LedgerWriteFailed,
                format!("{err:#}"),
                report,
            )
            .await;
        }
    };

    // Phase 3B: Ensure a validated post-disposition local snapshot exists
    // BEFORE deleting any pre-disposition snapshot.
    let clean_snapshot = match ensure_clean_local_snapshot(backups, &ledger, fault).await {
        Ok(path) => {
            report.clean_snapshot = Some(path.clone());
            path
        }
        Err(err) => {
            return record_cleanup_failure(
                pool,
                run_id,
                DispositionErrorCode::CleanSnapshotFailed,
                format!("{err:#}"),
                report,
            )
            .await;
        }
    };

    // Phase 3C: Delete pre-disposition local controlled artifacts (backups/,
    // pre-migration, pre-restore .db/.damaged, .partial/.tmp, exports/).
    match clean_local_artifacts(backups, &ledger, &clean_snapshot, fault).await {
        Ok(removed) => report.removed_local_files = removed,
        Err(err) => {
            return record_cleanup_failure(
                pool,
                run_id,
                DispositionErrorCode::LocalCleanupFailed,
                format!("{err:#}"),
                report,
            )
            .await;
        }
    }

    // Phase 3D: Off-server copy replacement, ledger mirror, and pre-disposition
    // copy cleanup (when CONSOLEBOOK_BACKUP_COPY_DIR is configured).
    if let Some(copy_dir) = backups.copy_dir.as_deref() {
        match clean_off_server_copies(backups, copy_dir, &ledger, &clean_snapshot, fault).await {
            Ok(removed) => report.removed_copy_files = removed,
            Err((code, detail)) => {
                return record_cleanup_failure(pool, run_id, code, detail, report).await;
            }
        }
    } else {
        report.copy_cleanup_detail =
            Some("no off-server backup copy directory configured".to_owned());
    }

    // Final verification: no pre-disposition snapshot remains locally.
    let remaining_local =
        find_pre_disposition_snapshots(&backups.data_dir.backups(), &ledger).await?;
    if !remaining_local.is_empty() {
        return record_cleanup_failure(
            pool,
            run_id,
            DispositionErrorCode::LocalCleanupFailed,
            format!("pre-disposition local snapshot(s) still present: {remaining_local:?}"),
            report,
        )
        .await;
    }

    // Phase 4: Monotonic completion & minimal audit event.
    disposition_recovery::mark_run_completed(pool, run_id, now).await?;
    audit::record_for_subject(
        pool,
        EventKind::DispositionCompleted,
        Some(actor),
        None,
        Subject::DispositionRun(run_id),
    )
    .await?;

    report.state = DispositionRunState::Completed;
    report.last_error_code = None;
    Ok(report)
}

async fn load_run_state(pool: &SqlitePool, run_id: i64) -> Result<DispositionRunState> {
    let raw: String = sqlx::query_scalar("SELECT state FROM disposition_run WHERE id = ?1")
        .bind(run_id)
        .fetch_one(pool)
        .await
        .context("reading disposition_run state")?;
    DispositionRunState::parse(&raw)
        .with_context(|| format!("unknown disposition_run state '{raw}'"))
}

async fn record_cleanup_failure(
    pool: &SqlitePool,
    run_id: i64,
    code: DispositionErrorCode,
    detail: String,
    mut report: DispositionExecutionReport,
) -> Result<DispositionExecutionReport> {
    disposition_recovery::record_step_failure(pool, run_id, code).await?;
    report.state = DispositionRunState::CopyCleanupPending;
    report.last_error_code = Some(code);
    report.copy_cleanup_detail = Some(detail);
    Ok(report)
}

async fn reclaim_residual_bytes(pool: &SqlitePool, fault: DispositionExecutionFault) -> Result<()> {
    if fault == DispositionExecutionFault::DuringVacuum {
        bail!("injected fault during VACUUM residual-byte reclamation");
    }
    let mut conn = pool
        .acquire()
        .await
        .context("acquiring connection for VACUUM")?;
    checkpoint_wal_truncate(&mut conn, "pre-VACUUM").await?;
    sqlx::query("VACUUM")
        .execute(&mut *conn)
        .await
        .context("running VACUUM to reclaim deleted record bytes")?;
    checkpoint_wal_truncate(&mut conn, "post-VACUUM").await?;
    Ok(())
}

async fn checkpoint_wal_truncate(
    conn: &mut sqlx::pool::PoolConnection<sqlx::Sqlite>,
    stage: &str,
) -> Result<()> {
    let (busy, log, checkpointed): (i64, i64, i64) =
        sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
            .fetch_one(&mut **conn)
            .await
            .with_context(|| format!("running {stage} wal_checkpoint(TRUNCATE)"))?;
    if busy != 0 || log != checkpointed || log > 0 {
        bail!(
            "{stage} wal_checkpoint(TRUNCATE) incomplete \
             (busy={busy}, log={log}, checkpointed={checkpointed})"
        );
    }
    Ok(())
}

async fn sync_local_ledger(
    pool: &SqlitePool,
    backups: &BackupSettings,
    now: i64,
    fault: DispositionExecutionFault,
) -> Result<DispositionLedger> {
    if fault == DispositionExecutionFault::DuringLedgerWrite {
        bail!("injected fault during local recovery ledger write");
    }
    disposition_recovery::sync_local_ledger_from_db(&backups.data_dir, pool, now).await
}

async fn ensure_clean_local_snapshot(
    backups: &BackupSettings,
    ledger: &DispositionLedger,
    fault: DispositionExecutionFault,
) -> Result<PathBuf> {
    if fault == DispositionExecutionFault::DuringCleanSnapshot {
        bail!("injected fault during clean post-disposition snapshot");
    }
    // Reuse an already-validated clean post-disposition local snapshot if a
    // previous attempt succeeded at this step before a later cleanup failure.
    let existing = backup::own_backups_in(&backups.data_dir.backups())?;
    for candidate in existing.into_iter().rev() {
        if is_clean_post_disposition_snapshot(&candidate, ledger).await {
            return Ok(candidate);
        }
    }
    let report = backup::run_after_restore(&backups.data_dir)
        .await
        .context("taking fresh post-disposition local snapshot")?;
    Ok(report.snapshot)
}

async fn clean_local_artifacts(
    backups: &BackupSettings,
    ledger: &DispositionLedger,
    protected_snapshot: &Path,
    fault: DispositionExecutionFault,
) -> Result<Vec<PathBuf>> {
    if fault == DispositionExecutionFault::DuringLocalCleanup {
        bail!("injected fault during local controlled-copy cleanup");
    }
    let inventory = disposition_recovery::enumerate_controlled_artifacts(&backups.data_dir, None)?;
    let mut removed = Vec::new();

    for path in inventory
        .local_snapshots
        .into_iter()
        .chain(inventory.local_premigration_snapshots)
    {
        if path == protected_snapshot || is_clean_post_disposition_snapshot(&path, ledger).await {
            continue;
        }
        std::fs::remove_file(&path)
            .with_context(|| format!("removing pre-disposition snapshot {}", path.display()))?;
        removed.push(path);
    }

    for path in inventory
        .local_prerestore_files
        .into_iter()
        .chain(inventory.local_temp_files)
        .chain(inventory.staged_exports)
    {
        std::fs::remove_file(&path)
            .with_context(|| format!("removing pre-disposition artifact {}", path.display()))?;
        removed.push(path);
    }

    Ok(removed)
}

async fn clean_off_server_copies(
    backups: &BackupSettings,
    copy_dir: &Path,
    ledger: &DispositionLedger,
    clean_snapshot: &Path,
    fault: DispositionExecutionFault,
) -> std::result::Result<Vec<PathBuf>, (DispositionErrorCode, String)> {
    if let Err(err) = disposition_recovery::verify_copy_dir_reachable_for_recovery(
        copy_dir,
        &backups.data_dir.backups(),
    ) {
        return Err((DispositionErrorCode::CopyDirUnreachable, err.to_string()));
    }

    // Verify copy_dir ownership BEFORE deleting or writing anything so a
    // misconfigured copy directory belonging to another installation is never
    // modified, even if it holds a snapshot with the same basename.
    verify_copy_dir_ownership(copy_dir, &ledger.installation_id).await?;

    // 1. Copy the fresh clean snapshot off-server FIRST (before deleting any
    //    older off-server copy) and mirror the updated recovery ledger.
    let clean_copy_path =
        ensure_clean_off_server_copy(backups, copy_dir, ledger, clean_snapshot).await?;

    if fault == DispositionExecutionFault::DuringCopyCleanup {
        return Err((
            DispositionErrorCode::CopyCleanupFailed,
            "injected fault during off-server copy cleanup".to_owned(),
        ));
    }

    // 2. Remove every pre-disposition snapshot/artifact in copy_dir while
    //    keeping clean post-disposition copies and disposition-ledger.json.
    let inventory =
        disposition_recovery::enumerate_controlled_artifacts(&backups.data_dir, Some(copy_dir))
            .map_err(|err| (DispositionErrorCode::CopyCleanupFailed, format!("{err:#}")))?;

    let CopyArtifactsInventory::Reachable {
        snapshots,
        premigration_snapshots,
        prerestore_files,
        temp_files,
        ..
    } = inventory.copy
    else {
        return Err((
            DispositionErrorCode::CopyDirUnreachable,
            format!("copy directory {} is unreachable", copy_dir.display()),
        ));
    };

    let mut removed = Vec::new();
    for path in snapshots.into_iter().chain(premigration_snapshots) {
        if path == clean_copy_path || is_clean_post_disposition_snapshot(&path, ledger).await {
            continue;
        }
        std::fs::remove_file(&path).map_err(|err| {
            (
                DispositionErrorCode::CopyCleanupFailed,
                format!("removing off-server copy {}: {err}", path.display()),
            )
        })?;
        removed.push(path);
    }

    for path in prerestore_files.into_iter().chain(temp_files) {
        std::fs::remove_file(&path).map_err(|err| {
            (
                DispositionErrorCode::CopyCleanupFailed,
                format!("removing off-server artifact {}: {err}", path.display()),
            )
        })?;
        removed.push(path);
    }

    let remaining = find_pre_disposition_snapshots(copy_dir, ledger)
        .await
        .map_err(|err| (DispositionErrorCode::CopyCleanupFailed, format!("{err:#}")))?;
    if !remaining.is_empty() {
        return Err((
            DispositionErrorCode::CopyCleanupFailed,
            format!("pre-disposition off-server copy(ies) still present: {remaining:?}"),
        ));
    }

    Ok(removed)
}

/// Ensures `clean_snapshot` is mirrored in `copy_dir` along with the updated
/// recovery ledger. Assumes `verify_copy_dir_ownership` has already verified
/// that `copy_dir` does not belong to another installation, and re-checks any
/// existing file at `clean_copy_path` before replacing it.
async fn ensure_clean_off_server_copy(
    backups: &BackupSettings,
    copy_dir: &Path,
    ledger: &DispositionLedger,
    clean_snapshot: &Path,
) -> std::result::Result<PathBuf, (DispositionErrorCode, String)> {
    let Some(clean_name) = clean_snapshot.file_name() else {
        return Err((
            DispositionErrorCode::CopyCleanupFailed,
            "clean snapshot path has no file name".to_owned(),
        ));
    };
    let clean_copy_path = copy_dir.join(clean_name);
    if is_clean_post_disposition_snapshot(&clean_copy_path, ledger).await {
        if let Err(err) =
            disposition_recovery::mirror_ledger_to_copy_dir(&backups.data_dir, copy_dir, ledger)
                .await
        {
            return Err((DispositionErrorCode::CopyCleanupFailed, format!("{err:#}")));
        }
        return Ok(clean_copy_path);
    }
    if clean_copy_path.exists() {
        if let Ok(other_id) = backup::snapshot_installation(&clean_copy_path).await
            && other_id != ledger.installation_id
        {
            return Err((
                DispositionErrorCode::CopyCleanupFailed,
                format!(
                    "refusing to remove foreign snapshot {} belonging to installation {other_id}",
                    clean_copy_path.display()
                ),
            ));
        }
        if let Err(err) = std::fs::remove_file(&clean_copy_path) {
            return Err((
                DispositionErrorCode::CopyCleanupFailed,
                format!(
                    "removing unverified off-server copy {}: {err}",
                    clean_copy_path.display()
                ),
            ));
        }
    }
    if let Err(err) = backup_copy::copy_snapshot_with_data_dir(
        &backups.data_dir,
        clean_snapshot,
        copy_dir,
        backups.keep,
    )
    .await
    {
        return Err((DispositionErrorCode::CopyCleanupFailed, format!("{err:#}")));
    }
    Ok(clean_copy_path)
}

/// Verifies that every readable snapshot and any recovery ledger mirror in
/// `copy_dir` belongs to `expected_installation_id`. Refuses before any file is
/// written or removed if another installation owns the directory.
async fn verify_copy_dir_ownership(
    copy_dir: &Path,
    expected_installation_id: &str,
) -> std::result::Result<(), (DispositionErrorCode, String)> {
    let snapshots = backup::snapshots_in(copy_dir)
        .map_err(|err| (DispositionErrorCode::CopyCleanupFailed, format!("{err:#}")))?;
    for path in &snapshots {
        if let Ok(other_id) = backup::snapshot_installation(path).await
            && other_id != expected_installation_id
        {
            return Err((
                DispositionErrorCode::CopyCleanupFailed,
                format!(
                    "backup copy directory {} holds snapshot {} of installation {other_id}, \
                     not {expected_installation_id}; refusing to modify foreign copies",
                    copy_dir.display(),
                    path.display()
                ),
            ));
        }
    }
    let mirror_path = copy_dir.join(disposition_recovery::DISPOSITION_LEDGER_FILENAME);
    match disposition_recovery::read_ledger_if_present(&mirror_path) {
        Ok(Some(mirror)) if mirror.installation_id != expected_installation_id => Err((
            DispositionErrorCode::CopyCleanupFailed,
            format!(
                "backup copy directory {} holds recovery ledger of installation {}, \
                 not {expected_installation_id}; refusing to modify foreign copies",
                copy_dir.display(),
                mirror.installation_id
            ),
        )),
        Ok(_) => Ok(()),
        Err(err) => Err((
            DispositionErrorCode::CopyCleanupFailed,
            format!(
                "reading recovery ledger mirror {}: {err:#}",
                mirror_path.display()
            ),
        )),
    }
}

/// True iff `snapshot` is a readable SQLite snapshot of the same installation
/// that contains zero disposed records/hashes from `ledger` AND already
/// reflects every disposed `evaluation_record_id` in `ledger` (so older
/// pre-creation snapshots are also cleaned, leaving only genuine
/// post-disposition snapshots).
async fn is_clean_post_disposition_snapshot(snapshot: &Path, ledger: &DispositionLedger) -> bool {
    let Ok(check) = disposition_recovery::inspect_snapshot_against_ledger(snapshot, ledger).await
    else {
        return false;
    };
    if check.installation_id != ledger.installation_id || check.contains_disposed_content() {
        return false;
    }
    let Ok(pool) = crate::storage::open_snapshot(snapshot).await else {
        return false;
    };
    let snap_ledger = disposition_recovery::build_ledger_from_db(&pool, 0).await;
    pool.close().await;
    let Ok(snap_ledger) = snap_ledger else {
        return false;
    };
    let snap_ids: std::collections::BTreeSet<i64> = snap_ledger
        .entries
        .iter()
        .map(|e| e.evaluation_record_id)
        .collect();
    ledger
        .entries
        .iter()
        .all(|e| snap_ids.contains(&e.evaluation_record_id))
}

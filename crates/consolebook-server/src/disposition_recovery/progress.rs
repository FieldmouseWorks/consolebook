//! Typed disposition recovery states, controlled-artifact inventory, and
//! durable `disposition_run` progress interfaces for `#214` (ADR 0030 §5–§6; #213).
//!
//! Read-only inspection functions never create or modify files or database
//! state so `doctor` and `/api/backups/status` keep their strict read-only
//! guarantees.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use sqlx::SqlitePool;

use crate::backup::{self, PRE_MIGRATION_PREFIX};
use crate::data_dir::DataDir;
use crate::disposition_evidence::{
    DispositionErrorCode, DispositionMethod, DispositionReasonCode, DispositionRunState,
};

use super::ledger::{
    DISPOSITION_LEDGER_FILENAME, DispositionLedger, build_ledger_from_db, read_ledger_if_present,
};
use super::verify_copy_dir_reachable_for_recovery;

/// Explicit non-guarantee statement surfaced beside controlled recovery status
/// so Consolebook never claims bytes outside its control were destroyed.
pub const EXTERNAL_COPIES_NON_GUARANTEE: &str = "Downloaded exports, trainee packets, external \
     backups, and host/storage snapshots are outside Consolebook's control and cannot be deleted \
     or checked by Consolebook.";

/// Durable view of one `disposition_run` row (opaque metadata only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DispositionRunRecord {
    pub id: i64,
    pub actor_user_id: i64,
    pub confirmation_digest: String,
    pub authority_reference: String,
    pub reason_code: DispositionReasonCode,
    pub method: DispositionMethod,
    pub state: DispositionRunState,
    pub started_at: i64,
    pub completed_at: Option<i64>,
    pub last_error_code: Option<DispositionErrorCode>,
    pub event_count: i64,
}

/// Compact summary of an incomplete disposition run surfaced in `doctor` and
/// `/api/backups/status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IncompleteRunSummary {
    pub id: i64,
    pub state: DispositionRunState,
    pub started_at: i64,
    pub last_error_code: Option<DispositionErrorCode>,
    pub event_count: i64,
}

/// State of the installation's local recovery ledger (`data/instance/disposition-ledger.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LocalLedgerStatus {
    /// No ledger file exists and no disposition events exist in the database.
    Absent,
    /// Valid canonical ledger matching all database disposition events.
    Present { entry_count: usize, updated_at: i64 },
    /// Database has disposition events missing from the local ledger file
    /// (or the local ledger file is missing altogether).
    Stale { missing_count: usize },
    /// Local ledger file is unreadable, non-canonical, corrupt, or belongs to
    /// another installation.
    Corrupt { problem: String },
}

/// State of the off-server recovery ledger mirror (`CONSOLEBOOK_BACKUP_COPY_DIR/disposition-ledger.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CopyLedgerStatus {
    /// No copy directory is configured.
    NotConfigured,
    /// Copy directory is configured, and no dispositions exist yet to mirror.
    NotRequired { directory: String },
    /// Copy directory is configured but unreachable (missing, not a directory,
    /// or on the same filesystem as local backups).
    Unreachable { directory: String, problem: String },
    /// Local installation has dispositions, but no recovery ledger mirror
    /// exists yet in the reachable copy directory.
    Missing { directory: String },
    /// Reachable copy directory holds a mirror missing one or more local
    /// disposition entries.
    Behind {
        directory: String,
        local_count: usize,
        copy_count: usize,
    },
    /// Reachable copy directory holds a valid mirror covering every local
    /// disposition entry.
    Current {
        directory: String,
        entry_count: usize,
    },
    /// Copy directory holds a corrupt, non-canonical, or wrong-installation
    /// recovery ledger.
    Corrupt { directory: String, problem: String },
}

/// Overall disposition recovery health state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryHealthState {
    Clear,
    IncompleteDisposition,
    LedgerError,
    LedgerStale,
    CopyLedgerUnavailable,
    CopyLedgerBehind,
}

/// Read-only disposition recovery health summary reported by `doctor` and
/// `/api/backups/status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecoveryHealth {
    pub state: RecoveryHealthState,
    pub incomplete_runs: Vec<IncompleteRunSummary>,
    pub local_ledger: LocalLedgerStatus,
    pub copy_ledger: CopyLedgerStatus,
    pub external_copies_note: &'static str,
}

/// Application-controlled files in `CONSOLEBOOK_BACKUP_COPY_DIR` (read-only inventory).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CopyArtifactsInventory {
    NotConfigured,
    Unreachable {
        directory: String,
        problem: String,
    },
    Reachable {
        directory: String,
        snapshots: Vec<PathBuf>,
        premigration_snapshots: Vec<PathBuf>,
        prerestore_files: Vec<PathBuf>,
        temp_files: Vec<PathBuf>,
        has_recovery_ledger: bool,
    },
}

/// Read-only inventory of every application-controlled backup, snapshot, and
/// export location that `#214`'s copy-cleanup phase manages (ADR 0030 §5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ControlledArtifactsInventory {
    pub local_snapshots: Vec<PathBuf>,
    pub local_premigration_snapshots: Vec<PathBuf>,
    pub local_prerestore_files: Vec<PathBuf>,
    pub local_temp_files: Vec<PathBuf>,
    pub staged_exports: Vec<PathBuf>,
    pub copy: CopyArtifactsInventory,
    pub external_copies_note: &'static str,
}

const PRE_RESTORE_PREFIX: &str = "consolebook-prerestore-";

/// Enumerates all application-controlled artifacts read-only without modifying
/// any directory or file.
pub fn enumerate_controlled_artifacts(
    data_dir: &DataDir,
    copy_dir: Option<&Path>,
) -> Result<ControlledArtifactsInventory> {
    let (local_snapshots, local_premigration_snapshots, local_prerestore_files, local_temp_files) =
        scan_backup_dir(&data_dir.backups())?;
    let staged_exports = scan_staged_exports(&data_dir.exports())?;

    let copy = match copy_dir {
        None => CopyArtifactsInventory::NotConfigured,
        Some(dir) => match verify_copy_dir_reachable_for_recovery(dir, &data_dir.backups()) {
            Err(err) => CopyArtifactsInventory::Unreachable {
                directory: dir.display().to_string(),
                problem: err.to_string(),
            },
            Ok(()) => match scan_backup_dir(dir) {
                Ok((snapshots, premigration_snapshots, prerestore_files, temp_files)) => {
                    CopyArtifactsInventory::Reachable {
                        directory: dir.display().to_string(),
                        snapshots,
                        premigration_snapshots,
                        prerestore_files,
                        temp_files,
                        has_recovery_ledger: dir.join(DISPOSITION_LEDGER_FILENAME).is_file(),
                    }
                }
                Err(err) => CopyArtifactsInventory::Unreachable {
                    directory: dir.display().to_string(),
                    problem: format!("{err:#}"),
                },
            },
        },
    };

    Ok(ControlledArtifactsInventory {
        local_snapshots,
        local_premigration_snapshots,
        local_prerestore_files,
        local_temp_files,
        staged_exports,
        copy,
        external_copies_note: EXTERNAL_COPIES_NON_GUARANTEE,
    })
}

type BackupDirScan = (Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>);

fn scan_backup_dir(dir: &Path) -> Result<BackupDirScan> {
    let mut scheduled = Vec::new();
    let mut premigration = Vec::new();
    let mut prerestore = Vec::new();
    let mut temps = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok((scheduled, premigration, prerestore, temps));
        }
        Err(err) => {
            return Err(err).with_context(|| format!("listing {}", dir.display()));
        }
    };

    for entry in entries {
        let entry = entry.context("reading directory entry")?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let ext = path.extension().and_then(|e| e.to_str());
        if matches!(ext, Some("partial" | "tmp")) {
            temps.push(path);
        } else if name.starts_with(PRE_RESTORE_PREFIX) && matches!(ext, Some("db" | "damaged")) {
            prerestore.push(path);
        } else if name.starts_with(PRE_MIGRATION_PREFIX) && ext == Some("db") {
            premigration.push(path);
        } else if name.starts_with("consolebook-") && ext == Some("db") {
            scheduled.push(path);
        }
    }

    scheduled.sort();
    premigration.sort();
    prerestore.sort();
    temps.sort();
    Ok((scheduled, premigration, prerestore, temps))
}

fn scan_staged_exports(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(files),
        Err(err) => {
            return Err(err).with_context(|| format!("listing {}", dir.display()));
        }
    };
    for entry in entries {
        let entry = entry.context("reading exports directory entry")?;
        if entry.path().is_file() {
            files.push(entry.path());
        }
    }
    files.sort();
    Ok(files)
}

/// Lists all `disposition_run` rows ordered oldest first. Read-only; returns
/// empty on pre-0018 databases without `disposition_run`.
pub async fn list_all_runs(pool: &SqlitePool) -> Result<Vec<DispositionRunRecord>> {
    list_runs_filtered(pool, false).await
}

/// Lists incomplete `disposition_run` rows (`db_committed_vacuum_pending` or
/// `copy_cleanup_pending`) ordered oldest first. Read-only.
pub async fn list_incomplete_runs(pool: &SqlitePool) -> Result<Vec<DispositionRunRecord>> {
    list_runs_filtered(pool, true).await
}

async fn list_runs_filtered(
    pool: &SqlitePool,
    incomplete_only: bool,
) -> Result<Vec<DispositionRunRecord>> {
    let has_table: i64 = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'disposition_run')",
    )
    .fetch_one(pool)
    .await
    .context("checking for disposition_run table")?;
    if has_table == 0 {
        return Ok(Vec::new());
    }

    let query = if incomplete_only {
        "SELECT r.id, r.actor_user_id, r.confirmation_digest, r.authority_reference, \
                r.reason_code, r.method, r.state, r.started_at, r.completed_at, \
                r.last_error_code, \
                (SELECT COUNT(*) FROM disposition_event e WHERE e.disposition_run_id = r.id) \
         FROM disposition_run r \
         WHERE r.state != 'completed' \
         ORDER BY r.id ASC"
    } else {
        "SELECT r.id, r.actor_user_id, r.confirmation_digest, r.authority_reference, \
                r.reason_code, r.method, r.state, r.started_at, r.completed_at, \
                r.last_error_code, \
                (SELECT COUNT(*) FROM disposition_event e WHERE e.disposition_run_id = r.id) \
         FROM disposition_run r \
         ORDER BY r.id ASC"
    };

    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        i64,
        i64,
        String,
        String,
        String,
        String,
        String,
        i64,
        Option<i64>,
        Option<String>,
        i64,
    )> = sqlx::query_as(query)
        .fetch_all(pool)
        .await
        .context("reading disposition_run rows")?;

    let mut runs = Vec::with_capacity(rows.len());
    for (
        id,
        actor_user_id,
        confirmation_digest,
        authority_reference,
        reason_code,
        method,
        state,
        started_at,
        completed_at,
        last_error_code,
        event_count,
    ) in rows
    {
        let reason_code = DispositionReasonCode::parse(&reason_code)
            .with_context(|| format!("unknown disposition_run reason_code '{reason_code}'"))?;
        let method = DispositionMethod::parse(&method)
            .with_context(|| format!("unknown disposition_run method '{method}'"))?;
        let state = DispositionRunState::parse(&state)
            .with_context(|| format!("unknown disposition_run state '{state}'"))?;
        let last_error_code = match last_error_code {
            Some(code) => Some(
                DispositionErrorCode::parse(&code)
                    .with_context(|| format!("unknown disposition_run error code '{code}'"))?,
            ),
            None => None,
        };
        runs.push(DispositionRunRecord {
            id,
            actor_user_id,
            confirmation_digest,
            authority_reference,
            reason_code,
            method,
            state,
            started_at,
            completed_at,
            last_error_code,
            event_count,
        });
    }
    Ok(runs)
}

/// Persists a closed step error code on an incomplete `disposition_run` without
/// changing its state (durable progress interface for `#214`). Fails closed if
/// the run does not exist or is already `completed`.
pub async fn record_step_failure(
    pool: &SqlitePool,
    run_id: i64,
    error_code: DispositionErrorCode,
) -> Result<()> {
    let affected = sqlx::query(
        "UPDATE disposition_run SET last_error_code = ?1 WHERE id = ?2 AND state != 'completed'",
    )
    .bind(error_code.as_str())
    .bind(run_id)
    .execute(pool)
    .await
    .context("recording disposition_run step failure")?
    .rows_affected();
    if affected != 1 {
        bail!("disposition_run {run_id} not found or already completed");
    }
    Ok(())
}

/// Monotonically advances a `disposition_run` from `db_committed_vacuum_pending`
/// to `copy_cleanup_pending` (durable progress interface for `#214`). Idempotent
/// if the run is already in `copy_cleanup_pending`.
pub async fn advance_to_copy_cleanup_pending(
    pool: &SqlitePool,
    run_id: i64,
    error_code: Option<DispositionErrorCode>,
) -> Result<()> {
    let affected = sqlx::query(
        "UPDATE disposition_run \
         SET state = 'copy_cleanup_pending', last_error_code = ?1 \
         WHERE id = ?2 AND state IN ('db_committed_vacuum_pending', 'copy_cleanup_pending')",
    )
    .bind(error_code.map(DispositionErrorCode::as_str))
    .bind(run_id)
    .execute(pool)
    .await
    .context("advancing disposition_run to copy_cleanup_pending")?
    .rows_affected();
    if affected != 1 {
        bail!("disposition_run {run_id} not found or already completed");
    }
    Ok(())
}

/// Transitions an incomplete `disposition_run` from `copy_cleanup_pending` to
/// `completed`, clearing `last_error_code` and recording `completed_at`
/// (durable progress interface for `#214`). Refuses if the run is still in
/// `db_committed_vacuum_pending` or already `completed`.
pub async fn mark_run_completed(pool: &SqlitePool, run_id: i64, completed_at: i64) -> Result<()> {
    let affected = sqlx::query(
        "UPDATE disposition_run \
         SET state = 'completed', completed_at = ?1, last_error_code = NULL \
         WHERE id = ?2 AND state = 'copy_cleanup_pending'",
    )
    .bind(completed_at)
    .bind(run_id)
    .execute(pool)
    .await
    .context("marking disposition_run completed")?
    .rows_affected();
    if affected != 1 {
        bail!("disposition_run {run_id} is not in copy_cleanup_pending");
    }
    Ok(())
}

/// Read-only recovery health inspection combining database `disposition_run` /
/// `disposition_event` state with the local and off-server recovery ledgers.
pub async fn inspect_recovery_health(
    pool: &SqlitePool,
    data_dir: &DataDir,
    copy_dir: Option<&Path>,
) -> Result<RecoveryHealth> {
    let db_ledger = build_ledger_from_db(pool, 0).await?;
    let incomplete_runs: Vec<IncompleteRunSummary> = list_incomplete_runs(pool)
        .await?
        .into_iter()
        .map(|run| IncompleteRunSummary {
            id: run.id,
            state: run.state,
            started_at: run.started_at,
            last_error_code: run.last_error_code,
            event_count: run.event_count,
        })
        .collect();

    let (local_ledger, local_parsed) =
        inspect_local_ledger(data_dir, &db_ledger.installation_id, &db_ledger);
    let expected_ids = expected_disposed_ids(&db_ledger, local_parsed.as_ref());
    let copy_ledger = inspect_copy_ledger(
        data_dir,
        copy_dir,
        &db_ledger.installation_id,
        &expected_ids,
    );

    let state = classify_health(&incomplete_runs, &local_ledger, &copy_ledger);
    Ok(RecoveryHealth {
        state,
        incomplete_runs,
        local_ledger,
        copy_ledger,
        external_copies_note: EXTERNAL_COPIES_NON_GUARANTEE,
    })
}

/// Read-only recovery health inspection from filesystem artifacts alone (for
/// synchronous callers without a database pool).
#[must_use]
pub fn inspect_recovery_health_files_only(
    data_dir: &DataDir,
    copy_dir: Option<&Path>,
) -> RecoveryHealth {
    let local_path = data_dir.disposition_ledger();
    let (local_ledger, local_parsed) = match read_ledger_if_present(&local_path) {
        Ok(None) => (LocalLedgerStatus::Absent, None),
        Ok(Some(ledger)) => (
            LocalLedgerStatus::Present {
                entry_count: ledger.entries.len(),
                updated_at: ledger.updated_at,
            },
            Some(ledger),
        ),
        Err(err) => (
            LocalLedgerStatus::Corrupt {
                problem: err.to_string(),
            },
            None,
        ),
    };

    let copy_ledger = match (&local_parsed, copy_dir) {
        (_, None) => CopyLedgerStatus::NotConfigured,
        (Some(local), Some(dir)) => {
            let expected: BTreeSet<i64> = local
                .entries
                .iter()
                .map(|e| e.evaluation_record_id)
                .collect();
            inspect_copy_ledger(data_dir, Some(dir), &local.installation_id, &expected)
        }
        (None, Some(dir)) => inspect_copy_ledger_without_id(data_dir, dir),
    };

    let state = classify_health(&[], &local_ledger, &copy_ledger);
    RecoveryHealth {
        state,
        incomplete_runs: Vec::new(),
        local_ledger,
        copy_ledger,
        external_copies_note: EXTERNAL_COPIES_NON_GUARANTEE,
    }
}

fn inspect_local_ledger(
    data_dir: &DataDir,
    installation_id: &str,
    db_ledger: &DispositionLedger,
) -> (LocalLedgerStatus, Option<DispositionLedger>) {
    let local_path = data_dir.disposition_ledger();
    match read_ledger_if_present(&local_path) {
        Err(err) => (
            LocalLedgerStatus::Corrupt {
                problem: err.to_string(),
            },
            None,
        ),
        Ok(None) if db_ledger.entries.is_empty() => (LocalLedgerStatus::Absent, None),
        Ok(None) => (
            LocalLedgerStatus::Stale {
                missing_count: db_ledger.entries.len(),
            },
            None,
        ),
        Ok(Some(local)) if local.installation_id != installation_id => (
            LocalLedgerStatus::Corrupt {
                problem: format!(
                    "ledger belongs to installation {}, not {installation_id}",
                    local.installation_id
                ),
            },
            None,
        ),
        Ok(Some(local)) => {
            let local_ids: BTreeSet<i64> = local
                .entries
                .iter()
                .map(|e| e.evaluation_record_id)
                .collect();
            let missing_count = db_ledger
                .entries
                .iter()
                .filter(|e| !local_ids.contains(&e.evaluation_record_id))
                .count();
            if missing_count > 0 {
                (LocalLedgerStatus::Stale { missing_count }, Some(local))
            } else {
                (
                    LocalLedgerStatus::Present {
                        entry_count: local.entries.len(),
                        updated_at: local.updated_at,
                    },
                    Some(local),
                )
            }
        }
    }
}

fn expected_disposed_ids(
    db_ledger: &DispositionLedger,
    local: Option<&DispositionLedger>,
) -> BTreeSet<i64> {
    let mut ids: BTreeSet<i64> = db_ledger
        .entries
        .iter()
        .map(|e| e.evaluation_record_id)
        .collect();
    if let Some(local) = local {
        ids.extend(local.entries.iter().map(|e| e.evaluation_record_id));
    }
    ids
}

fn inspect_copy_ledger(
    data_dir: &DataDir,
    copy_dir: Option<&Path>,
    installation_id: &str,
    expected_ids: &BTreeSet<i64>,
) -> CopyLedgerStatus {
    let Some(dir) = copy_dir else {
        return CopyLedgerStatus::NotConfigured;
    };
    let directory = dir.display().to_string();
    if let Err(err) = verify_copy_dir_reachable_for_recovery(dir, &data_dir.backups()) {
        return if expected_ids.is_empty() {
            CopyLedgerStatus::NotRequired { directory }
        } else {
            CopyLedgerStatus::Unreachable {
                directory,
                problem: err.to_string(),
            }
        };
    }

    let mirror_path = dir.join(DISPOSITION_LEDGER_FILENAME);
    match read_ledger_if_present(&mirror_path) {
        Err(err) => CopyLedgerStatus::Corrupt {
            directory,
            problem: err.to_string(),
        },
        Ok(None) if expected_ids.is_empty() => CopyLedgerStatus::NotRequired { directory },
        Ok(None) => CopyLedgerStatus::Missing { directory },
        Ok(Some(mirror)) if mirror.installation_id != installation_id => {
            CopyLedgerStatus::Corrupt {
                directory,
                problem: format!(
                    "mirror belongs to installation {}, not {installation_id}",
                    mirror.installation_id
                ),
            }
        }
        Ok(Some(mirror)) => {
            let mirror_ids: BTreeSet<i64> = mirror
                .entries
                .iter()
                .map(|e| e.evaluation_record_id)
                .collect();
            let missing = expected_ids
                .iter()
                .filter(|id| !mirror_ids.contains(id))
                .count();
            if missing > 0 {
                CopyLedgerStatus::Behind {
                    directory,
                    local_count: expected_ids.len(),
                    copy_count: mirror.entries.len(),
                }
            } else {
                CopyLedgerStatus::Current {
                    directory,
                    entry_count: mirror.entries.len(),
                }
            }
        }
    }
}

fn inspect_copy_ledger_without_id(data_dir: &DataDir, dir: &Path) -> CopyLedgerStatus {
    let directory = dir.display().to_string();
    if verify_copy_dir_reachable_for_recovery(dir, &data_dir.backups()).is_err() {
        return CopyLedgerStatus::NotRequired { directory };
    }
    let mirror_path = dir.join(DISPOSITION_LEDGER_FILENAME);
    match read_ledger_if_present(&mirror_path) {
        Err(err) => CopyLedgerStatus::Corrupt {
            directory,
            problem: err.to_string(),
        },
        Ok(None) => CopyLedgerStatus::NotRequired { directory },
        Ok(Some(mirror)) => CopyLedgerStatus::Current {
            directory,
            entry_count: mirror.entries.len(),
        },
    }
}

fn classify_health(
    incomplete_runs: &[IncompleteRunSummary],
    local_ledger: &LocalLedgerStatus,
    copy_ledger: &CopyLedgerStatus,
) -> RecoveryHealthState {
    if !incomplete_runs.is_empty() {
        return RecoveryHealthState::IncompleteDisposition;
    }
    if matches!(local_ledger, LocalLedgerStatus::Corrupt { .. })
        || matches!(copy_ledger, CopyLedgerStatus::Corrupt { .. })
    {
        return RecoveryHealthState::LedgerError;
    }
    if matches!(local_ledger, LocalLedgerStatus::Stale { .. }) {
        return RecoveryHealthState::LedgerStale;
    }
    if matches!(copy_ledger, CopyLedgerStatus::Unreachable { .. }) {
        return RecoveryHealthState::CopyLedgerUnavailable;
    }
    if matches!(
        copy_ledger,
        CopyLedgerStatus::Missing { .. } | CopyLedgerStatus::Behind { .. }
    ) {
        return RecoveryHealthState::CopyLedgerBehind;
    }
    RecoveryHealthState::Clear
}

/// Checks if `dir` contains any pre-disposition snapshot files according to
/// `ledger` (read-only helper for `#214` / `#218`).
pub async fn find_pre_disposition_snapshots(
    dir: &Path,
    ledger: &DispositionLedger,
) -> Result<Vec<PathBuf>> {
    if ledger.entries.is_empty() {
        return Ok(Vec::new());
    }
    let mut matches = Vec::new();
    for snapshot in backup::snapshots_in(dir)? {
        if let Ok(check) = super::ledger::inspect_snapshot_against_ledger(&snapshot, ledger).await
            && check.contains_disposed_content()
        {
            matches.push(snapshot);
        }
    }
    Ok(matches)
}

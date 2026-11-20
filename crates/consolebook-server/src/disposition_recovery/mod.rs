//! Disposition-aware backup, copy, and restore recovery protocol (ADR 0030; #213).
//!
//! Ensures lawful disposition evidence survives restoring an older database or
//! recovering onto a clean installation, prevents pre-disposition snapshots from
//! being silently resurrected or republished off the server, and exposes typed
//! recovery states and durable progress interfaces for the later disposition
//! executor (`#214`).
//!
//! No destruction endpoint lives here. Consistent with ADR 0027, Consolebook
//! backup copies remain plain SQLite files on agency-protected storage; any
//! externally encrypted copy must be decrypted at the storage/operator layer
//! before `restore`, after which it passes through the exact same recovery
//! ledger enforcement.

pub mod ledger;
pub mod progress;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::backup_copy;
use crate::data_dir::DataDir;
use crate::storage;

pub use ledger::{
    DISPOSITION_LEDGER_FILENAME, DISPOSITION_LEDGER_FORMAT, DISPOSITION_LEDGER_FORMAT_VERSION,
    DispositionLedger, DispositionLedgerEntry, LedgerError, SnapshotDispositionCheck,
    advance_record_id_floor, build_ledger_from_db, encode_canonical,
    inspect_snapshot_against_ledger, merge_ledgers, mirror_ledger_to_copy_dir, parse_and_verify,
    read_ledger_if_present, read_required_ledger, sync_local_ledger_from_db, write_ledger_atomic,
};
pub use progress::{
    ControlledArtifactsInventory, CopyArtifactsInventory, CopyLedgerStatus, DispositionRunRecord,
    EXTERNAL_COPIES_NON_GUARANTEE, IncompleteRunSummary, LocalLedgerStatus, RecoveryHealth,
    RecoveryHealthState, advance_to_copy_cleanup_pending, enumerate_controlled_artifacts,
    inspect_recovery_health, inspect_recovery_health_files_only, list_all_runs,
    list_incomplete_runs, mark_run_completed, record_step_failure,
};

/// Which recovery ledger source was consulted during a restore check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LedgerSource {
    LocalFile { path: String, entry_count: usize },
    CopyDirMirror { path: String, entry_count: usize },
    ExplicitFile { path: String, entry_count: usize },
    LiveDatabase { entry_count: usize },
}

/// Summary of disposition recovery ledger checks performed during `restore`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RestoreLedgerReport {
    /// At least one recovery ledger source was consulted and the snapshot
    /// passed without resurrecting any disposed record or content hash.
    Checked {
        sources: Vec<LedgerSource>,
        entries_checked: usize,
    },
    /// No disposition recovery ledger existed at any consulted location
    /// (for example, a clean server with no mirror or an installation that
    /// has never executed a disposition). Consolebook states this plainly
    /// rather than claiming resurrection protection it could not check.
    NoLedgerAvailable { warning: String },
}

/// Recovery evidence collected before promoting a restore snapshot.
#[derive(Debug, Clone)]
pub struct CollectedRestoreEvidence {
    pub effective_ledger: Option<DispositionLedger>,
    pub sources: Vec<LedgerSource>,
    pub had_ledger_file: bool,
}

impl CollectedRestoreEvidence {
    #[must_use]
    pub fn to_report(&self) -> RestoreLedgerReport {
        if self.sources.is_empty() {
            RestoreLedgerReport::NoLedgerAvailable {
                warning: "no disposition recovery ledger was available to consult; Consolebook \
                          cannot verify whether this snapshot predates a prior disposition on \
                          another host"
                    .to_owned(),
            }
        } else {
            let entries_checked = self
                .effective_ledger
                .as_ref()
                .map_or(0, |ledger| ledger.entries.len());
            RestoreLedgerReport::Checked {
                sources: self.sources.clone(),
                entries_checked,
            }
        }
    }
}

/// Consults every applicable recovery ledger source before a restore touches
/// the live database, failing closed on corrupt, mismatched, stale/missing, or
/// unreachable evidence and refusing any snapshot that would resurrect a
/// disposed record or version content hash.
pub async fn verify_snapshot_for_restore(
    data_dir: &DataDir,
    snapshot: &Path,
    snapshot_installation_id: &str,
    backup_copy_dir: Option<&Path>,
    explicit_ledger_path: Option<&Path>,
) -> Result<CollectedRestoreEvidence> {
    let mut sources = Vec::new();
    let mut ledgers: Vec<DispositionLedger> = Vec::new();
    let mut file_covered_ids: BTreeSet<i64> = BTreeSet::new();
    let mut had_ledger_file = false;

    collect_file_ledgers_for_restore(
        data_dir,
        snapshot_installation_id,
        backup_copy_dir,
        explicit_ledger_path,
        &mut sources,
        &mut ledgers,
        &mut file_covered_ids,
        &mut had_ledger_file,
    )?;

    collect_live_db_ledger_for_restore(
        data_dir,
        snapshot_installation_id,
        &file_covered_ids,
        &mut sources,
        &mut ledgers,
    )
    .await?;

    let effective_ledger = if ledgers.is_empty() {
        None
    } else {
        let refs: Vec<&DispositionLedger> = ledgers.iter().collect();
        Some(merge_ledgers(snapshot_installation_id, &refs)?)
    };

    if let Some(ledger) = &effective_ledger
        && !ledger.entries.is_empty()
    {
        let check = inspect_snapshot_against_ledger(snapshot, ledger).await?;
        if check.contains_disposed_content() {
            bail!(
                "refusing to restore {}: snapshot contains lawfully disposed record(s) \
                 (record ids: {:?}, matched content hashes: {}); restore a post-disposition \
                 snapshot instead",
                snapshot.display(),
                check.disposed_record_ids_present,
                check.disposed_content_hashes_present.len()
            );
        }
    }

    Ok(CollectedRestoreEvidence {
        effective_ledger,
        sources,
        had_ledger_file,
    })
}

#[allow(clippy::too_many_arguments)]
fn collect_file_ledgers_for_restore(
    data_dir: &DataDir,
    snapshot_installation_id: &str,
    backup_copy_dir: Option<&Path>,
    explicit_ledger_path: Option<&Path>,
    sources: &mut Vec<LedgerSource>,
    ledgers: &mut Vec<DispositionLedger>,
    file_covered_ids: &mut BTreeSet<i64>,
    had_ledger_file: &mut bool,
) -> Result<()> {
    let local_path = data_dir.disposition_ledger();
    if let Some(local) = read_ledger_if_present(&local_path)? {
        ensure_matching_installation(&local_path, snapshot_installation_id, &local)?;
        *had_ledger_file = true;
        file_covered_ids.extend(local.entries.iter().map(|e| e.evaluation_record_id));
        sources.push(LedgerSource::LocalFile {
            path: local_path.display().to_string(),
            entry_count: local.entries.len(),
        });
        ledgers.push(local);
    }

    if let Some(copy_dir) = backup_copy_dir {
        verify_copy_dir_reachable_for_recovery(copy_dir, &data_dir.backups())?;
        let mirror_path = copy_dir.join(DISPOSITION_LEDGER_FILENAME);
        if let Some(mirror) = read_ledger_if_present(&mirror_path)? {
            ensure_matching_installation(&mirror_path, snapshot_installation_id, &mirror)?;
            *had_ledger_file = true;
            file_covered_ids.extend(mirror.entries.iter().map(|e| e.evaluation_record_id));
            sources.push(LedgerSource::CopyDirMirror {
                path: mirror_path.display().to_string(),
                entry_count: mirror.entries.len(),
            });
            ledgers.push(mirror);
        }
    }

    if let Some(explicit_path) = explicit_ledger_path {
        let explicit = read_required_ledger(explicit_path)?;
        ensure_matching_installation(explicit_path, snapshot_installation_id, &explicit)?;
        *had_ledger_file = true;
        file_covered_ids.extend(explicit.entries.iter().map(|e| e.evaluation_record_id));
        sources.push(LedgerSource::ExplicitFile {
            path: explicit_path.display().to_string(),
            entry_count: explicit.entries.len(),
        });
        ledgers.push(explicit);
    }

    Ok(())
}

fn ensure_matching_installation(
    path: &Path,
    expected: &str,
    ledger: &DispositionLedger,
) -> std::result::Result<(), LedgerError> {
    if ledger.installation_id != expected {
        return Err(LedgerError::InstallationMismatch {
            path: path.to_path_buf(),
            expected: expected.to_owned(),
            actual: ledger.installation_id.clone(),
        });
    }
    Ok(())
}

async fn collect_live_db_ledger_for_restore(
    data_dir: &DataDir,
    snapshot_installation_id: &str,
    file_covered_ids: &BTreeSet<i64>,
    sources: &mut Vec<LedgerSource>,
    ledgers: &mut Vec<DispositionLedger>,
) -> Result<()> {
    // A catastrophically damaged live DB must not block its own recovery from
    // intact ledger files, so unreadable live DBs are skipped here. Use the
    // WAL-aware read-only diagnostic opener so committed disposition events
    // still residing only in `-wal` after an unclean shutdown are observed.
    let db_path = data_dir.database();
    if db_path.exists()
        && let Ok(live_pool) = storage::open_diagnostic(&db_path).await
    {
        let live_id = storage::installation_id(&live_pool).await.ok();
        if live_id.as_deref() == Some(snapshot_installation_id) {
            let live_ledger = build_ledger_from_db(&live_pool, 0).await?;
            live_pool.close().await;
            if !live_ledger.entries.is_empty() {
                let missing: Vec<i64> = live_ledger
                    .entries
                    .iter()
                    .map(|e| e.evaluation_record_id)
                    .filter(|id| !file_covered_ids.contains(id))
                    .collect();
                if !missing.is_empty() {
                    return Err(LedgerError::StaleOrMissingLocalLedger {
                        missing_record_ids: missing,
                    }
                    .into());
                }
                sources.push(LedgerSource::LiveDatabase {
                    entry_count: live_ledger.entries.len(),
                });
                ledgers.push(live_ledger);
            }
        } else {
            live_pool.close().await;
        }
    }
    Ok(())
}

/// Checks that a snapshot is safe to copy/republish to `copy_dir` under all
/// available disposition recovery evidence (local ledger, live DB if present,
/// and any existing copy-dir ledger mirror). Fails closed on corrupt,
/// mismatched, or stale evidence and refuses snapshots carrying disposed
/// records or version content hashes.
///
/// Returns the merged recovery ledger (if any entries exist) so the caller can
/// mirror it alongside the clean snapshot.
pub async fn verify_snapshot_for_publication(
    data_dir: Option<&DataDir>,
    snapshot: &Path,
    copy_dir: &Path,
) -> Result<Option<DispositionLedger>> {
    let mirror_path = copy_dir.join(DISPOSITION_LEDGER_FILENAME);
    let mirror = read_ledger_if_present(&mirror_path)?;

    let inferred_root: Option<PathBuf> = data_dir.map_or_else(
        || {
            snapshot
                .parent()
                .filter(|p| p.file_name().is_some_and(|n| n == "backups"))
                .and_then(Path::parent)
                .map(Path::to_path_buf)
        },
        |d| Some(d.root().to_path_buf()),
    );

    let mut local_path_and_ledger: Option<(PathBuf, DispositionLedger)> = None;
    let mut live_ledger: Option<DispositionLedger> = None;

    if let Some(root) = &inferred_root {
        let local_data = DataDir::new(root);
        let local_path = local_data.disposition_ledger();
        if let Some(local) = read_ledger_if_present(&local_path)? {
            local_path_and_ledger = Some((local_path, local));
        }

        // Use the WAL-aware read-only diagnostic opener on the live database
        // so committed disposition events still in `-wal` are never missed.
        let db_path = local_data.database();
        if db_path.exists()
            && let Ok(live_pool) = storage::open_diagnostic(&db_path).await
        {
            // Pre-migration or uninitialized databases may lack an `instance`
            // row or `disposition_event` table; only read when identity exists.
            if storage::installation_id(&live_pool).await.is_ok() {
                let built = build_ledger_from_db(&live_pool, 0).await?;
                if !built.entries.is_empty() {
                    live_ledger = Some(built);
                }
            }
            live_pool.close().await;
        }
    }

    // Fast path when no recovery ledger file and no DB disposition events
    // exist yet (for example, a pre-migration snapshot from an uninitialized
    // schema before any disposition has ever occurred).
    if mirror.is_none() && local_path_and_ledger.is_none() && live_ledger.is_none() {
        return Ok(None);
    }

    let snapshot_installation_id = crate::backup::snapshot_installation(snapshot).await?;
    let mut ledgers: Vec<DispositionLedger> = Vec::new();
    let mut local_covered_ids: BTreeSet<i64> = BTreeSet::new();

    if let Some(mirror) = mirror {
        if mirror.installation_id != snapshot_installation_id {
            return Err(LedgerError::InstallationMismatch {
                path: mirror_path,
                expected: snapshot_installation_id,
                actual: mirror.installation_id,
            }
            .into());
        }
        ledgers.push(mirror);
    }

    if let Some((local_path, local)) = local_path_and_ledger {
        if local.installation_id != snapshot_installation_id {
            return Err(LedgerError::InstallationMismatch {
                path: local_path,
                expected: snapshot_installation_id,
                actual: local.installation_id,
            }
            .into());
        }
        local_covered_ids.extend(local.entries.iter().map(|e| e.evaluation_record_id));
        ledgers.push(local);
    }

    if let Some(live) = live_ledger
        && live.installation_id == snapshot_installation_id
    {
        let missing: Vec<i64> = live
            .entries
            .iter()
            .map(|e| e.evaluation_record_id)
            .filter(|id| !local_covered_ids.contains(id))
            .collect();
        if !missing.is_empty() {
            return Err(LedgerError::StaleOrMissingLocalLedger {
                missing_record_ids: missing,
            }
            .into());
        }
        ledgers.push(live);
    }

    if ledgers.is_empty() {
        return Ok(None);
    }

    let refs: Vec<&DispositionLedger> = ledgers.iter().collect();
    let merged = merge_ledgers(&snapshot_installation_id, &refs)?;
    if !merged.entries.is_empty() {
        let check = inspect_snapshot_against_ledger(snapshot, &merged).await?;
        if check.contains_disposed_content() {
            bail!(
                "refusing to copy {} off the server: snapshot contains lawfully disposed \
                 record(s) (record ids: {:?}, matched content hashes: {})",
                snapshot.display(),
                check.disposed_record_ids_present,
                check.disposed_content_hashes_present.len()
            );
        }
    }

    Ok((!merged.entries.is_empty()).then_some(merged))
}

/// Checks that `copy_dir` is an existing directory on a different filesystem
/// than `local_backups_dir` (refusing unmounted mount points).
pub(crate) fn verify_copy_dir_reachable_for_recovery(
    copy_dir: &Path,
    local_backups_dir: &Path,
) -> std::result::Result<(), LedgerError> {
    let metadata = std::fs::metadata(copy_dir).map_err(|err| LedgerError::CopyDirUnavailable {
        directory: copy_dir.to_path_buf(),
        problem: err.to_string(),
    })?;
    if !metadata.is_dir() {
        return Err(LedgerError::CopyDirUnavailable {
            directory: copy_dir.to_path_buf(),
            problem: "not a directory".to_owned(),
        });
    }
    if local_backups_dir.exists() {
        match backup_copy::shares_filesystem(copy_dir, local_backups_dir) {
            Ok(None) => {}
            Ok(Some(problem)) => {
                return Err(LedgerError::CopyDirUnavailable {
                    directory: copy_dir.to_path_buf(),
                    problem: problem.to_owned(),
                });
            }
            Err(err) => {
                return Err(LedgerError::CopyDirUnavailable {
                    directory: copy_dir.to_path_buf(),
                    problem: format!("{err:#}"),
                });
            }
        }
    }
    Ok(())
}

/// Checks that a freshly taken local snapshot agrees with the installation's
/// local recovery ledger and that the local ledger covers every
/// `disposition_event` recorded in the snapshot. Fails closed if the local
/// ledger is corrupt, mismatched, missing/stale relative to database
/// disposition events, or shows disposed records still present in the snapshot.
pub async fn verify_local_snapshot_against_ledger(
    data_dir: &DataDir,
    snapshot: &Path,
) -> Result<()> {
    let ledger_path = data_dir.disposition_ledger();
    let local = read_ledger_if_present(&ledger_path).with_context(|| {
        format!(
            "checking local disposition ledger {}",
            ledger_path.display()
        )
    })?;

    let snap_pool = storage::open_snapshot(snapshot).await?;
    let snapshot_id = match storage::installation_id(&snap_pool).await {
        Ok(id) => id,
        Err(err) => {
            snap_pool.close().await;
            if local.is_some() {
                return Err(err).with_context(|| {
                    format!(
                        "reading installation identity of {} with local ledger present",
                        snapshot.display()
                    )
                });
            }
            // Pre-migration uninitialized snapshots may not have an `instance`
            // row yet when no disposition ledger exists.
            return Ok(());
        }
    };
    let snap_db_ledger = build_ledger_from_db(&snap_pool, 0).await?;
    snap_pool.close().await;

    let Some(local) = local else {
        if !snap_db_ledger.entries.is_empty() {
            let missing = snap_db_ledger
                .entries
                .iter()
                .map(|e| e.evaluation_record_id)
                .collect();
            return Err(LedgerError::StaleOrMissingLocalLedger {
                missing_record_ids: missing,
            }
            .into());
        }
        return Ok(());
    };

    if local.installation_id != snapshot_id {
        return Err(LedgerError::InstallationMismatch {
            path: ledger_path,
            expected: snapshot_id,
            actual: local.installation_id,
        }
        .into());
    }

    let local_ids: BTreeSet<i64> = local
        .entries
        .iter()
        .map(|e| e.evaluation_record_id)
        .collect();
    let missing: Vec<i64> = snap_db_ledger
        .entries
        .iter()
        .map(|e| e.evaluation_record_id)
        .filter(|id| !local_ids.contains(id))
        .collect();
    if !missing.is_empty() {
        return Err(LedgerError::StaleOrMissingLocalLedger {
            missing_record_ids: missing,
        }
        .into());
    }

    if !local.entries.is_empty() {
        let check = inspect_snapshot_against_ledger(snapshot, &local).await?;
        if check.contains_disposed_content() {
            bail!(
                "snapshot {} contains disposed record(s) listed in {} (record ids: {:?}, \
                 matched content hashes: {})",
                snapshot.display(),
                ledger_path.display(),
                check.disposed_record_ids_present,
                check.disposed_content_hashes_present.len()
            );
        }
    }
    Ok(())
}

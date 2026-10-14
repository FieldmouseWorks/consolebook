//! Validated backups: consistent snapshot, validation, explicit
//! durability, retention.
//!
//! A backup is a `VACUUM INTO` snapshot (consistent even while the database
//! is in use), validated with `SQLite`'s integrity check, then fsynced along
//! with its directory, audited, and finally subject to retention pruning.
//! The scheduler in `scheduler.rs` drives this automatically from `serve`;
//! the `backup` command drives it manually. Copies off the server are
//! `backup_copy.rs` (ADR 0024).

use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sqlx::SqlitePool;
use time::OffsetDateTime;
use time::format_description::BorrowedFormatItem;
use time::macros::format_description;

use crate::audit::{self, EventKind};
use crate::data_dir::DataDir;
use crate::serve_lock::ServeLock;
use crate::storage;

const SNAPSHOT_STAMP: &[BorrowedFormatItem<'_>] =
    format_description!("[year][month][day]T[hour][minute][second]Z");

/// Default number of snapshots retention keeps.
pub const DEFAULT_KEEP: usize = 14;

/// Outcome of one completed, validated backup.
#[derive(Debug)]
pub struct BackupReport {
    pub snapshot: PathBuf,
    pub size_bytes: u64,
    pub pruned: Vec<PathBuf>,
}

/// Takes a consistent snapshot of the live database into `backups/`,
/// validates it, makes it durable, records the audit event, and prunes to
/// the retention count. Fails without leaving a snapshot behind if
/// validation fails.
pub async fn run(data_dir: &DataDir, keep: usize) -> Result<BackupReport> {
    let pool = storage::open_existing(&data_dir.database()).await?;
    let snapshot = match take_snapshot(&pool, data_dir, SCHEDULED_PREFIX).await {
        Ok(snapshot) => snapshot,
        Err(err) => {
            pool.close().await;
            return Err(err);
        }
    };
    audit::record(&pool, EventKind::BackupCompleted, None, None).await?;
    pool.close().await;
    finish(data_dir, snapshot, Some(keep))
}

/// VACUUM INTO a new snapshot, validate it, and make it durable. Writes
/// nothing to the live database, so it works on a schema this build has
/// not migrated yet.
async fn take_snapshot(pool: &SqlitePool, data_dir: &DataDir, prefix: &str) -> Result<PathBuf> {
    let stamp = OffsetDateTime::now_utc()
        .format(SNAPSHOT_STAMP)
        .context("formatting snapshot timestamp")?;
    let snapshot = data_dir.backups().join(format!("{prefix}{stamp}.db"));
    if snapshot.exists() {
        bail!(
            "snapshot {} already exists; refusing to overwrite a backup",
            snapshot.display()
        );
    }

    let snapshot_str = snapshot
        .to_str()
        .context("backup path is not valid UTF-8")?;
    sqlx::query("VACUUM INTO ?1")
        .bind(snapshot_str)
        .execute(pool)
        .await
        .context("taking VACUUM INTO snapshot")?;

    if let Err(err) = validate_snapshot(&snapshot).await {
        // A snapshot that fails validation must not look like a usable backup.
        let _ = std::fs::remove_file(&snapshot);
        return Err(err.context("snapshot failed validation and was removed"));
    }

    persist(&snapshot)?;
    Ok(snapshot)
}

/// Size the new snapshot and, given a retention count, prune to it.
fn finish(data_dir: &DataDir, snapshot: PathBuf, keep: Option<usize>) -> Result<BackupReport> {
    let size_bytes = std::fs::metadata(&snapshot)
        .with_context(|| format!("reading snapshot metadata {}", snapshot.display()))?
        .len();
    let pruned = match keep {
        Some(keep) => prune(data_dir, keep)?,
        None => Vec::new(),
    };
    Ok(BackupReport {
        snapshot,
        size_bytes,
        pruned,
    })
}

/// The snapshots currently in `backups/`, oldest first by the UTC time in
/// their names; this listing is discovery only — nothing about a backup's
/// validity is inferred from its name.
pub fn snapshots(data_dir: &DataDir) -> Result<Vec<PathBuf>> {
    snapshots_in(&data_dir.backups())
}

/// The snapshot files in `dir` (local `backups/` or a copy directory),
/// oldest first by the UTC time in their names.
pub fn snapshots_in(dir: &Path) -> Result<Vec<PathBuf>> {
    let dir = dir.to_path_buf();
    let mut found = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(found),
        Err(err) => {
            return Err(err).with_context(|| format!("listing backups in {}", dir.display()));
        }
    };
    for entry in entries {
        let entry = entry.context("reading backup directory entry")?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("consolebook-") && name.ends_with(".db") {
            found.push(entry.path());
        }
    }
    // By the stamp, not the whole name: `consolebook-prerestore-<stamp>`
    // would otherwise sort after every `consolebook-<stamp>`, and stay
    // "newest" for good after a restore (#105).
    found.sort_by_key(|path| snapshot_order(path));
    Ok(found)
}

/// The UTC stamp a snapshot name ends with, then the whole name.
fn snapshot_order(path: &Path) -> (String, String) {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = name.strip_suffix(".db").unwrap_or(&name);
    let stamp = stem.rsplit('-').next().unwrap_or(stem).to_owned();
    (stamp, name)
}

/// Unix mtime of the newest snapshot, for scheduling and diagnostics.
pub fn latest_snapshot_mtime(data_dir: &DataDir) -> Result<Option<i64>> {
    let mut latest: Option<i64> = None;
    for snapshot in snapshots(data_dir)? {
        let modified = std::fs::metadata(&snapshot)
            .and_then(|m| m.modified())
            .with_context(|| format!("reading snapshot mtime {}", snapshot.display()))?;
        let unix = modified
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        latest = Some(latest.map_or(unix, |current| current.max(unix)));
    }
    Ok(latest)
}

/// Deletes the oldest snapshots beyond `keep`. Never deletes the last
/// remaining snapshot, whatever `keep` says. Returns what was removed.
pub fn prune(data_dir: &DataDir, keep: usize) -> Result<Vec<PathBuf>> {
    prune_in(&data_dir.backups(), keep)
}

/// [`prune`] for any directory of snapshots, a copy directory included.
pub fn prune_in(dir: &Path, keep: usize) -> Result<Vec<PathBuf>> {
    let keep = keep.max(1);
    let all = snapshots_in(dir)?;
    let excess = all.len().saturating_sub(keep);
    let mut pruned = Vec::with_capacity(excess);
    for snapshot in all.into_iter().take(excess) {
        std::fs::remove_file(&snapshot)
            .with_context(|| format!("pruning snapshot {}", snapshot.display()))?;
        pruned.push(snapshot);
    }
    Ok(pruned)
}

/// Opens the snapshot as its own database and requires a clean integrity check.
pub async fn validate_snapshot(snapshot: &Path) -> Result<()> {
    let pool = storage::open_existing(snapshot).await?;
    let verdict = storage::integrity_check(&pool).await?;
    pool.close().await;
    if verdict != ["ok"] {
        bail!("integrity check reported: {}", verdict.join("; "));
    }
    Ok(())
}

/// Snapshots `run` takes: `consolebook-<stamp>.db`.
const SCHEDULED_PREFIX: &str = "consolebook-";

/// Snapshots taken before a migration: `consolebook-premigration-<stamp>.db`.
pub const PRE_MIGRATION_PREFIX: &str = "consolebook-premigration-";

/// Pre-migration snapshots kept, at most. A migration that fails on every
/// start, under a service manager that keeps restarting it, replaces
/// copies of the same pre-migration state instead of filling the disk.
pub const PRE_MIGRATION_KEEP: usize = 3;

/// Whether this executable would migrate the database. Reads only.
async fn migration_pending(data_dir: &DataDir) -> Result<bool> {
    let database = data_dir.database();
    if !database.exists() {
        return Ok(false);
    }
    let pool = storage::open_diagnostic(&database).await?;
    let pending = storage::pending_migrations(&pool).await;
    pool.close().await;
    Ok(!pending?.is_empty())
}

/// Takes a validated snapshot before this executable migrates an existing
/// database, so an upgrade that goes wrong has a recovery point from the
/// moment before it changed anything (there is no downgrade; ADR 0023).
///
/// Returns `None` when there is no database yet or nothing to migrate. Any
/// failure is returned, so startup fails closed rather than migrating
/// without the snapshot.
///
/// Ordinary retention is left to the next scheduled backup, since the
/// caller may not know the configured count; only pre-migration snapshots
/// beyond [`PRE_MIGRATION_KEEP`] are pruned here.
pub async fn before_migrating(data_dir: &DataDir) -> Result<Option<PreMigrationBackup>> {
    if !migration_pending(data_dir).await? {
        return Ok(None);
    }
    // Nothing is written to the old schema: the audit event waits for the
    // migrated database (`PreMigrationBackup::record`).
    let pool = storage::open_existing(&data_dir.database()).await?;
    let snapshot = take_snapshot(&pool, data_dir, PRE_MIGRATION_PREFIX).await;
    pool.close().await;
    let report = snapshot
        .and_then(|snapshot| finish(data_dir, snapshot, None))
        .and_then(|mut report| {
            report.pruned = prune_pre_migration(data_dir)?;
            Ok(report)
        })
        .context("taking the pre-migration backup")?;
    Ok(Some(PreMigrationBackup { report }))
}

/// Deletes the oldest pre-migration snapshots beyond [`PRE_MIGRATION_KEEP`],
/// leaving every other snapshot alone.
fn prune_pre_migration(data_dir: &DataDir) -> Result<Vec<PathBuf>> {
    let pre_migration: Vec<PathBuf> = snapshots(data_dir)?
        .into_iter()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(PRE_MIGRATION_PREFIX))
        })
        .collect();
    let excess = pre_migration.len().saturating_sub(PRE_MIGRATION_KEEP);
    let mut pruned = Vec::with_capacity(excess);
    for snapshot in pre_migration.into_iter().take(excess) {
        std::fs::remove_file(&snapshot)
            .with_context(|| format!("pruning snapshot {}", snapshot.display()))?;
        pruned.push(snapshot);
    }
    Ok(pruned)
}

/// A validated snapshot of the database as it was before this build
/// migrated it. Its audit event belongs in the migrated database, so the
/// caller records it once migration has run.
#[must_use = "record the backup's audit event once the database is migrated"]
#[derive(Debug)]
pub struct PreMigrationBackup {
    report: BackupReport,
}

impl PreMigrationBackup {
    /// The snapshot, for logging before migration.
    #[must_use]
    pub fn snapshot(&self) -> &Path {
        &self.report.snapshot
    }

    /// Records the audit event in the migrated database.
    pub async fn record(self, migrated: &SqlitePool) -> Result<BackupReport> {
        audit::record(migrated, EventKind::BackupCompleted, None, None).await?;
        Ok(self.report)
    }
}

/// Opens the installation's database for use, migrating it if this build
/// needs to. The one way an executable migrates: when a migration is
/// pending it first takes the validated pre-migration backup, and refuses
/// to migrate without it.
///
/// Migrating needs the installation to itself, or a running server could
/// commit records after the snapshot and then run against the new schema.
/// `serve` passes the lock it holds; any other command takes it here when a
/// migration is pending, and refuses while a server holds it. With nothing
/// pending, no lock is needed, so `recover` works beside a running server.
///
/// With an off-server copy configured, the snapshot is also copied there
/// before migrating, so a failed upgrade's recovery point does not live
/// only on the database's disk. That copy is best effort: an unreachable
/// share is reported, and the next start's catch-up retries it.
pub async fn open_migrated(
    data_dir: &DataDir,
    held: Option<&ServeLock>,
    copy: Option<OffServerCopy<'_>>,
) -> Result<SqlitePool> {
    let _exclusive = match held {
        None if migration_pending(data_dir).await? => Some(ServeLock::acquire(data_dir).context(
            "this version migrates the database, which needs the installation to itself; \
             stop the running service, then run this command again",
        )?),
        _ => None,
    };
    // Checked again under the lock, before anything is taken or changed.
    let pre_migration = before_migrating(data_dir).await?;
    if let Some(backup) = &pre_migration {
        tracing::info!(
            snapshot = %backup.snapshot().display(),
            "this version migrates the database; took a validated backup first \
             (to go back: stop the service, install the previous release, and restore this snapshot)"
        );
        if let Some(copy) = copy {
            match crate::backup_copy::copy_snapshot(backup.snapshot(), copy.dir, copy.keep) {
                Ok(report) => tracing::info!(
                    copy = %report.copy.display(),
                    sha256 = report.sha256,
                    "pre-migration backup copied off the server and verified"
                ),
                Err(err) => tracing::warn!(
                    "could not copy the pre-migration backup off the server: {err:#}; \
                     it is in backups/, and the next start retries"
                ),
            }
        }
    }
    let pool = storage::open(&data_dir.database()).await?;
    if let Some(backup) = pre_migration {
        backup.record(&pool).await?;
    }
    Ok(pool)
}

/// The off-server copy directory and its retention count (ADR 0024).
#[derive(Debug, Clone, Copy)]
pub struct OffServerCopy<'a> {
    pub dir: &'a Path,
    pub keep: usize,
}

/// Explicit durability step: fsync a file and its directory entry.
pub fn persist(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|f| f.sync_all())
        .with_context(|| format!("syncing {}", path.display()))?;
    let dir = path.parent().context("path has no parent directory")?;
    File::open(dir)
        .and_then(|f| f.sync_all())
        .with_context(|| format!("syncing directory {}", dir.display()))?;
    Ok(())
}

//! Automatic backup scheduling.
//!
//! `serve` spawns one background task that keeps the newest snapshot no
//! older than the configured interval. The decision function is pure and
//! tested; the task just applies it on a coarse tick, so a missed tick
//! (sleep, clock jump, slow backup) self-corrects on the next one.

use std::time::{Duration, Instant};

use sqlx::SqlitePool;
use time::OffsetDateTime;

use crate::backup;
use crate::backup_copy::{self, BackupSettings};
use crate::capabilities::Capability;
use crate::notices::{self, NoticeKind};

/// Default interval between automatic backups.
pub const DEFAULT_INTERVAL_HOURS: u64 = 24;

/// How often the scheduler re-evaluates whether a backup is due.
const TICK: Duration = Duration::from_secs(60);

/// Whether a backup is due now, given the newest snapshot's mtime.
/// No snapshot at all means due immediately.
#[must_use]
pub fn backup_due(latest_snapshot_mtime: Option<i64>, now: i64, interval_secs: i64) -> bool {
    match latest_snapshot_mtime {
        None => true,
        Some(latest) => now.saturating_sub(latest) >= interval_secs,
    }
}

/// How long a failed off-server copy waits before the next attempt.
const COPY_RETRY: Duration = Duration::from_mins(15);

/// Runs the automatic-backup loop until the process exits. Failures are
/// logged, surfaced to administrators as in-app notices, and retried on
/// the schedule; they never crash the server. With a copy directory, each
/// validated snapshot is then copied off the server (ADR 0024), and a copy
/// that failed or fell behind is retried every [`COPY_RETRY`].
pub async fn run(settings: BackupSettings, pool: SqlitePool) {
    let data_dir = &settings.data_dir;
    let keep = settings.keep;
    let interval_secs =
        i64::try_from(settings.interval_hours.saturating_mul(3600)).unwrap_or(i64::MAX);
    let mut next_copy_attempt = Instant::now();
    loop {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let due = match backup::latest_snapshot_mtime(data_dir) {
            Ok(latest) => backup_due(latest, now, interval_secs),
            Err(err) => {
                tracing::error!("cannot inspect backups directory: {err:#}");
                false
            }
        };
        if due {
            match backup::run(data_dir, keep).await {
                Ok(report) => {
                    tracing::info!(
                        snapshot = %report.snapshot.display(),
                        size_bytes = report.size_bytes,
                        pruned = report.pruned.len(),
                        "automatic backup complete and validated"
                    );
                    // A fresh snapshot is copied at once, not on the retry clock.
                    next_copy_attempt = Instant::now();
                }
                Err(err) => {
                    tracing::error!("automatic backup failed: {err:#}");
                    report_failure(
                        &pool,
                        NoticeKind::BackupFailed,
                        &backup_failure_message(&err),
                    )
                    .await;
                }
            }
        }
        if let Some(copy_dir) = &settings.copy_dir
            && Instant::now() >= next_copy_attempt
        {
            match backup_copy::catch_up(data_dir, copy_dir, keep) {
                Ok(Some(copy)) => tracing::info!(
                    copy = %copy.copy.display(),
                    sha256 = copy.sha256,
                    pruned = copy.pruned.len(),
                    "backup copied off the server and verified"
                ),
                Ok(None) => {}
                Err(err) => {
                    tracing::error!("backup copy failed: {err:#}");
                    report_failure(
                        &pool,
                        NoticeKind::BackupCopyFailed,
                        &copy_failure_message(&err),
                    )
                    .await;
                    next_copy_attempt = Instant::now() + COPY_RETRY;
                }
            }
        }
        tokio::time::sleep(TICK).await;
    }
}

fn backup_failure_message(err: &anyhow::Error) -> String {
    format!(
        "Automatic backup failed: {err:#}. Backups retry on schedule; \
         check disk space and `consolebook doctor`, and treat repeated \
         failures as urgent."
    )
}

fn copy_failure_message(err: &anyhow::Error) -> String {
    format!(
        "Copying the latest backup off this server failed: {err:#}. The local \
         backup is intact, but it shares a disk with the database. The copy \
         retries every 15 minutes; check that the backup copy directory is \
         mounted and writable."
    )
}

/// Surfaces a backup or copy failure to every administrator as a persisted
/// notice. Deduplication in the notices service keeps a repeatedly failing
/// backup (or copy) at one unread notice per administrator.
async fn report_failure(pool: &SqlitePool, kind: NoticeKind, message: &str) {
    match notices::notify_capability_holders(pool, Capability::ManageUsers, kind, message).await {
        Ok(created) if created > 0 => {
            tracing::info!(
                created,
                kind = kind.as_str(),
                "failure surfaced as in-app notices"
            );
        }
        Ok(_) => {}
        Err(notify_err) => {
            tracing::error!("could not create failure notices: {notify_err:#}");
        }
    }
}

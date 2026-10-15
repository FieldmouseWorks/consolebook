//! Backup copies off the server, and backup health (ADR 0024).
//!
//! A local snapshot in `backups/` shares a disk with the database it
//! protects. When a copy directory is configured (a second disk or a mounted
//! network share), each validated local snapshot is copied there: written
//! under a temporary name, fsynced, verified byte for byte against the
//! snapshot by SHA-256, renamed into place, its directory fsynced, and the
//! copy directory pruned to the same retention. Verification reads the copy
//! back through the operating system, which may answer from its cache; it
//! proves what was written, not the remote medium.
//!
//! A copy directory on the same filesystem as the local snapshots is never
//! used or counted: that is what an unmounted share or disk looks like (its
//! empty mount point), and a copy there shares the failure it exists to
//! survive.
//!
//! A copy directory holds one installation's backups. A copy is refused
//! when the directory's newest copy belongs to another installation: a new
//! server set up to replace a lost one would otherwise copy its own empty
//! database there as the newest backup, and prune the real ones (#115).
//! Only this installation's own backups are copied, never the database a
//! restore set aside.
//!
//! Health is read from the files themselves, never from stored state: a
//! copy is current exactly when the newest local backup's file name is
//! present in a copy directory on another filesystem.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::backup;
use crate::capabilities::{self, Capability};
use crate::data_dir::DataDir;

/// How `serve` was configured to take and copy backups.
#[derive(Debug, Clone)]
pub struct BackupSettings {
    pub data_dir: DataDir,
    pub interval_hours: u64,
    pub keep: usize,
    pub copy_dir: Option<PathBuf>,
}

/// Outcome of one verified copy.
#[derive(Debug)]
pub struct CopyReport {
    pub copy: PathBuf,
    pub sha256: String,
    pub pruned: Vec<PathBuf>,
}

/// Copies one validated local snapshot into `copy_dir` and prunes there.
///
/// Refuses a missing or non-directory target, one on the snapshot's own
/// filesystem, one holding another installation's backups, and an existing
/// copy of the same name. A copy that does not match the snapshot, or whose
/// rename cannot be made durable, is removed so the next attempt makes it
/// again; the local snapshot is never touched.
pub async fn copy_snapshot(snapshot: &Path, copy_dir: &Path, keep: usize) -> Result<CopyReport> {
    let name = snapshot
        .file_name()
        .context("snapshot path has no file name")?
        .to_owned();
    let local_dir = snapshot
        .parent()
        .context("snapshot path has no parent directory")?;
    let metadata = std::fs::metadata(copy_dir).with_context(|| {
        format!(
            "backup copy directory {} is not reachable",
            copy_dir.display()
        )
    })?;
    if !metadata.is_dir() {
        bail!(
            "backup copy directory {} is not a directory",
            copy_dir.display()
        );
    }
    if let Some(problem) = shares_filesystem(copy_dir, local_dir)? {
        bail!("backup copy directory {}: {problem}", copy_dir.display());
    }
    ensure_same_installation(snapshot, copy_dir).await?;
    let target = copy_dir.join(&name);
    if target.exists() {
        bail!(
            "{} already exists; refusing to overwrite a backup copy",
            target.display()
        );
    }
    let mut partial_name = name;
    partial_name.push(".partial");
    let partial = copy_dir.join(partial_name);
    // A partial file is only ever left by an interrupted copy.
    let _ = std::fs::remove_file(&partial);

    let expected = sha256_file(snapshot)?;
    std::fs::copy(snapshot, &partial)
        .with_context(|| format!("copying {} to {}", snapshot.display(), partial.display()))?;
    File::open(&partial)
        .and_then(|file| file.sync_all())
        .with_context(|| format!("syncing {}", partial.display()))?;
    let actual = sha256_file(&partial)?;
    if actual != expected {
        let _ = std::fs::remove_file(&partial);
        bail!(
            "the copy of {} does not match the snapshot (sha256 {actual}, expected {expected}); removed it",
            snapshot.display()
        );
    }
    std::fs::rename(&partial, &target)
        .with_context(|| format!("naming the copy {}", target.display()))?;
    if let Err(err) = backup::persist(&target) {
        // A copy whose name is in place counts as current, so one that may
        // not survive a crash must not keep that name.
        let _ = std::fs::remove_file(&target);
        return Err(err.context(format!(
            "{} was removed so it is copied again",
            target.display()
        )));
    }
    let pruned = backup::prune_in(copy_dir, keep)?;
    Ok(CopyReport {
        copy: target,
        sha256: actual,
        pruned,
    })
}

/// Copies the newest local snapshot when the copy directory lacks it, so a
/// copy that failed (or a copy directory added later) catches up without
/// waiting for the next backup. When the newest copy is already there, it
/// finishes retention instead, which a prune that failed after its copy
/// left undone; that prune deletes nothing until the directory is known to
/// be this installation's, since a file of the same name may be another's.
/// Returns `None` when no copy was made.
pub async fn catch_up(
    data_dir: &DataDir,
    copy_dir: &Path,
    keep: usize,
) -> Result<Option<CopyReport>> {
    let Some(newest) = backup::own_backups_in(&data_dir.backups())?.pop() else {
        return Ok(None);
    };
    let name = newest
        .file_name()
        .context("snapshot path has no file name")?;
    if !copy_dir.join(name).exists() {
        return copy_snapshot(&newest, copy_dir, keep).await.map(Some);
    }
    if let Some(problem) = shares_filesystem(copy_dir, &data_dir.backups())? {
        bail!("backup copy directory {}: {problem}", copy_dir.display());
    }
    if backup::snapshots_in(copy_dir)?.len() > keep.max(1) {
        ensure_same_installation(&newest, copy_dir).await?;
        backup::prune_in(copy_dir, keep)?;
    }
    Ok(None)
}

/// Refuses `snapshot` when the copy directory belongs to another
/// installation: the one in its newest readable backup or, failing any,
/// its newest readable copy of any kind (an earlier release copied
/// pre-restore snapshots too). Both are read from the files, so a copy
/// directory written before this check existed is protected too. A file
/// that cannot be read as a backup restores nothing, so it neither claims
/// the directory nor blocks the copy that replaces it. A copy directory
/// with nothing readable belongs to whoever copies there first.
async fn ensure_same_installation(snapshot: &Path, copy_dir: &Path) -> Result<()> {
    let own = backup::own_backups_in(copy_dir)?;
    let others = backup::snapshots_in(copy_dir)?
        .into_iter()
        .filter(|path| !own.contains(path));
    let mut candidates: Vec<PathBuf> = own.iter().rev().cloned().collect();
    candidates.extend(others.rev());
    let mut theirs = None;
    for copy in &candidates {
        if let Ok(id) = backup::snapshot_installation(copy).await {
            theirs = Some(id);
            break;
        }
    }
    let Some(theirs) = theirs else {
        return Ok(());
    };
    let ours = backup::snapshot_installation(snapshot).await?;
    if theirs != ours {
        bail!(
            "backup copy directory {} holds the backups of installation {theirs}, and this \
             is installation {ours}; copying here would bury them. If this server replaces \
             that installation, stop the service and restore the newest copy there \
             (docs/install.md); otherwise give this installation its own copy directory",
            copy_dir.display()
        );
    }
    Ok(())
}

const SAME_FILESYSTEM: &str = "is on the same filesystem as the local backups, so it is not \
     off this server's disk; if it should be a network share or another disk, it is not mounted";

/// `Some(reason)` when `copy_dir` is on the same filesystem as `local_dir`.
#[cfg(unix)]
fn shares_filesystem(copy_dir: &Path, local_dir: &Path) -> Result<Option<&'static str>> {
    use std::os::unix::fs::MetadataExt;
    let device = |path: &Path| {
        std::fs::metadata(path)
            .map(|metadata| metadata.dev())
            .with_context(|| format!("reading {}", path.display()))
    };
    Ok((device(copy_dir)? == device(local_dir)?).then_some(SAME_FILESYSTEM))
}

#[cfg(not(unix))]
fn shares_filesystem(_copy_dir: &Path, _local_dir: &Path) -> Result<Option<&'static str>> {
    Ok(None)
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("opening {} to hash it", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("reading {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

// ------------------------------------------------------------------ health

/// Backup health as an administrator sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackupStatus {
    pub interval_hours: u64,
    pub keep: usize,
    pub local: LocalStatus,
    pub copy: CopyStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalStatus {
    pub directory: String,
    pub count: usize,
    /// File name of the newest backup of this installation (never a database
    /// a restore set aside), which carries its UTC timestamp.
    pub newest: Option<String>,
    /// Unix time the newest snapshot was written.
    pub newest_at: Option<i64>,
}

/// Where off-server copies stand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CopyStatus {
    /// No copy directory is configured: every backup shares the database's disk.
    NotConfigured,
    /// Configured, but the directory cannot be listed.
    Unreachable { directory: String, problem: String },
    /// Reachable, but it lacks the newest local snapshot.
    Behind {
        directory: String,
        count: usize,
        newest: Option<String>,
    },
    /// Holds the newest local snapshot.
    Current {
        directory: String,
        count: usize,
        newest: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusRefusal {
    /// Backup health belongs to whoever administers the installation.
    CapabilityRequired,
}

/// Backup health for a holder of `manage_users`.
pub async fn status_for(
    pool: &SqlitePool,
    actor_user_id: i64,
    settings: &BackupSettings,
) -> Result<std::result::Result<BackupStatus, StatusRefusal>> {
    if !capabilities::user_has(pool, actor_user_id, Capability::ManageUsers).await? {
        return Ok(Err(StatusRefusal::CapabilityRequired));
    }
    status(settings).map(Ok)
}

/// Backup health from the files themselves.
pub fn status(settings: &BackupSettings) -> Result<BackupStatus> {
    let local_dir = settings.data_dir.backups();
    let local = backup::snapshots(&settings.data_dir)?;
    let newest_local = newest_own_backup(&settings.data_dir)?;
    let copy = copy_status_of(
        settings.copy_dir.as_deref(),
        &local_dir,
        newest_local.as_deref(),
    );
    Ok(BackupStatus {
        interval_hours: settings.interval_hours,
        keep: settings.keep,
        local: LocalStatus {
            directory: local_dir.display().to_string(),
            count: local.len(),
            newest: newest_local,
            newest_at: backup::latest_snapshot_mtime(&settings.data_dir)?,
        },
        copy,
    })
}

/// Where off-server copies stand, from the files alone. Reads only: it
/// never creates, writes, or repairs the copy directory, so `doctor` can
/// use it (ADR 0003).
pub fn copy_status(data_dir: &DataDir, copy_dir: Option<&Path>) -> Result<CopyStatus> {
    Ok(copy_status_of(
        copy_dir,
        &data_dir.backups(),
        newest_own_backup(data_dir)?.as_deref(),
    ))
}

/// The file name of the newest backup that catch-up would copy.
fn newest_own_backup(data_dir: &DataDir) -> Result<Option<String>> {
    Ok(backup::own_backups_in(&data_dir.backups())?
        .last()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned()))
}

fn copy_status_of(
    copy_dir: Option<&Path>,
    local_dir: &Path,
    newest_local: Option<&str>,
) -> CopyStatus {
    let Some(dir) = copy_dir else {
        return CopyStatus::NotConfigured;
    };
    let directory = dir.display().to_string();
    match std::fs::metadata(dir) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return CopyStatus::Unreachable {
                directory,
                problem: "not a directory".to_owned(),
            };
        }
        Err(err) => {
            return CopyStatus::Unreachable {
                directory,
                problem: err.to_string(),
            };
        }
    }
    match shares_filesystem(dir, local_dir) {
        Ok(None) => {}
        Ok(Some(problem)) => {
            return CopyStatus::Unreachable {
                directory,
                problem: problem.to_owned(),
            };
        }
        Err(err) => {
            return CopyStatus::Unreachable {
                directory,
                problem: format!("{err:#}"),
            };
        }
    }
    let copies = match backup::snapshots_in(dir) {
        Ok(copies) => copies,
        Err(err) => {
            return CopyStatus::Unreachable {
                directory,
                problem: format!("{err:#}"),
            };
        }
    };
    let names: Vec<String> = copies
        .iter()
        .filter_map(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    match newest_local {
        Some(newest) if names.iter().any(|name| name == newest) => CopyStatus::Current {
            directory,
            count: names.len(),
            newest: newest.to_owned(),
        },
        _ => CopyStatus::Behind {
            directory,
            count: names.len(),
            newest: names.last().cloned(),
        },
    }
}

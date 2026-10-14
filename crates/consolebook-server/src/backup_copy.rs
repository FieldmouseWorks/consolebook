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
//! Health is read from the files themselves, never from stored state: a
//! copy is current exactly when the newest local snapshot's file name is
//! present in the copy directory.

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
/// Refuses a missing or non-directory target and an existing copy of the
/// same name. A copy that does not match the snapshot is removed; the local
/// snapshot is never touched.
pub fn copy_snapshot(snapshot: &Path, copy_dir: &Path, keep: usize) -> Result<CopyReport> {
    let name = snapshot
        .file_name()
        .context("snapshot path has no file name")?
        .to_owned();
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
    backup::persist(&target)?;
    let pruned = backup::prune_in(copy_dir, keep)?;
    Ok(CopyReport {
        copy: target,
        sha256: actual,
        pruned,
    })
}

/// Copies the newest local snapshot when the copy directory lacks it, so a
/// copy that failed (or a copy directory added later) catches up without
/// waiting for the next backup. Returns `None` when there is nothing to do.
pub fn catch_up(data_dir: &DataDir, copy_dir: &Path, keep: usize) -> Result<Option<CopyReport>> {
    let Some(newest) = backup::snapshots(data_dir)?.pop() else {
        return Ok(None);
    };
    let name = newest
        .file_name()
        .context("snapshot path has no file name")?;
    if copy_dir.join(name).exists() {
        return Ok(None);
    }
    copy_snapshot(&newest, copy_dir, keep).map(Some)
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
    /// File name of the newest snapshot, which carries its UTC timestamp.
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
    let newest_local = local
        .last()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned());
    let copy = match &settings.copy_dir {
        None => CopyStatus::NotConfigured,
        Some(dir) => copy_status(dir, newest_local.as_deref()),
    };
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

fn copy_status(dir: &Path, newest_local: Option<&str>) -> CopyStatus {
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

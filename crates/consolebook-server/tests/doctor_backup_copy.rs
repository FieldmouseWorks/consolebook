//! `doctor` reports the backup copy directory in the same states as
//! `/api/backups/status`, and never writes there (ADR 0003, ADR 0024).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use consolebook_server::data_dir::DataDir;
use consolebook_server::doctor::{self, Finding, Verdict};
use consolebook_server::{backup, backup_copy, storage};

struct Installation {
    tmp: tempfile::TempDir,
    /// Another filesystem, standing in for a mounted share.
    other_fs: tempfile::TempDir,
    data_dir: DataDir,
    snapshot: PathBuf,
}

impl Installation {
    /// A migrated installation with one validated local snapshot.
    async fn new() -> Self {
        let tmp = tempfile::tempdir().expect("temp dir");
        let data_dir = DataDir::new(tmp.path().join("data"));
        data_dir.ensure_layout().expect("layout");
        storage::open(&data_dir.database())
            .await
            .expect("open")
            .close()
            .await;
        let snapshot = backup::run(&data_dir, 14).await.expect("backup").snapshot;
        let other_fs = other_filesystem(tmp.path());
        Self {
            tmp,
            other_fs,
            data_dir,
            snapshot,
        }
    }

    fn share(&self) -> PathBuf {
        let dir = self.other_fs.path().join("offsite");
        std::fs::create_dir_all(&dir).expect("share");
        dir
    }

    async fn copy_finding(&self, copy_dir: Option<&Path>) -> Finding {
        let options = doctor::Options {
            backup_copy_dir: copy_dir,
            ..doctor::Options::default()
        };
        doctor::run(&self.data_dir, &options)
            .await
            .into_iter()
            .find(|finding| finding.check == "backup copy")
            .expect("doctor reports the backup copy")
    }
}

/// `/dev/shm` is a separate tmpfs on Linux hosts and CI runners; fail
/// loudly rather than test nothing.
fn other_filesystem(beside: &Path) -> tempfile::TempDir {
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir_in("/dev/shm").expect("these tests need /dev/shm (a tmpfs)");
    let dev = |p: &Path| std::fs::metadata(p).expect("metadata").dev();
    assert_ne!(
        dev(dir.path()),
        dev(beside),
        "/dev/shm must be another filesystem"
    );
    dir
}

/// Every entry's name, length, and modification time.
fn listing(dir: &Path) -> Vec<(String, u64, SystemTime)> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .expect("list")
        .map(|entry| {
            let entry = entry.expect("entry");
            let metadata = entry.metadata().expect("metadata");
            (
                entry.file_name().to_string_lossy().into_owned(),
                metadata.len(),
                metadata.modified().expect("mtime"),
            )
        })
        .collect();
    entries.sort();
    entries
}

#[tokio::test]
async fn not_configured_is_a_warning() {
    let fx = Installation::new().await;
    let finding = fx.copy_finding(None).await;
    assert_eq!(finding.verdict, Verdict::Warn, "{finding:?}");
    assert!(finding.detail.starts_with("not_configured:"), "{finding:?}");
}

#[tokio::test]
async fn an_unreachable_copy_directory_is_reported_and_never_created() {
    let fx = Installation::new().await;
    let missing = fx.other_fs.path().join("not-mounted");
    let finding = fx.copy_finding(Some(&missing)).await;
    assert_eq!(finding.verdict, Verdict::Warn, "{finding:?}");
    assert!(finding.detail.starts_with("unreachable:"), "{finding:?}");
    assert!(
        !missing.exists(),
        "doctor must not create the copy directory"
    );

    // A directory on the database's own disk is no copy at all.
    let local = fx.tmp.path().join("same-disk");
    std::fs::create_dir(&local).expect("local dir");
    let finding = fx.copy_finding(Some(&local)).await;
    assert!(finding.detail.starts_with("unreachable:"), "{finding:?}");
    assert!(finding.detail.contains("same filesystem"), "{finding:?}");
    assert!(listing(&local).is_empty());
}

#[tokio::test]
async fn a_copy_directory_without_the_newest_snapshot_is_behind() {
    let fx = Installation::new().await;
    let share = fx.share();
    std::fs::write(
        share.join("consolebook-20260101T000000Z.db"),
        b"invented older copy",
    )
    .expect("older copy");
    let before = listing(&share);
    let finding = fx.copy_finding(Some(&share)).await;
    assert_eq!(finding.verdict, Verdict::Warn, "{finding:?}");
    assert!(finding.detail.starts_with("behind:"), "{finding:?}");
    assert!(finding.detail.contains("has 1 copy"), "{finding:?}");
    assert_eq!(
        listing(&share),
        before,
        "doctor wrote to the copy directory"
    );
}

#[tokio::test]
async fn a_copy_directory_holding_the_newest_snapshot_is_current() {
    let fx = Installation::new().await;
    let share = fx.share();
    backup_copy::copy_snapshot(&fx.snapshot, &share, 14)
        .await
        .expect("copy");
    let before = listing(&share);
    let finding = fx.copy_finding(Some(&share)).await;
    assert_eq!(finding.verdict, Verdict::Ok, "{finding:?}");
    assert!(finding.detail.starts_with("current:"), "{finding:?}");
    assert!(!doctor::has_failure(
        &doctor::run(
            &fx.data_dir,
            &doctor::Options {
                backup_copy_dir: Some(&share),
                ..doctor::Options::default()
            }
        )
        .await
    ));
    assert_eq!(
        listing(&share),
        before,
        "doctor wrote to the copy directory"
    );
}

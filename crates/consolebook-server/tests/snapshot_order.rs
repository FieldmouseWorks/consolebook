//! Snapshots order by the time in their names, pre-restore ones included
//! (#105). File contents are invented; nothing here opens them.

use std::path::Path;

use consolebook_server::backup;
use consolebook_server::backup_copy::{self, BackupSettings, CopyStatus};
use consolebook_server::data_dir::DataDir;

const EARLY: &str = "consolebook-20261014T100000Z.db";
const SET_ASIDE: &str = "consolebook-prerestore-20261014T110000Z.db";
const LATEST: &str = "consolebook-20261014T120000Z.db";

fn names(paths: &[std::path::PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| {
            path.file_name()
                .expect("name")
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

fn write(dir: &Path, name: &str) {
    std::fs::write(dir.join(name), format!("invented {name}")).expect("write");
}

fn installation() -> (tempfile::TempDir, DataDir) {
    let tmp = tempfile::tempdir().expect("temp dir");
    let data_dir = DataDir::new(tmp.path().join("data"));
    data_dir.ensure_layout().expect("layout");
    // Written out of order, so the listing cannot lean on creation order.
    for name in [LATEST, SET_ASIDE, EARLY] {
        write(&data_dir.backups(), name);
    }
    (tmp, data_dir)
}

/// `/dev/shm` is a separate tmpfs on Linux hosts and CI runners.
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

#[test]
fn a_pre_restore_snapshot_sorts_by_its_time() {
    let (_tmp, data_dir) = installation();
    assert_eq!(
        names(&backup::snapshots(&data_dir).expect("list")),
        [EARLY, SET_ASIDE, LATEST]
    );
}

#[test]
fn retention_removes_the_oldest_by_time() {
    let (_tmp, data_dir) = installation();
    let pruned = backup::prune(&data_dir, 2).expect("prune");
    assert_eq!(names(&pruned), [EARLY]);
    assert_eq!(
        names(&backup::snapshots(&data_dir).expect("list")),
        [SET_ASIDE, LATEST]
    );
}

#[tokio::test]
async fn a_share_holding_only_the_set_aside_database_is_behind() {
    let (tmp, data_dir) = installation();
    let other = other_filesystem(tmp.path());
    let share = other.path().join("offsite");
    std::fs::create_dir(&share).expect("share");
    write(&share, SET_ASIDE);
    let settings = BackupSettings {
        data_dir: data_dir.clone(),
        interval_hours: 24,
        keep: 14,
        copy_dir: Some(share.clone()),
    };
    let status = backup_copy::status(&settings).expect("status");
    assert_eq!(status.local.newest.as_deref(), Some(LATEST));
    assert!(
        matches!(status.copy, CopyStatus::Behind { .. }),
        "{:?}",
        status.copy
    );

    // Catching up copies the latest backup, not the database it replaced.
    let report = backup_copy::catch_up(&data_dir, &share, 14)
        .await
        .expect("catch up")
        .expect("a copy was due");
    assert_eq!(
        report.copy.file_name().expect("name").to_string_lossy(),
        LATEST
    );
}

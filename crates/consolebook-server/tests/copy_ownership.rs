//! A copy directory holds one installation's backups (#115). A new server
//! set up to replace a lost one must not copy its own empty database there
//! as the newest backup, and once it restores one of those copies, copying
//! resumes from the restored installation.

use std::path::{Path, PathBuf};

use consolebook_server::backup_copy::{self, CopyStatus};
use consolebook_server::data_dir::DataDir;
use consolebook_server::{backup, doctor, restore, storage};

/// A migrated installation in `root`, with one validated backup.
async fn installation(root: &Path) -> (DataDir, String, PathBuf) {
    let data_dir = DataDir::new(root);
    data_dir.ensure_layout().expect("layout");
    let pool = storage::open(&data_dir.database()).await.expect("open");
    let id = storage::installation_id(&pool).await.expect("id");
    pool.close().await;
    let snapshot = backup::run(&data_dir, 14).await.expect("backup").snapshot;
    (data_dir, id, snapshot)
}

/// `/dev/shm` is a separate tmpfs on Linux hosts and CI runners; fail
/// loudly rather than test nothing.
fn share(beside: &Path) -> (tempfile::TempDir, PathBuf) {
    use std::os::unix::fs::MetadataExt;
    let other = tempfile::tempdir_in("/dev/shm").expect("these tests need /dev/shm (a tmpfs)");
    let dev = |p: &Path| std::fs::metadata(p).expect("metadata").dev();
    assert_ne!(
        dev(other.path()),
        dev(beside),
        "/dev/shm must be another filesystem"
    );
    let dir = other.path().join("offsite");
    std::fs::create_dir(&dir).expect("share");
    (other, dir)
}

/// Every file name in `dir`, sorted, sidecars and partial files included.
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("list")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

fn name(path: &Path) -> String {
    path.file_name()
        .expect("name")
        .to_string_lossy()
        .into_owned()
}

#[tokio::test]
async fn a_replacement_server_never_copies_over_the_installation_it_replaces() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let (lost, lost_id, lost_backup) = installation(&tmp.path().join("lost")).await;
    let (_other, share) = share(tmp.path());
    backup_copy::copy_snapshot(&lost_backup, &share, 14)
        .await
        .expect("the lost server's copy");
    drop(lost);
    // Snapshot names carry the second, and the replacement is set up later.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    // The replacement starts with the same copy directory, as the install
    // guide has it, and its scheduler backs up its new, empty database.
    let (replacement, new_id, _) = installation(&tmp.path().join("replacement")).await;
    assert_ne!(new_id, lost_id);
    let before = listing(&share);
    let err = backup_copy::catch_up(&replacement, &share, 14)
        .await
        .expect_err("another installation's backups are in the copy directory");
    let message = format!("{err:#}");
    assert!(message.contains(&lost_id), "{message}");
    assert!(message.contains("restore the newest copy"), "{message}");
    assert_eq!(
        listing(&share),
        before,
        "the refusal wrote to the copy directory (a copy, a partial, or a SQLite sidecar)"
    );
    assert!(
        matches!(
            backup_copy::copy_status(&replacement, Some(&share)).expect("status"),
            CopyStatus::Behind { .. }
        ),
        "the replacement's own backups are not off the server"
    );

    // Restoring the lost installation's newest copy makes it this one, and
    // copying resumes from the restored database, never from the empty
    // database the restore set aside.
    let newest_copy = backup::snapshots_in(&share)
        .expect("copies")
        .pop()
        .expect("a copy");
    let restored = restore::run(&replacement, &newest_copy)
        .await
        .expect("restore");
    assert_eq!(restored.installation_id, lost_id);
    let set_aside = restored
        .pre_restore_snapshot
        .expect("the empty database set aside");
    let restored_backup = restored.backup.expect("the restored database backed up");
    assert_eq!(
        backup::snapshot_installation(&restored_backup)
            .await
            .expect("id"),
        lost_id
    );

    let copy = backup_copy::catch_up(&replacement, &share, 14)
        .await
        .expect("copying resumes")
        .expect("the restored backup was not yet copied");
    assert_eq!(name(&copy.copy), name(&restored_backup));
    assert!(
        !share.join(name(&set_aside)).exists(),
        "the database a restore set aside was copied off the server"
    );
    for copy in backup::snapshots_in(&share).expect("copies") {
        assert_eq!(
            backup::snapshot_installation(&copy).await.expect("id"),
            lost_id,
            "{} is another installation's",
            copy.display()
        );
    }
    assert!(
        matches!(
            backup_copy::copy_status(&replacement, Some(&share)).expect("status"),
            CopyStatus::Current { .. }
        ),
        "the copy directory holds the newest backup"
    );
    // doctor names the same newest backup, not the database set aside.
    let options = doctor::Options {
        backup_copy_dir: Some(&share),
        ..doctor::Options::default()
    };
    let findings = doctor::run(&replacement, &options).await;
    let backups = findings
        .iter()
        .find(|finding| finding.check == "backups")
        .expect("doctor reports backups");
    assert!(
        backups.detail.ends_with(&name(&restored_backup)),
        "{backups:?}"
    );
}

#[tokio::test]
async fn a_newer_pre_restore_snapshot_is_never_copied_or_counted() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let (data_dir, _, snapshot) = installation(&tmp.path().join("data")).await;
    let (_other, share) = share(tmp.path());
    backup_copy::copy_snapshot(&snapshot, &share, 14)
        .await
        .expect("copy");

    // Newer than every backup by its stamp, as the one a restore leaves is.
    let set_aside = data_dir
        .backups()
        .join("consolebook-prerestore-29991231T235959Z.db");
    std::fs::copy(&snapshot, &set_aside).expect("set aside");

    assert!(
        matches!(
            backup_copy::copy_status(&data_dir, Some(&share)).expect("status"),
            CopyStatus::Current { .. }
        ),
        "a pre-restore snapshot made the copy directory look behind"
    );
    assert!(
        backup_copy::catch_up(&data_dir, &share, 14)
            .await
            .expect("catch up")
            .is_none(),
        "nothing of this installation's is missing from the copy directory"
    );
    assert!(!share.join(name(&set_aside)).exists());
}

#[tokio::test]
async fn a_copy_directory_with_this_installations_backups_keeps_taking_them() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let (data_dir, _, first) = installation(&tmp.path().join("data")).await;
    let (_other, share) = share(tmp.path());
    backup_copy::copy_snapshot(&first, &share, 14)
        .await
        .expect("first copy");
    // Snapshot names carry the second; the next backup needs a new one.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let second = backup::run(&data_dir, 14).await.expect("backup").snapshot;
    backup_copy::copy_snapshot(&second, &share, 14)
        .await
        .expect("the same installation's next backup");
    assert_eq!(listing(&share), [name(&first), name(&second)]);
}

#[tokio::test]
async fn a_foreign_copy_with_the_same_name_is_never_pruned() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let (_lost, _, first) = installation(&tmp.path().join("lost")).await;
    let (_other, share) = share(tmp.path());
    backup_copy::copy_snapshot(&first, &share, 14)
        .await
        .expect("copy");
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let (replacement, _, own) = installation(&tmp.path().join("replacement")).await;
    // Names carry only the second: give the replacement's newest backup the
    // name of a copy the lost installation already has there.
    let collided = replacement.backups().join(name(&first));
    std::fs::rename(&own, &collided).expect("rename");
    std::fs::copy(&first, share.join("consolebook-20200101T000000Z.db")).expect("older copy");

    let before = listing(&share);
    let err = backup_copy::catch_up(&replacement, &share, 1)
        .await
        .expect_err("retention in another installation's copy directory");
    assert!(
        format!("{err:#}").contains("holds the backups of installation"),
        "{err:#}"
    );
    assert_eq!(
        listing(&share),
        before,
        "another installation's copy was pruned"
    );
}

#[tokio::test]
async fn a_copy_directory_holding_only_a_pre_restore_copy_is_still_owned() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let (_lost, lost_id, snapshot) = installation(&tmp.path().join("lost")).await;
    let (_other, share) = share(tmp.path());
    // An earlier release copied set-aside databases off the server too.
    std::fs::copy(
        &snapshot,
        share.join("consolebook-prerestore-20200101T000000Z.db"),
    )
    .expect("legacy copy");
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let (_replacement, _, own) = installation(&tmp.path().join("replacement")).await;

    let before = listing(&share);
    let err = backup_copy::copy_snapshot(&own, &share, 1)
        .await
        .expect_err("the directory holds another installation's copy");
    assert!(format!("{err:#}").contains(&lost_id), "{err:#}");
    assert_eq!(listing(&share), before);
}

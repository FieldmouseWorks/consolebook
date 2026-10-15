//! Backup copies off the server and backup health (ADR 0024). All fixtures
//! are invented.

use std::path::Path;

use axum::body::Body;
use axum::http::header::COOKIE;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use consolebook_server::backup;
use consolebook_server::backup_copy::{self, BackupSettings, CopyStatus, StatusRefusal};
use consolebook_server::capabilities::{self, TRAINER_BUNDLE};
use consolebook_server::data_dir::DataDir;
use consolebook_server::{http, secrets, setup, storage, users};

mod support;

struct Fixture {
    tmp: tempfile::TempDir,
    /// A directory on another filesystem, standing in for a mounted share.
    other_fs: tempfile::TempDir,
    data_dir: DataDir,
    pool: sqlx::SqlitePool,
    admin_id: i64,
}

impl Fixture {
    async fn new() -> Self {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let data_dir = DataDir::new(tmp.path().join("data"));
        data_dir.ensure_layout().expect("create layout");
        let pool = storage::open(&data_dir.database()).await.expect("open");
        let code = setup::issue_setup_code(&pool)
            .await
            .expect("issue")
            .expect("uninitialized")
            .0;
        let admin_id = setup::initialize(
            &pool,
            &code.raw,
            "Example County Communications",
            "avery.admin",
            "Avery Admin",
            support::password(),
        )
        .await
        .expect("initialize")
        .expect("accepted");
        let other_fs = other_filesystem(tmp.path());
        Self {
            tmp,
            other_fs,
            data_dir,
            pool,
            admin_id,
        }
    }

    fn copy_dir(&self) -> std::path::PathBuf {
        let dir = self.other_fs.path().join("offsite");
        std::fs::create_dir_all(&dir).expect("create copy dir");
        dir
    }

    fn settings(&self, copy_dir: Option<&Path>) -> BackupSettings {
        BackupSettings {
            data_dir: self.data_dir.clone(),
            interval_hours: 24,
            keep: 2,
            copy_dir: copy_dir.map(Path::to_path_buf),
        }
    }
}

/// Copies must land on another filesystem. `/dev/shm` is a separate tmpfs
/// on Linux hosts and CI runners; fail loudly rather than test nothing.
fn other_filesystem(beside: &Path) -> tempfile::TempDir {
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir_in("/dev/shm").expect("these tests need /dev/shm (a tmpfs)");
    let dev = |p: &Path| std::fs::metadata(p).expect("metadata").dev();
    assert_ne!(
        dev(dir.path()),
        dev(beside),
        "/dev/shm must be a different filesystem from the temp directory"
    );
    dir
}

#[tokio::test]
async fn a_validated_snapshot_is_copied_byte_for_byte_and_pruned_there() {
    let fx = Fixture::new().await;
    let copy_dir = fx.copy_dir();
    // Two older copies already there; retention of 2 keeps only the newest two.
    for old in [
        "consolebook-20200101T000000Z.db",
        "consolebook-20200102T000000Z.db",
    ] {
        std::fs::write(copy_dir.join(old), b"invented older copy").expect("seed");
    }
    let report = backup::run(&fx.data_dir, 2).await.expect("backup");

    let copy = backup_copy::copy_snapshot(&report.snapshot, &copy_dir, 2)
        .await
        .expect("copy");

    assert_eq!(copy.copy.file_name(), report.snapshot.file_name());
    assert_eq!(
        std::fs::read(&copy.copy).expect("copy bytes"),
        std::fs::read(&report.snapshot).expect("snapshot bytes")
    );
    assert_eq!(copy.sha256.len(), 64);
    let remaining: Vec<String> = backup::snapshots_in(&copy_dir)
        .expect("list")
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(remaining.len(), 2);
    assert!(!remaining.contains(&"consolebook-20200101T000000Z.db".to_owned()));
    assert!(
        std::fs::read_dir(&copy_dir).expect("list").all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".partial")),
        "no partial file is left behind"
    );
    // Copying the same snapshot again is refused rather than overwritten.
    assert!(
        backup_copy::copy_snapshot(&report.snapshot, &copy_dir, 2)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_copy_that_cannot_be_made_leaves_the_local_snapshot_intact() {
    let fx = Fixture::new().await;
    let report = backup::run(&fx.data_dir, 2).await.expect("backup");
    let before = std::fs::read(&report.snapshot).expect("snapshot bytes");

    let missing = fx.tmp.path().join("not-mounted");
    let err = backup_copy::copy_snapshot(&report.snapshot, &missing, 2)
        .await
        .expect_err("missing");
    assert!(format!("{err:#}").contains("not reachable"), "{err:#}");

    let not_a_dir = fx.tmp.path().join("a-file");
    std::fs::write(&not_a_dir, b"invented").expect("write");
    let err = backup_copy::copy_snapshot(&report.snapshot, &not_a_dir, 2)
        .await
        .expect_err("file");
    assert!(format!("{err:#}").contains("not a directory"), "{err:#}");

    assert_eq!(std::fs::read(&report.snapshot).expect("after"), before);
    backup::validate_snapshot(&report.snapshot)
        .await
        .expect("the local snapshot still validates");
}

#[tokio::test]
async fn a_copy_directory_on_the_local_filesystem_is_never_used_or_counted() {
    // An unmounted share leaves its empty mount point on the local disk.
    let fx = Fixture::new().await;
    let report = backup::run(&fx.data_dir, 2).await.expect("backup");
    let mount_point = fx.tmp.path().join("share-not-mounted");
    std::fs::create_dir_all(&mount_point).expect("mount point");

    let err = backup_copy::copy_snapshot(&report.snapshot, &mount_point, 2)
        .await
        .expect_err("local");
    assert!(format!("{err:#}").contains("same filesystem"), "{err:#}");
    assert!(
        backup_copy::catch_up(&fx.data_dir, &mount_point, 2)
            .await
            .is_err(),
        "the retry refuses it too"
    );
    assert_eq!(
        std::fs::read_dir(&mount_point).expect("list").count(),
        0,
        "nothing is written to the local stand-in"
    );

    let status = backup_copy::status(&fx.settings(Some(&mount_point))).expect("status");
    match status.copy {
        CopyStatus::Unreachable { problem, .. } => {
            assert!(problem.contains("not mounted"), "{problem}");
        }
        other => panic!("expected unreachable, got {other:?}"),
    }
}

#[tokio::test]
async fn catch_up_copies_only_what_the_copy_directory_lacks() {
    let fx = Fixture::new().await;
    let copy_dir = fx.copy_dir();
    assert!(
        backup_copy::catch_up(&fx.data_dir, &copy_dir, 2)
            .await
            .expect("nothing yet")
            .is_none(),
        "no local snapshot, nothing to copy"
    );
    backup::run(&fx.data_dir, 2).await.expect("backup");
    assert!(
        backup_copy::catch_up(&fx.data_dir, &copy_dir, 2)
            .await
            .expect("copy")
            .is_some()
    );
    assert!(
        backup_copy::catch_up(&fx.data_dir, &copy_dir, 2)
            .await
            .expect("current")
            .is_none(),
        "a current copy directory is not copied to again"
    );

    // A prune that failed after its copy is finished by the next catch-up.
    for old in [
        "consolebook-20200101T000000Z.db",
        "consolebook-20200102T000000Z.db",
        "consolebook-20200103T000000Z.db",
    ] {
        std::fs::write(copy_dir.join(old), b"invented older copy").expect("seed");
    }
    assert!(
        backup_copy::catch_up(&fx.data_dir, &copy_dir, 2)
            .await
            .expect("retention")
            .is_none()
    );
    let names: Vec<String> = backup::snapshots_in(&copy_dir)
        .expect("list")
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names.contains(&"consolebook-20200103T000000Z.db".to_owned()));
}

#[tokio::test]
async fn status_reports_unconfigured_behind_current_and_unreachable() {
    let fx = Fixture::new().await;
    backup::run(&fx.data_dir, 2).await.expect("backup");

    let status = backup_copy::status(&fx.settings(None)).expect("status");
    assert_eq!(status.copy, CopyStatus::NotConfigured);
    assert_eq!(status.local.count, 1);
    assert!(status.local.newest_at.is_some());

    let copy_dir = fx.copy_dir();
    let status = backup_copy::status(&fx.settings(Some(&copy_dir))).expect("status");
    assert!(
        matches!(status.copy, CopyStatus::Behind { count: 0, .. }),
        "{status:?}"
    );

    backup_copy::catch_up(&fx.data_dir, &copy_dir, 2)
        .await
        .expect("copy");
    let status = backup_copy::status(&fx.settings(Some(&copy_dir))).expect("status");
    assert!(
        matches!(status.copy, CopyStatus::Current { count: 1, .. }),
        "{status:?}"
    );

    let gone = fx.tmp.path().join("unmounted-share");
    let status = backup_copy::status(&fx.settings(Some(&gone))).expect("status");
    assert!(
        matches!(status.copy, CopyStatus::Unreachable { .. }),
        "{status:?}"
    );
}

#[tokio::test]
async fn only_administrators_read_backup_health() {
    let fx = Fixture::new().await;
    let hash = secrets::hash_password(support::password()).expect("hash");
    let mut conn = fx.pool.acquire().await.expect("conn");
    let trainer = users::create(&mut conn, "jordan.trainer", "Jordan Trainer", "", "", &hash)
        .await
        .expect("create");
    capabilities::grant_bundle(&mut conn, trainer, &TRAINER_BUNDLE, Some(fx.admin_id))
        .await
        .expect("grant");
    drop(conn);

    let settings = fx.settings(None);
    assert_eq!(
        backup_copy::status_for(&fx.pool, trainer, &settings)
            .await
            .expect("read"),
        Err(StatusRefusal::CapabilityRequired)
    );

    let app = http::router(http::AppState::new(fx.pool.clone()).with_backups(settings));
    let session = support::login(app.clone(), "avery.admin", support::password()).await;
    let cookie = format!("{}={session}", http::SESSION_COOKIE);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/backups/status")
                .header(COOKIE, cookie)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("status");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(body["copy"]["state"], "not_configured");
    assert_eq!(body["keep"], 2);
}

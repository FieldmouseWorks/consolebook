//! A validated backup before an upgrade migrates the database (#94).
//!
//! An "older installation" here is a database migrated by every migration
//! except this build's newest, which is what an earlier release leaves.

use std::borrow::Cow;
use std::path::Path;

use sqlx::SqlitePool;
use sqlx::migrate::Migrator;

use consolebook_server::backup;
use consolebook_server::backup_copy;
use consolebook_server::data_dir::DataDir;
use consolebook_server::storage::{self, MIGRATOR};

async fn connect(path: &Path) -> SqlitePool {
    SqlitePool::connect(&format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .expect("connect")
}

/// A database as the previous release left it: all but the newest migration.
async fn older_installation(data_dir: &DataDir) {
    installation_at(data_dir, MIGRATOR.migrations.len() - 1).await;
}

/// A database migrated by only the first `count` migrations.
async fn installation_at(data_dir: &DataDir, count: usize) {
    let all = &MIGRATOR.migrations;
    // The fields are semver-exempt in sqlx; a test that stops compiling on
    // an upgrade is the right signal to rebuild this fixture.
    let previous = Migrator {
        migrations: Cow::Owned(all[..count].to_vec()),
        ignore_missing: false,
        locking: true,
        no_tx: false,
    };
    let pool = connect(&data_dir.database()).await;
    previous.run(&pool).await.expect("older migrations");
    pool.close().await;
}

async fn applied(path: &Path) -> i64 {
    let pool = connect(path).await;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE success = 1")
        .fetch_one(&pool)
        .await
        .expect("count");
    pool.close().await;
    count
}

async fn backups_audited(path: &Path) -> i64 {
    let pool = connect(path).await;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_event WHERE kind = 'backup_completed'")
            .fetch_one(&pool)
            .await
            .expect("count");
    pool.close().await;
    count
}

fn newest_version() -> i64 {
    MIGRATOR
        .iter()
        .map(|m| m.version)
        .max()
        .expect("migrations")
}

#[tokio::test]
async fn an_upgrade_backs_up_the_old_schema_before_migrating() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let data_dir = DataDir::new(tmp.path().join("data"));
    data_dir.ensure_layout().expect("layout");
    older_installation(&data_dir).await;
    let older = applied(&data_dir.database()).await;

    let pool = storage::open_diagnostic(&data_dir.database())
        .await
        .expect("open");
    assert_eq!(
        storage::pending_migrations(&pool).await.expect("pending"),
        vec![newest_version()]
    );
    pool.close().await;

    let backup = backup::before_migrating(&data_dir, 14)
        .await
        .expect("backup")
        .expect("a migration is pending, so a snapshot is taken");
    assert_eq!(
        applied(backup.snapshot()).await,
        older,
        "the snapshot holds the schema from before the upgrade"
    );
    backup::validate_snapshot(backup.snapshot())
        .await
        .expect("the snapshot validates");
    assert_eq!(
        backups_audited(&data_dir.database()).await,
        0,
        "nothing is written to the old schema"
    );

    let pool = storage::open(&data_dir.database()).await.expect("migrate");
    backup.record(&pool).await.expect("audit");
    pool.close().await;
    assert_eq!(applied(&data_dir.database()).await, older + 1);
    assert_eq!(backups_audited(&data_dir.database()).await, 1);
    assert!(
        backup::before_migrating(&data_dir, 14)
            .await
            .expect("check")
            .is_none(),
        "nothing is pending once migrated"
    );
}

#[tokio::test]
async fn the_pre_migration_backup_is_the_one_copied_off_the_server() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let data_dir = DataDir::new(tmp.path().join("data"));
    data_dir.ensure_layout().expect("layout");
    older_installation(&data_dir).await;
    let backup = backup::before_migrating(&data_dir, 14)
        .await
        .expect("backup")
        .expect("a migration is pending");
    let snapshot = backup.snapshot().to_path_buf();
    let pool = storage::open(&data_dir.database()).await.expect("migrate");
    backup.record(&pool).await.expect("audit");
    pool.close().await;

    // The scheduler's first tick: the snapshot is fresh, so no new backup
    // is due, and catch-up copies the newest one, which is this snapshot.
    let share = tempfile::tempdir_in("/dev/shm").expect("these tests need /dev/shm (a tmpfs)");
    let copy = backup_copy::catch_up(&data_dir, share.path(), 14)
        .expect("copy")
        .expect("the snapshot was not yet copied");
    assert_eq!(copy.copy.file_name(), snapshot.file_name());
}

/// The oldest schema there is has no audit table at all, so a backup that
/// wrote its audit event before migrating would fail there.
#[tokio::test]
async fn an_installation_from_the_first_migration_upgrades() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let data_dir = DataDir::new(tmp.path().join("data"));
    data_dir.ensure_layout().expect("layout");
    installation_at(&data_dir, 1).await;

    let backup = backup::before_migrating(&data_dir, 14)
        .await
        .expect("a schema without an audit table is backed up")
        .expect("migrations are pending");
    assert_eq!(applied(backup.snapshot()).await, 1);
    let pool = storage::open(&data_dir.database()).await.expect("migrate");
    backup.record(&pool).await.expect("audit after migrating");
    pool.close().await;
    assert_eq!(backups_audited(&data_dir.database()).await, 1);
}

#[tokio::test]
async fn a_new_or_current_installation_takes_no_extra_backup() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let data_dir = DataDir::new(tmp.path().join("data"));
    data_dir.ensure_layout().expect("layout");
    assert!(
        backup::before_migrating(&data_dir, 14)
            .await
            .expect("no database")
            .is_none()
    );
    storage::open(&data_dir.database()).await.expect("create");
    assert!(
        backup::before_migrating(&data_dir, 14)
            .await
            .expect("current")
            .is_none()
    );
    assert_eq!(backup::snapshots(&data_dir).expect("list").len(), 0);
}

#[tokio::test]
async fn startup_does_not_migrate_when_the_backup_cannot_be_written() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let data_dir = DataDir::new(tmp.path().join("data"));
    std::fs::create_dir_all(data_dir.root()).expect("root");
    // backups/ is a file, so no snapshot can be written there.
    std::fs::write(data_dir.backups(), b"invented obstruction").expect("obstruct");
    older_installation(&data_dir).await;
    let older = applied(&data_dir.database()).await;

    let err = backup::before_migrating(&data_dir, 14)
        .await
        .expect_err("no snapshot, so no go");
    assert!(
        format!("{err:#}").contains("pre-migration backup"),
        "{err:#}"
    );
    assert_eq!(
        applied(&data_dir.database()).await,
        older,
        "the database is untouched"
    );
}

#[tokio::test]
async fn an_older_build_refuses_a_newer_database_plainly() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let data_dir = DataDir::new(tmp.path().join("data"));
    data_dir.ensure_layout().expect("layout");
    storage::open(&data_dir.database())
        .await
        .expect("create")
        .close()
        .await;
    // What a later release leaves: one more applied migration than this
    // build knows.
    let future = newest_version() + 1;
    let pool = connect(&data_dir.database()).await;
    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
         VALUES (?, 'invented future migration', 1, X'00', 0)",
    )
    .bind(future)
    .execute(&pool)
    .await
    .expect("invented future migration");
    pool.close().await;
    let before = applied(&data_dir.database()).await;

    assert!(
        backup::before_migrating(&data_dir, 14)
            .await
            .expect("check")
            .is_none(),
        "nothing this build knows is pending"
    );
    let err = storage::open(&data_dir.database())
        .await
        .expect_err("a newer database is refused");
    let message = format!("{err:#}");
    assert!(
        message.contains("upgraded by a newer Consolebook"),
        "{message}"
    );
    assert!(message.contains(&future.to_string()), "{message}");
    assert_eq!(applied(&data_dir.database()).await, before);
}

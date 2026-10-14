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
use consolebook_server::serve_lock::ServeLock;
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

    let backup = backup::before_migrating(&data_dir)
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
        backup::before_migrating(&data_dir)
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
    let backup = backup::before_migrating(&data_dir)
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

    let backup = backup::before_migrating(&data_dir)
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
        backup::before_migrating(&data_dir)
            .await
            .expect("no database")
            .is_none()
    );
    storage::open(&data_dir.database()).await.expect("create");
    assert!(
        backup::before_migrating(&data_dir)
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

    let err = backup::before_migrating(&data_dir)
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
        backup::before_migrating(&data_dir)
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

#[tokio::test]
async fn opening_for_use_backs_up_migrates_and_audits() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let data_dir = DataDir::new(tmp.path().join("data"));
    data_dir.ensure_layout().expect("layout");
    older_installation(&data_dir).await;
    let older = applied(&data_dir.database()).await;

    let pool = backup::open_migrated(&data_dir, None, None)
        .await
        .expect("open");
    pool.close().await;
    assert_eq!(backup::snapshots(&data_dir).expect("list").len(), 1);
    assert_eq!(applied(&data_dir.database()).await, older + 1);
    assert_eq!(backups_audited(&data_dir.database()).await, 1);

    let pool = backup::open_migrated(&data_dir, None, None)
        .await
        .expect("reopen");
    pool.close().await;
    assert_eq!(
        backup::snapshots(&data_dir).expect("list").len(),
        1,
        "nothing pending, nothing more taken"
    );
}

/// Every command that opens the database for use (`serve`, `setup-code`,
/// `recover`) migrates through `open_migrated`, so none can upgrade an
/// installation without its pre-migration backup. `restore` migrates the
/// copy it restores, whose source snapshot stays in `backups/`.
#[test]
fn the_executable_migrates_only_through_open_migrated() {
    let main = include_str!("../src/main.rs");
    assert!(
        !main.contains("storage::open("),
        "src/main.rs opens the database with storage::open; use backup::open_migrated"
    );
    assert!(main.contains("backup::open_migrated("));
}

fn names(data_dir: &DataDir) -> Vec<String> {
    backup::snapshots(data_dir)
        .expect("list")
        .iter()
        .map(|path| {
            path.file_name()
                .expect("name")
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

/// A migration that fails on every start, under a service manager that
/// keeps restarting it, must not fill the disk with snapshots.
#[tokio::test]
async fn repeated_attempts_keep_a_bounded_number_of_pre_migration_snapshots() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let data_dir = DataDir::new(tmp.path().join("data"));
    data_dir.ensure_layout().expect("layout");
    older_installation(&data_dir).await;
    // Four earlier attempts at the same upgrade, and an ordinary backup.
    for n in 1..=4 {
        std::fs::write(
            data_dir
                .backups()
                .join(format!("consolebook-premigration-20260101T00000{n}Z.db")),
            b"invented earlier attempt",
        )
        .expect("earlier attempt");
    }
    let ordinary = "consolebook-20260101T000000Z.db";
    std::fs::write(data_dir.backups().join(ordinary), b"invented backup").expect("ordinary");

    let backup = backup::before_migrating(&data_dir)
        .await
        .expect("backup")
        .expect("a migration is pending");
    let taken = backup
        .snapshot()
        .file_name()
        .expect("name")
        .to_string_lossy()
        .into_owned();
    assert!(taken.starts_with(backup::PRE_MIGRATION_PREFIX), "{taken}");
    let names = names(&data_dir);
    let pre_migration: Vec<&String> = names
        .iter()
        .filter(|name| name.starts_with(backup::PRE_MIGRATION_PREFIX))
        .collect();
    assert_eq!(pre_migration.len(), backup::PRE_MIGRATION_KEEP, "{names:?}");
    assert!(names.contains(&taken), "{names:?}");
    assert!(names.iter().any(|name| name == ordinary), "{names:?}");
    assert!(
        !names.iter().any(|name| name.ends_with("T000001Z.db")),
        "the oldest attempt goes first: {names:?}"
    );
}

#[tokio::test]
async fn a_running_server_blocks_a_migrating_command_but_not_an_ordinary_one() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let data_dir = DataDir::new(tmp.path().join("data"));
    data_dir.ensure_layout().expect("layout");
    older_installation(&data_dir).await;
    let older = applied(&data_dir.database()).await;

    let server = ServeLock::acquire(&data_dir).expect("a server holds the installation");
    let err = backup::open_migrated(&data_dir, None, None)
        .await
        .expect_err("migrating beside a running server is refused");
    assert!(
        format!("{err:#}").contains("stop the running service"),
        "{err:#}"
    );
    assert_eq!(applied(&data_dir.database()).await, older, "not migrated");
    assert!(names(&data_dir).is_empty(), "no snapshot taken");
    drop(server);

    backup::open_migrated(&data_dir, None, None)
        .await
        .expect("migrates once the server is gone")
        .close()
        .await;
    assert_eq!(applied(&data_dir.database()).await, older + 1);

    // Nothing pending: `recover` works beside a running server.
    let _server = ServeLock::acquire(&data_dir).expect("a server holds the installation");
    backup::open_migrated(&data_dir, None, None)
        .await
        .expect("an ordinary command needs no lock")
        .close()
        .await;
}

/// A pre-migration snapshot is ordered by its time like any other, so a
/// later ordinary backup is the newest: the one copied off the server and
/// the one copy status asks after.
#[tokio::test]
async fn a_later_backup_is_newer_than_the_pre_migration_snapshot() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let data_dir = DataDir::new(tmp.path().join("data"));
    data_dir.ensure_layout().expect("layout");
    std::fs::write(
        data_dir
            .backups()
            .join("consolebook-premigration-20260101T000000Z.db"),
        b"invented pre-migration snapshot",
    )
    .expect("pre-migration");
    let later = "consolebook-20260102T000000Z.db";
    std::fs::write(data_dir.backups().join(later), b"invented later backup").expect("later");

    assert_eq!(names(&data_dir).last().map(String::as_str), Some(later));
    let share = tempfile::tempdir_in("/dev/shm").expect("these tests need /dev/shm (a tmpfs)");
    let copy = backup_copy::catch_up(&data_dir, share.path(), 14)
        .expect("copy")
        .expect("the later backup was not yet copied");
    assert_eq!(
        copy.copy.file_name().expect("name").to_string_lossy(),
        later
    );
}

/// A failed upgrade most needs its recovery point off the server, and the
/// scheduler that would copy it never starts then: the copy happens before
/// migrating. An unreachable share does not block the upgrade.
#[tokio::test]
async fn the_pre_migration_backup_is_copied_off_the_server_before_migrating() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let data_dir = DataDir::new(tmp.path().join("data"));
    data_dir.ensure_layout().expect("layout");
    older_installation(&data_dir).await;
    let share = tempfile::tempdir_in("/dev/shm").expect("these tests need /dev/shm (a tmpfs)");

    backup::open_migrated(
        &data_dir,
        None,
        Some(backup::OffServerCopy {
            dir: share.path(),
            keep: 14,
        }),
    )
    .await
    .expect("open")
    .close()
    .await;
    let local = names(&data_dir);
    let copied: Vec<String> = backup::snapshots_in(share.path())
        .expect("list")
        .iter()
        .map(|path| {
            path.file_name()
                .expect("name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        copied, local,
        "the pre-migration snapshot is off the server"
    );

    let tmp = tempfile::tempdir().expect("temp dir");
    let data_dir = DataDir::new(tmp.path().join("data"));
    data_dir.ensure_layout().expect("layout");
    older_installation(&data_dir).await;
    let older = applied(&data_dir.database()).await;
    backup::open_migrated(
        &data_dir,
        None,
        Some(backup::OffServerCopy {
            dir: &share.path().join("not-mounted"),
            keep: 14,
        }),
    )
    .await
    .expect("an unreachable share does not block the upgrade")
    .close()
    .await;
    assert_eq!(applied(&data_dir.database()).await, older + 1);
}

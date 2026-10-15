//! `doctor` judges the newest backup's age against the interval the service
//! uses, not the default (#117).

use std::time::{Duration, SystemTime};

use consolebook_server::data_dir::DataDir;
use consolebook_server::doctor::{self, Finding, Verdict};
use consolebook_server::{backup, storage};

/// A migrated installation whose only backup is `age` old.
async fn installation_with_backup_aged(age: Duration) -> (tempfile::TempDir, DataDir) {
    let tmp = tempfile::tempdir().expect("temp dir");
    let data_dir = DataDir::new(tmp.path().join("data"));
    data_dir.ensure_layout().expect("layout");
    storage::open(&data_dir.database())
        .await
        .expect("open")
        .close()
        .await;
    let snapshot = backup::run(&data_dir, 14).await.expect("backup").snapshot;
    std::fs::File::options()
        .write(true)
        .open(&snapshot)
        .and_then(|file| file.set_modified(SystemTime::now() - age))
        .expect("age the backup");
    (tmp, data_dir)
}

async fn backups_finding(data_dir: &DataDir, interval_hours: u64) -> Finding {
    let options = doctor::Options {
        backup_interval_hours: interval_hours,
        ..doctor::Options::default()
    };
    doctor::run(data_dir, &options)
        .await
        .into_iter()
        .find(|finding| finding.check == "backups")
        .expect("doctor reports backups")
}

#[tokio::test]
async fn an_hourly_service_with_a_two_hour_old_backup_is_overdue() {
    let (_tmp, data_dir) = installation_with_backup_aged(Duration::from_hours(2)).await;
    let finding = backups_finding(&data_dir, 1).await;
    assert_eq!(finding.verdict, Verdict::Warn, "{finding:?}");
    assert!(finding.detail.contains("1-hour"), "{finding:?}");
    // The default interval would have called it fine.
    let default = backups_finding(&data_dir, 24).await;
    assert_eq!(default.verdict, Verdict::Ok, "{default:?}");
}

#[tokio::test]
async fn a_two_day_service_with_a_thirty_hour_old_backup_is_on_schedule() {
    let (_tmp, data_dir) = installation_with_backup_aged(Duration::from_hours(30)).await;
    let finding = backups_finding(&data_dir, 48).await;
    assert_eq!(finding.verdict, Verdict::Ok, "{finding:?}");
    let default = backups_finding(&data_dir, 24).await;
    assert_eq!(default.verdict, Verdict::Warn, "{default:?}");
}

//! `doctor` reports every applied migration history that `serve` refuses
//! (#138), and says the history is fine only when `serve` would accept it.
//!
//! Each refusing condition is shown twice on the same scratch database: a
//! `FAIL` from `doctor` and a refusal from the real `serve` command, so the
//! parity is observed rather than assumed. The intact installation is the
//! positive control for both.

use std::io::BufRead;
use std::process::{Command, ExitStatus, Output, Stdio};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use consolebook_server::data_dir::DataDir;
use consolebook_server::doctor::{self, Finding, Verdict};
use consolebook_server::storage::{self, MIGRATOR};

/// A migrated, stopped installation.
async fn installation() -> (tempfile::TempDir, DataDir) {
    let tmp = tempfile::tempdir().expect("scratch directory");
    let data = DataDir::new(tmp.path().join("data"));
    data.ensure_layout().expect("layout");
    storage::open(&data.database())
        .await
        .expect("initialize")
        .close()
        .await;
    (tmp, data)
}

fn newest_version() -> i64 {
    MIGRATOR
        .iter()
        .map(|migration| migration.version)
        .max()
        .expect("embedded migrations")
}

/// Rewrites the migration history the way a different build, a crash, or a
/// hand edit would leave it.
async fn alter_history(data: &DataDir, statement: &'static str, version: i64) {
    let pool = storage::open_existing(&data.database())
        .await
        .expect("open for the fixture");
    let changed = sqlx::query(statement)
        .bind(version)
        .execute(&pool)
        .await
        .expect("alter migration history")
        .rows_affected();
    pool.close().await;
    assert_eq!(changed, 1, "{statement}");
}

async fn migration_findings(data: &DataDir) -> Vec<Finding> {
    doctor::run(data, &doctor::Options::default())
        .await
        .into_iter()
        .filter(|finding| finding.check == "migrations")
        .collect()
}

fn command(data: &DataDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_consolebook-server"));
    // Only what the test passes reaches the process.
    command.env_clear().arg("--data-dir").arg(data.root());
    command
}

fn run_doctor(data: &DataDir) -> Output {
    command(data)
        .arg("doctor")
        .output()
        .expect("doctor process")
}

/// How the real `serve` command ended.
#[derive(Debug)]
enum Served {
    /// It logged that it is listening: it accepted the database.
    Listening,
    /// It exited first, with this status and log (`serve` logs to stdout).
    Exited(ExitStatus, String),
}

/// Runs `serve` until it either exits or starts listening, then stops it.
fn run_serve(data: &DataDir) -> Served {
    let mut child = command(data)
        .args(["serve", "--bind", "127.0.0.1:0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("serve process");
    let stdout = child.stdout.take().expect("serve stdout");
    let (lines, logged) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufRead::lines(std::io::BufReader::new(stdout)) {
            let Ok(line) = line else { break };
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut log = String::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match logged.recv_timeout(left) {
            Ok(line) if line.contains("listening") => {
                child.kill().expect("stop serve");
                child.wait().expect("reap serve");
                return Served::Listening;
            }
            Ok(line) => {
                log.push_str(&line);
                log.push('\n');
            }
            // Standard output closed: the process exited.
            Err(RecvTimeoutError::Disconnected) => {
                return Served::Exited(child.wait().expect("serve status"), log);
            }
            Err(RecvTimeoutError::Timeout) => {
                child.kill().expect("stop serve");
                panic!("serve neither refused nor started listening within a minute: {log}");
            }
        }
    }
}

fn assert_serve_refuses(data: &DataDir, reason: &str) {
    match run_serve(data) {
        Served::Exited(status, log) => {
            assert_eq!(status.code(), Some(1), "{log}");
            assert!(log.contains(reason), "{log}");
        }
        Served::Listening => panic!("serve must refuse this database, not start"),
    }
}

/// Whether `text` names migration `version` (and not, say, `version`0).
fn names_migration(text: &str, version: i64) -> bool {
    let needle = format!("migration {version}");
    text.match_indices(&needle).any(|(at, _)| {
        !text[at + needle.len()..]
            .chars()
            .next()
            .is_some_and(|next| next.is_ascii_digit())
    })
}

fn assert_doctor_exits_failing(data: &DataDir, version: i64) {
    let output = run_doctor(data);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.lines().any(|line| line.starts_with("FAIL")
            && line.contains("migrations")
            && names_migration(line, version)),
        "{stdout}"
    );
}

/// The one migrations finding is a failure that names `version`.
fn assert_fails_naming(findings: &[Finding], version: i64, meaning: &str) -> String {
    assert_eq!(findings.len(), 1, "{findings:?}");
    let finding = &findings[0];
    assert_eq!(finding.verdict, Verdict::Fail, "{finding:?}");
    assert!(names_migration(&finding.detail, version), "{finding:?}");
    assert!(finding.detail.contains(meaning), "{finding:?}");
    finding.detail.clone()
}

#[tokio::test]
async fn an_intact_history_is_ok_and_the_service_starts() {
    let (_tmp, data) = installation().await;

    let findings = migration_findings(&data).await;
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].verdict, Verdict::Ok, "{findings:?}");
    assert_eq!(
        findings[0].detail,
        format!("{} applied, up to date", MIGRATOR.iter().count())
    );
    let output = run_doctor(&data);
    assert!(output.status.success(), "{output:?}");

    let served = run_serve(&data);
    assert!(
        matches!(served, Served::Listening),
        "serve must accept an intact history: {served:?}"
    );
}

#[tokio::test]
async fn a_changed_checksum_fails_doctor_and_the_service_refuses_it() {
    let (_tmp, data) = installation().await;
    let version = newest_version();
    alter_history(
        &data,
        "UPDATE _sqlx_migrations SET checksum = zeroblob(length(checksum)) WHERE version = ?1",
        version,
    )
    .await;

    let detail = assert_fails_naming(
        &migration_findings(&data).await,
        version,
        "a different build",
    );
    assert!(detail.contains("restore"), "{detail}");
    assert_doctor_exits_failing(&data, version);

    assert_serve_refuses(
        &data,
        &format!("migration {version} was previously applied but has been modified"),
    );
}

#[tokio::test]
async fn a_failed_migration_fails_doctor_and_the_service_refuses_it() {
    let (_tmp, data) = installation().await;
    let version = newest_version();
    alter_history(
        &data,
        "UPDATE _sqlx_migrations SET success = 0 WHERE version = ?1",
        version,
    )
    .await;

    // Reported once, as failed, and not also as pending.
    assert_fails_naming(&migration_findings(&data).await, version, "did not finish");
    assert_doctor_exits_failing(&data, version);

    assert_serve_refuses(&data, &format!("migration {version} is partially applied"));
}

#[tokio::test]
async fn a_migration_this_build_does_not_embed_fails_doctor_and_the_service_refuses_it() {
    let (_tmp, data) = installation().await;
    let future = newest_version() + 1;
    alter_history(
        &data,
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
         VALUES (?1, 'invented future migration', 1, X'00', 0)",
        future,
    )
    .await;

    // The same words startup uses for this condition.
    assert_fails_naming(
        &migration_findings(&data).await,
        future,
        "upgraded by a newer Consolebook",
    );
    assert_doctor_exits_failing(&data, future);

    assert_serve_refuses(&data, "upgraded by a newer Consolebook");
}

#[tokio::test]
async fn every_refusing_condition_is_reported_not_only_the_first() {
    let (_tmp, data) = installation().await;
    let first = MIGRATOR.iter().next().expect("migrations").version;
    let newest = newest_version();
    alter_history(
        &data,
        "UPDATE _sqlx_migrations SET checksum = zeroblob(length(checksum)) WHERE version = ?1",
        first,
    )
    .await;
    alter_history(
        &data,
        "UPDATE _sqlx_migrations SET success = 0 WHERE version = ?1",
        newest,
    )
    .await;

    let findings = migration_findings(&data).await;
    assert_eq!(findings.len(), 2, "{findings:?}");
    assert!(findings.iter().all(|f| f.verdict == Verdict::Fail));
    assert!(names_migration(&findings[0].detail, first), "{findings:?}");
    assert!(names_migration(&findings[1].detail, newest), "{findings:?}");
}

#[tokio::test]
async fn a_pending_migration_is_still_reported_as_pending() {
    let (_tmp, data) = installation().await;
    let version = newest_version();
    alter_history(
        &data,
        "DELETE FROM _sqlx_migrations WHERE version = ?1",
        version,
    )
    .await;

    let findings = migration_findings(&data).await;
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].detail, format!("pending: {version}"));
}

/// `storage::migration_history` mirrors what `SQLx`'s migrator refuses under
/// this configuration. If it changes (missing versions ignored, or another
/// table), the mirror and these tests must change with it.
#[test]
fn the_migrator_configuration_the_diagnosis_assumes() {
    assert!(!MIGRATOR.ignore_missing);
    assert_eq!(MIGRATOR.table_name, "_sqlx_migrations");
    assert!(
        MIGRATOR
            .iter()
            .all(|migration| migration.migration_type.is_up_migration())
    );
}

//! State an earlier build persisted still opens after a dependency
//! upgrade (#136).
//!
//! Two kinds of stored state outlive the build that wrote them: the
//! `_sqlx_migrations` rows in every deployed database, which startup
//! checks against the embedded migrations, and the Argon2 PHC strings in
//! `user.password_hash`. Both fixtures here were written by the build
//! before the upgrade, never by this one.

use consolebook_server::secrets;
use consolebook_server::storage::{self, MIGRATOR};

mod support;

/// The rows the released v0.2.0 binary wrote; see the file's header.
const RELEASED_MIGRATIONS: &str = include_str!("fixtures/migrations-v0.2.0.txt");

/// An invented password's hash, produced by `secrets::hash_password` at
/// 48ccb8e (argon2 0.5.3) on 2026-10-15.
const PHC_FROM_ARGON2_0_5: &str = "$argon2id$v=19$m=19456,t=2,p=1$PduNS0DMJqQGjmBncj2+yQ$2pk+V9LpC3xOGaqmdDoOXKvKaqdXW9d89a6nOArr7hw";
const PHC_PASSWORD: &str = "invented-compat-passphrase-136";

#[derive(Debug, PartialEq, Eq)]
struct MigrationRow {
    version: i64,
    checksum: Vec<u8>,
    description: String,
}

fn released_rows() -> Vec<MigrationRow> {
    RELEASED_MIGRATIONS
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(|line| {
            let mut fields = line.splitn(3, ' ');
            let version = fields.next().expect("version").parse().expect("number");
            let hex = fields.next().expect("checksum");
            let checksum = (0..hex.len())
                .step_by(2)
                .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).expect("hex"))
                .collect();
            let description = fields.next().expect("description").to_owned();
            MigrationRow {
                version,
                checksum,
                description,
            }
        })
        .collect()
}

async fn stored_rows(db: &std::path::Path) -> Vec<MigrationRow> {
    let pool = storage::open_diagnostic(db).await.expect("open read-only");
    let rows: Vec<(i64, Vec<u8>, String, bool)> = sqlx::query_as(
        "SELECT version, checksum, description, success FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(&pool)
    .await
    .expect("migration rows");
    pool.close().await;
    rows.into_iter()
        .map(|(version, checksum, description, success)| {
            assert!(success, "migration {version} is recorded as unsuccessful");
            MigrationRow {
                version,
                checksum,
                description,
            }
        })
        .collect()
}

/// Every released migration is still embedded, under the same version,
/// description, and checksum. A checksum change here would make every
/// deployed database refuse to start.
#[test]
fn embedded_migrations_keep_the_released_checksums() {
    let released = released_rows();
    assert_eq!(released.len(), 16, "the fixture lists v0.2.0's migrations");
    for row in &released {
        let embedded = MIGRATOR
            .iter()
            .find(|migration| migration.version == row.version)
            .unwrap_or_else(|| panic!("migration {} is no longer embedded", row.version));
        assert_eq!(
            MigrationRow {
                version: embedded.version,
                checksum: embedded.checksum.to_vec(),
                description: embedded.description.to_string(),
            },
            *row
        );
    }
}

/// A database this build creates records exactly the released rows, and
/// a database holding them opens. The control alters one stored checksum
/// and startup must refuse it, so the comparison is shown to be live.
#[tokio::test]
async fn a_database_holding_the_released_rows_opens_and_a_modified_one_does_not() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("consolebook.db");
    storage::open(&db).await.expect("create").close().await;

    let released = released_rows();
    let stored = stored_rows(&db).await;
    assert_eq!(stored[..released.len()], released[..]);

    storage::open(&db).await.expect("reopen").close().await;

    let pool = storage::open_existing(&db)
        .await
        .expect("open for the control");
    let changed = sqlx::query(
        "UPDATE _sqlx_migrations SET checksum = zeroblob(length(checksum)) WHERE version = 16",
    )
    .execute(&pool)
    .await
    .expect("alter one checksum")
    .rows_affected();
    pool.close().await;
    assert_eq!(changed, 1);
    let err = storage::open(&db)
        .await
        .expect_err("a modified migration row must refuse to start");
    assert!(
        format!("{err:#}").contains("migration 16 was previously applied but has been modified"),
        "{err:#}"
    );
}

/// A password hashed before the upgrade still signs in; a wrong one
/// still fails, even when only its final character differs.
#[test]
fn a_password_hashed_by_the_earlier_build_still_verifies() {
    assert!(secrets::verify_password(PHC_PASSWORD, PHC_FROM_ARGON2_0_5));
    let (prefix, last) = PHC_PASSWORD.split_at(PHC_PASSWORD.len() - 1);
    let near_miss = format!("{prefix}{}", if last == "7" { '8' } else { '7' });
    assert!(!secrets::verify_password(&near_miss, PHC_FROM_ARGON2_0_5));
    assert!(!secrets::verify_password(
        support::wrong_password(),
        PHC_FROM_ARGON2_0_5
    ));
}

/// New hashes keep the parameters the earlier build recorded, so a
/// rollback to it can still verify them.
#[test]
fn new_hashes_keep_the_recorded_parameters() {
    let fresh = secrets::hash_password(PHC_PASSWORD).expect("hash");
    assert!(
        fresh.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
        "{fresh}"
    );
    assert_eq!(fresh.len(), PHC_FROM_ARGON2_0_5.len(), "{fresh}");
    assert_ne!(fresh, PHC_FROM_ARGON2_0_5, "the salt is fresh");
    assert!(secrets::verify_password(PHC_PASSWORD, &fresh));
}

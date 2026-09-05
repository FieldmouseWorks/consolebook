//! Contended writes must validate after reserving SQLite's writer. All data
//! is invented. Child modules own program, training, and account scenarios.

use std::{fmt::Debug, future::Future, time::Duration};

use consolebook_server::{capabilities, programs, storage, users};
use sqlx::{ConnectOptions, Connection, SqlitePool, sqlite::SqliteConnectOptions};

#[path = "write_transactions/accounts.rs"]
mod account_writes;
#[path = "write_transactions/programs.rs"]
mod program_writes;
#[path = "write_transactions/training.rs"]
mod training_writes;

const ACTOR: i64 = 1;
const TRAINEE: i64 = 2;
const TRAINER: i64 = 3;
const OTHER_TRAINER: i64 = 4;
const PASSWORD: &str = "invented-passphrase-1";

struct Fixture {
    tmp: tempfile::TempDir,
    pool: SqlitePool,
}

impl Fixture {
    async fn empty() -> Self {
        let tmp = tempfile::tempdir().expect("scratch");
        let pool = storage::open(&tmp.path().join("consolebook.db"))
            .await
            .expect("open");
        Self { tmp, pool }
    }

    async fn new() -> Self {
        let fx = Self::empty().await;
        let mut tx = storage::write_tx(&fx.pool).await.expect("seed transaction");
        for (name, bundle) in [
            ("avery.admin", capabilities::ADMINISTRATOR_BUNDLE.as_slice()),
            ("taylor.trainee", capabilities::TRAINEE_BUNDLE.as_slice()),
            ("jordan.trainer", capabilities::TRAINER_BUNDLE.as_slice()),
            ("rowan.trainer", capabilities::TRAINER_BUNDLE.as_slice()),
        ] {
            let id = users::create(&mut tx, name, name, "", "", "unused-invented-hash")
                .await
                .expect("seed user");
            capabilities::grant_bundle(&mut tx, id, bundle, None)
                .await
                .expect("grants");
        }
        tx.commit().await.expect("seed commit");
        fx
    }

    async fn draft(&self) -> (i64, i64) {
        let program = programs::create_program(&self.pool, ACTOR, "Invented County Program")
            .await
            .expect("call")
            .expect("program");
        let version = programs::create_version(&self.pool, ACTOR, program, &content())
            .await
            .expect("call")
            .expect("version");
        (program, version)
    }

    async fn published(&self) -> (i64, i64) {
        let (program, version) = self.draft().await;
        programs::publish_version(&self.pool, ACTOR, version)
            .await
            .expect("call")
            .expect("publish");
        (program, version)
    }

    async fn probe(&self) {
        let mut conn = SqliteConnectOptions::new()
            .filename(self.tmp.path().join("consolebook.db"))
            .busy_timeout(Duration::ZERO)
            .connect()
            .await
            .expect("probe connection");
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut conn)
            .await
            .expect("refusal released write lock");
        sqlx::query("ROLLBACK")
            .execute(&mut conn)
            .await
            .expect("probe rollback");
        conn.close().await.expect("probe close");
    }
}

fn content() -> programs::VersionContent {
    serde_json::from_value(serde_json::json!({
        "name": "Invented County Program", "label": "rev A", "description": "Invented fixture",
        "phases": [{"name": "Phase One", "description": "", "presentation_number": 1}],
        "phase_transitions": [], "competencies": [], "rating_scales": [],
        "rating_modifiers": [], "evaluation_forms": [{
            "record_type": "daily_report", "name": "Invented Daily", "instructions": "",
            "competencies": [], "narratives": [{"prompt": "Invented observations", "required": false}]
        }], "citations": []
    }))
    .expect("content")
}

/// Force contention before allowing either service to commit. Both futures
/// are polled while a separate connection owns the write reservation. Deferred
/// check-then-write paths fail on promotion during this interval; immediate
/// writers wait, then validate serially. No production test hooks are needed.
fn contend<A, B>(
    pool: &SqlitePool,
    left: A,
    right: B,
) -> impl Future<Output = (A::Output, B::Output)>
where
    A: Future,
    B: Future,
{
    let (left, right) = (Box::pin(left), Box::pin(right));
    async move {
        let blocker = storage::write_tx(pool).await.expect("hold writer");
        let pair = async { tokio::join!(left, right) };
        tokio::pin!(pair);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut pair)
                .await
                .is_err(),
            "competing writers must wait for the reservation, not fail on read promotion"
        );
        blocker.rollback().await.expect("release writer");
        tokio::time::timeout(Duration::from_secs(10), pair)
            .await
            .expect("bounded completion")
    }
}

type OutcomePair<T, E> = (anyhow::Result<Result<T, E>>, anyhow::Result<Result<T, E>>);

fn one_winner<T: Debug, E: Debug + PartialEq>(pair: OutcomePair<T, E>, refusal: &E) -> T {
    let left = pair
        .0
        .expect("left returns a domain outcome, never an internal error");
    let right = pair
        .1
        .expect("right returns a domain outcome, never an internal error");
    match (left, right) {
        (Ok(value), Err(error)) | (Err(error), Ok(value)) => {
            assert_eq!(&error, refusal);
            value
        }
        outcomes => panic!("expected one success and one refusal: {outcomes:?}"),
    }
}

#[tokio::test]
async fn wal_deferred_snapshot_reproduction() {
    let fx = Fixture::new().await;
    let mut stale = fx.pool.begin().await.expect("deferred reader");
    let _: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM program")
        .fetch_one(&mut *stale)
        .await
        .expect("snapshot");
    programs::create_program(&fx.pool, ACTOR, "Concurrent invented program")
        .await
        .expect("call")
        .expect("winner");
    let err =
        sqlx::query("INSERT INTO program (name, created_at) VALUES ('Stale invented program', 1)")
            .execute(&mut *stale)
            .await
            .expect_err("stale snapshot cannot promote");
    assert_eq!(
        err.as_database_error()
            .expect("SQLite error")
            .code()
            .as_deref(),
        Some("517")
    );
    stale.rollback().await.expect("rollback stale reader");
}

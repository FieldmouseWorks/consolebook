//! `SQLite` storage with explicit, verified connection invariants.
//!
//! Writable connections use explicit options that enable foreign-key enforcement,
//! WAL journaling, an intentional synchronous mode, a bounded busy timeout, and
//! application-owned migrations. Startup fails closed if any invariant does
//! not hold; `doctor` uses read-only options and reports observed mismatches.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use sqlx::{Executor, Row, Sqlite};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

/// Busy timeout applied to every connection.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Begins an immediate write transaction before transactional validation.
/// All application-owned write transactions use this reservation (ADR 0019).
/// A concurrent writer waits up to the connection's busy timeout, then checks
/// committed state; exceeding that timeout remains an operational error.
pub async fn write_tx(pool: &SqlitePool) -> sqlx::Result<sqlx::Transaction<'static, sqlx::Sqlite>> {
    pool.begin_with("BEGIN IMMEDIATE").await
}

/// Ends a write transaction on a typed refusal with rollback awaited, so
/// the write lock is released before the refusal returns. Dropping a `SQLx`
/// transaction only queues rollback on its connection's worker (ADR 0019).
pub async fn refuse<T, E>(
    tx: sqlx::Transaction<'static, sqlx::Sqlite>,
    refusal: E,
) -> Result<std::result::Result<T, E>> {
    tx.rollback().await.context("rolling back refused write")?;
    Ok(Err(refusal))
}

/// Embedded, application-owned migrations.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// One verified connection invariant, for `doctor` output and startup checks.
#[derive(Debug, Clone)]
pub struct InvariantCheck {
    pub name: &'static str,
    pub expected: String,
    pub actual: String,
}

impl InvariantCheck {
    #[must_use]
    pub fn holds(&self) -> bool {
        self.expected.eq_ignore_ascii_case(&self.actual)
    }
}

/// Options for writable startup and backup connections.
///
/// `create_if_missing` is only enabled by [`open`]. Diagnostics use
/// [`open_diagnostic`] and must never inherit the journal-mode setter.
fn connect_options(db_path: &Path, create: bool) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(create)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(BUSY_TIMEOUT)
}

async fn connect(db_path: &Path, create: bool) -> Result<SqlitePool> {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(connect_options(db_path, create))
        .await
        .with_context(|| format!("opening database {}", db_path.display()))?;
    Ok(pool)
}

/// Opens (creating if necessary) the database, runs migrations, verifies the
/// connection invariants, and ensures the instance identity row exists.
pub async fn open(db_path: &Path) -> Result<SqlitePool> {
    let pool = connect(db_path, true).await?;

    // Refusals `SQLx` would not word plainly, or would reach only after
    // trying to migrate. It words the others itself.
    let history = migration_history(&pool).await?;
    if let Some(refusal) = history.refusals.iter().find(|refusal| {
        matches!(
            refusal,
            RefusedHistory::Newer(_) | RefusedHistory::MissingHistory
        )
    }) {
        bail!("{refusal}");
    }

    MIGRATOR
        .run(&pool)
        .await
        .context("running database migrations")?;

    let failed: Vec<InvariantCheck> = verify_invariants(&pool)
        .await?
        .into_iter()
        .filter(|check| !check.holds())
        .collect();
    if !failed.is_empty() {
        let summary: Vec<String> = failed
            .iter()
            .map(|c| format!("{} (expected {}, got {})", c.name, c.expected, c.actual))
            .collect();
        bail!("database invariants violated: {}", summary.join(", "));
    }

    ensure_instance_identity(&pool).await?;
    Ok(pool)
}

/// Migrations this executable would apply to an existing database, in order.
///
/// Empty for a database that is up to date, and for one that has never been
/// migrated (a new installation), which has nothing to lose. Reads only.
pub async fn pending_migrations(pool: &SqlitePool) -> Result<Vec<i64>> {
    let Some(recorded) = recorded_history(pool).await? else {
        return Ok(Vec::new());
    };
    let applied: Vec<i64> = recorded
        .iter()
        .filter(|row| row.success)
        .map(|row| row.version)
        .collect();
    if applied.is_empty() {
        return Ok(Vec::new());
    }
    Ok(MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .map(|migration| migration.version)
        .filter(|version| !applied.contains(version))
        .collect())
}

/// A migration history this executable refuses to start on. [`open`]
/// refuses each of these, through its own check or `SQLx`'s migrator
/// (`sqlx-core` 0.9 `Migrator::run_direct`): a version it does not embed
/// (`ignore_missing` is off), a row recorded as unsuccessful, a stored
/// checksum that differs from the embedded migration's, or no recorded
/// history in a database that already holds a schema (migration 1 cannot
/// run over existing tables).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusedHistory {
    /// No migration is recorded (the table is missing or empty), yet the
    /// database already holds schema objects of its own.
    MissingHistory,
    /// A newer release applied a migration this build does not embed.
    Newer(i64),
    /// The row is recorded as not successful. `SQLx` 0.9 records only
    /// successes, so this comes from a hand edit or another tool.
    Unfinished(i64),
    /// The stored checksum differs from this build's migration.
    Changed(i64),
}

impl std::fmt::Display for RefusedHistory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingHistory => write!(
                f,
                "this database's migration history is missing (no migration is recorded) while it already holds data, and the service refuses to start on it: migration 1 cannot run over the existing tables; restore the snapshot taken before the upgrade, or the newest backup of this installation (docs/install.md, \"Upgrading\")"
            ),
            Self::Newer(version) => write!(
                f,
                "this database was upgraded by a newer Consolebook (it has migration {version}, which this version does not know); install that release again, or restore the snapshot taken before the upgrade (docs/install.md, \"Upgrading\")"
            ),
            Self::Unfinished(version) => write!(
                f,
                "migration {version} is recorded as unsuccessful, and the service refuses to start on it; restore the snapshot taken before the upgrade (docs/install.md, \"Upgrading\")"
            ),
            Self::Changed(version) => write!(
                f,
                "migration {version} was applied by a different build's version of it (its recorded checksum differs from this build's), and the service refuses to start on it; restore the snapshot taken before the upgrade, or a backup taken while this version was running the service, or run the build that applied it (docs/install.md, \"Upgrading\")"
            ),
        }
    }
}

/// The recorded migration history, judged against this build. Reads only.
#[derive(Debug)]
pub struct MigrationHistory {
    /// Rows recorded in `_sqlx_migrations`, successful or not; zero when the
    /// table is missing.
    pub recorded: usize,
    /// Every recorded migration startup would refuse, in version order.
    pub refusals: Vec<RefusedHistory>,
    /// Embedded migrations with no recorded row, which startup applies.
    pub unapplied: Vec<i64>,
}

/// Judges the recorded history against this build. Startup ([`open`])
/// refuses on its refusals and `doctor` reports them. Reads only, so
/// `doctor` can use it (ADR 0016).
///
/// With no recorded migration, a database is pristine only when it holds no
/// schema objects besides `SQLite`'s own and the migrations table; anything
/// else is [`RefusedHistory::MissingHistory`].
pub async fn migration_history(pool: &SqlitePool) -> Result<MigrationHistory> {
    let recorded = recorded_history(pool).await?.unwrap_or_default();
    let refusals = if recorded.is_empty() {
        let objects: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE name NOT LIKE 'sqlite\\_%' ESCAPE '\\' AND name != '_sqlx_migrations'",
        )
        .fetch_one(pool)
        .await
        .context("looking for an existing schema")?;
        if objects == 0 {
            Vec::new()
        } else {
            vec![RefusedHistory::MissingHistory]
        }
    } else {
        refusals(&recorded)
    };
    let unapplied = MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .map(|migration| migration.version)
        .filter(|version| recorded.iter().all(|row| row.version != *version))
        .collect();
    Ok(MigrationHistory {
        recorded: recorded.len(),
        refusals,
        unapplied,
    })
}

/// One row of `_sqlx_migrations`.
struct RecordedMigration {
    version: i64,
    checksum: Vec<u8>,
    success: bool,
}

/// Every row startup would refuse, in version order. Each row gets the
/// refusal `SQLx` would give it (unsuccessful first, then a version this
/// build does not embed, then an up migration's checksum). Startup stops
/// at its first refusal, the lowest unsuccessful row before any unknown
/// version before any changed checksum; `doctor` reports them all.
fn refusals(recorded: &[RecordedMigration]) -> Vec<RefusedHistory> {
    recorded
        .iter()
        .filter_map(|row| {
            if !row.success {
                return Some(RefusedHistory::Unfinished(row.version));
            }
            if !MIGRATOR.version_exists(row.version) {
                return Some(RefusedHistory::Newer(row.version));
            }
            MIGRATOR
                .iter()
                .filter(|migration| migration.migration_type.is_up_migration())
                .find(|migration| migration.version == row.version)
                .filter(|migration| *migration.checksum != *row.checksum)
                .map(|_| RefusedHistory::Changed(row.version))
        })
        .collect()
}

/// Every recorded migration row, in version order; `None` when the database
/// has never been migrated.
async fn recorded_history(pool: &SqlitePool) -> Result<Option<Vec<RecordedMigration>>> {
    let tracked: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_optional(pool)
    .await
    .context("looking for the migrations table")?;
    if tracked.is_none() {
        return Ok(None);
    }
    let rows: Vec<(i64, Vec<u8>, bool)> =
        sqlx::query_as("SELECT version, checksum, success FROM _sqlx_migrations ORDER BY version")
            .fetch_all(pool)
            .await
            .context("reading applied migrations")?;
    Ok(Some(
        rows.into_iter()
            .map(|(version, checksum, success)| RecordedMigration {
                version,
                checksum,
                success,
            })
            .collect(),
    ))
}

/// Opens an existing database without creating one and without migrating.
/// Used by backup; applies writable connection settings, including WAL.
pub async fn open_existing(db_path: &Path) -> Result<SqlitePool> {
    if !db_path.exists() {
        bail!("database {} does not exist", db_path.display());
    }
    connect(db_path, false).await
}

/// Opens an existing database read-only, without migrations or journal changes.
///
/// Connection-local settings match startup, but `SQLx`'s unset journal-mode
/// default preserves the database's mode. SQLite may create WAL sidecars and
/// update shared-memory coordination state; see ADR 0016 for filesystem limits.
/// Never use `immutable`: diagnostics must see live WAL commits and take locks.
pub async fn open_diagnostic(db_path: &Path) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(false)
        .read_only(true)
        .foreign_keys(true)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(BUSY_TIMEOUT);
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .with_context(|| format!("opening database {} read-only", db_path.display()))
}

/// Opens a finished snapshot file read-only as `immutable`: `SQLite` takes no
/// locks and creates no journal or WAL sidecars, so reading a copy on a
/// network share leaves nothing behind there. Only for files nothing writes
/// to again; a live database needs [`open_diagnostic`].
pub async fn open_snapshot(snapshot: &Path) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(snapshot)
        .create_if_missing(false)
        .read_only(true)
        .immutable(true);
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .with_context(|| format!("opening snapshot {} read-only", snapshot.display()))
}

/// Reads all four PRAGMAs on one connection. Only WAL journal mode persists;
/// the other checks describe this connection, not a running server's settings.
pub async fn verify_invariants(pool: &SqlitePool) -> Result<Vec<InvariantCheck>> {
    let mut connection = pool.acquire().await?;
    let foreign_keys: i64 = sqlx::query("PRAGMA foreign_keys")
        .fetch_one(&mut *connection)
        .await?
        .get(0);
    let journal_mode: String = sqlx::query("PRAGMA journal_mode")
        .fetch_one(&mut *connection)
        .await?
        .get(0);
    // 1 = NORMAL. SQLite reports synchronous numerically.
    let synchronous: i64 = sqlx::query("PRAGMA synchronous")
        .fetch_one(&mut *connection)
        .await?
        .get(0);
    let busy_timeout_ms: i64 = sqlx::query("PRAGMA busy_timeout")
        .fetch_one(&mut *connection)
        .await?
        .get(0);

    Ok(vec![
        InvariantCheck {
            name: "foreign_keys",
            expected: "1".into(),
            actual: foreign_keys.to_string(),
        },
        InvariantCheck {
            name: "journal_mode",
            expected: "wal".into(),
            actual: journal_mode,
        },
        InvariantCheck {
            name: "synchronous",
            expected: "1".into(),
            actual: synchronous.to_string(),
        },
        InvariantCheck {
            name: "busy_timeout_ms",
            expected: i64::try_from(BUSY_TIMEOUT.as_millis())
                .expect("busy timeout fits in i64")
                .to_string(),
            actual: busy_timeout_ms.to_string(),
        },
    ])
}

/// Inserts the single instance identity row on first initialization.
async fn ensure_instance_identity(pool: &SqlitePool) -> Result<()> {
    let created_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .context("formatting instance creation instant")?;
    sqlx::query(
        "INSERT INTO instance (id, installation_id, created_at_utc)
         VALUES (1, ?1, ?2)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(created_at)
    .execute(pool)
    .await
    .context("initializing instance identity")?;
    Ok(())
}

/// The opaque, stable identifier of this installation.
pub async fn installation_id<'e>(executor: impl Executor<'e, Database = Sqlite>) -> Result<String> {
    let row = sqlx::query("SELECT installation_id FROM instance WHERE id = 1")
        .fetch_one(executor)
        .await
        .context("reading instance identity")?;
    Ok(row.get(0))
}

/// Runs `SQLite`'s own integrity check and returns its verdict lines.
pub async fn integrity_check(pool: &SqlitePool) -> Result<Vec<String>> {
    let rows = sqlx::query("PRAGMA integrity_check")
        .fetch_all(pool)
        .await
        .context("running integrity check")?;
    Ok(rows.into_iter().map(|row| row.get(0)).collect())
}

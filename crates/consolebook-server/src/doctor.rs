//! `consolebook doctor`: diagnose an installation without changing it.
//!
//! Doctor never creates or writes the database, migrates, or sets journal mode.
//! SQLite may create WAL sidecars or update their coordination state (ADR 0016).
//! It reports what it finds; the operator decides what to do.

use std::path::Path;

use anyhow::Result;
use sqlx::SqlitePool;

use crate::backup_copy::{self, CopyStatus};

use crate::data_dir::DataDir;
use crate::storage;

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug)]
pub struct Finding {
    pub check: String,
    pub verdict: Verdict,
    pub detail: String,
}

impl Finding {
    fn ok(check: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            check: check.into(),
            verdict: Verdict::Ok,
            detail: detail.into(),
        }
    }
    fn warn(check: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            check: check.into(),
            verdict: Verdict::Warn,
            detail: detail.into(),
        }
    }
    fn fail(check: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            check: check.into(),
            verdict: Verdict::Fail,
            detail: detail.into(),
        }
    }
}

/// How the service is configured, for the checks that depend on it.
/// `doctor` reads no configuration files; the caller passes what the
/// service uses.
#[derive(Debug, Clone, Copy)]
pub struct Options<'a> {
    /// The configured off-server copy directory, if any (ADR 0024).
    pub backup_copy_dir: Option<&'a Path>,
    /// Hours between the service's automatic backups.
    pub backup_interval_hours: u64,
}

impl Default for Options<'_> {
    fn default() -> Self {
        Self {
            backup_copy_dir: None,
            backup_interval_hours: crate::scheduler::DEFAULT_INTERVAL_HOURS,
        }
    }
}

/// Runs every diagnostic and returns the findings. The caller decides how to
/// render them and what exit code the process deserves.
pub async fn run(data_dir: &DataDir, options: &Options<'_>) -> Vec<Finding> {
    let backup_copy_dir = options.backup_copy_dir;
    let mut findings = Vec::new();

    check_layout(data_dir, &mut findings);

    let db_path = data_dir.database();
    if !db_path.exists() {
        findings.push(Finding::fail(
            "database",
            format!(
                "{} does not exist; run `consolebook serve` to initialize",
                db_path.display()
            ),
        ));
        return findings;
    }

    match storage::open_diagnostic(&db_path).await {
        Err(err) => findings.push(Finding::fail(
            "database",
            format!(
                "cannot open read-only: {err:#}; WAL requires readable, usable -wal/-shm sidecars or permission to create them; doctor does not repair or bypass locking"
            ),
        )),
        Ok(pool) => {
            findings.push(Finding::ok("database", format!("{}", db_path.display())));
            check_invariants(&pool, &mut findings).await;
            check_migrations(&pool, &mut findings).await;
            check_integrity(&pool, &mut findings).await;
            check_identity(&pool, &mut findings).await;
            check_initialization(&pool, &mut findings).await;
            pool.close().await;
        }
    }

    check_backups(data_dir, options.backup_interval_hours, &mut findings);
    check_backup_copy(data_dir, backup_copy_dir, &mut findings);
    findings
}

#[must_use]
pub fn has_failure(findings: &[Finding]) -> bool {
    findings.iter().any(|f| f.verdict == Verdict::Fail)
}

fn check_layout(data_dir: &DataDir, findings: &mut Vec<Finding>) {
    for (name, path) in [
        ("data directory", data_dir.root().to_path_buf()),
        ("backups directory", data_dir.backups()),
        ("exports directory", data_dir.exports()),
        ("instance directory", data_dir.instance()),
    ] {
        if path.is_dir() {
            findings.push(Finding::ok(name, format!("{}", path.display())));
        } else {
            findings.push(Finding::fail(name, format!("missing: {}", path.display())));
        }
    }
}

async fn check_invariants(pool: &SqlitePool, findings: &mut Vec<Finding>) {
    match storage::verify_invariants(pool).await {
        Err(err) => findings.push(Finding::fail(
            "connection invariants",
            format!("could not verify: {err:#}"),
        )),
        Ok(checks) => {
            for check in checks {
                let name = format!("pragma {}", check.name);
                let scope = if check.name == "journal_mode" {
                    "observed database mode; WAL is persisted"
                } else {
                    "diagnostic connection only; does not inspect server connections"
                };
                if check.holds() {
                    findings.push(Finding::ok(name, format!("{} ({scope})", check.actual)));
                } else {
                    findings.push(Finding::fail(
                        name,
                        format!(
                            "expected {}, got {} ({scope})",
                            check.expected, check.actual
                        ),
                    ));
                }
            }
        }
    }
}

/// Reports each recorded migration `serve` would refuse to start on, using
/// the same judgment as startup's own check ([`storage::migration_history`]).
async fn check_migrations(pool: &SqlitePool, findings: &mut Vec<Finding>) {
    const CHECK: &str = "migrations";
    let history = match storage::migration_history(pool).await {
        Err(err) => {
            findings.push(Finding::fail(
                CHECK,
                format!("cannot read migration history: {err:#}"),
            ));
            return;
        }
        Ok(None) => {
            findings.push(Finding::fail(
                CHECK,
                "cannot read migration history: the database has no _sqlx_migrations table",
            ));
            return;
        }
        Ok(Some(history)) => history,
    };
    for refusal in &history.refusals {
        findings.push(Finding::fail(CHECK, refusal.to_string()));
    }
    if !history.unapplied.is_empty() {
        let pending: Vec<String> = history.unapplied.iter().map(ToString::to_string).collect();
        findings.push(Finding::fail(
            CHECK,
            format!("pending: {}", pending.join(", ")),
        ));
    }
    if history.refusals.is_empty() && history.unapplied.is_empty() {
        findings.push(Finding::ok(
            CHECK,
            format!("{} applied, up to date", history.recorded),
        ));
    }
}

async fn check_integrity(pool: &SqlitePool, findings: &mut Vec<Finding>) {
    match storage::integrity_check(pool).await {
        Err(err) => findings.push(Finding::fail("integrity", format!("{err:#}"))),
        Ok(verdict) if verdict == ["ok"] => findings.push(Finding::ok("integrity", "ok")),
        Ok(verdict) => findings.push(Finding::fail("integrity", verdict.join("; "))),
    }
}

async fn check_identity(pool: &SqlitePool, findings: &mut Vec<Finding>) {
    match storage::installation_id(pool).await {
        Err(err) => findings.push(Finding::fail("instance identity", format!("{err:#}"))),
        Ok(id) => findings.push(Finding::ok("instance identity", id)),
    }
}

async fn check_initialization(pool: &SqlitePool, findings: &mut Vec<Finding>) {
    match crate::setup::is_initialized(pool).await {
        Err(err) => findings.push(Finding::fail(
            "initialization",
            format!("cannot determine: {err:#}"),
        )),
        Ok(false) => findings.push(Finding::warn(
            "initialization",
            "not initialized; start the server and complete first-run setup",
        )),
        Ok(true) => {
            let admins: Result<i64, sqlx::Error> = sqlx::query_scalar(
                "SELECT COUNT(DISTINCT user_id) FROM capability_grant WHERE capability = ?1",
            )
            .bind(crate::capabilities::Capability::ManageUsers.as_str())
            .fetch_one(pool)
            .await;
            match admins {
                Err(err) => findings.push(Finding::fail(
                    "initialization",
                    format!("cannot count administrators: {err}"),
                )),
                Ok(0) => findings.push(Finding::fail(
                    "initialization",
                    "initialized but no user holds manage_users; restore from a backup taken before the grants were lost",
                )),
                Ok(n) => findings.push(Finding::ok(
                    "initialization",
                    format!("initialized, {n} administrator(s)"),
                )),
            }
        }
    }
}

fn check_backups(data_dir: &DataDir, interval_hours: u64, findings: &mut Vec<Finding>) {
    if !data_dir.backups().is_dir() {
        return; // Layout check already reported the missing directory.
    }
    let snapshots = match crate::backup::snapshots(data_dir) {
        Ok(snapshots) => snapshots,
        Err(err) => {
            findings.push(Finding::fail("backups", format!("{err:#}")));
            return;
        }
    };
    if snapshots.is_empty() {
        findings.push(Finding::warn(
            "backups",
            "no snapshots yet; the server takes one automatically, or run `consolebook backup`",
        ));
        return;
    }
    // Name the newest backup of this installation, not a database a restore
    // set aside, which the copy line does not count either (#115).
    let own = crate::backup::own_backups_in(&data_dir.backups()).unwrap_or_default();
    let Some(latest) = own.last() else {
        findings.push(Finding::warn(
            "backups",
            "no backup of this installation yet, only a database a restore set aside; \
             the server takes one automatically, or run `consolebook backup`",
        ));
        return;
    };
    let latest_name = latest
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let age = crate::backup::latest_snapshot_mtime(data_dir)
        .ok()
        .flatten()
        .map(|mtime| time::OffsetDateTime::now_utc().unix_timestamp() - mtime);
    let stale_after = i64::try_from(interval_hours.saturating_mul(3600)).unwrap_or(i64::MAX);
    match age {
        Some(age) if age > stale_after => findings.push(Finding::warn(
            "backups",
            format!(
                "{} snapshot(s), newest is {} hours old (older than the {interval_hours}-hour \
                 backup interval; if the service uses another, pass --backup-interval-hours)",
                snapshots.len(),
                age / 3600,
            ),
        )),
        _ => findings.push(Finding::ok(
            "backups",
            format!("{} snapshot(s), latest {latest_name}", snapshots.len()),
        )),
    }
}

/// The states match `/api/backups/status`. None is a failure: the
/// installation still works, but backups that never leave the server are
/// lost with it, so anything short of current is a warning.
fn check_backup_copy(data_dir: &DataDir, copy_dir: Option<&Path>, findings: &mut Vec<Finding>) {
    const CHECK: &str = "backup copy";
    if !data_dir.backups().is_dir() {
        return; // Layout check already reported the missing directory.
    }
    let status = match backup_copy::copy_status(data_dir, copy_dir) {
        Ok(status) => status,
        Err(err) => {
            findings.push(Finding::fail(CHECK, format!("{err:#}")));
            return;
        }
    };
    findings.push(match status {
        CopyStatus::NotConfigured => Finding::warn(
            CHECK,
            "not_configured: every backup shares a disk with the database; pass --backup-copy-dir as configured for the service (docs/install.md)",
        ),
        CopyStatus::Unreachable { directory, problem } => {
            Finding::warn(CHECK, format!("unreachable: {directory}: {problem}"))
        }
        CopyStatus::Behind {
            directory,
            count,
            newest,
        } => Finding::warn(
            CHECK,
            format!(
                "behind: {directory} has {count} cop{} (newest {}) but not the newest local snapshot; the server retries every 15 minutes",
                if count == 1 { "y" } else { "ies" },
                newest.as_deref().unwrap_or("none"),
            ),
        ),
        CopyStatus::Current {
            directory,
            count,
            newest,
        } => Finding::ok(
            CHECK,
            format!("current: {directory} has {count}, newest {newest}"),
        ),
    });
}

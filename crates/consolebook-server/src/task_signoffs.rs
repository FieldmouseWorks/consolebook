//! Task signoffs (docs/domain-model.md `TaskSignoff`; ADR 0013;
//! Milestone 4 slice 4).
//!
//! A signoff is a versioned record that a configured task was observed
//! or demonstrated: append-only rows per (enrollment, task) where the
//! latest row answers the current state. The first signoff takes
//! authoring scope; every later row is an override taking
//! `review_evaluation` and a recorded reason, and a revocation exists
//! only where there is something to revoke. Migration 0013 holds the
//! pinning, ordering, reason shape, and permanence raw.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection, SqlitePool};
use time::OffsetDateTime;

use crate::audit::{self, EventKind, Subject};
use crate::capabilities::{self, Capability};
use crate::{assignments, lifecycle, storage};

/// Typed refusals for recording a signoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignoffRefusal {
    NoSuchEnrollment,
    /// The task is not in the enrollment's pinned version's vocabulary.
    NoSuchTask,
    CapabilityRequired,
    /// An override explains itself (#32 decision 6; ADR 0013).
    ReasonRequired,
    /// A revocation supersedes a signoff.
    NothingToRevoke,
}

/// The closed signoff kind set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignoffKind {
    Observed,
    Demonstrated,
    Revoked,
}

impl SignoffKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Demonstrated => "demonstrated",
            Self::Revoked => "revoked",
        }
    }
}

/// Records one signoff row. The first for a task takes authoring scope
/// (a coordinator, or an assigned evaluation author); any later row is
/// an override taking `review_evaluation` and a non-blank reason.
pub async fn record(
    pool: &SqlitePool,
    actor_user_id: i64,
    enrollment_id: i64,
    task_id: i64,
    kind: SignoffKind,
    reason: &str,
) -> Result<std::result::Result<(), SignoffRefusal>> {
    let Some(version_id): Option<i64> =
        sqlx::query_scalar("SELECT program_version_id FROM enrollment WHERE id = ?1")
            .bind(enrollment_id)
            .fetch_optional(pool)
            .await
            .context("reading enrollment")?
    else {
        return Ok(Err(SignoffRefusal::NoSuchEnrollment));
    };
    let pinned: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM task WHERE id = ?1 AND program_version_id = ?2")
            .bind(task_id)
            .bind(version_id)
            .fetch_optional(pool)
            .await
            .context("checking task")?;
    if pinned.is_none() {
        return Ok(Err(SignoffRefusal::NoSuchTask));
    }
    let coordinator =
        capabilities::user_has(pool, actor_user_id, Capability::AssignTraining).await?;
    let author_in_scope = capabilities::user_has(pool, actor_user_id, Capability::AuthorEvaluation)
        .await?
        && assignments::is_assigned(pool, actor_user_id, enrollment_id).await?;
    let reviewer =
        capabilities::user_has(pool, actor_user_id, Capability::ReviewEvaluation).await?;
    let reason = reason.trim();

    let mut tx = storage::write_tx(pool).await.context("starting signoff")?;
    let prior: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM task_signoff WHERE enrollment_id = ?1 AND task_id = ?2 LIMIT 1",
    )
    .bind(enrollment_id)
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await
    .context("checking prior signoffs")?;
    if prior.is_some() {
        // An override supersedes recorded state: explicit authority and
        // a recorded reason (ADR 0013).
        if !reviewer {
            return storage::refuse(tx, SignoffRefusal::CapabilityRequired).await;
        }
        if reason.is_empty() {
            return storage::refuse(tx, SignoffRefusal::ReasonRequired).await;
        }
    } else {
        if !coordinator && !author_in_scope {
            return storage::refuse(tx, SignoffRefusal::CapabilityRequired).await;
        }
        if kind == SignoffKind::Revoked {
            return storage::refuse(tx, SignoffRefusal::NothingToRevoke).await;
        }
    }
    let signer_name: String = sqlx::query_scalar("SELECT display_name FROM user WHERE id = ?1")
        .bind(actor_user_id)
        .fetch_one(&mut *tx)
        .await
        .context("reading signer name")?;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    sqlx::query(
        "INSERT INTO task_signoff
             (enrollment_id, task_id, kind, reason, signed_by,
              signed_by_display_name, signed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(enrollment_id)
    .bind(task_id)
    .bind(kind.as_str())
    .bind(reason)
    .bind(actor_user_id)
    .bind(&signer_name)
    .bind(now)
    .execute(&mut *tx)
    .await
    .context("recording signoff")?;
    let trainee: i64 = sqlx::query_scalar("SELECT user_id FROM enrollment WHERE id = ?1")
        .bind(enrollment_id)
        .fetch_one(&mut *tx)
        .await
        .context("reading trainee")?;
    audit::record_for_subject(
        &mut *tx,
        EventKind::TaskSignoffRecorded,
        Some(actor_user_id),
        Some(trainee),
        Subject::Enrollment(enrollment_id),
    )
    .await?;
    tx.commit().await.context("committing signoff")?;
    Ok(Ok(()))
}

/// One task's row in the signoff matrix: the pinned task and its
/// current state, from the latest row's stored snapshot.
#[derive(Debug, Serialize)]
pub struct MatrixRow {
    pub task_id: i64,
    pub competency_category: String,
    pub competency_name: String,
    pub prompt: String,
    pub kind: Option<String>,
    pub reason: Option<String>,
    pub signed_by_display_name: Option<String>,
    pub signed_at: Option<i64>,
    pub history: i64,
}

/// Every task of the enrollment's pinned version with its current
/// signoff state, for readers the enrollment history is open to.
pub async fn matrix(
    pool: &SqlitePool,
    actor_user_id: i64,
    enrollment_id: i64,
) -> Result<std::result::Result<Vec<MatrixRow>, SignoffRefusal>> {
    let Some(version_id): Option<i64> =
        sqlx::query_scalar("SELECT program_version_id FROM enrollment WHERE id = ?1")
            .bind(enrollment_id)
            .fetch_optional(pool)
            .await
            .context("reading enrollment")?
    else {
        return Ok(Err(SignoffRefusal::NoSuchEnrollment));
    };
    if !lifecycle::may_read(pool, actor_user_id, enrollment_id).await? {
        return Ok(Err(SignoffRefusal::CapabilityRequired));
    }
    let rows = sqlx::query(
        "SELECT t.id AS task_id, c.category, c.name, t.prompt,
                s.kind, s.reason, s.signed_by_display_name, s.signed_at,
                (SELECT COUNT(*) FROM task_signoff h
                 WHERE h.enrollment_id = ?1 AND h.task_id = t.id) AS history
         FROM task t
         JOIN competency c ON c.id = t.competency_id
         LEFT JOIN task_signoff s
             ON s.id = (SELECT MAX(s2.id) FROM task_signoff s2
                        WHERE s2.enrollment_id = ?1 AND s2.task_id = t.id)
         WHERE t.program_version_id = ?2
         ORDER BY c.category COLLATE NOCASE, c.name COLLATE NOCASE,
                  t.sort_order, t.id",
    )
    .bind(enrollment_id)
    .bind(version_id)
    .fetch_all(pool)
    .await
    .context("listing the signoff matrix")?;
    Ok(Ok(rows
        .iter()
        .map(|row| MatrixRow {
            task_id: row.get("task_id"),
            competency_category: row.get("category"),
            competency_name: row.get("name"),
            prompt: row.get("prompt"),
            kind: row.get("kind"),
            reason: row.get("reason"),
            signed_by_display_name: row.get("signed_by_display_name"),
            signed_at: row.get("signed_at"),
            history: row.get("history"),
        })
        .collect()))
}

// ---------------------------------------------- complete history read (#49)

/// One retained signoff row as history reports it: the stored snapshots,
/// never a live re-derivation. `signoff_id` is the installation's row
/// identity, so ascending it is recorded order (ADR 0013; the packet's
/// authoritative history semantics, ADR 0015).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignoffEntryRow {
    pub signoff_id: i64,
    pub kind: SignoffKind,
    /// The override's recorded reason; empty for a first signoff.
    pub reason: String,
    pub signed_by_user_id: i64,
    /// The signer's name as stored at the act, so a later rename never
    /// rewrites what the record says.
    pub signed_by_display_name: String,
    pub signed_at: i64,
}

/// One task with every signoff recorded against it, oldest first. Tasks
/// carry the configuration of the program version they belong to, which
/// is the version that was pinned when they were signed: a version
/// change starts a fresh vocabulary, so a task of an earlier version is
/// reported as exactly that rather than dropped or reattributed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignoffTaskRow {
    pub task_id: i64,
    pub prompt: String,
    pub competency_category: String,
    pub competency_name: String,
    pub program_version_id: i64,
    pub program_version_number: i64,
    pub program_version_label: String,
    /// Whether this task belongs to the enrollment's currently pinned
    /// version, so the interface can separate current tasks from the
    /// retained vocabulary of earlier versions.
    pub in_current_version: bool,
    /// The latest retained row's kind, or `null` when nothing was ever
    /// signed off for this task. `revoked` is therefore distinguishable
    /// from never signed off (issue #49).
    pub current_kind: Option<SignoffKind>,
    /// Every retained row, oldest first, never a summary.
    pub signoffs: Vec<SignoffEntryRow>,
}

/// The complete signoff read for one enrollment (#49).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignoffHistory {
    pub enrollment_id: i64,
    pub trainee_user_id: i64,
    pub current_program_version_id: i64,
    pub current_program_version_number: i64,
    pub current_program_version_label: String,
    /// Every task with a retained signoff, plus every task of the
    /// currently pinned version whether or not it was signed off,
    /// ordered by competency and task as the recording interface orders
    /// them.
    pub tasks: Vec<SignoffTaskRow>,
}

/// The complete retained signoff history of one enrollment, for whoever
/// may read its training history or the trainee reading their own.
///
/// This is the read the trainee packet's signoff document already makes,
/// expressed as a service contract: it is driven by the append-only
/// `task_signoff` rows themselves, so signoffs recorded under an earlier
/// program version stay discoverable after a version change, and it
/// never projects the history through the current pin (`matrix` does
/// that, for its different purpose).
///
/// Authorization is evaluated on the read's own connection inside its
/// transaction, so permission and contents describe one committed state,
/// and the request never acquires a second pooled connection while
/// holding one (a one-connection pool must not deadlock).
pub async fn history(
    pool: &SqlitePool,
    actor_user_id: i64,
    enrollment_id: i64,
) -> Result<std::result::Result<SignoffHistory, SignoffRefusal>> {
    let mut tx = pool.begin().await.context("beginning signoff read")?;
    let Some((trainee_user_id, current_version_id)) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT user_id, program_version_id FROM enrollment WHERE id = ?1",
    )
    .bind(enrollment_id)
    .fetch_optional(&mut *tx)
    .await
    .context("reading enrollment")?
    else {
        return Ok(Err(SignoffRefusal::NoSuchEnrollment));
    };
    if !may_read_history(&mut tx, actor_user_id, enrollment_id, trainee_user_id).await? {
        return Ok(Err(SignoffRefusal::CapabilityRequired));
    }
    let (current_number, current_label): (i64, String) =
        sqlx::query_as("SELECT version_number, label FROM program_version WHERE id = ?1")
            .bind(current_version_id)
            .fetch_one(&mut *tx)
            .await
            .context("reading the pinned program version")?;
    let rows = load_history_rows(&mut tx, enrollment_id, current_version_id).await?;
    tx.commit().await.context("ending signoff read")?;
    Ok(Ok(SignoffHistory {
        enrollment_id,
        trainee_user_id,
        current_program_version_id: current_version_id,
        current_program_version_number: current_number,
        current_program_version_label: current_label,
        tasks: rows,
    }))
}

/// Who may read an enrollment's complete signoff history: the readers the
/// training-history rule already admits, plus the trainee holding
/// `view_own_records` on their own enrollment. It does not widen
/// `lifecycle::may_read`, so no other read or editing workflow moves with
/// it, and it grants no write: recording still takes its own authority
/// (`record`, above, unchanged).
async fn may_read_history(
    conn: &mut SqliteConnection,
    actor_user_id: i64,
    enrollment_id: i64,
    trainee_user_id: i64,
) -> Result<bool> {
    if lifecycle::may_read_on(&mut *conn, actor_user_id, enrollment_id).await? {
        return Ok(true);
    }
    if actor_user_id != trainee_user_id {
        return Ok(false);
    }
    capabilities::user_has_on(conn, actor_user_id, Capability::ViewOwnRecords).await
}

/// The enrollment's retained signoff history: every task with any
/// signoff, plus every task of the currently pinned version, each with
/// its own rows in recorded order.
async fn load_history_rows(
    conn: &mut SqliteConnection,
    enrollment_id: i64,
    current_version_id: i64,
) -> Result<Vec<SignoffTaskRow>> {
    let rows = sqlx::query(
        "SELECT t.id AS task_id, t.program_version_id AS program_version_id,
                t.prompt AS prompt, c.category AS competency_category,
                c.name AS competency_name, pv.version_number AS program_version_number,
                pv.label AS program_version_label,
                s.id AS signoff_id, s.kind AS kind, s.reason AS reason,
                s.signed_by AS signed_by, s.signed_by_display_name AS signed_by_display_name,
                s.signed_at AS signed_at
         FROM task t
         JOIN competency c ON c.id = t.competency_id
         JOIN program_version pv ON pv.id = t.program_version_id
         LEFT JOIN task_signoff s
             ON s.task_id = t.id AND s.enrollment_id = ?1
         WHERE t.program_version_id = ?2
            OR EXISTS (SELECT 1 FROM task_signoff x
                       WHERE x.enrollment_id = ?1 AND x.task_id = t.id)
         ORDER BY c.category COLLATE NOCASE, c.name COLLATE NOCASE,
                  t.sort_order, t.id, s.id",
    )
    .bind(enrollment_id)
    .bind(current_version_id)
    .fetch_all(&mut *conn)
    .await
    .context("reading the signoff history")?;

    let mut tasks: Vec<SignoffTaskRow> = Vec::new();
    for row in &rows {
        let task_id: i64 = row.get("task_id");
        let signoff_id: Option<i64> = row.get("signoff_id");
        if tasks.last().map(|task| task.task_id) != Some(task_id) {
            let version_id: i64 = row.get("program_version_id");
            tasks.push(SignoffTaskRow {
                task_id,
                prompt: row.get("prompt"),
                competency_category: row.get("competency_category"),
                competency_name: row.get("competency_name"),
                program_version_id: version_id,
                program_version_number: row.get("program_version_number"),
                program_version_label: row.get("program_version_label"),
                in_current_version: version_id == current_version_id,
                current_kind: None,
                signoffs: Vec::new(),
            });
        }
        let Some(signoff_id) = signoff_id else {
            // A task of the pinned version with nothing signed off yet.
            continue;
        };
        let kind: SignoffKind = closed_kind(row.get("kind"))?;
        let task = tasks
            .last_mut()
            .context("a signoff row without its task row")?;
        task.current_kind = Some(kind);
        task.signoffs.push(SignoffEntryRow {
            signoff_id,
            kind,
            reason: row.get("reason"),
            signed_by_user_id: row.get("signed_by"),
            signed_by_display_name: row.get("signed_by_display_name"),
            signed_at: row.get("signed_at"),
        });
    }
    Ok(tasks)
}

/// A stored discriminator, parsed into the closed set migration 0013
/// constrains. Any other value is corruption to surface, never a string
/// to pass along.
fn closed_kind(stored: &str) -> Result<SignoffKind> {
    serde_json::from_value(serde_json::Value::String(stored.to_owned()))
        .with_context(|| format!("stored signoff kind {stored:?} is outside its closed set"))
}

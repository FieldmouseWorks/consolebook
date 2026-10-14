//! How far a new installation has come from first run to its first training
//! session, for the administrator's "Getting started" checklist
//! (docs/install.md). Counts only: nothing here names a person, a program, or
//! a record, and nothing is written.

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::SqlitePool;

use crate::capabilities::{self, Capability};

/// Installation-wide counts, one per checklist step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Progress {
    /// Users who can review evaluations (the Coordinator bundle).
    pub coordinators: i64,
    /// Users who can author evaluations (the Trainer bundle).
    pub trainers: i64,
    /// Users who can read their own records (the Trainee bundle).
    pub trainees: i64,
    pub programs: i64,
    pub published_versions: i64,
    pub enrollments: i64,
    pub assignments: i64,
    pub sessions: i64,
    pub finalized_records: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressRefusal {
    /// Setup progress belongs to whoever administers users.
    CapabilityRequired,
}

/// The installation's setup progress, for a holder of `manage_users`.
pub async fn progress(
    pool: &SqlitePool,
    actor_user_id: i64,
) -> Result<std::result::Result<Progress, ProgressRefusal>> {
    if !capabilities::user_has(pool, actor_user_id, Capability::ManageUsers).await? {
        return Ok(Err(ProgressRefusal::CapabilityRequired));
    }
    let row: (i64, i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
             (SELECT COUNT(*) FROM capability_grant WHERE capability = ?1),
             (SELECT COUNT(*) FROM capability_grant WHERE capability = ?2),
             (SELECT COUNT(*) FROM capability_grant WHERE capability = ?3),
             (SELECT COUNT(*) FROM program),
             (SELECT COUNT(*) FROM program_version WHERE published_at IS NOT NULL),
             (SELECT COUNT(*) FROM enrollment),
             (SELECT COUNT(*) FROM training_assignment),
             (SELECT COUNT(*) FROM training_session),
             (SELECT COUNT(DISTINCT evaluation_record_id) FROM evaluation_version)",
    )
    .bind(Capability::ReviewEvaluation.as_str())
    .bind(Capability::AuthorEvaluation.as_str())
    .bind(Capability::ViewOwnRecords.as_str())
    .fetch_one(pool)
    .await
    .context("counting setup progress")?;
    Ok(Ok(Progress {
        coordinators: row.0,
        trainers: row.1,
        trainees: row.2,
        programs: row.3,
        published_versions: row.4,
        enrollments: row.5,
        assignments: row.6,
        sessions: row.7,
        finalized_records: row.8,
    }))
}

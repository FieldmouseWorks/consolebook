//! Draft workspace snapshot projection and response vocabulary.
//! Lifecycle state and shared record/event interfaces belong to the parent.

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::{Row, SqlitePool};

use super::{DraftRefusal, DraftStatus, load_record, status_of};
use crate::capabilities::{self, Capability};
use crate::draft_content;

/// One contributor event, presented.
#[derive(Debug, Clone, Serialize)]
pub struct ContributorEventRow {
    pub id: i64,
    pub kind: String,
    pub actor_user_id: i64,
    pub actor_display_name: String,
    pub to_user_id: Option<i64>,
    pub to_display_name: Option<String>,
    pub recorded_at: i64,
}

/// A covered session, for the workspace header.
#[derive(Debug, Clone, Serialize)]
pub struct CoveredSession {
    pub session_id: i64,
    pub business_date: String,
    pub timezone: String,
    pub local_start: String,
    pub local_end: Option<String>,
}

/// A snapshot's metadata; content stays in the database until review
/// needs it.
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotMeta {
    pub id: i64,
    pub reason: String,
    pub taken_at: i64,
    pub taken_by: Option<i64>,
}

/// An eligible ownership recipient for the transfer picker.
#[derive(Debug, Clone, Serialize)]
pub struct EligibleRecipient {
    pub user_id: i64,
    pub display_name: String,
}

/// One review decision, presented with its comment — the workflow's
/// permanent verdicts.
#[derive(Debug, Clone, Serialize)]
pub struct ReviewDecisionRow {
    pub id: i64,
    pub reviewer_user_id: i64,
    pub reviewer_display_name: String,
    pub decision: String,
    pub comment: String,
    pub decided_at: i64,
}

/// The draft's workflow view: identity, status, attribution, coverage.
#[derive(Debug, Serialize)]
pub struct DraftDetail {
    pub id: i64,
    pub enrollment_id: i64,
    pub program_version_id: i64,
    pub evaluation_form_id: i64,
    pub owner_user_id: i64,
    pub owner_display_name: String,
    pub status: DraftStatus,
    pub trainee_user_id: i64,
    pub trainee_display_name: String,
    pub program_name: String,
    pub version_number: i64,
    /// The pinned form's record type; weekly summaries carry links.
    pub record_type: String,
    /// The pinned daily links (weekly summaries; empty otherwise).
    pub summary_links: Vec<crate::summaries::SummaryLink>,
    pub sessions: Vec<CoveredSession>,
    pub events: Vec<ContributorEventRow>,
    pub snapshots: Vec<SnapshotMeta>,
    pub eligible_recipients: Vec<EligibleRecipient>,
    pub decisions: Vec<ReviewDecisionRow>,
    /// Whether the caller may decide this draft right now: a qualified
    /// non-contributor reviewer looking at a submitted draft.
    pub viewer_may_review: bool,
    /// Whether the caller may attempt finalization: a reviewer looking
    /// at an unfinalized record. Completion rules answer at the act
    /// (ADR 0011).
    pub viewer_may_finalize: bool,
    /// Whether the caller may open an amendment: a reviewer looking at
    /// a finalized record with no reopening in progress.
    pub viewer_may_amend: bool,
    /// The latest finalized version's number, when any exists.
    pub latest_version_number: Option<i64>,
    /// The open amendment reopening this record's working copy, if any.
    pub open_amendment: Option<crate::amendments::AmendmentView>,
    pub created_at: i64,
    /// The working copy's optimistic-concurrency revision; every save
    /// carries the revision it read.
    pub revision: i64,
}

/// The workspace read: the workflow view, the pinned form skeleton, and
/// the working copy, assembled under one database snapshot so the
/// revision describes exactly the content returned beside it.
#[derive(Debug)]
pub struct DraftWorkspace {
    pub detail: DraftDetail,
    pub form: draft_content::FormSkeleton,
    pub content: draft_content::DraftContent,
}

/// Reads the workspace, gated like the record's reads. Every data read
/// shares one transaction: a concurrent save moves the whole view or
/// none of it, never the content without its revision.
#[allow(clippy::too_many_lines)]
pub async fn workspace(
    pool: &SqlitePool,
    actor_user_id: i64,
    record_id: i64,
) -> Result<std::result::Result<DraftWorkspace, DraftRefusal>> {
    let mut conn = pool.acquire().await.context("acquiring connection")?;
    let Some(record) = load_record(&mut conn, record_id).await? else {
        return Ok(Err(DraftRefusal::NoSuchRecord));
    };
    drop(conn);
    let Some(grant) = crate::draft_access::read_grant(pool, actor_user_id, &record).await? else {
        return Ok(Err(DraftRefusal::CapabilityRequired));
    };
    // An own-record reader is authorized for the finalized record, not
    // the workflow: what exists solely for the workflow (the transfer
    // picker's roster, snapshot bookkeeping) is redacted below.
    let workflow_reader = grant == crate::draft_access::ReadGrant::Workflow;
    let reviewer_capability =
        capabilities::user_has(pool, actor_user_id, Capability::ReviewEvaluation).await?;
    let mut conn = pool.begin().await.context("starting workspace read")?;
    // Reread inside the snapshot: this revision is the one the returned
    // content answers to.
    let Some(record) = load_record(&mut conn, record_id).await? else {
        return Ok(Err(DraftRefusal::NoSuchRecord));
    };
    // For an own-record reader the record presents as its sealed self:
    // an in-progress correction is a draft about them, not theirs to
    // see, so the status reads finalized, the working copy stays out of
    // the response, and the streams stop at the sealed cycle's marks
    // (the envelope carries exactly that history).
    let scope = crate::amendments::open_scope(&mut conn, record_id).await?;
    let (event_cap, decision_cap) = match (&scope, workflow_reader) {
        (Some(scope), false) => (scope.opened_after_event_id, scope.opened_after_decision_id),
        _ => (i64::MAX, i64::MAX),
    };
    let status = if workflow_reader {
        status_of(&mut conn, record_id).await?
    } else {
        DraftStatus::Finalized
    };
    let viewer_may_review = reviewer_capability
        && status == DraftStatus::Submitted
        && !crate::draft_access::is_contributor(&mut conn, record_id, actor_user_id).await?;
    let viewer_may_finalize =
        workflow_reader && reviewer_capability && status != DraftStatus::Finalized;
    let viewer_may_amend =
        workflow_reader && reviewer_capability && status == DraftStatus::Finalized;
    let latest_version_number: Option<i64> = sqlx::query_scalar(
        "SELECT MAX(version_number) FROM evaluation_version WHERE evaluation_record_id = ?1",
    )
    .bind(record_id)
    .fetch_one(&mut *conn)
    .await
    .context("reading latest version number")?;
    let open_amendment = if workflow_reader {
        scope.map(|scope| crate::amendments::AmendmentView {
            reason: scope.reason,
            opened_by_display_name: scope.opened_by_display_name,
            opened_at: scope.opened_at,
        })
    } else {
        None
    };
    let header = sqlx::query(
        "SELECT r.created_at, e.user_id AS trainee_user_id,
                tu.display_name AS trainee_display_name,
                ou.display_name AS owner_display_name,
                pv.name AS program_name, pv.version_number,
                f.record_type
         FROM evaluation_record r
         JOIN enrollment e ON e.id = r.enrollment_id
         JOIN user tu ON tu.id = e.user_id
         JOIN user ou ON ou.id = r.owner_user_id
         JOIN program_version pv ON pv.id = r.program_version_id
         JOIN evaluation_form f ON f.id = r.evaluation_form_id
         WHERE r.id = ?1",
    )
    .bind(record_id)
    .fetch_one(&mut *conn)
    .await
    .context("reading record header")?;
    // Working-copy state travels to workflow readers; an own-record
    // reader's coverage is the sealed envelope's daily_reports.
    let summary_links = if workflow_reader {
        crate::summaries::links(&mut conn, record_id).await?
    } else {
        Vec::new()
    };
    let sessions = sqlx::query(
        "SELECT s.id, s.business_date, s.timezone, s.local_start, s.local_end
         FROM evaluation_session es
         JOIN training_session s ON s.id = es.training_session_id
         WHERE es.evaluation_record_id = ?1
         ORDER BY s.utc_start, s.id",
    )
    .bind(record_id)
    .fetch_all(&mut *conn)
    .await
    .context("listing covered sessions")?
    .iter()
    .map(|row| CoveredSession {
        session_id: row.get("id"),
        business_date: row.get("business_date"),
        timezone: row.get("timezone"),
        local_start: row.get("local_start"),
        local_end: row.get("local_end"),
    })
    .collect();
    let events = sqlx::query(
        "SELECT ce.id, ce.kind, ce.actor_user_id, au.display_name AS actor_display_name,
                ce.to_user_id, tu.display_name AS to_display_name, ce.recorded_at
         FROM contributor_event ce
         JOIN user au ON au.id = ce.actor_user_id
         LEFT JOIN user tu ON tu.id = ce.to_user_id
         WHERE ce.evaluation_record_id = ?1 AND ce.id <= ?2
         ORDER BY ce.id",
    )
    .bind(record_id)
    .bind(event_cap)
    .fetch_all(&mut *conn)
    .await
    .context("listing contributor events")?
    .iter()
    .map(|row| ContributorEventRow {
        id: row.get("id"),
        kind: row.get("kind"),
        actor_user_id: row.get("actor_user_id"),
        actor_display_name: row.get("actor_display_name"),
        to_user_id: row.get("to_user_id"),
        to_display_name: row.get("to_display_name"),
        recorded_at: row.get("recorded_at"),
    })
    .collect();
    let snapshots = if workflow_reader {
        sqlx::query(
            "SELECT id, reason, taken_at, taken_by FROM draft_snapshot
             WHERE evaluation_record_id = ?1 ORDER BY id",
        )
        .bind(record_id)
        .fetch_all(&mut *conn)
        .await
        .context("listing snapshots")?
        .iter()
        .map(|row| SnapshotMeta {
            id: row.get("id"),
            reason: row.get("reason"),
            taken_at: row.get("taken_at"),
            taken_by: row.get("taken_by"),
        })
        .collect()
    } else {
        Vec::new()
    };
    // The transfer picker: authors within the record's scope, minus the
    // current owner. Workflow readers only — the roster of possible
    // recipients names staff who may never touch this record, which is
    // not an own-record reader's to see.
    let eligible_recipients = if workflow_reader {
        sqlx::query(
            "SELECT DISTINCT u.id, u.display_name
             FROM user u
             JOIN capability_grant cg ON cg.user_id = u.id AND cg.capability = ?3
             WHERE u.id != ?2
               AND u.id IN (
                   SELECT ta.trainer_user_id FROM training_assignment ta
                   JOIN evaluation_record r ON r.enrollment_id = ta.enrollment_id
                   WHERE r.id = ?1 AND ta.ended_at IS NULL
                   UNION
                   SELECT st.trainer_user_id FROM session_trainer st
                   JOIN evaluation_session es ON es.training_session_id = st.session_id
                   WHERE es.evaluation_record_id = ?1
               )
             ORDER BY u.display_name COLLATE NOCASE",
        )
        .bind(record_id)
        .bind(record.owner_user_id)
        .bind(Capability::AuthorEvaluation.as_str())
        .fetch_all(&mut *conn)
        .await
        .context("listing eligible recipients")?
        .iter()
        .map(|row| EligibleRecipient {
            user_id: row.get("id"),
            display_name: row.get("display_name"),
        })
        .collect()
    } else {
        Vec::new()
    };
    let decisions = sqlx::query(
        "SELECT rd.id, rd.reviewer_user_id, u.display_name AS reviewer_display_name,
                rd.decision, rd.comment, rd.decided_at
         FROM review_decision rd
         JOIN user u ON u.id = rd.reviewer_user_id
         WHERE rd.evaluation_record_id = ?1 AND rd.id <= ?2
         ORDER BY rd.id",
    )
    .bind(record_id)
    .bind(decision_cap)
    .fetch_all(&mut *conn)
    .await
    .context("listing review decisions")?
    .iter()
    .map(|row| ReviewDecisionRow {
        id: row.get("id"),
        reviewer_user_id: row.get("reviewer_user_id"),
        reviewer_display_name: row.get("reviewer_display_name"),
        decision: row.get("decision"),
        comment: row.get("comment"),
        decided_at: row.get("decided_at"),
    })
    .collect();
    let form = draft_content::skeleton(
        &mut conn,
        record.program_version_id,
        record.evaluation_form_id,
    )
    .await?;
    // The working copy travels only to workflow readers; an own-record
    // reader's content is the stored envelope, fetched separately.
    let content = if workflow_reader {
        draft_content::content(&mut conn, record_id).await?
    } else {
        draft_content::DraftContent {
            ratings: Vec::new(),
            narratives: Vec::new(),
        }
    };
    Ok(Ok(DraftWorkspace {
        detail: DraftDetail {
            id: record.id,
            enrollment_id: record.enrollment_id,
            program_version_id: record.program_version_id,
            evaluation_form_id: record.evaluation_form_id,
            owner_user_id: record.owner_user_id,
            owner_display_name: header.get("owner_display_name"),
            status,
            trainee_user_id: header.get("trainee_user_id"),
            trainee_display_name: header.get("trainee_display_name"),
            program_name: header.get("program_name"),
            version_number: header.get("version_number"),
            record_type: header.get("record_type"),
            summary_links,
            sessions,
            events,
            snapshots,
            eligible_recipients,
            decisions,
            viewer_may_review,
            viewer_may_finalize,
            viewer_may_amend,
            latest_version_number,
            open_amendment,
            created_at: header.get("created_at"),
            revision: record.revision,
        },
        form,
        content,
    }))
}

//! Append-only audit events for security-sensitive actions.
//!
//! The `audit_event` table refuses UPDATE and DELETE at the database level
//! (migration 0002). Events carry no record content, narratives, or secret
//! material — only what happened, when, and to whom.

use anyhow::{Context, Result};
use sqlx::{Executor, Sqlite};
use time::OffsetDateTime;

use crate::closed_vocabulary::closed_vocabulary;

closed_vocabulary! {
    /// The authentication- and configuration-era event vocabulary. Later
    /// milestones extend this with record-lifecycle kinds.
    pub enum EventKind {
        RetentionAuthorityGranted => "retention_authority_granted",
        RetentionAuthorityRevoked => "retention_authority_revoked",
        RetentionPolicyCreated => "retention_policy_created",
        RecordHoldCreated => "record_hold_created",
        RecordHoldReplaced => "record_hold_replaced",
        RecordHoldReleased => "record_hold_released",
        SetupCompleted => "setup_completed",
        LoginSucceeded => "login_succeeded",
        LoginFailed => "login_failed",
        Logout => "logout",
        ResetCodeIssued => "reset_code_issued",
        ResetCodeUsed => "reset_code_used",
        RecoveryCodeIssued => "recovery_code_issued",
        BackupCompleted => "backup_completed",
        RestoreCompleted => "restore_completed",
        ProgramCreated => "program_created",
        ProgramVersionCreated => "program_version_created",
        ProgramVersionPublished => "program_version_published",
        ProgramVersionImported => "program_version_imported",
        ProgramVersionDiscarded => "program_version_discarded",
        UserCreated => "user_created",
        EnrollmentCreated => "enrollment_created",
        EnrollmentWithdrawn => "enrollment_withdrawn",
        EnrollmentCompleted => "enrollment_completed",
        EnrollmentReinstated => "enrollment_reinstated",
        EnrollmentVersionChanged => "enrollment_version_changed",
        PhaseEventRecorded => "phase_event_recorded",
        AssignmentCreated => "assignment_created",
        AssignmentEnded => "assignment_ended",
        SessionCreated => "session_created",
        SessionUpdated => "session_updated",
        SessionClosed => "session_closed",
        SessionTrainerAdded => "session_trainer_added",
        SessionTrainerRemoved => "session_trainer_removed",
        DraftCreated => "draft_created",
        DraftOwnershipTransferred => "draft_ownership_transferred",
        DraftSubmitted => "draft_submitted",
        DraftReviewDecided => "draft_review_decided",
        DraftFinalized => "draft_finalized",
        AcknowledgmentRecorded => "acknowledgment_recorded",
        AmendmentOpened => "amendment_opened",
        TaskSignoffRecorded => "task_signoff_recorded",
        RecordExported => "record_exported",
        TraineePacketExported => "trainee_packet_exported",
        InstallationExported => "installation_exported",
    }
}

/// A domain row an event acted on. Stored as (`subject_kind`, `subject_id`)
/// with deliberately no foreign key (migration 0004): the audit trail is
/// append-only and must never block lawful disposition of its subjects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    RetentionPolicy(i64),
    RecordHold(i64),
    Program(i64),
    ProgramVersion(i64),
    Enrollment(i64),
    Assignment(i64),
    Session(i64),
    Record(i64),
}

impl Subject {
    /// What kind of row the subject is.
    #[must_use]
    pub fn kind(self) -> SubjectKind {
        match self {
            Self::RetentionPolicy(_) => SubjectKind::RetentionPolicy,
            Self::RecordHold(_) => SubjectKind::RecordHold,
            Self::Program(_) => SubjectKind::Program,
            Self::ProgramVersion(_) => SubjectKind::ProgramVersion,
            Self::Enrollment(_) => SubjectKind::Enrollment,
            Self::Assignment(_) => SubjectKind::Assignment,
            Self::Session(_) => SubjectKind::Session,
            Self::Record(_) => SubjectKind::Record,
        }
    }

    #[must_use]
    pub fn kind_str(self) -> &'static str {
        self.kind().as_str()
    }

    #[must_use]
    pub fn id(self) -> i64 {
        match self {
            Self::RetentionPolicy(id)
            | Self::RecordHold(id)
            | Self::Program(id)
            | Self::ProgramVersion(id)
            | Self::Enrollment(id)
            | Self::Assignment(id)
            | Self::Session(id)
            | Self::Record(id) => id,
        }
    }
}

closed_vocabulary! {
    /// The stored `subject_kind` vocabulary: one kind per [`Subject`]
    /// variant, closed, and only ever added to.
    pub enum SubjectKind {
        RetentionPolicy => "retention_policy",
        RecordHold => "record_hold",
        Program => "program",
        ProgramVersion => "program_version",
        Enrollment => "enrollment",
        Assignment => "assignment",
        /// A training session.
        Session => "session",
        /// An evaluation record.
        Record => "record",
    }
}

/// Records one event. Callers inside a transaction pass the transaction so
/// the event commits or rolls back with the action it describes.
pub async fn record<'e>(
    executor: impl Executor<'e, Database = Sqlite>,
    kind: EventKind,
    actor_user_id: Option<i64>,
    subject_user_id: Option<i64>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO audit_event (occurred_at, kind, actor_user_id, subject_user_id)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(OffsetDateTime::now_utc().unix_timestamp())
    .bind(kind.as_str())
    .bind(actor_user_id)
    .bind(subject_user_id)
    .execute(executor)
    .await
    .context("recording audit event")?;
    Ok(())
}

/// Records one event about a domain subject, optionally also naming the
/// person it concerns (`subject_user_id`). Callers inside a transaction
/// pass the transaction so the event commits or rolls back with the action
/// it describes.
pub async fn record_for_subject<'e>(
    executor: impl Executor<'e, Database = Sqlite>,
    kind: EventKind,
    actor_user_id: Option<i64>,
    subject_user_id: Option<i64>,
    subject: Subject,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO audit_event
             (occurred_at, kind, actor_user_id, subject_user_id, subject_kind, subject_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(OffsetDateTime::now_utc().unix_timestamp())
    .bind(kind.as_str())
    .bind(actor_user_id)
    .bind(subject_user_id)
    .bind(subject.kind_str())
    .bind(subject.id())
    .execute(executor)
    .await
    .context("recording audit event")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each closed vocabulary names every value once, and parsing a stored
    /// name gives back the value that wrote it.
    #[test]
    fn vocabularies_are_closed_and_round_trip() {
        let mut names: Vec<&str> = EventKind::ALL.iter().map(|kind| kind.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), EventKind::ALL.len());
        for kind in EventKind::ALL.iter().copied() {
            assert_eq!(EventKind::parse(kind.as_str()), Some(kind));
        }
        let mut names: Vec<&str> = SubjectKind::ALL.iter().map(|kind| kind.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SubjectKind::ALL.len());
        for kind in SubjectKind::ALL.iter().copied() {
            assert_eq!(SubjectKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(EventKind::parse("invented_unknown_kind"), None);
        assert_eq!(SubjectKind::parse("invented_unknown_kind"), None);
    }
}

//! Append-only audit events for security-sensitive actions.
//!
//! The `audit_event` table refuses UPDATE and DELETE at the database level
//! (migration 0002). Events carry no record content, narratives, or secret
//! material — only what happened, when, and to whom.

use anyhow::{Context, Result};
use sqlx::{Executor, Sqlite};
use time::OffsetDateTime;

/// The authentication- and configuration-era event vocabulary. Later
/// milestones extend this with record-lifecycle kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    RetentionAuthorityGranted,
    RetentionAuthorityRevoked,
    RetentionPolicyCreated,
    RecordHoldCreated,
    RecordHoldReplaced,
    RecordHoldReleased,
    SetupCompleted,
    LoginSucceeded,
    LoginFailed,
    Logout,
    ResetCodeIssued,
    ResetCodeUsed,
    RecoveryCodeIssued,
    BackupCompleted,
    RestoreCompleted,
    ProgramCreated,
    ProgramVersionCreated,
    ProgramVersionPublished,
    ProgramVersionImported,
    ProgramVersionDiscarded,
    UserCreated,
    EnrollmentCreated,
    EnrollmentWithdrawn,
    EnrollmentCompleted,
    EnrollmentReinstated,
    EnrollmentVersionChanged,
    PhaseEventRecorded,
    AssignmentCreated,
    AssignmentEnded,
    SessionCreated,
    SessionUpdated,
    SessionClosed,
    SessionTrainerAdded,
    SessionTrainerRemoved,
    DraftCreated,
    DraftOwnershipTransferred,
    DraftSubmitted,
    DraftReviewDecided,
    DraftFinalized,
    AcknowledgmentRecorded,
    AmendmentOpened,
    TaskSignoffRecorded,
    RecordExported,
    TraineePacketExported,
    InstallationExported,
}

impl EventKind {
    /// Every kind, once. Kinds are only ever added, never removed: an
    /// older installation's audit trail must keep exporting.
    pub const ALL: [Self; 45] = [
        Self::RetentionAuthorityGranted,
        Self::RetentionAuthorityRevoked,
        Self::RetentionPolicyCreated,
        Self::RecordHoldCreated,
        Self::RecordHoldReplaced,
        Self::RecordHoldReleased,
        Self::SetupCompleted,
        Self::LoginSucceeded,
        Self::LoginFailed,
        Self::Logout,
        Self::ResetCodeIssued,
        Self::ResetCodeUsed,
        Self::RecoveryCodeIssued,
        Self::BackupCompleted,
        Self::RestoreCompleted,
        Self::ProgramCreated,
        Self::ProgramVersionCreated,
        Self::ProgramVersionPublished,
        Self::ProgramVersionImported,
        Self::ProgramVersionDiscarded,
        Self::UserCreated,
        Self::EnrollmentCreated,
        Self::EnrollmentWithdrawn,
        Self::EnrollmentCompleted,
        Self::EnrollmentReinstated,
        Self::EnrollmentVersionChanged,
        Self::PhaseEventRecorded,
        Self::AssignmentCreated,
        Self::AssignmentEnded,
        Self::SessionCreated,
        Self::SessionUpdated,
        Self::SessionClosed,
        Self::SessionTrainerAdded,
        Self::SessionTrainerRemoved,
        Self::DraftCreated,
        Self::DraftOwnershipTransferred,
        Self::DraftSubmitted,
        Self::DraftReviewDecided,
        Self::DraftFinalized,
        Self::AcknowledgmentRecorded,
        Self::AmendmentOpened,
        Self::TaskSignoffRecorded,
        Self::RecordExported,
        Self::TraineePacketExported,
        Self::InstallationExported,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RetentionAuthorityGranted => "retention_authority_granted",
            Self::RetentionAuthorityRevoked => "retention_authority_revoked",
            Self::RetentionPolicyCreated => "retention_policy_created",
            Self::RecordHoldCreated => "record_hold_created",
            Self::RecordHoldReplaced => "record_hold_replaced",
            Self::RecordHoldReleased => "record_hold_released",
            Self::SetupCompleted => "setup_completed",
            Self::LoginSucceeded => "login_succeeded",
            Self::LoginFailed => "login_failed",
            Self::Logout => "logout",
            Self::ResetCodeIssued => "reset_code_issued",
            Self::ResetCodeUsed => "reset_code_used",
            Self::RecoveryCodeIssued => "recovery_code_issued",
            Self::BackupCompleted => "backup_completed",
            Self::RestoreCompleted => "restore_completed",
            Self::ProgramCreated => "program_created",
            Self::ProgramVersionCreated => "program_version_created",
            Self::ProgramVersionPublished => "program_version_published",
            Self::ProgramVersionImported => "program_version_imported",
            Self::ProgramVersionDiscarded => "program_version_discarded",
            Self::UserCreated => "user_created",
            Self::EnrollmentCreated => "enrollment_created",
            Self::EnrollmentWithdrawn => "enrollment_withdrawn",
            Self::EnrollmentCompleted => "enrollment_completed",
            Self::EnrollmentReinstated => "enrollment_reinstated",
            Self::EnrollmentVersionChanged => "enrollment_version_changed",
            Self::PhaseEventRecorded => "phase_event_recorded",
            Self::AssignmentCreated => "assignment_created",
            Self::AssignmentEnded => "assignment_ended",
            Self::SessionCreated => "session_created",
            Self::SessionUpdated => "session_updated",
            Self::SessionClosed => "session_closed",
            Self::SessionTrainerAdded => "session_trainer_added",
            Self::SessionTrainerRemoved => "session_trainer_removed",
            Self::DraftCreated => "draft_created",
            Self::DraftOwnershipTransferred => "draft_ownership_transferred",
            Self::DraftSubmitted => "draft_submitted",
            Self::DraftReviewDecided => "draft_review_decided",
            Self::DraftFinalized => "draft_finalized",
            Self::AcknowledgmentRecorded => "acknowledgment_recorded",
            Self::AmendmentOpened => "amendment_opened",
            Self::TaskSignoffRecorded => "task_signoff_recorded",
            Self::RecordExported => "record_exported",
            Self::TraineePacketExported => "trainee_packet_exported",
            Self::InstallationExported => "installation_exported",
        }
    }

    /// The stored kind as this build knows it, or `None` for a kind it
    /// does not: the closed vocabulary the installation export parses
    /// stored values against (docs/formats/installation-export.md).
    #[must_use]
    pub fn parse(stored: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == stored)
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

/// The stored `subject_kind` vocabulary: one kind per [`Subject`]
/// variant, closed, and only ever added to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubjectKind {
    RetentionPolicy,
    RecordHold,
    Program,
    ProgramVersion,
    Enrollment,
    Assignment,
    /// A training session.
    Session,
    /// An evaluation record.
    Record,
}

impl SubjectKind {
    /// Every kind, once.
    pub const ALL: [Self; 8] = [
        Self::RetentionPolicy,
        Self::RecordHold,
        Self::Program,
        Self::ProgramVersion,
        Self::Enrollment,
        Self::Assignment,
        Self::Session,
        Self::Record,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RetentionPolicy => "retention_policy",
            Self::RecordHold => "record_hold",
            Self::Program => "program",
            Self::ProgramVersion => "program_version",
            Self::Enrollment => "enrollment",
            Self::Assignment => "assignment",
            Self::Session => "session",
            Self::Record => "record",
        }
    }

    /// The stored kind as this build knows it, or `None`.
    #[must_use]
    pub fn parse(stored: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == stored)
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
        for kind in EventKind::ALL {
            assert_eq!(EventKind::parse(kind.as_str()), Some(kind));
        }
        let mut names: Vec<&str> = SubjectKind::ALL.iter().map(|kind| kind.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SubjectKind::ALL.len());
        for kind in SubjectKind::ALL {
            assert_eq!(SubjectKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(EventKind::parse("invented_unknown_kind"), None);
        assert_eq!(SubjectKind::parse("invented_unknown_kind"), None);
    }
}

//! Closed retention-administration vocabulary. No policy here authorizes deletion.
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

macro_rules! vocabulary {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }
        impl $name {
            #[must_use]
            pub fn as_str(self) -> &'static str { match self { $(Self::$variant => $value),+ } }
            pub(super) fn from_db(value: &str) -> Result<Self> {
                match value { $($value => Ok(Self::$variant)),+, _ => bail!("invalid stored retention vocabulary") }
            }
        }
    };
}
vocabulary!(RecordClass { DailyReport => "daily_report", WeeklySummary => "weekly_summary", PhaseEvaluation => "phase_evaluation", DispositionEvent => "disposition_event" });
vocabulary!(RetentionTrigger { FinalizedAt => "finalized_at", EnrollmentClosedAt => "enrollment_closed_at", DisposedAt => "disposed_at" });
vocabulary!(RetentionAction { Retain => "retain", Destroy => "destroy" });
vocabulary!(HoldKind { Litigation => "litigation", AnticipatedLitigation => "anticipated_litigation", Audit => "audit", Investigation => "investigation", PublicRecordsRequest => "public_records_request", Other => "other" });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HoldScope {
    Installation,
    Enrollment { enrollment_id: i64 },
    Record { record_id: i64 },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyInput {
    pub record_class: RecordClass,
    pub expected_current_id: Option<i64>,
    pub authority: String,
    pub retention_trigger: RetentionTrigger,
    pub retention_days: i64,
    pub action: RetentionAction,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Policy {
    pub id: i64,
    pub record_class: RecordClass,
    pub version_number: i64,
    pub supersedes_id: Option<i64>,
    pub authority: String,
    pub retention_trigger: RetentionTrigger,
    pub retention_days: i64,
    pub action: RetentionAction,
    pub reason: String,
    pub created_by: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HoldInput {
    pub scope: HoldScope,
    pub kind: HoldKind,
    pub authority: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hold {
    pub id: i64,
    pub scope: HoldScope,
    pub kind: HoldKind,
    pub authority: String,
    pub reason: String,
    pub created_by: i64,
    pub created_at: i64,
    pub replaces_id: Option<i64>,
    pub release: Option<HoldRelease>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HoldRelease {
    pub released_by: i64,
    pub released_at: i64,
    pub reason: String,
    pub replacement_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuthorityEvent {
    pub id: i64,
    pub user_id: i64,
    pub granted: bool,
    pub actor_user_id: i64,
    pub reason: String,
    pub recorded_at: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RetentionRefusal {
    CapabilityRequired,
    Invalid(Vec<String>),
    StalePolicy,
    NoSuchEnrollment,
    NoSuchRecord,
    NoSuchHold,
    HoldReleased,
    NoSuchUser,
    AuthorityUnchanged,
}

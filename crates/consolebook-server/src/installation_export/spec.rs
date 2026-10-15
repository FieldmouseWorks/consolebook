//! The installation export's table specification: the one owning
//! definition of what the format carries (ADR 0028;
//! docs/formats/installation-export.md, Columns).
//!
//! Every carried table is listed (in `tables`) with its key and, per column, its
//! type, nullability, the table a reference resolves into, whether its
//! `program_version_id` must agree with the referenced row's, its closed
//! vocabulary, and any fixed text form. The exclusions are listed with
//! their reasons. The producer selects and maps columns from this list,
//! the verifier checks documents against it, and the coverage guard
//! ([`coverage_findings`]) compares it with the migrated schema both
//! ways, so a table or column cannot be added without deciding how the
//! format carries it.

use crate::audit::{EventKind, SubjectKind};
use crate::capabilities::Capability;
use crate::notices::NoticeKind;

mod coverage;
mod tables;

pub use coverage::{CoverageFinding, coverage_findings, in_lists};
pub use tables::TABLES;

/// How a column's value is written in a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    /// A JSON integer within the canonical subset's range.
    Integer,
    /// A JSON string, exactly as stored.
    Text,
    /// `true` or `false`, stored as `1` or `0`.
    Flag,
}

impl ColumnType {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Integer => "integer",
            Self::Text => "text",
            Self::Flag => "flag",
        }
    }

    /// The SQLite storage type the column is declared with.
    #[must_use]
    pub fn stored_type(self) -> &'static str {
        match self {
            Self::Integer | Self::Flag => "INTEGER",
            Self::Text => "TEXT",
        }
    }
}

/// A closed set of values a text column is drawn from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vocabulary {
    /// The list the stored table's `CHECK ... IN (...)` names.
    OneOf(&'static [&'static str]),
    /// `capabilities.rs`.
    Capability,
    /// `notices.rs`.
    NoticeKind,
    /// `audit.rs`, event kinds.
    AuditEventKind,
    /// `audit.rs`, subject kinds.
    AuditSubjectKind,
}

impl Vocabulary {
    /// The vocabulary's name, for the module-owned ones the format
    /// document refers to by name; `None` for a stored `CHECK` list.
    #[must_use]
    pub fn module_name(self) -> Option<&'static str> {
        match self {
            Self::OneOf(_) => None,
            Self::Capability => Some("capability"),
            Self::NoticeKind => Some("notice kind"),
            Self::AuditEventKind => Some("audit event kind"),
            Self::AuditSubjectKind => Some("audit subject kind"),
        }
    }

    /// Whether `value` is in the vocabulary. The module-owned ones are
    /// parsed by their owning type, never matched against a copy.
    #[must_use]
    pub fn admits(self, value: &str) -> bool {
        match self {
            Self::OneOf(values) => values.contains(&value),
            Self::Capability => Capability::parse(value).is_some(),
            Self::NoticeKind => NoticeKind::parse(value).is_some(),
            Self::AuditEventKind => EventKind::parse(value).is_some(),
            Self::AuditSubjectKind => SubjectKind::parse(value).is_some(),
        }
    }

    /// Every value, in the owner's order.
    #[must_use]
    pub fn values(self) -> Vec<&'static str> {
        match self {
            Self::OneOf(values) => values.to_vec(),
            Self::Capability => Capability::ALL.iter().map(|v| v.as_str()).collect(),
            Self::NoticeKind => NoticeKind::ALL.iter().map(|v| v.as_str()).collect(),
            Self::AuditEventKind => EventKind::ALL.iter().map(|v| v.as_str()).collect(),
            Self::AuditSubjectKind => SubjectKind::ALL.iter().map(|v| v.as_str()).collect(),
        }
    }
}

/// A fixed text form a text column's value takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextForm {
    /// 64 lowercase hexadecimal characters (a SHA-256).
    LowercaseHex64,
}

impl TextForm {
    #[must_use]
    pub fn admits(self, value: &str) -> bool {
        match self {
            Self::LowercaseHex64 => crate::export_verify::is_lowercase_hex_hash(value),
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LowercaseHex64 => "64 lowercase hex",
        }
    }
}

/// One carried column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    pub name: &'static str,
    pub ty: ColumnType,
    pub nullable: bool,
    /// The table whose `id` a non-null value names.
    pub references: Option<&'static str>,
    /// The row's `program_version_id` must equal the referenced row's
    /// (docs/formats/installation-export.md, Agreements).
    pub agrees: bool,
    pub vocabulary: Option<Vocabulary>,
    pub form: Option<TextForm>,
}

impl Column {
    const fn new(name: &'static str, ty: ColumnType) -> Self {
        Self {
            name,
            ty,
            nullable: false,
            references: None,
            agrees: false,
            vocabulary: None,
            form: None,
        }
    }

    const fn null(self) -> Self {
        Self {
            nullable: true,
            ..self
        }
    }

    const fn to(self, table: &'static str) -> Self {
        Self {
            references: Some(table),
            ..self
        }
    }

    /// A reference whose row must agree on `program_version_id`.
    const fn pinned(self, table: &'static str) -> Self {
        Self {
            references: Some(table),
            agrees: true,
            ..self
        }
    }

    const fn one_of(self, values: &'static [&'static str]) -> Self {
        Self {
            vocabulary: Some(Vocabulary::OneOf(values)),
            ..self
        }
    }

    const fn vocab(self, vocabulary: Vocabulary) -> Self {
        Self {
            vocabulary: Some(vocabulary),
            ..self
        }
    }

    const fn hex64(self) -> Self {
        Self {
            form: Some(TextForm::LowercaseHex64),
            ..self
        }
    }
}

const fn int(name: &'static str) -> Column {
    Column::new(name, ColumnType::Integer)
}

const fn text(name: &'static str) -> Column {
    Column::new(name, ColumnType::Text)
}

const fn flag(name: &'static str) -> Column {
    Column::new(name, ColumnType::Flag)
}

/// One carried table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Table {
    pub name: &'static str,
    /// The columns rows are strictly ascending by, in order.
    pub key: &'static [&'static str],
    /// Every carried column, in stored order.
    pub columns: &'static [Column],
}

impl Table {
    /// The document's entry name: `tables/{name}.json`.
    #[must_use]
    pub fn path(&self) -> String {
        super::table_path(self.name)
    }

    #[must_use]
    pub fn column(&self, name: &str) -> Option<&'static Column> {
        self.columns.iter().find(|column| column.name == name)
    }
}

/// Why the format does not carry a table or column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExclusionReason {
    /// It would let someone sign in, or it hashes a secret that would.
    Credential,
}

impl ExclusionReason {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Credential => "credential",
        }
    }
}

/// A table (`name`) or column (`table.column`) the format does not carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exclusion {
    pub name: &'static str,
    pub reason: ExclusionReason,
}

/// The closed exclusion list, ascending by name.
pub const EXCLUSIONS: [Exclusion; 5] = [
    Exclusion {
        name: "password_reset_code",
        reason: ExclusionReason::Credential,
    },
    Exclusion {
        name: "session",
        reason: ExclusionReason::Credential,
    },
    Exclusion {
        name: "session_secure_cookie",
        reason: ExclusionReason::Credential,
    },
    Exclusion {
        name: "setup_code",
        reason: ExclusionReason::Credential,
    },
    Exclusion {
        name: "user.password_hash",
        reason: ExclusionReason::Credential,
    },
];

/// Stored columns that travel as units rather than in a document:
/// a finalized version's canonical bytes are its `record.json`.
pub const UNIT_COLUMNS: [(&str, &str); 1] = [("evaluation_version", "canonical_bytes")];

/// The carried table named `name`.
#[must_use]
pub fn table(name: &str) -> Option<&'static Table> {
    TABLES.iter().find(|table| table.name == name)
}

/// The exclusion named `name` (a table, or `table.column`).
#[must_use]
pub fn exclusion(name: &str) -> Option<&'static Exclusion> {
    EXCLUSIONS.iter().find(|exclusion| exclusion.name == name)
}

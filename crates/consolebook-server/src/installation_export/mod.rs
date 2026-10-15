//! The complete installation export: every finalized version as a
//! record-export unit, and every other stored table as one canonical-JSON
//! document, except credentials (ADR 0028;
//! docs/formats/installation-export.md; #98).
//!
//! `spec` is the one owning definition of what the format carries;
//! `produce` reads one committed state and writes the archive with the
//! record export's container writer and unit streaming. Verification from
//! the archive alone is `installation_verify`'s, dispatched by
//! `export_verify` on the manifest's `format`.

pub mod produce;
pub mod spec;

use serde::{Deserialize, Serialize};

use crate::record_export::UnitEntry;

pub use produce::{InstallationExport, export, export_at, export_to};

/// Manifest discriminator; never changes.
pub const INSTALLATION_FORMAT: &str = "consolebook-installation-export";
/// Bumped by any change to the manifest, the table set, or any
/// document's columns, types, or rules.
pub const INSTALLATION_FORMAT_VERSION: i64 = 1;
/// The directory holding the table documents.
pub const TABLE_DIR: &str = "tables";

/// A table document's entry name: `tables/{table}.json`.
#[must_use]
pub fn table_path(table: &str) -> String {
    format!("{TABLE_DIR}/{table}.json")
}

/// One carried table as the manifest's ledger lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableEntry {
    pub name: String,
    pub path: String,
    pub rows: i64,
    /// SHA-256 of the document's bytes, lowercase hex.
    pub sha256: String,
}

/// One table or column the format does not carry, with its reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExcludedEntry {
    pub name: String,
    pub reason: String,
}

/// The installation manifest (`manifest.json` at the root).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallationManifest {
    pub format: String,
    pub format_version: i64,
    pub installation_id: String,
    pub exported_at: i64,
    pub units: Vec<UnitEntry>,
    pub tables: Vec<TableEntry>,
    pub excluded: Vec<ExcludedEntry>,
}

/// The ledger's closed exclusion list, as the manifest states it.
#[must_use]
pub fn excluded_entries() -> Vec<ExcludedEntry> {
    spec::EXCLUSIONS
        .iter()
        .map(|exclusion| ExcludedEntry {
            name: exclusion.name.to_owned(),
            reason: exclusion.reason.as_str().to_owned(),
        })
        .collect()
}

//! `cargo xtask drill`: the HTTP side of the scripted restore drill
//! (`scripts/restore-drill/run.sh`, docs/restore-drill.md). The script runs
//! the hosts, the backups and the restore; this sets up the installation the
//! drill loses ([`seed`]), reads back what an installation holds
//! ([`collect`]), and says whether the restore brought it all back
//! ([`compare`]).
//!
//! Everything it creates is invented: the agency, the people, the dates and
//! the narratives. Passwords are made for each run, never written in the
//! source (#126); the administrator's goes from `seed` to every later
//! `collect` in an [`account::Account`] file.

pub mod account;
pub mod client;
pub mod collect;
pub mod compare;
pub mod seed;

use std::fmt;

/// Why the drill stopped: a call the server refused, a transport failure, an
/// export that doesn't hold what it should, or a file it couldn't use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrillError(pub String);

impl fmt::Display for DrillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DrillError {}

/// A JSON value as a message shows it: a string without its quotes.
pub(crate) fn shown(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// `value`'s `key`, or an error that shows what came instead.
pub(crate) fn field<'a>(
    value: &'a serde_json::Value,
    key: &str,
) -> Result<&'a serde_json::Value, DrillError> {
    value
        .get(key)
        .ok_or_else(|| DrillError(format!("expected `{key}` in {value}")))
}

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

/// `value`'s `key`, or an error that names the keys it has instead.
pub(crate) fn field<'a>(
    value: &'a serde_json::Value,
    key: &str,
) -> Result<&'a serde_json::Value, DrillError> {
    value
        .get(key)
        .ok_or_else(|| DrillError(format!("expected `{key}` in {}", described(value))))
}

/// What a JSON value is, for an error: its kind, and an object's keys, but
/// never its values. An answer can carry a credential (a new user's
/// `reset_code`), and an error is printed and logged.
pub(crate) fn described(value: &serde_json::Value) -> String {
    use serde_json::Value;
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(_) => "a boolean".to_owned(),
        Value::Number(_) => "a number".to_owned(),
        Value::String(_) => "a string".to_owned(),
        Value::Array(_) => "a list".to_owned(),
        Value::Object(map) if map.is_empty() => "an object with no keys".to_owned(),
        Value::Object(map) => {
            let keys: Vec<String> = map.keys().map(|k| format!("`{k}`")).collect();
            format!("an object with keys {}", keys.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{described, field};

    #[test]
    fn an_error_names_keys_and_kinds_never_values() {
        let created = json!({ "username": "avery.admin", "reset_code": "R3SET-C0DE" });
        let why = field(&created, "id").unwrap_err().0;
        assert_eq!(
            why,
            "expected `id` in an object with keys `reset_code`, `username`"
        );
        for value in [json!("R3SET-C0DE"), json!(["R3SET-C0DE"]), json!({})] {
            let why = field(&value, "id").unwrap_err().0;
            assert!(!why.contains("R3SET"), "{why}");
        }
        assert_eq!(described(&json!(null)), "null");
        assert_eq!(described(&json!(7)), "a number");
        assert_eq!(described(&json!({})), "an object with no keys");
    }
}

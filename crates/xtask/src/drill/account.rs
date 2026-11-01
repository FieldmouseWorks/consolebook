//! An invented account's sign-in, made for one run of the drill.

use std::fmt;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::DrillError;

/// A username and the password made for it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Account {
    pub username: String,
    pub passphrase: String,
}

impl fmt::Debug for Account {
    /// Never prints the password.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Account")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

impl Account {
    /// `username` with a new password: `invented-` and 32 hex digits from OS
    /// randomness, 41 bytes, inside the product's length rules and never a
    /// username.
    pub fn invented(username: &str) -> Result<Self, DrillError> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes)
            .map_err(|e| DrillError(format!("no OS randomness for a password: {e}")))?;
        Ok(Self {
            username: username.to_owned(),
            passphrase: format!("invented-{:032x}", u128::from_le_bytes(bytes)),
        })
    }

    /// Writes it to `path`, a new file only its owner can read.
    pub fn write(&self, path: &Path) -> Result<(), DrillError> {
        let failed = |e: std::io::Error| DrillError(format!("{}: {e}", path.display()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(failed)?;
        let text = serde_json::to_string(self).map_err(|e| DrillError(e.to_string()))?;
        file.write_all(text.as_bytes())
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all())
            .map_err(failed)
    }

    /// The account [`Account::write`] wrote to `path`.
    pub fn read(path: &Path) -> Result<Self, DrillError> {
        let text =
            std::fs::read(path).map_err(|e| DrillError(format!("{}: {e}", path.display())))?;
        serde_json::from_slice(&text).map_err(|e| DrillError(format!("{}: {e}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::Account;

    #[test]
    fn an_account_goes_to_a_new_private_file_and_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("account.json");
        let account = Account::invented("avery.admin").expect("account");
        assert_eq!(account.passphrase.len(), 41);
        assert!(account.passphrase.starts_with("invented-"));
        assert_ne!(Account::invented("avery.admin").expect("account"), account);
        account.write(&path).expect("write");
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(Account::read(&path).expect("read"), account);
        // A second run never overwrites the sign-in of the first.
        let again = Account::invented("avery.admin").expect("account");
        assert!(again.write(&path).is_err());
        assert_eq!(Account::read(&path).expect("read"), account);
        assert!(!format!("{account:?}").contains(&account.passphrase));
    }

    #[test]
    fn an_account_file_with_more_than_a_sign_in_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("account.json");
        std::fs::write(
            &path,
            r#"{"username":"avery.admin","passphrase":"","role":"admin"}"#,
        )
        .expect("write");
        assert!(Account::read(&path).is_err());
    }
}

//! Passwords and backend secrets in the OS credential store (ADR-0009).

use keyring_core::{Entry, Error};

/// The keyring service every salvajocs secret is stored under.
const SERVICE: &str = "salvajocs";

/// Errors from the credential store.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// The store is missing, locked or failed; callers fall back to asking.
    #[error("credential store error: {0}")]
    Store(Error),
    /// The value exceeds what the store can hold (Windows: 2560 bytes); callers treat the secret
    /// as "ask every session".
    #[error("secret is too long for the credential store")]
    TooLong,
}

impl From<Error> for SecretError {
    fn from(err: Error) -> Self {
        match err {
            Error::TooLong(..) => Self::TooLong,
            err => Self::Store(err),
        }
    }
}

/// Which secret of a repository: its password or one secret backend option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretKey {
    /// The repository password.
    Password,
    /// The value of the backend option with this key.
    Option(String),
}

impl SecretKey {
    /// The keyring account: the scope, plus `#<key>` for an option. A scope is JSON, so the
    /// first `#` after its closing bracket is unambiguous.
    fn account(&self, scope: &str) -> String {
        match self {
            Self::Password => scope.to_string(),
            Self::Option(key) => format!("{scope}#{key}"),
        }
    }
}

/// Installs the platform store on first use; other platforms have none and fail with
/// `NoDefaultStore`.
fn entry(scope: &str, key: &SecretKey) -> Result<Entry, SecretError> {
    if keyring_core::get_default_store().is_none() {
        #[cfg(target_os = "linux")]
        keyring_core::set_default_store(dbus_secret_service_keyring_store::Store::new()?);
        #[cfg(windows)]
        keyring_core::set_default_store(windows_native_keyring_store::Store::new()?);
    }
    Ok(Entry::new(SERVICE, &key.account(scope))?)
}

/// Reads a secret of the repository identified by `scope` (see `AppConfig::secret_scope`);
/// `None` when the store has no such entry.
pub fn get(scope: &str, key: &SecretKey) -> Result<Option<String>, SecretError> {
    match entry(scope, key)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(Error::NoEntry) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// Stores a secret, replacing any previous value.
pub fn set(scope: &str, key: &SecretKey, value: &str) -> Result<(), SecretError> {
    Ok(entry(scope, key)?.set_password(value)?)
}

/// Removes a secret; removing one that does not exist succeeds.
pub fn delete(scope: &str, key: &SecretKey) -> Result<(), SecretError> {
    match entry(scope, key)?.delete_credential() {
        Ok(()) | Err(Error::NoEntry) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
mod tests;

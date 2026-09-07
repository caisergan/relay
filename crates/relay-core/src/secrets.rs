//! Where passwords and key passphrases live: the OS keychain, and nowhere else.
//!
//! `servers.json` holds a server's identity and never its secret — phase 5 has a test
//! that greps the file to keep it that way. The keychain is addressed by the server's
//! UUID rather than by `user@host`, so renaming a server or moving it to a new address
//! cannot orphan its credential or, worse, point it at somebody else's.
//!
//! Every call here blocks: macOS Keychain and Windows Credential Manager are
//! synchronous C APIs that can also show a system prompt. They run on a blocking
//! worker, never on the async runtime.

use async_trait::async_trait;

use crate::model::ServerConfig;
use crate::protocol::SecretSource;

/// The keychain service name. Stable across versions: changing it would strand every
/// credential a user has already saved.
pub const SERVICE: &str = "Relay";

/// Which secret an entry holds. The account key is `{server uuid}:{kind}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
    Password,
    Passphrase,
}

impl SecretKind {
    const fn suffix(self) -> &'static str {
        match self {
            SecretKind::Password => "password",
            SecretKind::Passphrase => "passphrase",
        }
    }
}

pub fn account(server: uuid::Uuid, kind: SecretKind) -> String {
    format!("{server}:{}", kind.suffix())
}

/// The real thing: macOS Keychain, Windows Credential Manager, Secret Service on Linux.
pub struct KeyringSecrets;

impl KeyringSecrets {
    pub fn new() -> Self {
        Self
    }

    /// Store a secret. Called by the shell when a person ticks "remember".
    pub async fn store(
        server: uuid::Uuid,
        kind: SecretKind,
        value: String,
    ) -> Result<(), keyring::Error> {
        let account = account(server, kind);
        blocking(move || keyring::Entry::new(SERVICE, &account)?.set_password(&value)).await
    }

    /// Remove a secret. Best effort by design: a server being deleted from the sidebar
    /// must not fail because a credential was already gone.
    pub async fn forget(server: uuid::Uuid, kind: SecretKind) {
        let account = account(server, kind);
        let outcome: Result<(), keyring::Error> =
            blocking(move || keyring::Entry::new(SERVICE, &account)?.delete_credential()).await;
        match outcome {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(err) => tracing::warn!(%err, "could not remove a stored credential"),
        }
    }

    async fn read(server: uuid::Uuid, kind: SecretKind) -> Option<String> {
        let account = account(server, kind);
        let outcome: Result<String, keyring::Error> =
            blocking(move || keyring::Entry::new(SERVICE, &account)?.get_password()).await;
        match outcome {
            Ok(value) => Some(value),
            // Nothing saved is the ordinary case: the caller falls back to prompting.
            Err(keyring::Error::NoEntry) => None,
            Err(err) => {
                // A locked keychain, a denied prompt, no store at all. None of these
                // are fatal — they mean "ask the person instead" — but they are worth
                // a line, because otherwise a keychain that never unlocks looks like a
                // server that keeps forgetting the password.
                tracing::warn!(%err, "keychain unavailable; falling back to prompting");
                None
            }
        }
    }
}

impl Default for KeyringSecrets {
    fn default() -> Self {
        Self::new()
    }
}

/// Run a blocking keychain call off the runtime.
///
/// A panic inside `spawn_blocking` (or a runtime shutting down under us) surfaces as a
/// join error, which is treated as "no credential" rather than propagated: there is no
/// version of that failure where the right move is to abandon the connection instead
/// of asking the user.
async fn blocking<T, F>(f: F) -> Result<T, keyring::Error>
where
    F: FnOnce() -> Result<T, keyring::Error> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(result) => result,
        Err(err) => {
            tracing::warn!(%err, "the keychain worker did not finish");
            Err(keyring::Error::NoEntry)
        }
    }
}

#[async_trait]
impl SecretSource for KeyringSecrets {
    async fn password(&self, server: &ServerConfig) -> Option<String> {
        Self::read(server.id, SecretKind::Password).await
    }

    async fn passphrase(&self, server: &ServerConfig) -> Option<String> {
        Self::read(server.id, SecretKind::Passphrase).await
    }
}

/// An in-memory source for tests and for `Ask` authentication, where nothing is stored.
#[derive(Default)]
pub struct MemorySecrets {
    entries: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

impl MemorySecrets {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, server: uuid::Uuid, kind: SecretKind, value: impl Into<String>) {
        self.entries
            .lock()
            .expect("memory secrets poisoned")
            .insert(account(server, kind), value.into());
    }

    fn get(&self, server: uuid::Uuid, kind: SecretKind) -> Option<String> {
        self.entries
            .lock()
            .expect("memory secrets poisoned")
            .get(&account(server, kind))
            .cloned()
    }
}

#[async_trait]
impl SecretSource for MemorySecrets {
    async fn password(&self, server: &ServerConfig) -> Option<String> {
        self.get(server.id, SecretKind::Password)
    }

    async fn passphrase(&self, server: &ServerConfig) -> Option<String> {
        self.get(server.id, SecretKind::Passphrase)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_are_keyed_by_identity_not_by_address() {
        let server = uuid::Uuid::new_v4();
        assert_eq!(
            account(server, SecretKind::Password),
            format!("{server}:password")
        );
        assert_ne!(
            account(server, SecretKind::Password),
            account(server, SecretKind::Passphrase),
            "a key passphrase and a login password are different secrets"
        );
    }
}

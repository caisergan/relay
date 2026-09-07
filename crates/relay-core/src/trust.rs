//! Which host keys this installation has agreed to.
//!
//! `known_hosts` in spirit, JSON on disk, one entry per `host:port`. The store answers
//! one question — *have we seen this endpoint, and is this the same key?* — because
//! those are three different sheets in the design: no prompt, a first-contact prompt,
//! and a red changed-key prompt. A store that only said yes or no would collapse the
//! third into the second, which is the case that actually matters.
//!
//! Writes are atomic (temp file plus rename). A trust file truncated by a crash would
//! silently un-pin every endpoint, and the next connection would look like a first
//! contact rather than the warning it should be.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{EngineError, Result};

/// What the store knows about one endpoint's key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustEntry {
    pub algo: String,
    /// The `SHA256:…` fingerprint exactly as OpenSSH renders it, so a person can
    /// compare it with `ssh-keyscan` output without converting anything.
    pub sha256: String,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

/// The answer a connecting session needs before it decides whether to ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustDecision {
    /// Pinned, and the key matches. Connect without bothering anyone.
    Known,
    /// Never seen. First contact: ask, and offer to remember.
    Unknown,
    /// Pinned, and the key is *different*. Ask, in red, with the previous fingerprint.
    Changed,
}

pub struct TrustStore {
    path: PathBuf,
    entries: Mutex<HashMap<String, TrustEntry>>,
}

impl TrustStore {
    /// Load, tolerating both a missing and an unreadable file.
    ///
    /// A store that refused to open would make a corrupted file mean "cannot connect
    /// to anything". Starting empty instead means every endpoint reads as first
    /// contact — the user is asked again rather than silently trusted, which is the
    /// safe direction to fail in.
    pub fn load(path: PathBuf) -> Self {
        let entries = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<HashMap<String, TrustEntry>>(&bytes) {
                Ok(entries) => entries,
                Err(err) => {
                    tracing::warn!(?path, %err, "trust store is unreadable; starting empty");
                    HashMap::new()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(err) => {
                tracing::warn!(?path, %err, "trust store could not be read; starting empty");
                HashMap::new()
            }
        };
        Self {
            path,
            entries: Mutex::new(entries),
        }
    }

    /// An in-memory store, for tests and for a build with nowhere to write.
    pub fn ephemeral() -> Self {
        Self {
            path: PathBuf::new(),
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, endpoint: &str) -> Option<TrustEntry> {
        self.lock().get(endpoint).cloned()
    }

    pub fn check(&self, endpoint: &str, sha256: &str) -> TrustDecision {
        match self.lock().get(endpoint) {
            None => TrustDecision::Unknown,
            Some(entry) if entry.sha256 == sha256 => TrustDecision::Known,
            Some(_) => TrustDecision::Changed,
        }
    }

    /// Pin a key, replacing whatever was there.
    ///
    /// `first_seen` survives a re-pin: how long an endpoint has been known is worth
    /// showing, and accepting a changed key does not restart that history.
    pub fn remember(&self, endpoint: &str, algo: &str, sha256: &str) -> Result<()> {
        {
            let mut entries = self.lock();
            let now = Utc::now();
            let first_seen = entries.get(endpoint).map(|e| e.first_seen).unwrap_or(now);
            entries.insert(
                endpoint.to_string(),
                TrustEntry {
                    algo: algo.to_string(),
                    sha256: sha256.to_string(),
                    first_seen,
                    last_seen: now,
                },
            );
        }
        self.persist()
    }

    /// Record that a pinned endpoint was reached again. Failing to write this is not
    /// worth surfacing: the pin itself is intact and the connection is fine.
    pub fn touch(&self, endpoint: &str) {
        {
            let mut entries = self.lock();
            let Some(entry) = entries.get_mut(endpoint) else {
                return;
            };
            entry.last_seen = Utc::now();
        }
        if let Err(err) = self.persist() {
            tracing::debug!(%err, "could not record the last-seen time");
        }
    }

    pub fn forget(&self, endpoint: &str) -> Result<()> {
        self.lock().remove(endpoint);
        self.persist()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, TrustEntry>> {
        self.entries.lock().expect("trust store poisoned")
    }

    fn persist(&self) -> Result<()> {
        if self.path.as_os_str().is_empty() {
            return Ok(()); // ephemeral
        }
        let json = {
            let entries = self.lock();
            serde_json::to_vec_pretty(&*entries)
                .map_err(|e| EngineError::protocol(format!("serialising the trust store: {e}")))?
        };
        write_atomically(&self.path, &json)
    }
}

/// Write via a sibling temp file and a rename.
///
/// Truncate-then-write is not acceptable here: a crash in the middle leaves an empty
/// trust file, every endpoint reverts to first contact, and the one prompt that means
/// "something is wrong" never appears again.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| EngineError::from_io(parent, &e))?;
    }
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, bytes).map_err(|e| EngineError::from_io(&temp, &e))?;
    std::fs::rename(&temp, path).map_err(|e| EngineError::from_io(path, &e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (TrustStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        (TrustStore::load(dir.path().join("trust.json")), dir)
    }

    #[test]
    fn an_unseen_endpoint_is_first_contact() {
        let (store, _dir) = store();
        assert_eq!(store.check("h:22", "SHA256:aaa"), TrustDecision::Unknown);
    }

    #[test]
    fn a_pinned_key_round_trips_through_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("trust.json");

        let store = TrustStore::load(path.clone());
        store
            .remember("h:22", "ssh-ed25519", "SHA256:aaa")
            .expect("write");
        assert_eq!(store.check("h:22", "SHA256:aaa"), TrustDecision::Known);

        // A second process, reading what the first wrote.
        let reopened = TrustStore::load(path);
        assert_eq!(reopened.check("h:22", "SHA256:aaa"), TrustDecision::Known);
        assert_eq!(
            reopened.get("h:22").map(|e| e.algo),
            Some("ssh-ed25519".to_string())
        );
    }

    #[test]
    fn a_different_key_on_a_pinned_endpoint_is_changed_not_unknown() {
        let (store, _dir) = store();
        store
            .remember("h:22", "ssh-ed25519", "SHA256:aaa")
            .expect("write");
        assert_eq!(
            store.check("h:22", "SHA256:bbb"),
            TrustDecision::Changed,
            "this is the sheet the design draws in red; it must not read as first contact"
        );
    }

    #[test]
    fn re_pinning_keeps_the_date_the_endpoint_was_first_trusted() {
        let (store, _dir) = store();
        store.remember("h:22", "ssh-ed25519", "SHA256:aaa").unwrap();
        let first = store.get("h:22").expect("entry").first_seen;

        store.remember("h:22", "ssh-ed25519", "SHA256:bbb").unwrap();
        let after = store.get("h:22").expect("entry");
        assert_eq!(after.first_seen, first, "history is not restarted");
        assert_eq!(after.sha256, "SHA256:bbb");
    }

    #[test]
    fn an_unreadable_store_fails_towards_asking_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("trust.json");
        std::fs::write(&path, b"{ this is not json").expect("write junk");

        let store = TrustStore::load(path);
        assert_eq!(
            store.check("h:22", "SHA256:aaa"),
            TrustDecision::Unknown,
            "a corrupt file must not silently trust anything"
        );
    }

    #[test]
    fn forgetting_an_endpoint_removes_the_pin() {
        let (store, _dir) = store();
        store.remember("h:22", "ssh-ed25519", "SHA256:aaa").unwrap();
        store.forget("h:22").unwrap();
        assert_eq!(store.check("h:22", "SHA256:aaa"), TrustDecision::Unknown);
    }
}

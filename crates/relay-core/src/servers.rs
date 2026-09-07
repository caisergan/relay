//! The saved server list, on disk.
//!
//! `servers.json` holds identity and preferences: name, host, port, username, which
//! authentication method to use. It never holds a password or a passphrase — those are
//! in the OS keychain, keyed by the server's UUID (see [`crate::secrets`]). Phase 5
//! greps this file in a test to keep the rule enforced rather than merely intended.
//!
//! Writes are atomic, for the same reason the trust store's are: a half-written file is
//! a lost server list.

use std::path::PathBuf;
use std::sync::Mutex;

use crate::error::{EngineError, Result};
use crate::model::{ServerConfig, ServerId};

pub struct ServerStore {
    path: PathBuf,
    servers: Mutex<Vec<ServerConfig>>,
}

impl ServerStore {
    /// Load, tolerating a missing or unreadable file.
    ///
    /// An empty sidebar is recoverable — the connect screen still works, and the file
    /// is rewritten on the next save. Refusing to start would not be.
    pub fn load(path: PathBuf) -> Self {
        let servers = match std::fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice::<Vec<ServerConfig>>(&bytes).unwrap_or_else(|err| {
                    tracing::warn!(?path, %err, "server list is unreadable; starting empty");
                    Vec::new()
                })
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(err) => {
                tracing::warn!(?path, %err, "server list could not be read; starting empty");
                Vec::new()
            }
        };
        Self {
            path,
            servers: Mutex::new(servers),
        }
    }

    pub fn ephemeral() -> Self {
        Self {
            path: PathBuf::new(),
            servers: Mutex::new(Vec::new()),
        }
    }

    pub fn list(&self) -> Vec<ServerConfig> {
        self.lock().clone()
    }

    pub fn get(&self, id: ServerId) -> Option<ServerConfig> {
        self.lock().iter().find(|s| s.id == id).cloned()
    }

    /// Insert or replace by id, and return what was stored.
    pub fn save(&self, config: ServerConfig) -> Result<ServerConfig> {
        {
            let mut servers = self.lock();
            match servers.iter_mut().find(|s| s.id == config.id) {
                Some(existing) => *existing = config.clone(),
                None => servers.push(config.clone()),
            }
        }
        self.persist()?;
        Ok(config)
    }

    pub fn delete(&self, id: ServerId) -> Result<()> {
        self.lock().retain(|s| s.id != id);
        self.persist()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<ServerConfig>> {
        self.servers.lock().expect("server store poisoned")
    }

    fn persist(&self) -> Result<()> {
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        let json = {
            let servers = self.lock();
            serde_json::to_vec_pretty(&*servers)
                .map_err(|e| EngineError::protocol(format!("serialising the server list: {e}")))?
        };
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| EngineError::from_io(parent, &e))?;
        }
        let temp = self.path.with_extension("json.tmp");
        std::fs::write(&temp, &json).map_err(|e| EngineError::from_io(&temp, &e))?;
        std::fs::rename(&temp, &self.path).map_err(|e| EngineError::from_io(&self.path, &e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AuthMethod, Proto};

    fn config(name: &str) -> ServerConfig {
        ServerConfig {
            id: uuid::Uuid::new_v4(),
            name: name.to_string(),
            host: "example.test".into(),
            port: 22,
            proto: Proto::Sftp,
            username: "ada".into(),
            auth: AuthMethod::Password,
            color: None,
            group: None,
            bookmarks: Vec::new(),
            initial_remote_path: None,
        }
    }

    #[test]
    fn servers_round_trip_through_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("servers.json");

        let store = ServerStore::load(path.clone());
        let saved = store.save(config("Staging")).expect("save");
        assert_eq!(ServerStore::load(path).list(), vec![saved]);
    }

    #[test]
    fn saving_the_same_id_replaces_rather_than_duplicates() {
        let store = ServerStore::ephemeral();
        let mut server = config("Staging");
        store.save(server.clone()).unwrap();
        server.name = "Production".into();
        store.save(server.clone()).unwrap();

        let list = store.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Production");
    }

    /// Every object key in a JSON value, at any depth.
    fn keys(value: &serde_json::Value, into: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, child) in map {
                    into.push(key.clone());
                    keys(child, into);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|i| keys(i, into)),
            _ => {}
        }
    }

    #[test]
    fn no_field_of_a_saved_server_can_hold_a_credential() {
        // The rule phase 5 audits, checked where it could first be broken: a secret can
        // only reach this file by someone adding a field to `ServerConfig`.
        //
        // Field *names*, not the serialised text. `"auth": {"kind": "password"}` names
        // an authentication method and is exactly what belongs here; a key called
        // `password` never would be.
        let server = config("Staging");
        let json = serde_json::to_value(&server).expect("serialise");
        let mut found = Vec::new();
        keys(&json, &mut found);

        for key in &found {
            let lowered = key.to_lowercase();
            assert!(
                !["password", "passphrase", "secret", "token", "key"].contains(&lowered.as_str()),
                "servers.json must never carry a credential field, found `{key}` in {found:?}"
            );
        }

        // And the saved auth method really is only a discriminant.
        assert_eq!(json["auth"], serde_json::json!({ "kind": "password" }));
    }

    #[test]
    fn deleting_removes_only_the_named_server() {
        let store = ServerStore::ephemeral();
        let a = store.save(config("A")).unwrap();
        let b = store.save(config("B")).unwrap();
        store.delete(a.id).unwrap();
        assert_eq!(store.list(), vec![b]);
    }
}

//! User settings the engine and the interface both need to agree on.
//!
//! Only what phase 5 lists as essential for the SFTP release. Editor and preview
//! preferences arrive with the features that use them.
//!
//! Persisted in `settings.json` beside the server list, and for the same reasons: the
//! write is atomic because a half-written file is a lost configuration, and an
//! unreadable one starts from defaults rather than refusing to open the app. Losing a
//! theme preference is a nuisance; being unable to start is not.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::{EngineError, Result};
use crate::interact::ConflictAction;

/// The concurrency slider's range, from the design's settings sheet.
pub const MIN_CONCURRENCY: u8 = 1;
pub const MAX_CONCURRENCY: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    Light,
    Dark,
    /// Follow the operating system.
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum Density {
    Comfortable,
    Compact,
}

/// What to do when a transfer's destination already exists. `None` means ask.
pub type DefaultConflict = Option<ConflictAction>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub theme: Theme,
    pub density: Density,
    /// Simultaneous transfers, 1–8.
    pub concurrency: u8,
    /// `None` opens the conflict sheet every time.
    pub default_conflict: DefaultConflict,
    pub download_dir: Option<PathBuf>,
    /// Show dotfiles and Windows-hidden files in both panes.
    pub show_hidden: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            density: Density::Comfortable,
            concurrency: 3,
            default_conflict: None,
            download_dir: None,
            show_hidden: false,
        }
    }
}

impl Settings {
    /// Clamp anything a stale config file or a buggy client might send.
    pub fn normalised(mut self) -> Self {
        self.concurrency = self.concurrency.clamp(MIN_CONCURRENCY, MAX_CONCURRENCY);
        self
    }
}

/// `settings.json`, with the current values in front of it.
pub struct SettingsStore {
    path: PathBuf,
    settings: Mutex<Settings>,
}

impl SettingsStore {
    /// Load, tolerating a missing or unreadable file.
    ///
    /// Normalised on the way in as well as on the way out: a hand-edited file with a
    /// concurrency of 40 must not be able to open forty connections just because the
    /// slider that clamps it was never touched.
    pub fn load(path: PathBuf) -> Self {
        let settings = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Settings>(&bytes)
                .map(Settings::normalised)
                .unwrap_or_else(|err| {
                    tracing::warn!(?path, %err, "settings are unreadable; using defaults");
                    Settings::default()
                }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Settings::default(),
            Err(err) => {
                tracing::warn!(?path, %err, "settings could not be read; using defaults");
                Settings::default()
            }
        };
        Self {
            path,
            settings: Mutex::new(settings),
        }
    }

    /// In-memory only, for tests.
    pub fn ephemeral() -> Self {
        Self {
            path: PathBuf::new(),
            settings: Mutex::new(Settings::default()),
        }
    }

    pub fn get(&self) -> Settings {
        self.settings.lock().expect("settings poisoned").clone()
    }

    /// Store and persist, returning what was actually stored after clamping.
    ///
    /// A failed write is reported rather than swallowed: a person who changes a
    /// setting and is told nothing has every reason to believe it was saved.
    pub fn set(&self, settings: Settings) -> Result<Settings> {
        let normalised = settings.normalised();
        *self.settings.lock().expect("settings poisoned") = normalised.clone();
        self.persist(&normalised)?;
        Ok(normalised)
    }

    fn persist(&self, settings: &Settings) -> Result<()> {
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        let json = serde_json::to_vec_pretty(settings)
            .map_err(|e| EngineError::protocol(format!("serialising settings: {e}")))?;
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

    #[test]
    fn concurrency_is_clamped_to_the_designed_range() {
        let settings = Settings {
            concurrency: 99,
            ..Settings::default()
        }
        .normalised();
        assert_eq!(settings.concurrency, MAX_CONCURRENCY);
        let settings = Settings {
            concurrency: 0,
            ..Settings::default()
        }
        .normalised();
        assert_eq!(settings.concurrency, MIN_CONCURRENCY);
    }

    #[test]
    fn settings_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        let store = SettingsStore::load(path.clone());
        store
            .set(Settings {
                theme: Theme::Dark,
                concurrency: 6,
                default_conflict: Some(ConflictAction::Overwrite),
                ..Settings::default()
            })
            .unwrap();

        let reopened = SettingsStore::load(path);
        assert_eq!(reopened.get().theme, Theme::Dark);
        assert_eq!(reopened.get().concurrency, 6);
        assert_eq!(
            reopened.get().default_conflict,
            Some(ConflictAction::Overwrite)
        );
    }

    /// The file is a file: someone can edit it, and an old build can write a value a
    /// new one considers out of range.
    #[test]
    fn a_hand_edited_file_is_clamped_on_the_way_in() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            br#"{"theme":"system","density":"comfortable","concurrency":40,
                 "defaultConflict":null,"downloadDir":null,"showHidden":false}"#,
        )
        .unwrap();

        assert_eq!(
            SettingsStore::load(path).get().concurrency,
            MAX_CONCURRENCY,
            "a file cannot ask for more connections than the slider allows"
        );
    }

    #[test]
    fn unreadable_settings_start_from_defaults_rather_than_refusing_to_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"{ this is not json").unwrap();
        assert_eq!(SettingsStore::load(path).get(), Settings::default());
    }
}

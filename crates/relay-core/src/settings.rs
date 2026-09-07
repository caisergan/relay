//! User settings the engine and the interface both need to agree on.
//!
//! Only what phase 5 lists as essential for the SFTP release. Editor and preview
//! preferences arrive with the features that use them.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use specta::Type;

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
}

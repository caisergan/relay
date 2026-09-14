//! A small JSON file the interface records into as things change: the open tabs, the
//! arrangement of the window.
//!
//! Recorded as it changes rather than at exit, because an exit is not something to rely
//! on reaching — a force quit, a crash or a power cut runs no shutdown hook. And frozen
//! at the start of a clean exit, because the exit itself tears the interface down: every
//! tab it closes would otherwise be recorded as the state to come back to, and that would
//! be the last thing written.
//!
//! A missing or unreadable file is the default value, never a reason not to start.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{EngineError, Result};

pub struct Recorded<T> {
    path: PathBuf,
    current: Mutex<T>,
    frozen: AtomicBool,
}

impl<T> Recorded<T>
where
    T: Serialize + DeserializeOwned + Default + Clone + PartialEq,
{
    pub fn load(path: PathBuf) -> Self {
        let value = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<T>(&bytes).unwrap_or_else(|err| {
                tracing::warn!(?path, %err, "a recorded file is unreadable; starting from defaults");
                T::default()
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => T::default(),
            Err(err) => {
                tracing::warn!(?path, %err, "a recorded file could not be read; starting from defaults");
                T::default()
            }
        };
        Self {
            path,
            current: Mutex::new(value),
            frozen: AtomicBool::new(false),
        }
    }

    /// In-memory only, for tests.
    pub fn ephemeral() -> Self {
        Self::load(PathBuf::new())
    }

    pub fn get(&self) -> T {
        self.current
            .lock()
            .expect("recorded value poisoned")
            .clone()
    }

    /// Record a new value, unless the app is on its way out.
    pub fn set(&self, value: T) -> Result<()> {
        self.update(|current| *current = value)
    }

    /// Change part of the value and record the result.
    ///
    /// An unchanged value is not rewritten: the interface calls this far more often than
    /// anything a restore reads actually changes. The lock is held across the write, so two
    /// calls cannot interleave on the temporary file, and neither can lose the other's part.
    pub fn update(&self, change: impl FnOnce(&mut T)) -> Result<()> {
        if self.frozen.load(Ordering::SeqCst) {
            return Ok(());
        }
        let mut current = self.current.lock().expect("recorded value poisoned");
        let mut next = current.clone();
        change(&mut next);
        if next == *current {
            return Ok(());
        }
        self.persist(&next)?;
        *current = next;
        Ok(())
    }

    /// Stop recording. Called as a clean exit begins; see the module docs.
    pub fn freeze(&self) {
        self.frozen.store(true, Ordering::SeqCst);
    }

    fn persist(&self, value: &T) -> Result<()> {
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        let json = serde_json::to_vec_pretty(value).map_err(|e| {
            EngineError::protocol(format!("serialising {}: {e}", self.path.display()))
        })?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| EngineError::from_io(parent, &e))?;
        }
        let temp = self.path.with_extension("json.tmp");
        std::fs::write(&temp, &json).map_err(|e| EngineError::from_io(&temp, &e))?;
        std::fs::rename(&temp, &self.path).map_err(|e| EngineError::from_io(&self.path, &e))
    }
}

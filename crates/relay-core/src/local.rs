//! The local pane, enumerated in Rust.
//!
//! The two panes must agree about what a file *is* — hidden-ness, size, modification
//! time, sort order — so the local side does not go through a browser API. It also
//! means the same rules apply on macOS and Windows, where "hidden" means two different
//! things (a leading dot, and a filesystem attribute).
//!
//! Every function here blocks. Callers run them on a blocking worker and drop the
//! result if the user has already navigated away.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::error::{EngineError, Result};
use crate::model::{FileKind, LocalEntry};
use crate::wire::Bytes;

/// Where a new session's local pane starts.
pub fn default_dir() -> PathBuf {
    home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

/// Roots for the breadcrumb's root menu: volumes on macOS, drive letters on Windows.
pub fn roots() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut roots = vec![PathBuf::from("/")];
        if let Ok(entries) = std::fs::read_dir("/Volumes") {
            roots.extend(entries.flatten().map(|e| e.path()));
        }
        roots
    }
    #[cfg(windows)]
    {
        // Probing beats a Win32 call here: no extra dependency, and a drive that
        // cannot be stat'd is one the pane could not open anyway.
        (b'A'..=b'Z')
            .map(|letter| PathBuf::from(format!("{}:\\", letter as char)))
            .filter(|p| p.exists())
            .collect()
    }
    #[cfg(all(not(target_os = "macos"), not(windows)))]
    {
        vec![PathBuf::from("/")]
    }
}

/// Directories first, then names, case-insensitively — matching the remote pane.
pub fn list_dir(path: &Path) -> Result<Vec<LocalEntry>> {
    let read = std::fs::read_dir(path).map_err(|e| EngineError::from_io(path, &e))?;

    let mut entries = Vec::new();
    for entry in read {
        // One unreadable entry must not fail the whole listing.
        let Ok(entry) = entry else { continue };
        let entry_path = entry.path();
        let Some(name) = entry_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
        else {
            continue;
        };

        // `symlink_metadata` first: a broken link should still appear in the pane.
        let meta = match std::fs::symlink_metadata(&entry_path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };

        let (kind, target_kind) = if meta.is_symlink() {
            let target = std::fs::metadata(&entry_path)
                .ok()
                .map(|m| kind_of(m.is_dir()));
            (FileKind::Symlink, target)
        } else {
            (kind_of(meta.is_dir()), None)
        };

        entries.push(LocalEntry {
            name: name.clone(),
            path: entry_path,
            kind,
            target_kind,
            size: Bytes(if meta.is_dir() { 0 } else { meta.len() }),
            modified: meta.modified().ok().map(DateTime::<Utc>::from),
            hidden: is_hidden(&name, &meta),
            readonly: meta.permissions().readonly(),
        });
    }

    entries.sort_by(|a, b| {
        let a_dir = a.kind == FileKind::Dir || a.target_kind == Some(FileKind::Dir);
        let b_dir = b.kind == FileKind::Dir || b.target_kind == Some(FileKind::Dir);
        b_dir
            .cmp(&a_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

fn kind_of(is_dir: bool) -> FileKind {
    if is_dir {
        FileKind::Dir
    } else {
        FileKind::File
    }
}

fn is_hidden(name: &str, meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0 {
            return true;
        }
    }
    #[cfg(not(windows))]
    {
        let _ = meta;
    }
    name.starts_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_directories_before_files_and_marks_dotfiles() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("zebra")).unwrap();
        std::fs::write(dir.path().join("alpha.txt"), b"hi").unwrap();
        std::fs::write(dir.path().join(".hidden"), b"x").unwrap();

        let entries = list_dir(dir.path()).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names[0], "zebra", "directories sort first: {names:?}");
        assert!(entries.iter().find(|e| e.name == ".hidden").unwrap().hidden);
        assert_eq!(
            entries.iter().find(|e| e.name == "alpha.txt").unwrap().size,
            Bytes(2)
        );
    }

    #[test]
    fn a_missing_directory_maps_to_not_found_for_the_designed_pane_state() {
        let err = list_dir(Path::new("/definitely/not/here")).unwrap_err();
        assert!(matches!(err, EngineError::NotFound { .. }), "got {err:?}");
    }

    /// The other half of the pair the breadcrumb's error panes switch on. The mapping
    /// itself is unit-tested in `error.rs`; this proves a real unreadable directory
    /// reaches it, rather than surfacing as a generic `LocalIo`.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_directory_maps_to_permission_denied() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let result = list_dir(&locked);

        // Restore first, so a failing assertion still leaves a removable directory.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

        match result {
            // The mode bits were not enforced — running as root, or a filesystem that
            // ignores them. Both happen in CI containers, and neither leaves anything
            // to assert about a denial that did not occur.
            Ok(_) => (),
            Err(err) => assert!(
                matches!(err, EngineError::PermissionDenied { .. }),
                "got {err:?}"
            ),
        }
    }

    /// `roots()` feeds the breadcrumb's root menu. Whatever it names must be somewhere
    /// the pane can actually open, or the menu offers dead entries.
    #[test]
    fn roots_are_listable_directories() {
        let roots = roots();
        assert!(!roots.is_empty(), "a machine always has at least one root");
        for root in &roots {
            assert!(root.is_dir(), "{} is not a directory", root.display());
        }
    }
}

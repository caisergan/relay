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
use crate::model::{DirSize, FileKind, LocalEntry, MEASURE_MAX_DEPTH, MEASURE_MAX_ENTRIES};
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

/// What a folder adds up to.
///
/// Breadth-first with an explicit stack rather than recursion, so a deep tree cannot
/// overflow the stack, and bounded by [`MEASURE_MAX_DEPTH`] and [`MEASURE_MAX_ENTRIES`]
/// so a right-click on `/` is a wait and not a hang.
///
/// **Symlinks are counted, not followed.** `du` behaves the same way, and for the same
/// two reasons: a link into a parent makes the walk infinite, and a link to a 40 GB
/// file elsewhere on the disk would be counted as if this folder held it.
///
/// Note that [`crate::walk`] — the walk behind a recursive *transfer* — does follow a
/// symlinked directory, so a folder full of links will transfer more than it measures.
/// The two are deliberately different: a measurement is an answer to "how much is in
/// here", where counting a link to `/usr` would be a lie, and a transfer is an
/// instruction to copy what the user pointed at.
///
/// **An unreadable subdirectory is skipped, not fatal.** A folder with one
/// permission-denied child still has a size worth reporting, and the alternative — an
/// error where a number should be — tells the user nothing about the other 99%. The
/// total is a floor either way, which is what `truncated` is for.
pub fn measure(root: &Path) -> Result<DirSize> {
    // The root itself has to be readable. Anything below it may not be.
    let read = std::fs::read_dir(root).map_err(|e| EngineError::from_io(root, &e))?;

    let mut out = DirSize::default();
    let mut seen: u32 = 0;
    let mut stack: Vec<(std::fs::ReadDir, usize)> = vec![(read, 0)];

    while let Some((mut dir, depth)) = stack.pop() {
        let mut deeper = Vec::new();
        for entry in dir.by_ref() {
            let Ok(entry) = entry else { continue };
            if seen >= MEASURE_MAX_ENTRIES {
                out.truncated = true;
                return Ok(out);
            }
            seen += 1;

            // `symlink_metadata`, so a link is measured as the link it is.
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if meta.is_dir() {
                out.folders += 1;
                if depth + 1 >= MEASURE_MAX_DEPTH {
                    out.truncated = true;
                } else if let Ok(next) = std::fs::read_dir(entry.path()) {
                    deeper.push((next, depth + 1));
                } else {
                    // Permission denied, or it went away while we walked.
                    out.truncated = true;
                }
            } else {
                out.files += 1;
                out.bytes = Bytes(out.bytes.0 + meta.len());
            }
        }
        stack.extend(deeper);
    }
    Ok(out)
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

    fn write(path: &Path, bytes: usize) {
        std::fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    #[test]
    fn measure_totals_a_tree_and_counts_what_is_in_it() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a.bin"), 100);
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        write(&dir.path().join("nested/b.bin"), 200);
        std::fs::create_dir(dir.path().join("nested/deeper")).unwrap();
        write(&dir.path().join("nested/deeper/c.bin"), 300);

        let out = measure(dir.path()).unwrap();
        assert_eq!(out.bytes.0, 600);
        assert_eq!(out.files, 3);
        assert_eq!(out.folders, 2);
        assert!(!out.truncated);
    }

    #[test]
    fn measure_of_an_empty_folder_is_zero_rather_than_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let out = measure(dir.path()).unwrap();
        assert_eq!(out.bytes.0, 0);
        assert_eq!(out.files, 0);
        assert_eq!(out.folders, 0);
        assert!(!out.truncated);
    }

    /// The folder asked about has to exist; that is the caller's error, not a zero.
    #[test]
    fn measure_of_a_missing_folder_fails() {
        let dir = tempfile::tempdir().unwrap();
        assert!(measure(&dir.path().join("nope")).is_err());
    }

    /// A link into its own parent makes a followed walk infinite. Counting the link
    /// without descending it is what `du` does, and it terminates.
    #[cfg(unix)]
    #[test]
    fn measure_counts_a_symlink_without_following_it() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("real.bin"), 50);
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        // A loop: sub/back points at the root that contains it.
        std::os::unix::fs::symlink(dir.path(), dir.path().join("sub/back")).unwrap();

        // Terminating at all is the assertion: following the link would recurse into
        // the directory that contains it, for ever.
        let out = measure(dir.path()).unwrap();
        // The link is an entry in its own right — `symlink_metadata` calls it a file,
        // and its few bytes are the path it holds, not the tree it points at.
        assert_eq!(
            out.files, 2,
            "the real file, and the link counted as an entry"
        );
        assert_eq!(out.folders, 1, "sub, but not the link inside it");
        assert!(
            out.bytes.0 >= 50 && out.bytes.0 < 1_000,
            "the real file plus the link's own few bytes, not the tree again: {}",
            out.bytes.0
        );
        assert!(!out.truncated);
    }

    /// A link to something huge elsewhere must not be billed to this folder.
    #[cfg(unix)]
    #[test]
    fn measure_does_not_bill_a_linked_file_to_this_folder() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        write(&elsewhere.path().join("big.bin"), 10_000);
        std::os::unix::fs::symlink(
            elsewhere.path().join("big.bin"),
            dir.path().join("link.bin"),
        )
        .unwrap();

        let out = measure(dir.path()).unwrap();
        assert!(
            out.bytes.0 < 10_000,
            "counted the link, not the 10 KB it points at, but got {}",
            out.bytes.0
        );
        assert_eq!(out.files, 1);
    }

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

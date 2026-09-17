//! Somewhere on this machine to put a server's file so another application can open it.
//!
//! A preview is a download with a different destination. The file is copied into a
//! folder of Relay's own inside the system's temporary directory, then handed to the
//! application the person chose. Each preview gets a folder to itself, so the file keeps
//! its own name — an editor titles its window with it and picks a syntax from the
//! extension — and two previews of `index.html` from different directories never meet.
//!
//! Nothing here is precious. The system clears its temporary directory on its own
//! schedule, and Relay clears previews older than [`KEEP_FOR`] each time it starts.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use uuid::Uuid;

use crate::error::{EngineError, Result};

/// How long a preview outlives its download. A day covers any editor left open on it
/// through an ordinary working session; a copy older than that is not being looked at.
pub const KEEP_FOR: Duration = Duration::from_secs(24 * 60 * 60);

/// Relay's previews, inside the system's temporary directory.
pub fn root() -> PathBuf {
    std::env::temp_dir().join("relay-previews")
}

/// Where a preview of a file called `name` is downloaded to: a path inside a new folder
/// under `root` that exists by the time this returns and holds nothing else.
pub fn prepare(root: &Path, name: &str) -> Result<PathBuf> {
    // One path component and no more. A server can list a name that is not one here —
    // `..`, or a backslash on Windows — and joined as given it would put the download
    // somewhere other than the folder made for it.
    let separators: &[char] = if cfg!(windows) {
        &['/', '\\', '\0']
    } else {
        &['/', '\0']
    };
    if name.is_empty() || name == "." || name == ".." || name.contains(separators) {
        return Err(EngineError::LocalIo {
            path: name.to_string(),
            message: "this name cannot be used for a file on this computer".into(),
        });
    }
    let dir = root.join(Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir).map_err(|err| EngineError::from_io(&dir, &err))?;
    Ok(dir.join(name))
}

/// Whether `path` is a preview [`prepare`] made room for and a download has since filled:
/// a file directly inside one of the folders under `root`.
///
/// The shell opens nothing else, so the command that hands a file to an application
/// cannot be pointed at the rest of the disk. Compared after resolving links on both
/// sides, because the temporary directory is itself reached through one on a Mac.
pub fn is_preview(root: &Path, path: &Path) -> bool {
    let (Ok(root), Ok(path)) = (root.canonicalize(), path.canonicalize()) else {
        return false;
    };
    path.is_file() && path.parent().and_then(Path::parent) == Some(root.as_path())
}

/// Whether `app` is something to open a preview with: an application bundle on a Mac, a
/// program file anywhere else. The setting is a file a person can edit, so it is checked
/// before anything is launched with it rather than trusted for having been picked once.
pub fn check_application(app: &Path) -> Result<()> {
    let usable = if cfg!(target_os = "macos") {
        app.extension().is_some_and(|ext| ext == "app") && app.is_dir()
    } else {
        app.is_file()
    };
    if usable {
        Ok(())
    } else {
        Err(EngineError::LocalIo {
            path: app.display().to_string(),
            message: "not an application Relay can open previews with".into(),
        })
    }
}

/// Remove previews older than `keep_for`, judged by when their folder last changed —
/// which is when its download arrived. Best effort: one that cannot be removed now is
/// left for the next launch, or for the system. Returns how many went.
pub fn prune(root: &Path, keep_for: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    let now = SystemTime::now();
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| {
            entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|at| now.duration_since(at).ok())
                .is_some_and(|age| age > keep_for)
        })
        .filter(|entry| std::fs::remove_dir_all(entry.path()).is_ok())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_preview_gets_a_folder_of_its_own_and_keeps_its_name() {
        let root = tempfile::tempdir().unwrap();

        let first = prepare(root.path(), "index.html").unwrap();
        let second = prepare(root.path(), "index.html").unwrap();

        assert_ne!(first, second, "two previews of one name share a folder");
        for path in [&first, &second] {
            assert_eq!(path.file_name().unwrap(), "index.html");
            assert!(
                path.parent().unwrap().is_dir(),
                "the folder is made up front"
            );
            assert_eq!(path.parent().unwrap().parent().unwrap(), root.path());
            assert!(!path.exists(), "the file itself is the download's to write");
        }
    }

    #[test]
    fn a_name_that_is_not_one_file_is_refused() {
        let root = tempfile::tempdir().unwrap();
        for name in ["", ".", "..", "../outside", "a/b"] {
            assert!(prepare(root.path(), name).is_err(), "{name:?} was accepted");
        }
        assert_eq!(
            std::fs::read_dir(root.path()).unwrap().count(),
            0,
            "a refused name leaves no folder behind"
        );
    }

    #[test]
    fn only_a_downloaded_preview_counts_as_one() {
        let root = tempfile::tempdir().unwrap();
        let preview = prepare(root.path(), "notes.md").unwrap();
        assert!(
            !is_preview(root.path(), &preview),
            "nothing has been downloaded yet"
        );

        std::fs::write(&preview, b"# notes").unwrap();
        assert!(is_preview(root.path(), &preview));

        let folder = preview.parent().unwrap();
        let loose = root.path().join("loose.txt");
        std::fs::write(&loose, b"").unwrap();
        let deeper = folder.join("inner");
        std::fs::create_dir(&deeper).unwrap();
        std::fs::write(deeper.join("deep.txt"), b"").unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let outside = elsewhere.path().join("x").join("secret");
        std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
        std::fs::write(&outside, b"").unwrap();

        assert!(!is_preview(root.path(), folder), "a folder is not a file");
        assert!(
            !is_preview(root.path(), &loose),
            "not inside a preview folder"
        );
        assert!(!is_preview(root.path(), &deeper.join("deep.txt")));
        assert!(!is_preview(root.path(), &outside));
        assert!(
            !is_preview(root.path(), &folder.join("..").join("..").join("loose.txt")),
            "a path is judged by where it resolves to"
        );
    }

    #[test]
    fn previews_past_their_day_are_cleared_and_recent_ones_kept() {
        let root = tempfile::tempdir().unwrap();
        let old = prepare(root.path(), "old.log").unwrap();
        std::fs::write(&old, b"").unwrap();
        let recent = prepare(root.path(), "recent.log").unwrap();
        std::fs::write(&recent, b"").unwrap();
        let two_days_ago = SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60);
        std::fs::File::open(old.parent().unwrap())
            .unwrap()
            .set_modified(two_days_ago)
            .unwrap();

        assert_eq!(prune(root.path(), KEEP_FOR), 1);
        assert!(!old.parent().unwrap().exists());
        assert!(recent.exists());
    }

    #[test]
    fn clearing_a_root_that_was_never_made_does_nothing() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(prune(&root.path().join("never"), KEEP_FOR), 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_application_is_a_bundle_on_a_mac() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Sublime Text.app");
        std::fs::create_dir(&bundle).unwrap();
        let file = dir.path().join("Fake.app");
        std::fs::write(&file, b"").unwrap();
        let folder = dir.path().join("Utilities");
        std::fs::create_dir(&folder).unwrap();

        assert!(check_application(&bundle).is_ok());
        assert!(
            check_application(&file).is_err(),
            "a file named like a bundle"
        );
        assert!(
            check_application(&folder).is_err(),
            "a folder that is not one"
        );
        assert!(check_application(&dir.path().join("Gone.app")).is_err());
    }
}

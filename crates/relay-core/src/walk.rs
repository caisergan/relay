//! Turning a folder into the files inside it.
//!
//! A recursive transfer is not one job; it is a parent that owns however many files a
//! walk finds. The walk runs in its own task and enqueues children **as it discovers
//! them**, so a 5,000-file directory starts transferring within a second rather than
//! after the whole tree has been enumerated. The parent finishes when the walk has
//! ended *and* every child is terminal — either can be last, which is why
//! [`crate::scheduler`] decides completion from both ends.
//!
//! **Browsing has to stay usable while this runs.** A session serves listings from one
//! actor, in order, so a walk that queued a thousand `list` commands would put a
//! user's next directory a thousand places back. This walk therefore keeps exactly one
//! listing in flight: it asks for a directory, awaits it, and only then asks for the
//! next. The worst a person waits is one directory listing, whatever the size of the
//! tree behind it.
//!
//! **A symlinked directory can point at its own parent.** Following one without a
//! guard produces an infinite tree, and the first sign of it is a queue that never
//! stops growing. Every directory entered is recorded by path, and the walk refuses to
//! enter one twice or to go deeper than [`MAX_DEPTH`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::error::{EngineError, Result};
use crate::job::JobKind;
use crate::model::{Direction, JobId, ServerId, SessionId};
use crate::queue::{BatchId, JobSpec};
use crate::scheduler::{Report, Scheduler};
use crate::session::SessionHandle;

/// How deep a recursive transfer will go.
///
/// Deep enough that no real directory tree reaches it, shallow enough that a cycle the
/// path guard somehow misses still terminates.
pub const MAX_DEPTH: usize = 64;

/// How many files one folder job will queue.
///
/// A cap rather than an unbounded walk: every child is a row in memory and a row in
/// SQLite, and a tree with millions of files would exhaust both long before it
/// finished. Refusing with a number in the message is more useful than dying.
pub const MAX_CHILDREN: usize = 100_000;

/// How many children are enqueued at once.
///
/// Small enough that transfers begin almost immediately, large enough that a big tree
/// is not a database transaction per file.
const CHUNK: usize = 64;

/// Everything a walk needs. The session for listings and directory creation, the
/// scheduler for the children it finds.
pub struct Walk {
    pub parent: JobId,
    pub batch: BatchId,
    pub session: Arc<SessionHandle>,
    pub session_id: SessionId,
    pub server_id: ServerId,
    pub queue: Scheduler,
    pub direction: Direction,
    /// The folder itself, on each side.
    pub remote_root: String,
    pub local_root: PathBuf,
    pub cancel: CancellationToken,
}

/// Walk a folder and queue what is in it.
///
/// Always reports an outcome for the parent: [`Report::Scanned`] when the tree was
/// enumerated, or a failure when it could not be. A walk that returned silently would
/// leave the folder in `Scanning` for the life of the app.
pub async fn run(walk: Walk) {
    let parent = walk.parent;
    let report = walk.queue.reporter();
    match enumerate(&walk).await {
        Ok(children) => {
            report
                .send(Report::Scanned {
                    job: parent,
                    children,
                })
                .await;
        }
        Err(error) => {
            report
                .send(Report::Finished {
                    job: parent,
                    result: Err(error),
                })
                .await;
        }
    }
}

/// One queue of directories still to visit, drained breadth-first.
struct Pending {
    /// Relative to the roots, so both sides are derived from one path.
    relative: PathBuf,
    depth: usize,
}

async fn enumerate(walk: &Walk) -> Result<u32> {
    let mut queued = 0usize;
    let mut seen: HashSet<String> = HashSet::new();
    let mut frontier = vec![Pending {
        relative: PathBuf::new(),
        depth: 0,
    }];
    let mut batch: Vec<JobSpec> = Vec::with_capacity(CHUNK);

    while let Some(dir) = frontier.pop() {
        if walk.cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        // A person who cancels a folder mid-walk should not watch it keep queueing.
        if walk.is_finished().await {
            return Err(EngineError::Cancelled);
        }
        if dir.depth >= MAX_DEPTH {
            continue;
        }

        let (dirs, files) = walk.read(&dir.relative).await?;

        for name in dirs {
            let child = dir.relative.join(&name);
            // A symlinked directory can point back up its own tree. Recording the path
            // rather than trusting the depth cap alone means the walk *stops* rather
            // than merely bottoming out sixty-four levels down.
            if !seen.insert(walk.remote_path(&child)) {
                continue;
            }
            // The destination directory has to exist before its children land in it,
            // and creating it here means an empty subdirectory is still transferred.
            walk.make_dir(&child).await?;
            frontier.push(Pending {
                relative: child,
                depth: dir.depth + 1,
            });
        }

        for name in files {
            if queued >= MAX_CHILDREN {
                walk.flush(&mut batch).await?;
                return Err(EngineError::Unsupported {
                    operation: format!(
                        "a folder with more than {MAX_CHILDREN} files in one transfer"
                    ),
                });
            }
            batch.push(walk.spec(&dir.relative.join(&name)));
            queued += 1;
            if batch.len() >= CHUNK {
                // Queued as they are found, so the first files are moving while the
                // rest of the tree is still being read.
                walk.flush(&mut batch).await?;
            }
        }
    }

    walk.flush(&mut batch).await?;
    Ok(queued.min(u32::MAX as usize) as u32)
}

impl Walk {
    /// The directories and files directly inside `relative`, from whichever side is
    /// the source.
    async fn read(&self, relative: &Path) -> Result<(Vec<String>, Vec<String>)> {
        match self.direction {
            Direction::Down => {
                // Quiet: a recursive transfer walks directories the user is not
                // looking at, and announcing each one replaced the listing in their
                // pane as the walk descended.
                let entries = self.session.list_quiet(&self.remote_path(relative)).await?;
                Ok(split(
                    entries
                        .into_iter()
                        .map(|entry| (entry.name.clone(), entry.is_dir())),
                ))
            }
            Direction::Up => {
                let path = self.local_root.join(relative);
                let entries = tokio::task::spawn_blocking(move || crate::local::list_dir(&path))
                    .await
                    .map_err(|err| EngineError::protocol(format!("local walk failed: {err}")))??;
                Ok(split(entries.into_iter().map(|entry| {
                    let is_dir = entry.kind == crate::model::FileKind::Dir
                        || entry.target_kind == Some(crate::model::FileKind::Dir);
                    (entry.name, is_dir)
                })))
            }
        }
    }

    /// Create the destination directory for `relative`.
    async fn make_dir(&self, relative: &Path) -> Result<()> {
        match self.direction {
            Direction::Down => {
                let path = self.local_root.join(relative);
                tokio::fs::create_dir_all(&path)
                    .await
                    .map_err(|err| EngineError::from_io(&path, &err))
            }
            Direction::Up => {
                // An existing directory is the outcome we wanted, so a failure here is
                // only interesting if the directory still is not there afterwards.
                let path = self.remote_path(relative);
                match self.session.mkdir(&path).await {
                    Ok(()) => Ok(()),
                    Err(err) => match self.session.stat(&path).await {
                        Ok(Some(entry)) if entry.is_dir() => Ok(()),
                        _ => Err(err),
                    },
                }
            }
        }
    }

    fn spec(&self, relative: &Path) -> JobSpec {
        let remote_path = self.remote_path(relative);
        JobSpec {
            session: self.session_id,
            server_id: self.server_id,
            kind: JobKind::File,
            direction: self.direction,
            item: format!("{:?}:{}", self.direction, remote_path),
            remote_path,
            local_path: self.local_root.join(relative),
            size: None,
            parent: Some(self.parent),
        }
    }

    async fn flush(&self, batch: &mut Vec<JobSpec>) -> Result<()> {
        if batch.is_empty() {
            return Ok(());
        }
        self.queue
            .enqueue(self.batch, std::mem::take(batch))
            .await
            .map(|_| ())
    }

    async fn is_finished(&self) -> bool {
        self.queue
            .state(self.parent)
            .await
            .is_none_or(|state| state.is_terminal())
    }

    fn remote_path(&self, relative: &Path) -> String {
        remote_path(&self.remote_root, relative)
    }
}

/// Remote paths are always `/`-separated, whatever the local platform spells its own
/// with. Joining a Windows `PathBuf` into one would produce backslashes the server
/// reads as part of a filename rather than as a separator.
fn remote_path(root: &str, relative: &Path) -> String {
    let mut path = root.trim_end_matches('/').to_string();
    for part in relative.components() {
        path.push('/');
        path.push_str(&part.as_os_str().to_string_lossy());
    }
    path
}

fn split(entries: impl Iterator<Item = (String, bool)>) -> (Vec<String>, Vec<String>) {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for (name, is_dir) in entries {
        // `.` and `..` are not children, and following either is how a walk finds
        // itself again with the depth counter as its only way out.
        if name == "." || name == ".." {
            continue;
        }
        if is_dir { &mut dirs } else { &mut files }.push(name);
    }
    (dirs, files)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The path arithmetic decides where every file in a recursive transfer lands, and
    /// it is pure — so it is tested without a session or a queue anywhere near it.
    #[test]
    fn remote_paths_stay_slash_separated_and_do_not_double_up() {
        assert_eq!(remote_path("/var/www", Path::new("")), "/var/www");
        assert_eq!(
            remote_path("/var/www", Path::new("assets")),
            "/var/www/assets"
        );
        assert_eq!(
            remote_path("/var/www", Path::new("assets/img")),
            "/var/www/assets/img"
        );
        assert_eq!(
            remote_path("/var/www/", Path::new("a")),
            "/var/www/a",
            "a trailing slash must not double up"
        );
        assert_eq!(remote_path("/", Path::new("etc")), "/etc");
    }

    #[test]
    fn the_current_and_parent_directories_are_not_children() {
        let (dirs, files) = split(
            [
                (".".to_string(), true),
                ("..".to_string(), true),
                ("real".to_string(), true),
                ("file.txt".to_string(), false),
            ]
            .into_iter(),
        );
        assert_eq!(
            dirs,
            ["real"],
            "following `..` is how a walk finds itself again"
        );
        assert_eq!(files, ["file.txt"]);
    }
}

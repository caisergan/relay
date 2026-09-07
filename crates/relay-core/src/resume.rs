//! Deciding whether a partial transfer may be continued.
//!
//! The rule this module exists to enforce, stated once: **nothing here trusts a
//! length.** A `.relaypart` file next to a destination proves nothing about who wrote
//! it. A partial that is exactly as long as the checkpoint says proves nothing about
//! what is in it. A source whose size has not changed proves nothing about whether it
//! is the same file — a deploy that rewrites a build artefact commonly produces a new
//! file of identical size, sometimes with an identical mtime.
//!
//! So a resume is offered only when all four of these hold:
//!
//! 1. A [`ResumeRecord`] exists, written by this engine before the first byte moved.
//!    That is the ownership claim; the filename is not.
//! 2. The source still matches the facts recorded when the partial was created.
//! 3. The partial's first `checkpoint` bytes hash to the recorded digest.
//! 4. The **source's** first `checkpoint` bytes hash to the same digest.
//!
//! Point 4 is the expensive one and the one that matters. Without it the engine would
//! be checking the partial against itself: the digest was computed from those bytes,
//! so of course they match. Reading the source prefix back is what turns "these are
//! the bytes we wrote" into "these are the bytes that belong at the front of the file
//! we are about to finish". It costs as much I/O as re-sending the prefix would, which
//! is why resume is worth offering on a large file and not on a small one.
//!
//! Anything short of all four produces a reason, in words, and the job restarts from
//! zero. There is no path here that resumes on a maybe.

use std::path::Path;

use crate::digest::{Rolling, rolling_over_prefix};
use crate::model::{Direction, FileFacts};
use crate::protocol::TransferLane;
use crate::queue::ResumeRecord;
use crate::wire::Bytes;

/// What a resume check needs to look at.
pub struct Verify<'a> {
    pub record: &'a ResumeRecord,
    pub direction: Direction,
    /// The source as it is right now, or `None` when it could not be stat'd.
    pub source_now: Option<&'a FileFacts>,
    /// This job's local side — the partial for a download, the source for an upload.
    pub local_path: &'a Path,
    /// This job's remote side, likewise.
    pub remote_path: &'a str,
}

/// The outcome of a check. `Ok` carries the running hash positioned at the checkpoint,
/// so the resumed transfer continues one hash rather than starting one it could never
/// complete.
pub type Verdict = std::result::Result<Rolling, String>;

/// Decide whether `record` may be resumed, reading both sides to find out.
pub async fn check(v: Verify<'_>, lane: &mut dyn TransferLane) -> Verdict {
    let checkpoint = v.record.checkpoint.get();
    if checkpoint == 0 {
        return Err("nothing has been transferred yet".into());
    }

    source_unchanged(&v.record.source, v.source_now, v.record.checkpoint)?;

    // The local side is the partial for a download and the source for an upload. Either
    // way its first `checkpoint` bytes are the ones under discussion, and either way it
    // is the side whose hash the resumed transfer will carry on with.
    let local = match v.direction {
        Direction::Down => Path::new(&v.record.temporary_path),
        Direction::Up => v.local_path,
    };
    let rolling = match rolling_over_prefix(local, checkpoint).await {
        Ok(rolling) => rolling,
        Err(err) => return Err(err.to_string()),
    };
    if rolling.snapshot() != v.record.prefix_sha256 {
        return Err(format!(
            "{} no longer holds the bytes the checkpoint recorded",
            local.display()
        ));
    }

    // And the remote side, which is the half a local check cannot see.
    let remote = match v.direction {
        Direction::Down => v.remote_path,
        Direction::Up => v.record.temporary_path.as_str(),
    };
    match lane.prefix_digest(remote, checkpoint).await {
        Ok(digest) if digest == v.record.prefix_sha256 => Ok(rolling),
        Ok(_) => Err(format!(
            "{remote} no longer starts with the bytes already transferred"
        )),
        // Not "it does not match" but "we could not find out", which is a different
        // sentence for the user and the same decision for the engine.
        Err(err) => Err(format!("{remote} could not be checked: {err}")),
    }
}

/// Whether the source is still the file the partial was started from.
///
/// Split out and public because it is the part with no I/O in it, and because the
/// judgements here are the ones worth arguing with in a test rather than in a review.
pub fn source_unchanged(
    recorded: &FileFacts,
    now: Option<&FileFacts>,
    checkpoint: Bytes,
) -> std::result::Result<(), String> {
    let Some(now) = now else {
        return Err("the source could not be read to compare it".into());
    };
    if now.path != recorded.path {
        return Err("the source is a different path than the one recorded".into());
    }
    // Shorter than what has already been transferred: whatever this file is now, the
    // bytes on the destination cannot be its beginning and its end.
    if now.size.get() < checkpoint.get() {
        return Err(format!(
            "the source is now {} bytes, shorter than the {checkpoint} already transferred",
            now.size
        ));
    }
    if recorded.size.get() != 0 && now.size != recorded.size {
        return Err(format!(
            "the source was {} bytes and is now {}",
            recorded.size, now.size
        ));
    }
    // Only when both sides have one. A server that withholds mtimes must not make
    // every resume unverifiable — the digest check is what actually decides, and this
    // is the cheap filter in front of it.
    if let (Some(before), Some(after)) = (recorded.modified, now.modified)
        && before != after
    {
        return Err("the source has been modified since the transfer started".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, Utc};

    use super::*;

    fn facts(size: u64) -> FileFacts {
        FileFacts {
            path: "/remote/big.iso".into(),
            size: Bytes(size),
            modified: Some(Utc::now()),
            digest: None,
        }
    }

    #[test]
    fn an_unchanged_source_passes() {
        let before = facts(1000);
        assert!(source_unchanged(&before, Some(&before.clone()), Bytes(400)).is_ok());
    }

    /// The case the whole module is built around: a deploy replaces a file with a
    /// different build of the same size. Nothing about the length has changed, and
    /// resuming would splice two different files together.
    #[test]
    fn a_replacement_of_identical_size_is_caught_by_its_timestamp() {
        let before = facts(1000);
        let after = FileFacts {
            modified: before.modified.map(|at| at + TimeDelta::seconds(30)),
            ..before.clone()
        };
        let err = source_unchanged(&before, Some(&after), Bytes(400)).unwrap_err();
        assert!(err.contains("modified"), "{err}");
    }

    /// And when the timestamps *also* match, which happens, the digest check behind
    /// this one is the only thing left. This test records that the cheap filter
    /// deliberately lets that case through rather than pretending to catch it.
    #[test]
    fn a_replacement_that_matches_size_and_time_is_left_to_the_digest() {
        let before = facts(1000);
        assert!(
            source_unchanged(&before, Some(&before.clone()), Bytes(400)).is_ok(),
            "size and mtime cannot distinguish these; the prefix hash is what does"
        );
    }

    #[test]
    fn a_source_that_shrank_below_the_checkpoint_is_refused() {
        let before = facts(1000);
        let after = facts(100);
        let err = source_unchanged(&before, Some(&after), Bytes(400)).unwrap_err();
        assert!(err.contains("shorter"), "{err}");
    }

    #[test]
    fn a_source_that_cannot_be_read_is_not_assumed_unchanged() {
        let err = source_unchanged(&facts(1000), None, Bytes(400)).unwrap_err();
        assert!(err.contains("could not be read"), "{err}");
    }

    #[test]
    fn a_source_at_a_different_path_is_a_different_file() {
        let before = facts(1000);
        let elsewhere = FileFacts {
            path: "/remote/other.iso".into(),
            ..before.clone()
        };
        assert!(source_unchanged(&before, Some(&elsewhere), Bytes(400)).is_err());
    }

    /// A server that withholds modification times must not make resume impossible.
    /// The digest is what decides; this filter only ever rules things out early.
    #[test]
    fn a_missing_timestamp_does_not_by_itself_refuse_a_resume() {
        let before = FileFacts {
            modified: None,
            ..facts(1000)
        };
        let now = FileFacts {
            modified: Some(Utc::now()),
            ..facts(1000)
        };
        assert!(source_unchanged(&before, Some(&now), Bytes(400)).is_ok());
    }

    #[tokio::test]
    async fn a_checkpoint_of_zero_has_nothing_to_resume() {
        let record = ResumeRecord::new(facts(1000), "/tmp/.big.iso.relaypart");
        let mut lane = crate::mock::MockLane::over(crate::mock::MockFs::empty());
        let verdict = check(
            Verify {
                record: &record,
                direction: Direction::Down,
                source_now: Some(&facts(1000)),
                local_path: Path::new("/tmp/big.iso"),
                remote_path: "/remote/big.iso",
            },
            &mut lane,
        )
        .await;
        assert!(verdict.is_err());
    }
}

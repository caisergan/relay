//! Hashing the part of a file that has actually been transferred.
//!
//! A resume needs to know that the bytes already at the destination are the *same*
//! bytes the source still has. A length cannot say that: two files of identical size
//! are identical in exactly one respect, and splicing the tail of one onto the head of
//! another produces a file that passes every size check and is nonetheless garbage.
//!
//! So the engine records a digest of the prefix it is trusting, and re-derives it
//! before continuing. [`Rolling`] makes the recording side cheap — the bytes are
//! hashed as they stream past, and a checkpoint clones the hasher rather than
//! re-reading everything written so far, which would make checkpointing quadratic.
//! The verifying side is not cheap: [`prefix_of_file`] reads the prefix back. That
//! cost is the point. It is the only difference between "the same length" and "the
//! same bytes", and a resume that skips it is a guess.

use std::path::Path;

use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::error::{EngineError, Result};

/// How much is read at a time when verifying a prefix.
const READ_CHUNK: usize = 256 * 1024;

/// A SHA-256 over bytes as they go past, which can be asked for its digest so far
/// without disturbing the stream.
#[derive(Clone, Default)]
pub struct Rolling {
    hasher: Sha256,
    len: u64,
}

impl std::fmt::Debug for Rolling {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rolling({} bytes)", self.len)
    }
}

impl Rolling {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.hasher.update(bytes);
        self.len += bytes.len() as u64;
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The digest of everything hashed so far.
    ///
    /// Cloning rather than consuming is what keeps checkpointing linear: the stream
    /// carries on with the original hasher, so a checkpoint costs a clone instead of
    /// a re-read of everything written.
    pub fn snapshot(&self) -> String {
        hex::encode(self.hasher.clone().finalize())
    }
}

/// Hash the first `len` bytes of a local file, and hand back the running hash sitting
/// at that point.
///
/// Returning the [`Rolling`] rather than only its digest is what lets a resumed
/// transfer go on checkpointing. A hash that started partway through a file could
/// never speak for the bytes before it, so a job resumed once could never be resumed
/// again; seeding it from the prefix that was just verified costs nothing extra,
/// because those bytes had to be read anyway.
///
/// Refuses rather than pads when the file is shorter than `len`. A prefix that is not
/// all there is not a prefix, and accepting a truncated partial is the exact mistake
/// this module exists to prevent.
pub async fn rolling_over_prefix(path: &Path, len: u64) -> Result<Rolling> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|err| EngineError::from_io(path, &err))?;
    let mut rolling = Rolling::new();
    let mut left = len;
    let mut buffer = vec![0u8; READ_CHUNK.min(len.max(1) as usize)];

    while left > 0 {
        let want = buffer.len().min(left as usize);
        let read = file
            .read(&mut buffer[..want])
            .await
            .map_err(|err| EngineError::from_io(path, &err))?;
        if read == 0 {
            return Err(EngineError::ResumeUnverifiable {
                reason: format!(
                    "the partial file holds fewer than the {len} bytes the checkpoint claims"
                ),
            });
        }
        rolling.update(&buffer[..read]);
        left -= read as u64;
    }
    Ok(rolling)
}

/// SHA-256 of the first `len` bytes of a local file.
pub async fn prefix_of_file(path: &Path, len: u64) -> Result<String> {
    rolling_over_prefix(path, len)
        .await
        .map(|rolling| rolling.snapshot())
}

/// SHA-256 of a byte slice, for the in-memory backends.
pub fn of(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY: &str = crate::queue::EMPTY_SHA256;

    #[test]
    fn the_empty_prefix_has_a_real_digest_rather_than_a_placeholder() {
        assert_eq!(Rolling::new().snapshot(), EMPTY);
        assert_eq!(of(b""), EMPTY);
    }

    /// The property that makes checkpointing affordable: asking for the digest so far
    /// must not disturb the hash the stream is still building.
    #[test]
    fn a_snapshot_does_not_end_the_hash() {
        let mut rolling = Rolling::new();
        rolling.update(b"hello ");
        let halfway = rolling.snapshot();
        rolling.update(b"world");
        let whole = rolling.snapshot();

        assert_eq!(halfway, of(b"hello "));
        assert_eq!(whole, of(b"hello world"));
        assert_eq!(rolling.len(), 11);
    }

    #[tokio::test]
    async fn a_prefix_is_the_prefix_and_not_the_whole_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("part");
        tokio::fs::write(&path, b"hello world").await.unwrap();

        assert_eq!(prefix_of_file(&path, 5).await.unwrap(), of(b"hello"));
        assert_eq!(prefix_of_file(&path, 11).await.unwrap(), of(b"hello world"));
        assert_eq!(prefix_of_file(&path, 0).await.unwrap(), EMPTY);
    }

    /// The failure this exists to catch: a partial that was truncated — by a crash, by
    /// a disk filling up — must not be resumable just because a checkpoint says it
    /// should be that long.
    #[tokio::test]
    async fn a_partial_shorter_than_its_checkpoint_is_refused_not_padded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("part");
        tokio::fs::write(&path, b"hello").await.unwrap();

        let err = prefix_of_file(&path, 500).await.unwrap_err();
        assert!(
            matches!(err, EngineError::ResumeUnverifiable { .. }),
            "got {err:?}"
        );
    }

    /// Two files of the same length are the same length. That is all it means.
    #[test]
    fn a_matching_length_is_not_a_matching_prefix() {
        assert_ne!(of(b"aaaa"), of(b"bbbb"));
    }

    /// A job resumed once must be resumable again, which it only is if the verified
    /// prefix seeds the hash the resumed transfer carries on with.
    #[tokio::test]
    async fn a_verified_prefix_seeds_the_hash_the_resume_continues() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("part");
        tokio::fs::write(&path, b"hello ").await.unwrap();

        let mut rolling = rolling_over_prefix(&path, 6).await.unwrap();
        assert_eq!(rolling.len(), 6);
        rolling.update(b"world");

        assert_eq!(
            rolling.snapshot(),
            of(b"hello world"),
            "the resumed hash covers the whole file, not just its tail"
        );
    }
}

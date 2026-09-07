//! Every way a destination that already exists can be dealt with, in both directions.
//!
//! The matrix is small enough to enumerate and important enough to: each cell decides
//! what happens to a file somebody already has. Overwriting when they meant keep-both
//! destroys it; skipping when they meant overwrite silently leaves them with the old
//! one and a queue that says everything succeeded.
//!
//! Every case here runs through the real engine over the in-memory backend, so what is
//! being checked is the whole path — the settings default, the sheet, the propagation
//! of "apply to remaining", the keep-both naming, and the bytes that end up on disk.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use relay_core::EngineHub;
use relay_core::engine::{BackendFactory, Engine, TransferItem};
use relay_core::interact::{ConflictAction, Prompt, PromptReply};
use relay_core::job::JobState;
use relay_core::mock::{MockFactory, MockFs, MockOptions, NoSecrets};
use relay_core::model::{AuthMethod, Direction, Proto, ServerConfig, SessionId};
use relay_core::servers::ServerStore;
use relay_core::settings::{Settings, SettingsStore};
use relay_core::store::QueueStore;
use uuid::Uuid;

const REMOTE: &str = "/home/deploy/notes.md";
/// What the seeded mock filesystem holds at [`REMOTE`].
const REMOTE_BYTES: &[u8] = b"# notes\n";
const LOCAL_BYTES: &[u8] = b"mine, not the server's";

struct Harness {
    engine: Arc<Engine>,
    hub: Arc<EngineHub>,
    fs: MockFs,
    session: SessionId,
    server_id: Uuid,
    dir: tempfile::TempDir,
}

fn server() -> ServerConfig {
    ServerConfig {
        id: Uuid::new_v4(),
        name: "fixture".into(),
        host: "example.test".into(),
        port: 22,
        proto: Proto::Sftp,
        username: "ada".into(),
        auth: AuthMethod::Agent,
        color: None,
        group: None,
        bookmarks: Vec::new(),
        initial_remote_path: None,
    }
}

async fn harness(default_conflict: Option<ConflictAction>) -> Harness {
    let hub = EngineHub::start(&tokio::runtime::Handle::current());
    let fs = MockFs::seeded();
    let factory: Arc<dyn BackendFactory> =
        Arc::new(MockFactory::new(fs.clone(), MockOptions::default()));
    let settings = Arc::new(SettingsStore::ephemeral());
    settings
        .set(Settings {
            default_conflict,
            ..Settings::default()
        })
        .expect("ephemeral settings never fail to write");

    let engine = Arc::new(
        Engine::with_parts(
            Arc::clone(&hub),
            tokio::runtime::Handle::current(),
            factory,
            Arc::new(NoSecrets),
            Arc::new(ServerStore::ephemeral()),
            QueueStore::in_memory().await.expect("an in-memory queue"),
            settings,
        )
        .await
        .expect("the engine starts"),
    );

    let cfg = server();
    let server_id = cfg.id;
    let session = engine.open_session(cfg).expect("session opens");
    Harness {
        engine,
        hub,
        fs,
        session,
        server_id,
        dir: tempfile::tempdir().expect("tempdir"),
    }
}

impl Harness {
    /// Both sides hold a file, so whatever direction the transfer goes there is a
    /// destination in the way.
    fn seed(&self) -> PathBuf {
        let local = self.dir.path().join("notes.md");
        std::fs::write(&local, LOCAL_BYTES).expect("seed the local side");
        local
    }

    async fn enqueue(&self, direction: Direction, local: PathBuf) -> Uuid {
        self.engine
            .enqueue(
                Uuid::new_v4(),
                vec![TransferItem {
                    session: self.session,
                    server_id: self.server_id,
                    direction,
                    remote_path: REMOTE.into(),
                    local_path: local,
                    is_dir: false,
                }],
            )
            .await
            .expect("accepted")[0]
    }

    /// Answer the conflict sheet, once it exists.
    async fn answer(&self, action: ConflictAction, apply_to_remaining: bool) {
        let sub = self.hub.subscribe();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let prompt = self
                .hub
                .snapshot(sub.id)
                .expect("subscribed")
                .prompts
                .into_iter()
                .find(|p| matches!(p.prompt, Prompt::Conflict { .. }));
            if let Some(prompt) = prompt {
                self.hub
                    .prompts()
                    .resolve(
                        prompt.id,
                        PromptReply::Conflict {
                            action,
                            apply_to_remaining,
                        },
                    )
                    .expect("the sheet accepts an answer");
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the conflict sheet never opened"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn settled(&self, job: Uuid) -> JobState {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            let found = self
                .engine
                .queue()
                .snapshot()
                .await
                .into_iter()
                .find(|j| j.id == job)
                .map(|j| j.state);
            match found {
                Some(state) if state.is_terminal() => return state,
                _ if tokio::time::Instant::now() > deadline => panic!("the job never finished"),
                _ => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    }

    /// Every prompt the engine still considers unanswered.
    fn open_prompts(&self) -> Vec<relay_core::interact::PromptRequest> {
        self.hub
            .snapshot(self.hub.subscribe().id)
            .expect("subscribed")
            .prompts
    }

    /// Whatever `notes.md (2)` style files the local directory ended up with.
    fn extra_local(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.dir.path())
            .expect("dir")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name != "notes.md")
            .collect();
        names.sort();
        names
    }
}

// ------------------------------------------------------------------ downloads

#[tokio::test]
async fn overwrite_replaces_the_local_file() {
    let h = harness(None).await;
    let local = h.seed();
    let job = h.enqueue(Direction::Down, local.clone()).await;
    h.answer(ConflictAction::Overwrite, false).await;

    assert!(matches!(
        h.settled(job).await,
        JobState::Done { skipped: false, .. }
    ));
    assert_eq!(std::fs::read(&local).unwrap(), REMOTE_BYTES);
}

#[tokio::test]
async fn skip_leaves_the_local_file_exactly_as_it_was() {
    let h = harness(None).await;
    let local = h.seed();
    let job = h.enqueue(Direction::Down, local.clone()).await;
    h.answer(ConflictAction::Skip, false).await;

    assert!(
        matches!(h.settled(job).await, JobState::Done { skipped: true, .. }),
        "a skip finishes; it does not fail"
    );
    assert_eq!(std::fs::read(&local).unwrap(), LOCAL_BYTES);
    assert!(h.extra_local().is_empty(), "and writes nothing beside it");
}

#[tokio::test]
async fn keep_both_writes_a_numbered_name_and_leaves_the_original_alone() {
    let h = harness(None).await;
    let local = h.seed();
    let job = h.enqueue(Direction::Down, local.clone()).await;
    h.answer(ConflictAction::KeepBoth, false).await;

    assert!(matches!(
        h.settled(job).await,
        JobState::Done { skipped: false, .. }
    ));
    assert_eq!(
        std::fs::read(&local).unwrap(),
        LOCAL_BYTES,
        "the file that was there is untouched"
    );
    assert_eq!(
        h.extra_local(),
        ["notes (2).md"],
        "and the extension is kept, because `notes (2).md` opens and `notes.md (2)` does not"
    );
    assert_eq!(
        std::fs::read(h.dir.path().join("notes (2).md")).unwrap(),
        REMOTE_BYTES
    );
}

/// Resume is offered only when the engine has proven it safe, and it has not: there is
/// no ownership record for a transfer that never ran. Answering it anyway must not
/// produce a spliced file — it restarts, which is always safe.
#[tokio::test]
async fn resume_without_a_verified_partial_restarts_rather_than_splicing() {
    let h = harness(None).await;
    let local = h.seed();
    let job = h.enqueue(Direction::Down, local.clone()).await;
    h.answer(ConflictAction::Resume, false).await;

    assert!(matches!(
        h.settled(job).await,
        JobState::Done { skipped: false, .. }
    ));
    assert_eq!(
        std::fs::read(&local).unwrap(),
        REMOTE_BYTES,
        "the whole source, not a tail grafted onto the old file"
    );
}

// -------------------------------------------------------------------- uploads

#[tokio::test]
async fn overwrite_replaces_the_remote_file() {
    let h = harness(None).await;
    let local = h.seed();
    let job = h.enqueue(Direction::Up, local).await;
    h.answer(ConflictAction::Overwrite, false).await;

    assert!(matches!(
        h.settled(job).await,
        JobState::Done { skipped: false, .. }
    ));
    assert_eq!(h.fs.read_file(REMOTE).as_deref(), Some(LOCAL_BYTES));
}

#[tokio::test]
async fn skip_leaves_the_remote_file_exactly_as_it_was() {
    let h = harness(None).await;
    let local = h.seed();
    let job = h.enqueue(Direction::Up, local).await;
    h.answer(ConflictAction::Skip, false).await;

    assert!(matches!(
        h.settled(job).await,
        JobState::Done { skipped: true, .. }
    ));
    assert_eq!(h.fs.read_file(REMOTE).as_deref(), Some(REMOTE_BYTES));
}

#[tokio::test]
async fn keep_both_writes_a_numbered_remote_name() {
    let h = harness(None).await;
    let local = h.seed();
    let job = h.enqueue(Direction::Up, local).await;
    h.answer(ConflictAction::KeepBoth, false).await;

    assert!(matches!(
        h.settled(job).await,
        JobState::Done { skipped: false, .. }
    ));
    assert_eq!(
        h.fs.read_file(REMOTE).as_deref(),
        Some(REMOTE_BYTES),
        "the file that was there is untouched"
    );
    assert_eq!(
        h.fs.read_file("/home/deploy/notes (2).md").as_deref(),
        Some(LOCAL_BYTES)
    );
}

// ------------------------------------------------------------------- defaults

/// A saved default is a decision already made. It must not open a sheet.
#[tokio::test]
async fn a_saved_default_answers_without_asking() {
    for (action, expected) in [
        (ConflictAction::Overwrite, REMOTE_BYTES),
        (ConflictAction::Skip, LOCAL_BYTES),
    ] {
        let h = harness(Some(action)).await;
        let local = h.seed();
        let job = h.enqueue(Direction::Down, local.clone()).await;

        let state = h.settled(job).await;
        assert!(state.is_terminal(), "{action:?} finished on its own");
        assert_eq!(std::fs::read(&local).unwrap(), expected, "{action:?}");
        assert!(
            h.open_prompts().is_empty(),
            "{action:?} opened a sheet it should have answered itself"
        );
    }
}

/// A default of "resume" is a preference, and resume is a proof. With nothing to
/// resume from it falls back to starting over rather than to whatever the disk holds.
#[tokio::test]
async fn a_saved_default_of_resume_falls_back_to_starting_over() {
    let h = harness(Some(ConflictAction::Resume)).await;
    let local = h.seed();
    let job = h.enqueue(Direction::Down, local.clone()).await;

    assert!(matches!(
        h.settled(job).await,
        JobState::Done { skipped: false, .. }
    ));
    assert_eq!(std::fs::read(&local).unwrap(), REMOTE_BYTES);
}

// --------------------------------------------------------- apply to remaining

/// One answer for the whole gesture. Without this, dragging thirty files onto a folder
/// that already has them means thirty identical sheets.
#[tokio::test]
async fn apply_to_remaining_answers_the_rest_of_the_batch() {
    let h = harness(None).await;
    let batch = Uuid::new_v4();
    let names = ["one.md", "two.md", "three.md"];

    // Three sources on the server, three files already in the way locally.
    for name in names {
        h.fs.write_file(&format!("/home/deploy/{name}"), REMOTE_BYTES.to_vec());
        std::fs::write(h.dir.path().join(name), LOCAL_BYTES).expect("seed");
    }

    let jobs = h
        .engine
        .enqueue(
            batch,
            names
                .iter()
                .map(|name| TransferItem {
                    session: h.session,
                    server_id: h.server_id,
                    direction: Direction::Down,
                    remote_path: format!("/home/deploy/{name}"),
                    local_path: h.dir.path().join(name),
                    is_dir: false,
                })
                .collect(),
        )
        .await
        .expect("accepted");

    // One answer, for all of them.
    h.answer(ConflictAction::Overwrite, true).await;

    for job in &jobs {
        assert!(matches!(
            h.settled(*job).await,
            JobState::Done { skipped: false, .. }
        ));
    }
    for name in names {
        assert_eq!(
            std::fs::read(h.dir.path().join(name)).unwrap(),
            REMOTE_BYTES,
            "{name} took the answer given for its sibling"
        );
    }
    assert!(h.open_prompts().is_empty(), "no second sheet was opened");
}

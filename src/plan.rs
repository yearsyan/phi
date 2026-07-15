//! Versioned, session-scoped plan artifacts.
//!
//! Plans live outside the conversation transcript so that planning can be
//! resumed, reviewed, and approved without relying on the model to reproduce
//! the exact text. Updates use optimistic concurrency: revision `0` denotes a
//! plan that does not exist yet, and each successful update increments it.

use std::{
    collections::HashMap,
    fs,
    io::{ErrorKind, Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

/// Revision used when a session does not have a plan yet.
pub const EMPTY_PLAN_REVISION: u64 = 0;

/// Maximum UTF-8 byte length of plan content, excluding the revision header.
pub const MAX_PLAN_BYTES: usize = 1024 * 1024;

const MAX_SESSION_ID_BYTES: usize = 180;
// Encoding session IDs as lowercase hex avoids aliases on case-insensitive
// filesystems. Fixed-size path components also stay below platform filename
// limits even when the maximum-size session ID is used.
const SESSION_PATH_VERSION: &str = "v1";
const SESSION_PATH_COMPONENT_HEX_BYTES: usize = 64;
const ARTIFACT_FILE_NAME: &str = "plan.md";
const LOCK_FILE_NAME: &str = "plan.lock";
const REVISION_HEADER_PREFIX: &str = "<!-- phi-plan-revision: ";
const REVISION_HEADER_SUFFIX: &str = " -->";
const MAX_REVISION_METADATA_BYTES: usize = 64;
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The latest persisted plan for one session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanArtifact {
    pub session_id: String,
    pub revision: u64,
    pub content: String,
}

/// A snapshot of one session's plan held under the store's update lock.
///
/// Calls to [`PlanStore::update`] for the same session remain blocked until
/// this value is dropped. The guard is intentionally opaque so callers cannot
/// release the lock while retaining a value that appears protected.
#[must_use = "dropping the lease releases the plan update lock"]
pub struct LockedPlan {
    artifact: Option<PlanArtifact>,
    _guard: Box<dyn Send>,
}

impl LockedPlan {
    /// Creates a locked snapshot for a custom [`PlanStore`] implementation.
    ///
    /// `guard` must own a real update lock for the same session represented by
    /// `artifact` (or for the requested session when `artifact` is `None`). It
    /// must prevent every cooperating [`PlanStore::update`] for that session
    /// until its [`Drop`] implementation releases the lock. Passing an
    /// unrelated value such as `()` violates the [`PlanStore::lock_current`]
    /// contract and can reintroduce check-then-act races.
    ///
    /// ```
    /// use phi::{LockedPlan, PlanArtifact};
    ///
    /// // A real backend guard would own a database lease, file lock, or owned
    /// // async lock guard and release it from Drop.
    /// struct BackendSessionUpdateGuard;
    /// let lease = LockedPlan::with_guard(
    ///     Some(PlanArtifact {
    ///         session_id: "session".to_owned(),
    ///         revision: 1,
    ///         content: "plan".to_owned(),
    ///     }),
    ///     BackendSessionUpdateGuard,
    /// );
    /// assert_eq!(lease.artifact().unwrap().revision, 1);
    /// ```
    pub fn with_guard(artifact: Option<PlanArtifact>, guard: impl Send + 'static) -> Self {
        Self {
            artifact,
            _guard: Box::new(guard),
        }
    }

    /// Returns the plan observed while the update lock is held.
    pub fn artifact(&self) -> Option<&PlanArtifact> {
        self.artifact.as_ref()
    }
}

/// Failures returned by a [`PlanStore`].
#[derive(Debug, Error)]
pub enum PlanStoreError {
    #[error("invalid session ID: {0}")]
    InvalidSessionId(String),

    #[error(
        "plan revision conflict for session {session_id:?}: expected {expected}, current revision is {actual}"
    )]
    RevisionConflict {
        session_id: String,
        expected: u64,
        actual: u64,
    },

    #[error("plan revision is exhausted for session {session_id:?}")]
    RevisionExhausted { session_id: String },

    #[error("plan storage I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid plan data at {path}: {message}")]
    InvalidData { path: PathBuf, message: String },

    #[error("plan is too large: {actual_bytes} bytes exceeds the {max_bytes}-byte limit")]
    PlanTooLarge { actual_bytes: u64, max_bytes: usize },

    #[error("stored plan for session {session_id:?} has invalid revision {revision}")]
    InvalidStoredRevision { session_id: String, revision: u64 },
}

/// Persistence boundary for versioned plan artifacts.
#[async_trait]
pub trait PlanStore: Send + Sync {
    /// Returns the current plan, or `None` when the session has no plan.
    async fn current(&self, session_id: &str) -> Result<Option<PlanArtifact>, PlanStoreError>;

    /// Reads the current plan and holds its per-session update lock.
    ///
    /// Use this for short check-then-act critical sections such as verifying an
    /// approval revision immediately before changing execution mode. Drop the
    /// returned lease promptly; writers for the same session remain blocked
    /// while it is alive.
    async fn lock_current(&self, session_id: &str) -> Result<LockedPlan, PlanStoreError>;

    /// Replaces the plan if its current revision equals `expected_revision`.
    ///
    /// Use [`EMPTY_PLAN_REVISION`] to create the first plan. The returned
    /// artifact contains the incremented revision. Unless an implementation
    /// documents a stronger guarantee, cancellation must be treated as an
    /// indeterminate outcome: call [`PlanStore::current`] to reconcile before
    /// retrying instead of blindly replaying the same expected revision.
    async fn update(
        &self,
        session_id: &str,
        expected_revision: u64,
        content: String,
    ) -> Result<PlanArtifact, PlanStoreError>;
}

#[async_trait]
impl<T> PlanStore for Arc<T>
where
    T: PlanStore + ?Sized,
{
    async fn current(&self, session_id: &str) -> Result<Option<PlanArtifact>, PlanStoreError> {
        (**self).current(session_id).await
    }

    async fn lock_current(&self, session_id: &str) -> Result<LockedPlan, PlanStoreError> {
        (**self).lock_current(session_id).await
    }

    async fn update(
        &self,
        session_id: &str,
        expected_revision: u64,
        content: String,
    ) -> Result<PlanArtifact, PlanStoreError> {
        (**self)
            .update(session_id, expected_revision, content)
            .await
    }
}

/// Process-local plan persistence useful for tests and ephemeral sessions.
#[derive(Clone, Default)]
pub struct InMemoryPlanStore {
    plans: Arc<RwLock<HashMap<String, PlanArtifact>>>,
}

impl InMemoryPlanStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl PlanStore for InMemoryPlanStore {
    async fn current(&self, session_id: &str) -> Result<Option<PlanArtifact>, PlanStoreError> {
        validate_session_id(session_id)?;
        Ok(self.plans.read().await.get(session_id).cloned())
    }

    async fn lock_current(&self, session_id: &str) -> Result<LockedPlan, PlanStoreError> {
        validate_session_id(session_id)?;
        let guard = Arc::clone(&self.plans).read_owned().await;
        let artifact = guard.get(session_id).cloned();
        Ok(LockedPlan::with_guard(artifact, guard))
    }

    async fn update(
        &self,
        session_id: &str,
        expected_revision: u64,
        content: String,
    ) -> Result<PlanArtifact, PlanStoreError> {
        validate_session_id(session_id)?;
        validate_plan_content(&content)?;
        let mut plans = self.plans.write().await;
        let actual_revision = plans
            .get(session_id)
            .map_or(EMPTY_PLAN_REVISION, |plan| plan.revision);
        ensure_expected_revision(session_id, expected_revision, actual_revision)?;
        let revision = next_revision(session_id, actual_revision)?;
        let artifact = PlanArtifact {
            session_id: session_id.to_owned(),
            revision,
            content,
        };
        plans.insert(session_id.to_owned(), artifact.clone());
        Ok(artifact)
    }
}

/// One-file-per-session plan persistence rooted at a directory.
///
/// Session IDs are encoded as lowercase hexadecimal path components,
/// preventing path traversal and case-insensitive filesystem aliases. Updates
/// acquire a per-session OS file lock, write a temporary file in the artifact
/// directory, and atomically rename it over the previous revision. The file is
/// synced before the rename; parent-directory syncing is best effort, so a
/// successful update guarantees an atomically visible revision but not that the
/// directory entry will survive sudden power loss on every filesystem. Disk
/// operations run as blocking transactions: once started, dropping or timing
/// out an update future does not stop the transaction. A cancelled caller must
/// read the current revision to determine whether it committed.
#[derive(Clone, Debug)]
pub struct DiskPlanStore {
    root: PathBuf,
}

impl DiskPlanStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the exact artifact file used for `session_id`.
    pub fn artifact_path(&self, session_id: &str) -> Result<PathBuf, PlanStoreError> {
        validate_session_id(session_id)?;
        let encoded = encode_session_id(session_id);
        let mut path = self.root.join(SESSION_PATH_VERSION);
        for component in encoded.as_bytes().chunks(SESSION_PATH_COMPONENT_HEX_BYTES) {
            // `encoded` is ASCII, so every byte boundary is a UTF-8 boundary.
            path.push(std::str::from_utf8(component).expect("hex encoding must be valid UTF-8"));
        }
        Ok(path.join(ARTIFACT_FILE_NAME))
    }

    fn lock_path(&self, session_id: &str) -> Result<PathBuf, PlanStoreError> {
        let artifact = self.artifact_path(session_id)?;
        Ok(artifact.with_file_name(LOCK_FILE_NAME))
    }

    fn acquire_session_lock(&self, session_id: &str) -> Result<FileLockGuard, PlanStoreError> {
        let path = self.lock_path(session_id)?;
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options
            .open(&path)
            .map_err(|source| io_error(path.clone(), source))?;
        #[cfg(unix)]
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|source| io_error(path.clone(), source))?;
        file.lock()
            .map_err(|source| io_error(path.clone(), source))?;
        Ok(FileLockGuard { file })
    }

    fn read_unlocked(&self, session_id: &str) -> Result<Option<PlanArtifact>, PlanStoreError> {
        let path = self.artifact_path(session_id)?;
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(io_error(path, source)),
        };
        let stored_limit = MAX_PLAN_BYTES + MAX_REVISION_METADATA_BYTES;
        let metadata = file
            .metadata()
            .map_err(|source| io_error(path.clone(), source))?;
        if metadata.len() > stored_limit as u64 {
            return Err(PlanStoreError::PlanTooLarge {
                actual_bytes: metadata.len(),
                max_bytes: MAX_PLAN_BYTES,
            });
        }
        // The bounded read also protects against a file growing after metadata().
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(stored_limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|source| io_error(path.clone(), source))?;
        if bytes.len() > stored_limit {
            return Err(PlanStoreError::PlanTooLarge {
                actual_bytes: bytes.len() as u64,
                max_bytes: MAX_PLAN_BYTES,
            });
        }
        let (revision, content) = parse_plan_file(&path, &bytes)?;
        validate_plan_content(&content)?;
        if revision == EMPTY_PLAN_REVISION {
            return Err(PlanStoreError::InvalidStoredRevision {
                session_id: session_id.to_owned(),
                revision,
            });
        }
        Ok(Some(PlanArtifact {
            session_id: session_id.to_owned(),
            revision,
            content,
        }))
    }

    fn write_unlocked(&self, artifact: &PlanArtifact) -> Result<(), PlanStoreError> {
        let destination = self.artifact_path(&artifact.session_id)?;
        let serialized = format!(
            "{REVISION_HEADER_PREFIX}{}{REVISION_HEADER_SUFFIX}\n{}",
            artifact.revision, artifact.content
        );
        write_atomic(&destination, serialized.as_bytes())
    }

    fn current_blocking(&self, session_id: &str) -> Result<Option<PlanArtifact>, PlanStoreError> {
        let artifact_path = self.artifact_path(session_id)?;
        match fs::symlink_metadata(&artifact_path) {
            Ok(_) => {}
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(io_error(artifact_path, source)),
        }
        let _lock = self.acquire_session_lock(session_id)?;
        self.read_unlocked(session_id)
    }

    fn lock_current_blocking(&self, session_id: &str) -> Result<LockedPlan, PlanStoreError> {
        let artifact_path = self.artifact_path(session_id)?;
        let parent = artifact_path.parent().ok_or_else(|| {
            io_error(
                artifact_path.clone(),
                std::io::Error::new(ErrorKind::InvalidInput, "plan path has no parent directory"),
            )
        })?;
        // A missing plan must still be protected against concurrent creation,
        // so a locked read materializes its session directory and sidecar.
        fs::create_dir_all(parent).map_err(|source| io_error(parent.to_owned(), source))?;
        let guard = self.acquire_session_lock(session_id)?;
        let artifact = self.read_unlocked(session_id)?;
        Ok(LockedPlan::with_guard(artifact, guard))
    }

    fn update_blocking(
        &self,
        session_id: &str,
        expected_revision: u64,
        content: String,
    ) -> Result<PlanArtifact, PlanStoreError> {
        let artifact_path = self.artifact_path(session_id)?;
        let parent = artifact_path.parent().ok_or_else(|| {
            io_error(
                artifact_path.clone(),
                std::io::Error::new(ErrorKind::InvalidInput, "plan path has no parent directory"),
            )
        })?;
        fs::create_dir_all(parent).map_err(|source| io_error(parent.to_owned(), source))?;
        let _lock = self.acquire_session_lock(session_id)?;
        let actual_revision = self
            .read_unlocked(session_id)?
            .map_or(EMPTY_PLAN_REVISION, |plan| plan.revision);
        ensure_expected_revision(session_id, expected_revision, actual_revision)?;
        let artifact = PlanArtifact {
            session_id: session_id.to_owned(),
            revision: next_revision(session_id, actual_revision)?,
            content,
        };
        self.write_unlocked(&artifact)?;
        Ok(artifact)
    }
}

#[async_trait]
impl PlanStore for DiskPlanStore {
    async fn current(&self, session_id: &str) -> Result<Option<PlanArtifact>, PlanStoreError> {
        validate_session_id(session_id)?;
        let store = self.clone();
        let session_id = session_id.to_owned();
        let error_path = self.root.clone();
        tokio::task::spawn_blocking(move || store.current_blocking(&session_id))
            .await
            .map_err(|source| blocking_task_error(error_path, source))?
    }

    async fn lock_current(&self, session_id: &str) -> Result<LockedPlan, PlanStoreError> {
        validate_session_id(session_id)?;
        let store = self.clone();
        let session_id = session_id.to_owned();
        let error_path = self.root.clone();
        tokio::task::spawn_blocking(move || store.lock_current_blocking(&session_id))
            .await
            .map_err(|source| blocking_task_error(error_path, source))?
    }

    async fn update(
        &self,
        session_id: &str,
        expected_revision: u64,
        content: String,
    ) -> Result<PlanArtifact, PlanStoreError> {
        validate_session_id(session_id)?;
        validate_plan_content(&content)?;
        let store = self.clone();
        let session_id = session_id.to_owned();
        let error_path = self.root.clone();
        tokio::task::spawn_blocking(move || {
            store.update_blocking(&session_id, expected_revision, content)
        })
        .await
        .map_err(|source| blocking_task_error(error_path, source))?
    }
}

fn ensure_expected_revision(
    session_id: &str,
    expected: u64,
    actual: u64,
) -> Result<(), PlanStoreError> {
    if expected == actual {
        return Ok(());
    }
    Err(PlanStoreError::RevisionConflict {
        session_id: session_id.to_owned(),
        expected,
        actual,
    })
}

fn next_revision(session_id: &str, revision: u64) -> Result<u64, PlanStoreError> {
    revision
        .checked_add(1)
        .ok_or_else(|| PlanStoreError::RevisionExhausted {
            session_id: session_id.to_owned(),
        })
}

fn encode_session_id(session_id: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut encoded = String::with_capacity(session_id.len() * 2);
    for byte in session_id.bytes() {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn validate_session_id(session_id: &str) -> Result<(), PlanStoreError> {
    if session_id.trim().is_empty() {
        return Err(PlanStoreError::InvalidSessionId(
            "must not be empty".to_owned(),
        ));
    }
    if session_id.len() > MAX_SESSION_ID_BYTES {
        return Err(PlanStoreError::InvalidSessionId(format!(
            "must not exceed {MAX_SESSION_ID_BYTES} UTF-8 bytes"
        )));
    }
    Ok(())
}

fn validate_plan_content(content: &str) -> Result<(), PlanStoreError> {
    if content.len() <= MAX_PLAN_BYTES {
        return Ok(());
    }
    Err(PlanStoreError::PlanTooLarge {
        actual_bytes: content.len() as u64,
        max_bytes: MAX_PLAN_BYTES,
    })
}

fn parse_plan_file(path: &Path, bytes: &[u8]) -> Result<(u64, String), PlanStoreError> {
    let text = std::str::from_utf8(bytes).map_err(|source| PlanStoreError::InvalidData {
        path: path.to_owned(),
        message: format!("plan is not valid UTF-8: {source}"),
    })?;
    let (header, content) = text
        .split_once('\n')
        .ok_or_else(|| PlanStoreError::InvalidData {
            path: path.to_owned(),
            message: "missing revision header line".to_owned(),
        })?;
    // Keep files editable by tools that use Windows line endings.
    let header = header.strip_suffix('\r').unwrap_or(header);
    let revision_text = header
        .strip_prefix(REVISION_HEADER_PREFIX)
        .and_then(|value| value.strip_suffix(REVISION_HEADER_SUFFIX))
        .ok_or_else(|| PlanStoreError::InvalidData {
            path: path.to_owned(),
            message: "invalid revision header".to_owned(),
        })?;
    if revision_text.is_empty()
        || !revision_text.bytes().all(|byte| byte.is_ascii_digit())
        || (revision_text.len() > 1 && revision_text.starts_with('0'))
    {
        return Err(PlanStoreError::InvalidData {
            path: path.to_owned(),
            message: "invalid revision header".to_owned(),
        });
    }
    let revision = revision_text
        .parse::<u64>()
        .map_err(|source| PlanStoreError::InvalidData {
            path: path.to_owned(),
            message: format!("invalid plan revision: {source}"),
        })?;
    Ok((revision, content.to_owned()))
}

struct FileLockGuard {
    file: fs::File,
}

impl Drop for FileLockGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

struct TemporaryFileGuard {
    path: PathBuf,
    armed: bool,
}

impl TemporaryFileGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TemporaryFileGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), PlanStoreError> {
    let parent = path.parent().ok_or_else(|| {
        io_error(
            path.to_owned(),
            std::io::Error::new(ErrorKind::InvalidInput, "plan path has no parent directory"),
        )
    })?;
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".phi-plan.{}.{}.tmp", std::process::id(), sequence));

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temporary)
        .map_err(|source| io_error(temporary.clone(), source))?;
    // Arm cleanup only after this transaction has successfully created the
    // file, so a create_new collision never removes another process's file.
    let mut cleanup = TemporaryFileGuard::new(temporary.clone());
    file.write_all(bytes)
        .map_err(|source| io_error(temporary.clone(), source))?;
    file.sync_all()
        .map_err(|source| io_error(temporary.clone(), source))?;
    drop(file);
    fs::rename(&temporary, path).map_err(|source| io_error(path.to_owned(), source))?;
    cleanup.disarm();

    // The rename above is the commit point. Directory syncing is best effort:
    // reporting a post-commit sync error would invite callers to retry the same
    // CAS even though the new revision is already visible. In particular, this
    // does not promise power-loss durability on every platform/filesystem.
    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn blocking_task_error(path: PathBuf, source: tokio::task::JoinError) -> PlanStoreError {
    io_error(
        path,
        std::io::Error::other(format!("plan storage task failed: {source}")),
    )
}

fn io_error(path: PathBuf, source: std::io::Error) -> PlanStoreError {
    PlanStoreError::Io { path, source }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[derive(Clone, Default)]
    struct WrappingPlanStore {
        inner: InMemoryPlanStore,
    }

    #[async_trait::async_trait]
    impl PlanStore for WrappingPlanStore {
        async fn current(&self, session_id: &str) -> Result<Option<PlanArtifact>, PlanStoreError> {
            self.inner.current(session_id).await
        }

        async fn lock_current(&self, session_id: &str) -> Result<LockedPlan, PlanStoreError> {
            let inner_lease = self.inner.lock_current(session_id).await?;
            let artifact = inner_lease.artifact().cloned();
            Ok(LockedPlan::with_guard(artifact, inner_lease))
        }

        async fn update(
            &self,
            session_id: &str,
            expected_revision: u64,
            content: String,
        ) -> Result<PlanArtifact, PlanStoreError> {
            self.inner
                .update(session_id, expected_revision, content)
                .await
        }
    }

    fn assert_no_temporary_files(directory: &Path) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                assert_no_temporary_files(&path);
            } else {
                assert!(
                    !path
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .ends_with(".tmp"),
                    "temporary plan file was not cleaned up: {}",
                    path.display()
                );
            }
        }
    }

    #[tokio::test]
    async fn memory_store_isolates_sessions_and_increments_revisions() {
        let store = InMemoryPlanStore::new();

        assert_eq!(store.current("left").await.unwrap(), None);
        let left = store
            .update("left", EMPTY_PLAN_REVISION, "left plan".to_owned())
            .await
            .unwrap();
        let right = store
            .update("right", EMPTY_PLAN_REVISION, "right plan".to_owned())
            .await
            .unwrap();
        let left_updated = store
            .update("left", left.revision, "updated left plan".to_owned())
            .await
            .unwrap();

        assert_eq!(left.revision, 1);
        assert_eq!(right.revision, 1);
        assert_eq!(left_updated.revision, 2);
        assert_eq!(
            store.current("left").await.unwrap().unwrap().content,
            "updated left plan"
        );
        assert_eq!(
            store.current("right").await.unwrap().unwrap().content,
            "right plan"
        );
    }

    #[tokio::test]
    async fn memory_locked_plan_blocks_update_until_drop() {
        let store = InMemoryPlanStore::new();
        let initial = store
            .update("session", EMPTY_PLAN_REVISION, "initial".to_owned())
            .await
            .unwrap();
        let locked = store.lock_current("session").await.unwrap();
        assert_eq!(locked.artifact(), Some(&initial));

        let writer = store.clone();
        let update = tokio::spawn(async move {
            writer
                .update("session", initial.revision, "updated".to_owned())
                .await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!update.is_finished());

        drop(locked);
        let updated = update.await.unwrap().unwrap();
        assert_eq!(updated.revision, initial.revision + 1);
        assert_eq!(updated.content, "updated");
    }

    #[tokio::test]
    async fn custom_store_can_construct_a_locked_plan_with_a_backend_guard() {
        let store = WrappingPlanStore::default();
        let initial = store
            .update("session", EMPTY_PLAN_REVISION, "initial".to_owned())
            .await
            .unwrap();

        let locked = store.lock_current("session").await.unwrap();
        assert_eq!(locked.artifact(), Some(&initial));
        drop(locked);

        let updated = store
            .update("session", initial.revision, "updated".to_owned())
            .await
            .unwrap();
        assert_eq!(updated.revision, initial.revision + 1);
    }

    #[tokio::test]
    async fn stale_revision_is_rejected_without_overwriting_plan() {
        let store = InMemoryPlanStore::new();
        let first = store
            .update("session", EMPTY_PLAN_REVISION, "first".to_owned())
            .await
            .unwrap();

        let error = store
            .update("session", EMPTY_PLAN_REVISION, "stale".to_owned())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            PlanStoreError::RevisionConflict {
                expected: 0,
                actual: 1,
                ..
            }
        ));
        assert_eq!(store.current("session").await.unwrap(), Some(first));
    }

    #[tokio::test]
    async fn disk_store_persists_across_instances() {
        let directory = tempfile::tempdir().unwrap();
        let first_store = DiskPlanStore::new(directory.path());
        let first = first_store
            .update("session", EMPTY_PLAN_REVISION, "# Plan\n\nOne".to_owned())
            .await
            .unwrap();
        let updated = first_store
            .update("session", first.revision, "# Plan\n\nTwo".to_owned())
            .await
            .unwrap();

        let reopened = DiskPlanStore::new(directory.path());
        assert_eq!(reopened.current("session").await.unwrap(), Some(updated));

        let path = reopened.artifact_path("session").unwrap();
        let file = tokio::fs::read_to_string(path).await.unwrap();
        assert_eq!(file, "<!-- phi-plan-revision: 2 -->\n# Plan\n\nTwo");
    }

    #[tokio::test]
    async fn disk_store_serializes_competing_updates_across_instances() {
        let directory = tempfile::tempdir().unwrap();
        let left = DiskPlanStore::new(directory.path());
        let right = DiskPlanStore::new(directory.path());
        let initial = left
            .update("session", EMPTY_PLAN_REVISION, "initial".to_owned())
            .await
            .unwrap();

        // Hold the sidecar lock so both independently-created stores reach the
        // same cross-instance synchronization point before either can read.
        let lock_path = left.lock_path("session").unwrap();
        let lock_file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(lock_path)
            .unwrap();
        lock_file.lock().unwrap();
        let left_task = tokio::spawn(async move {
            left.update("session", initial.revision, "left".to_owned())
                .await
        });
        let right_task = tokio::spawn(async move {
            right
                .update("session", initial.revision, "right".to_owned())
                .await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!left_task.is_finished());
        assert!(!right_task.is_finished());
        lock_file.unlock().unwrap();

        let left_result = left_task.await.unwrap();
        let right_result = right_task.await.unwrap();

        assert_ne!(left_result.is_ok(), right_result.is_ok());
        let conflict = left_result.err().or_else(|| right_result.err()).unwrap();
        assert!(matches!(
            conflict,
            PlanStoreError::RevisionConflict {
                expected: 1,
                actual: 2,
                ..
            }
        ));
        let reopened = DiskPlanStore::new(directory.path());
        assert_eq!(
            reopened.current("session").await.unwrap().unwrap().revision,
            2
        );
    }

    #[tokio::test]
    async fn disk_locked_plan_blocks_an_independent_store_until_drop() {
        let directory = tempfile::tempdir().unwrap();
        let reader = DiskPlanStore::new(directory.path());
        let writer = DiskPlanStore::new(directory.path());
        let initial = reader
            .update("session", EMPTY_PLAN_REVISION, "initial".to_owned())
            .await
            .unwrap();
        let locked = reader.lock_current("session").await.unwrap();
        assert_eq!(locked.artifact(), Some(&initial));

        let update = tokio::spawn(async move {
            writer
                .update("session", initial.revision, "updated".to_owned())
                .await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!update.is_finished());

        drop(locked);
        let updated = update.await.unwrap().unwrap();
        assert_eq!(updated.revision, initial.revision + 1);
        assert_eq!(updated.content, "updated");
    }

    #[tokio::test]
    async fn lowercase_hex_paths_do_not_alias_on_case_insensitive_filesystems() {
        let directory = tempfile::tempdir().unwrap();
        let store = DiskPlanStore::new(directory.path());
        // These values used to encode to MDBH and MDBh with base64.
        let upper_path = store.artifact_path("00G").unwrap();
        let lower_path = store.artifact_path("00a").unwrap();

        assert_ne!(
            upper_path.to_string_lossy().to_lowercase(),
            lower_path.to_string_lossy().to_lowercase()
        );
        assert_eq!(
            upper_path.strip_prefix(directory.path()).unwrap(),
            Path::new("v1").join("303047").join("plan.md")
        );
        store
            .update("00G", EMPTY_PLAN_REVISION, "upper".to_owned())
            .await
            .unwrap();
        store
            .update("00a", EMPTY_PLAN_REVISION, "lower".to_owned())
            .await
            .unwrap();
        assert_eq!(
            store.current("00G").await.unwrap().unwrap().content,
            "upper"
        );
        assert_eq!(
            store.current("00a").await.unwrap().unwrap().content,
            "lower"
        );
    }

    #[tokio::test]
    async fn maximum_length_session_id_uses_bounded_path_components() {
        let directory = tempfile::tempdir().unwrap();
        let store = DiskPlanStore::new(directory.path());
        let session_id = "x".repeat(MAX_SESSION_ID_BYTES);
        let path = store.artifact_path(&session_id).unwrap();
        let relative = path.strip_prefix(directory.path()).unwrap();

        for component in relative.components() {
            let component = component.as_os_str().to_string_lossy();
            assert!(component.len() <= SESSION_PATH_COMPONENT_HEX_BYTES);
            assert_eq!(component, component.to_ascii_lowercase());
        }
        store
            .update(&session_id, EMPTY_PLAN_REVISION, "long id".to_owned())
            .await
            .unwrap();
        assert_eq!(
            store.current(&session_id).await.unwrap().unwrap().content,
            "long id"
        );
    }

    #[tokio::test]
    async fn cancelled_update_finishes_and_can_be_reconciled_without_temp_files() {
        let directory = tempfile::tempdir().unwrap();
        let store = DiskPlanStore::new(directory.path());
        let initial = store
            .update("session", EMPTY_PLAN_REVISION, "initial".to_owned())
            .await
            .unwrap();
        let lock_file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(store.lock_path("session").unwrap())
            .unwrap();
        lock_file.lock().unwrap();

        let timed_out = tokio::time::timeout(
            Duration::from_millis(50),
            store.update(
                "session",
                initial.revision,
                "committed after cancellation".to_owned(),
            ),
        )
        .await;
        assert!(timed_out.is_err());
        lock_file.unlock().unwrap();

        let reconciled = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let plan = store.current("session").await.unwrap().unwrap();
                if plan.revision == initial.revision + 1 {
                    break plan;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("detached blocking transaction did not finish");
        assert_eq!(reconciled.content, "committed after cancellation");
        assert_no_temporary_files(directory.path());
    }

    #[tokio::test]
    async fn encoded_filename_prevents_path_traversal() {
        let directory = tempfile::tempdir().unwrap();
        let store = DiskPlanStore::new(directory.path());
        let session_id = "../../outside";

        store
            .update(session_id, EMPTY_PLAN_REVISION, "safe".to_owned())
            .await
            .unwrap();
        let path = store.artifact_path(session_id).unwrap();

        assert!(path.starts_with(directory.path()));
        assert!(path.is_file());
        assert_eq!(
            store.current(session_id).await.unwrap().unwrap().content,
            "safe"
        );
        assert!(!directory.path().parent().unwrap().join("outside").exists());
    }

    #[tokio::test]
    async fn invalid_session_ids_are_rejected() {
        let memory = InMemoryPlanStore::new();
        assert!(matches!(
            memory.current(" \n\t").await.unwrap_err(),
            PlanStoreError::InvalidSessionId(_)
        ));

        let directory = tempfile::tempdir().unwrap();
        let disk = DiskPlanStore::new(directory.path());
        let too_long = "x".repeat(MAX_SESSION_ID_BYTES + 1);
        assert!(matches!(
            disk.update(&too_long, EMPTY_PLAN_REVISION, String::new())
                .await
                .unwrap_err(),
            PlanStoreError::InvalidSessionId(_)
        ));
        assert!(directory.path().read_dir().unwrap().next().is_none());
    }

    #[tokio::test]
    async fn malformed_revision_header_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let store = DiskPlanStore::new(directory.path());
        let path = store.artifact_path("session").unwrap();
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&path, "# Plan without metadata\nDo work")
            .await
            .unwrap();

        assert!(matches!(
            store.current("session").await.unwrap_err(),
            PlanStoreError::InvalidData { .. }
        ));
    }

    #[tokio::test]
    async fn crlf_revision_header_is_accepted() {
        let directory = tempfile::tempdir().unwrap();
        let store = DiskPlanStore::new(directory.path());
        let path = store.artifact_path("session").unwrap();
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(
            &path,
            "<!-- phi-plan-revision: 7 -->\r\n# Plan\r\n\r\nDo work",
        )
        .await
        .unwrap();

        let plan = store.current("session").await.unwrap().unwrap();
        assert_eq!(plan.revision, 7);
        assert_eq!(plan.content, "# Plan\r\n\r\nDo work");
    }

    #[tokio::test]
    async fn oversized_plan_is_rejected_without_creating_an_artifact() {
        let directory = tempfile::tempdir().unwrap();
        let store = DiskPlanStore::new(directory.path());
        let oversized = "x".repeat(MAX_PLAN_BYTES + 1);

        assert!(matches!(
            store
                .update("session", EMPTY_PLAN_REVISION, oversized)
                .await
                .unwrap_err(),
            PlanStoreError::PlanTooLarge { .. }
        ));
        assert_eq!(store.current("session").await.unwrap(), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn disk_plan_is_owner_readable_and_writable_only() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let store = DiskPlanStore::new(directory.path());
        store
            .update("session", EMPTY_PLAN_REVISION, "private".to_owned())
            .await
            .unwrap();

        let artifact_mode = tokio::fs::metadata(store.artifact_path("session").unwrap())
            .await
            .unwrap()
            .permissions()
            .mode();
        let lock_mode = tokio::fs::metadata(store.lock_path("session").unwrap())
            .await
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(artifact_mode & 0o777, 0o600);
        assert_eq!(lock_mode & 0o777, 0o600);
    }
}

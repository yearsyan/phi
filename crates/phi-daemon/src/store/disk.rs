use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

#[cfg(test)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use tokio::{
    fs,
    io::{AsyncWriteExt, BufWriter},
};

use super::{ControlStore, ControlStoreError, SessionRecord};
use crate::runtime::SessionId;

const FILE_PREFIX: &str = "session-";
const FILE_SUFFIX: &str = ".json";

/// JSON-backed control metadata with one file per activated session.
#[derive(Clone, Debug)]
pub struct DiskControlStore {
    root: PathBuf,
    #[cfg(test)]
    fail_next_directory_sync: Arc<AtomicBool>,
}

impl DiskControlStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            #[cfg(test)]
            fail_next_directory_sync: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn session_path(&self, session_id: SessionId) -> PathBuf {
        self.root
            .join(format!("{FILE_PREFIX}{session_id}{FILE_SUFFIX}"))
    }

    fn temporary_path(&self, session_id: SessionId) -> PathBuf {
        self.root.join(format!(
            ".{FILE_PREFIX}{session_id}-{}.tmp",
            SessionId::new()
        ))
    }

    async fn read_record(
        &self,
        path: &Path,
        expected: SessionId,
    ) -> Result<SessionRecord, ControlStoreError> {
        let bytes = fs::read(path)
            .await
            .map_err(|source| ControlStoreError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        let record: SessionRecord =
            serde_json::from_slice(&bytes).map_err(|source| ControlStoreError::Serialization {
                path: path.to_path_buf(),
                source,
            })?;
        if record.id != expected {
            return Err(ControlStoreError::SessionIdMismatch {
                path: path.to_path_buf(),
                expected,
                actual: record.id,
            });
        }
        Ok(record)
    }

    /// Best-effort durability barrier after an operation has been published.
    ///
    /// The hard-link/rename/unlink is the logical commit point: once it
    /// succeeds, returning an error would incorrectly invite the caller to
    /// retry or compensate an operation that is already visible. A directory
    /// sync failure therefore makes crash durability uncertain and is logged,
    /// but does not turn the committed operation into a failure.
    async fn sync_directory_after_commit(&self, operation: &'static str, session_id: SessionId) {
        #[cfg(test)]
        let result = if self.fail_next_directory_sync.swap(false, Ordering::SeqCst) {
            Err(std::io::Error::other(
                "injected post-commit directory sync failure",
            ))
        } else {
            sync_directory(&self.root).await
        };
        #[cfg(not(test))]
        let result = sync_directory(&self.root).await;

        if let Err(source) = result {
            tracing::warn!(
                %session_id,
                operation,
                path = %self.root.display(),
                error = %source,
                "control metadata commit is visible, but its directory sync failed"
            );
        }
    }

    #[cfg(test)]
    fn fail_next_directory_sync(&self) {
        self.fail_next_directory_sync.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl ControlStore for DiskControlStore {
    async fn create_session(&self, session: SessionRecord) -> Result<(), ControlStoreError> {
        fs::create_dir_all(&self.root)
            .await
            .map_err(|source| ControlStoreError::Io {
                path: self.root.clone(),
                source,
            })?;
        let path = self.session_path(session.id);
        let bytes = serialize_record(&path, &session)?;
        let temporary = self.temporary_path(session.id);
        let file = private_file_options()
            .open(&temporary)
            .await
            .map_err(|source| ControlStoreError::Io {
                path: temporary.clone(),
                source,
            })?;

        if let Err(source) = write_and_sync(file, &bytes).await {
            let _ = fs::remove_file(&temporary).await;
            return Err(ControlStoreError::Io {
                path: temporary,
                source,
            });
        }

        // A hard link publishes the already-fsynced inode atomically and,
        // unlike rename on Unix, never replaces an existing session record.
        let published = fs::hard_link(&temporary, &path).await;
        let _ = fs::remove_file(&temporary).await;
        match published {
            Ok(()) => {}
            Err(source) if source.kind() == ErrorKind::AlreadyExists => {
                return Err(ControlStoreError::AlreadyExists {
                    session_id: session.id,
                });
            }
            Err(source) => return Err(ControlStoreError::Io { path, source }),
        }
        self.sync_directory_after_commit("create", session.id).await;
        Ok(())
    }

    async fn update_session(&self, session: SessionRecord) -> Result<(), ControlStoreError> {
        let path = self.session_path(session.id);
        match fs::metadata(&path).await {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => {
                return Err(ControlStoreError::Io {
                    path,
                    source: std::io::Error::other("session metadata path is not a regular file"),
                });
            }
            Err(source) if source.kind() == ErrorKind::NotFound => {
                return Err(ControlStoreError::NotFound {
                    session_id: session.id,
                });
            }
            Err(source) => {
                return Err(ControlStoreError::Io { path, source });
            }
        }

        let bytes = serialize_record(&path, &session)?;
        let temporary = self.temporary_path(session.id);
        let file = private_file_options()
            .open(&temporary)
            .await
            .map_err(|source| ControlStoreError::Io {
                path: temporary.clone(),
                source,
            })?;
        if let Err(source) = write_and_sync(file, &bytes).await {
            let _ = fs::remove_file(&temporary).await;
            return Err(ControlStoreError::Io {
                path: temporary,
                source,
            });
        }
        if let Err(source) = fs::rename(&temporary, &path).await {
            let _ = fs::remove_file(&temporary).await;
            return Err(ControlStoreError::Io { path, source });
        }
        self.sync_directory_after_commit("update", session.id).await;
        Ok(())
    }

    async fn get_session(
        &self,
        session_id: SessionId,
    ) -> Result<Option<SessionRecord>, ControlStoreError> {
        let path = self.session_path(session_id);
        match self.read_record(&path, session_id).await {
            Ok(record) => Ok(Some(record)),
            Err(ControlStoreError::Io { source, .. }) if source.kind() == ErrorKind::NotFound => {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    async fn list_sessions(&self) -> Result<Vec<SessionRecord>, ControlStoreError> {
        let mut directory = match fs::read_dir(&self.root).await {
            Ok(directory) => directory,
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(ControlStoreError::Io {
                    path: self.root.clone(),
                    source,
                });
            }
        };
        let mut sessions = Vec::new();
        loop {
            let entry = directory
                .next_entry()
                .await
                .map_err(|source| ControlStoreError::Io {
                    path: self.root.clone(),
                    source,
                })?;
            let Some(entry) = entry else {
                break;
            };
            let path = entry.path();
            let Some(session_id) = session_id_from_path(&path) else {
                continue;
            };
            let file_type = entry
                .file_type()
                .await
                .map_err(|source| ControlStoreError::Io {
                    path: path.clone(),
                    source,
                })?;
            if file_type.is_file() {
                sessions.push(self.read_record(&path, session_id).await?);
            }
        }
        sessions.sort_unstable_by_key(|session| session.id.as_uuid());
        Ok(sessions)
    }

    async fn delete_session(&self, session_id: SessionId) -> Result<bool, ControlStoreError> {
        let path = self.session_path(session_id);
        match fs::remove_file(&path).await {
            Ok(()) => {
                self.sync_directory_after_commit("delete", session_id).await;
                Ok(true)
            }
            Err(source) if source.kind() == ErrorKind::NotFound => Ok(false),
            Err(source) => Err(ControlStoreError::Io { path, source }),
        }
    }
}

fn serialize_record(path: &Path, session: &SessionRecord) -> Result<Vec<u8>, ControlStoreError> {
    let mut bytes =
        serde_json::to_vec_pretty(session).map_err(|source| ControlStoreError::Serialization {
            path: path.to_path_buf(),
            source,
        })?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn private_file_options() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    options
}

async fn write_and_sync(file: fs::File, bytes: &[u8]) -> Result<(), std::io::Error> {
    let mut writer = BufWriter::new(file);
    writer.write_all(bytes).await?;
    writer.flush().await?;
    writer.get_ref().sync_all().await
}

async fn sync_directory(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        fs::File::open(path).await?.sync_all().await
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn session_id_from_path(path: &Path) -> Option<SessionId> {
    let name = path.file_name()?.to_str()?;
    let id = name.strip_prefix(FILE_PREFIX)?.strip_suffix(FILE_SUFFIX)?;
    id.parse().ok()
}

#[cfg(test)]
mod tests {
    use phi::{ReasoningEffort, Workspace};

    use super::*;

    fn temporary_directory() -> PathBuf {
        std::env::temp_dir().join(format!("phi-daemon-control-store-{}", SessionId::new()))
    }

    fn record() -> SessionRecord {
        SessionRecord::new(
            SessionId::new(),
            "default",
            "model-1",
            Some(ReasoningEffort::Medium),
        )
        .with_workspace(Workspace::new("/workspace/project"))
    }

    #[tokio::test]
    async fn creates_lists_updates_and_deletes_sessions() {
        let root = temporary_directory();
        let store = DiskControlStore::new(&root);
        let mut first = record();
        let second = record();

        assert!(store.list_sessions().await.unwrap().is_empty());
        store.create_session(second.clone()).await.unwrap();
        store.create_session(first.clone()).await.unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(store.session_path(first.id))
                    .await
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert_eq!(
            store.get_session(first.id).await.unwrap(),
            Some(first.clone())
        );

        first.model = "model-2".to_owned();
        first.title = Some("Disk-backed session".to_owned());
        first.pinned = true;
        first.reasoning_effort = Some(ReasoningEffort::High);
        first.config_revision = 1;
        store.update_session(first.clone()).await.unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(store.session_path(first.id))
                    .await
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert_eq!(
            store.get_session(first.id).await.unwrap(),
            Some(first.clone())
        );

        let mut expected = vec![first.clone(), second];
        expected.sort_unstable_by_key(|session| session.id.as_uuid());
        assert_eq!(store.list_sessions().await.unwrap(), expected);
        assert!(store.delete_session(first.id).await.unwrap());
        assert!(!store.delete_session(first.id).await.unwrap());
        assert_eq!(store.get_session(first.id).await.unwrap(), None);

        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn create_new_does_not_overwrite_an_existing_record() {
        let root = temporary_directory();
        let store = DiskControlStore::new(&root);
        let record = record();
        store.create_session(record.clone()).await.unwrap();

        let mut duplicate = record.clone();
        duplicate.model = "different".to_owned();
        assert!(matches!(
            store.create_session(duplicate).await,
            Err(ControlStoreError::AlreadyExists { session_id }) if session_id == record.id
        ));
        assert_eq!(store.get_session(record.id).await.unwrap(), Some(record));

        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn reads_legacy_session_metadata_without_workspace() {
        let root = temporary_directory();
        let store = DiskControlStore::new(&root);
        let session_id = SessionId::new();
        fs::create_dir_all(&root).await.unwrap();
        let path = store.session_path(session_id);
        fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "id": session_id,
                "profile_id": "default",
                "model": "legacy-model",
                "reasoning_effort": null,
                "config_revision": 0
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let record = store.get_session(session_id).await.unwrap().unwrap();
        assert_eq!(record.workspace, None);
        assert_eq!(record.agent_profile, None);
        assert_eq!(record.title, None);
        assert!(!record.pinned);

        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn post_commit_directory_sync_failure_still_reports_success() {
        let root = temporary_directory();
        let store = DiskControlStore::new(&root);
        let mut record = record();

        // The hard link has already made the record visible when the injected
        // sync failure occurs. Report success so a caller does not retry the
        // no-replace create and receive a misleading AlreadyExists error.
        store.fail_next_directory_sync();
        store.create_session(record.clone()).await.unwrap();
        assert_eq!(
            store.get_session(record.id).await.unwrap(),
            Some(record.clone())
        );

        // Rename is likewise the update commit point. The in-memory caller may
        // safely advance to this exact version even when directory durability
        // cannot be confirmed.
        record.model = "model-after-sync-failure".to_owned();
        record.config_revision = 1;
        store.fail_next_directory_sync();
        store.update_session(record.clone()).await.unwrap();
        assert_eq!(
            store.get_session(record.id).await.unwrap(),
            Some(record.clone())
        );

        // Apply the same commit rule to unlink so delete never reports that a
        // successfully removed record might still be present.
        store.fail_next_directory_sync();
        assert!(store.delete_session(record.id).await.unwrap());
        assert_eq!(store.get_session(record.id).await.unwrap(), None);

        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn update_requires_an_existing_record() {
        let root = temporary_directory();
        let store = DiskControlStore::new(&root);
        let record = record();

        assert!(matches!(
            store.update_session(record.clone()).await,
            Err(ControlStoreError::NotFound { session_id }) if session_id == record.id
        ));
        assert_eq!(store.get_session(record.id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn reports_the_path_and_source_for_invalid_json() {
        let root = temporary_directory();
        let store = DiskControlStore::new(&root);
        let session_id = SessionId::new();
        fs::create_dir_all(&root).await.unwrap();
        let path = store.session_path(session_id);
        fs::write(&path, b"not json").await.unwrap();

        assert!(matches!(
            store.get_session(session_id).await,
            Err(ControlStoreError::Serialization { path: error_path, .. }) if error_path == path
        ));

        fs::remove_dir_all(root).await.unwrap();
    }
}

use std::{
    collections::HashMap,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
};

#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    fs,
    io::AsyncWriteExt,
    sync::{Mutex, RwLock},
};

use crate::{
    Workspace,
    tool::{AgentMode, CapabilityMode},
    types::{Message, TokenUsage},
};

const DISK_FORMAT_VERSION: u32 = 1;
const MAX_SESSION_ID_BYTES: usize = 180;

/// Conversation state required to resume an agent, including opaque provider
/// replay data attached to assistant messages.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: String,
    /// Immutable working directory associated with this session.
    ///
    /// `None` preserves compatibility with sessions created before workspace
    /// association was introduced.
    #[serde(default)]
    pub workspace: Option<Workspace>,
    pub messages: Vec<Message>,
    pub last_usage: Option<TokenUsage>,
    pub cumulative_usage: TokenUsage,
    /// Execution mode restored when this session is resumed.
    ///
    /// The default preserves compatibility with snapshots written before
    /// modes were introduced.
    #[serde(default)]
    pub mode: AgentMode,
    /// Maximum tool capability restored when this session is resumed.
    ///
    /// Full access preserves compatibility with snapshots written before
    /// capability modes were introduced.
    #[serde(default)]
    pub capability_mode: CapabilityMode,
}

impl SessionSnapshot {
    pub fn new(id: impl Into<String>, messages: Vec<Message>) -> Result<Self, StorageError> {
        let id = id.into();
        validate_session_id(&id)?;
        Ok(Self {
            id,
            workspace: None,
            messages,
            last_usage: None,
            cumulative_usage: TokenUsage::default(),
            mode: AgentMode::default(),
            capability_mode: CapabilityMode::default(),
        })
    }

    pub fn with_workspace(mut self, workspace: Workspace) -> Self {
        self.workspace = Some(workspace);
        self
    }
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("invalid session ID: {0}")]
    InvalidSessionId(String),

    #[error("session storage I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid session data: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("invalid JSONL record at {path}, line {line}: {source}")]
    InvalidLogRecord {
        path: PathBuf,
        line: usize,
        #[source]
        source: serde_json::Error,
    },

    #[error("unsupported disk session format version {0}")]
    UnsupportedFormatVersion(u32),

    #[error("stored session ID {actual:?} does not match requested ID {expected:?}")]
    SessionIdMismatch { expected: String, actual: String },

    #[error(
        "session {session_id:?} is bound to workspace {stored:?}, not requested workspace {requested:?}"
    )]
    WorkspaceMismatch {
        session_id: String,
        stored: PathBuf,
        requested: Option<PathBuf>,
    },

    #[error("invalid transcript for session {session_id:?}: {message}")]
    InvalidTranscript { session_id: String, message: String },
}

/// Persistence boundary for normalized session snapshots.
#[async_trait]
pub trait SessionStorage: Send + Sync {
    async fn load(&self, session_id: &str) -> Result<Option<SessionSnapshot>, StorageError>;

    async fn save(&self, session: &SessionSnapshot) -> Result<(), StorageError>;

    /// Persists a snapshot whose first `previous_message_count` messages are
    /// already durable and unchanged.
    ///
    /// The default implementation preserves compatibility with snapshot-only
    /// stores. Append-oriented stores can override this method to avoid
    /// re-reading and comparing the complete transcript on every checkpoint.
    async fn save_incremental(
        &self,
        session: &SessionSnapshot,
        previous_message_count: usize,
    ) -> Result<(), StorageError> {
        let _ = previous_message_count;
        self.save(session).await
    }

    /// Persists a snapshot whose first `unchanged_message_count` messages are
    /// already durable and unchanged, replacing only the transcript tail.
    ///
    /// Snapshot-only stores may use the default full-save implementation.
    async fn save_replacing_from(
        &self,
        session: &SessionSnapshot,
        unchanged_message_count: usize,
    ) -> Result<(), StorageError> {
        let _ = unchanged_message_count;
        self.save(session).await
    }

    async fn delete(&self, session_id: &str) -> Result<(), StorageError>;
}

#[async_trait]
impl<T> SessionStorage for Arc<T>
where
    T: SessionStorage + ?Sized,
{
    async fn load(&self, session_id: &str) -> Result<Option<SessionSnapshot>, StorageError> {
        (**self).load(session_id).await
    }

    async fn save(&self, session: &SessionSnapshot) -> Result<(), StorageError> {
        (**self).save(session).await
    }

    async fn save_incremental(
        &self,
        session: &SessionSnapshot,
        previous_message_count: usize,
    ) -> Result<(), StorageError> {
        (**self)
            .save_incremental(session, previous_message_count)
            .await
    }

    async fn save_replacing_from(
        &self,
        session: &SessionSnapshot,
        unchanged_message_count: usize,
    ) -> Result<(), StorageError> {
        (**self)
            .save_replacing_from(session, unchanged_message_count)
            .await
    }

    async fn delete(&self, session_id: &str) -> Result<(), StorageError> {
        (**self).delete(session_id).await
    }
}

/// Process-local storage useful for tests and short-lived applications.
#[derive(Clone, Default)]
pub struct InMemorySessionStorage {
    sessions: Arc<RwLock<HashMap<String, SessionSnapshot>>>,
}

impl InMemorySessionStorage {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SessionStorage for InMemorySessionStorage {
    async fn load(&self, session_id: &str) -> Result<Option<SessionSnapshot>, StorageError> {
        validate_session_id(session_id)?;
        Ok(self.sessions.read().await.get(session_id).cloned())
    }

    async fn save(&self, session: &SessionSnapshot) -> Result<(), StorageError> {
        validate_session_id(&session.id)?;
        let mut sessions = self.sessions.write().await;
        if let Some(current) = sessions.get(&session.id) {
            validate_workspace_binding(
                &session.id,
                current.workspace.as_ref(),
                session.workspace.as_ref(),
            )?;
        }
        sessions.insert(session.id.clone(), session.clone());
        Ok(())
    }

    async fn delete(&self, session_id: &str) -> Result<(), StorageError> {
        validate_session_id(session_id)?;
        self.sessions.write().await.remove(session_id);
        Ok(())
    }
}

/// Incremental, append-only JSONL session storage rooted at a directory.
#[derive(Clone, Debug)]
pub struct DiskSessionStorage {
    root: PathBuf,
    io_locks: Arc<Mutex<HashMap<String, Weak<Mutex<()>>>>>,
    cursors: Arc<Mutex<HashMap<String, LogCursor>>>,
    #[cfg(test)]
    fail_next_post_write_sync: Arc<AtomicBool>,
}

impl DiskSessionStorage {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            io_locks: Arc::new(Mutex::new(HashMap::new())),
            cursors: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(test)]
            fail_next_post_write_sync: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn session_path(&self, session_id: &str) -> Result<PathBuf, StorageError> {
        validate_session_id(session_id)?;
        let encoded = URL_SAFE_NO_PAD.encode(session_id.as_bytes());
        Ok(self.root.join(format!("session-{encoded}.jsonl")))
    }

    async fn session_io_lock(&self, session_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.io_locks.lock().await;
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(session_id).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(Mutex::new(()));
        locks.insert(session_id.to_owned(), Arc::downgrade(&lock));
        lock
    }

    fn inject_post_write_sync_failure(&self) -> bool {
        #[cfg(test)]
        {
            self.fail_next_post_write_sync.swap(false, Ordering::SeqCst)
        }
        #[cfg(not(test))]
        {
            false
        }
    }

    #[cfg(test)]
    fn fail_next_post_write_sync(&self) {
        self.fail_next_post_write_sync.store(true, Ordering::SeqCst);
    }
}

#[derive(Serialize)]
struct StoredSessionRecordRef<'a> {
    format_version: u32,
    session_id: &'a str,
    #[serde(flatten)]
    event: StoredSessionEventRef<'a>,
}

#[derive(Deserialize)]
struct StoredSessionRecord {
    format_version: u32,
    session_id: String,
    #[serde(flatten)]
    event: StoredSessionEvent,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StoredSessionEventRef<'a> {
    Append {
        messages: &'a [Message],
        workspace: Option<&'a Workspace>,
        last_usage: Option<TokenUsage>,
        cumulative_usage: TokenUsage,
        mode: AgentMode,
        capability_mode: CapabilityMode,
    },
    Replace {
        messages: &'a [Message],
        workspace: Option<&'a Workspace>,
        last_usage: Option<TokenUsage>,
        cumulative_usage: TokenUsage,
        mode: AgentMode,
        capability_mode: CapabilityMode,
    },
    ReplaceTail {
        from: usize,
        messages: &'a [Message],
        workspace: Option<&'a Workspace>,
        last_usage: Option<TokenUsage>,
        cumulative_usage: TokenUsage,
        mode: AgentMode,
        capability_mode: CapabilityMode,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StoredSessionEvent {
    Append {
        messages: Vec<Message>,
        #[serde(default)]
        workspace: Option<Workspace>,
        last_usage: Option<TokenUsage>,
        cumulative_usage: TokenUsage,
        #[serde(default)]
        mode: AgentMode,
        #[serde(default)]
        capability_mode: CapabilityMode,
    },
    Replace {
        messages: Vec<Message>,
        #[serde(default)]
        workspace: Option<Workspace>,
        last_usage: Option<TokenUsage>,
        cumulative_usage: TokenUsage,
        #[serde(default)]
        mode: AgentMode,
        #[serde(default)]
        capability_mode: CapabilityMode,
    },
    ReplaceTail {
        from: usize,
        messages: Vec<Message>,
        #[serde(default)]
        workspace: Option<Workspace>,
        last_usage: Option<TokenUsage>,
        cumulative_usage: TokenUsage,
        #[serde(default)]
        mode: AgentMode,
        #[serde(default)]
        capability_mode: CapabilityMode,
    },
}

struct ParsedLog {
    snapshot: Option<SessionSnapshot>,
    valid_len: usize,
    file_len: usize,
    ends_with_newline: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LogCursor {
    message_count: usize,
    workspace: Option<Workspace>,
    last_usage: Option<TokenUsage>,
    cumulative_usage: TokenUsage,
    mode: AgentMode,
    capability_mode: CapabilityMode,
    valid_len: usize,
    file_len: usize,
    ends_with_newline: bool,
}

impl LogCursor {
    fn from_parsed(parsed: &ParsedLog) -> Self {
        let (message_count, workspace, last_usage, cumulative_usage, mode, capability_mode) =
            parsed
                .snapshot
                .as_ref()
                .map(|snapshot| {
                    (
                        snapshot.messages.len(),
                        snapshot.workspace.clone(),
                        snapshot.last_usage,
                        snapshot.cumulative_usage,
                        snapshot.mode,
                        snapshot.capability_mode,
                    )
                })
                .unwrap_or((
                    0,
                    None,
                    None,
                    TokenUsage::default(),
                    AgentMode::default(),
                    CapabilityMode::default(),
                ));
        Self {
            message_count,
            workspace,
            last_usage,
            cumulative_usage,
            mode,
            capability_mode,
            valid_len: parsed.valid_len,
            file_len: parsed.file_len,
            ends_with_newline: parsed.ends_with_newline,
        }
    }
}

#[async_trait]
impl SessionStorage for DiskSessionStorage {
    async fn load(&self, session_id: &str) -> Result<Option<SessionSnapshot>, StorageError> {
        validate_session_id(session_id)?;
        let lock = self.session_io_lock(session_id).await;
        let _guard = lock.lock().await;
        let parsed = read_log(&self.session_path(session_id)?, session_id).await?;
        self.cursors
            .lock()
            .await
            .insert(session_id.to_owned(), LogCursor::from_parsed(&parsed));
        Ok(parsed.snapshot)
    }

    async fn save(&self, session: &SessionSnapshot) -> Result<(), StorageError> {
        validate_session_id(&session.id)?;
        let lock = self.session_io_lock(&session.id).await;
        let _guard = lock.lock().await;
        let path = self.session_path(&session.id)?;
        fs::create_dir_all(&self.root)
            .await
            .map_err(|source| io_error(self.root.clone(), source))?;
        let parsed = read_log(&path, &session.id).await?;
        if let Some(previous) = parsed.snapshot.as_ref() {
            validate_workspace_binding(
                &session.id,
                previous.workspace.as_ref(),
                session.workspace.as_ref(),
            )?;
        }
        let event = match parsed.snapshot.as_ref() {
            Some(previous) if session.messages.starts_with(&previous.messages) => {
                StoredSessionEventRef::Append {
                    messages: &session.messages[previous.messages.len()..],
                    workspace: session.workspace.as_ref(),
                    last_usage: session.last_usage,
                    cumulative_usage: session.cumulative_usage,
                    mode: session.mode,
                    capability_mode: session.capability_mode,
                }
            }
            _ => StoredSessionEventRef::Replace {
                messages: &session.messages,
                workspace: session.workspace.as_ref(),
                last_usage: session.last_usage,
                cumulative_usage: session.cumulative_usage,
                mode: session.mode,
                capability_mode: session.capability_mode,
            },
        };
        let prior_message_count = parsed
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.messages.len());
        let cursor = append_record(
            &path,
            &session.id,
            event,
            &parsed,
            prior_message_count,
            self.inject_post_write_sync_failure(),
        )
        .await?;
        self.cursors.lock().await.insert(session.id.clone(), cursor);
        Ok(())
    }

    async fn save_incremental(
        &self,
        session: &SessionSnapshot,
        previous_message_count: usize,
    ) -> Result<(), StorageError> {
        validate_session_id(&session.id)?;
        if previous_message_count > session.messages.len() {
            return Err(StorageError::InvalidTranscript {
                session_id: session.id.clone(),
                message: format!(
                    "incremental save starts at message {previous_message_count}, but the snapshot contains only {} messages",
                    session.messages.len()
                ),
            });
        }

        let lock = self.session_io_lock(&session.id).await;
        let _guard = lock.lock().await;
        let path = self.session_path(&session.id)?;
        fs::create_dir_all(&self.root)
            .await
            .map_err(|source| io_error(self.root.clone(), source))?;

        let actual_file_len = match fs::metadata(&path).await {
            Ok(metadata) => metadata.len() as usize,
            Err(source) if source.kind() == ErrorKind::NotFound => 0,
            Err(source) => return Err(io_error(path.clone(), source)),
        };
        let cached = self.cursors.lock().await.get(&session.id).cloned();
        let cursor = match cached {
            Some(cursor)
                if cursor.file_len == actual_file_len
                    && cursor.message_count == previous_message_count =>
            {
                cursor
            }
            _ => {
                let parsed = read_log(&path, &session.id).await?;
                let cursor = LogCursor::from_parsed(&parsed);
                if cursor.message_count != previous_message_count {
                    return Err(StorageError::InvalidTranscript {
                        session_id: session.id.clone(),
                        message: format!(
                            "incremental save expected {previous_message_count} durable messages, but storage contains {}",
                            cursor.message_count
                        ),
                    });
                }
                cursor
            }
        };
        validate_workspace_binding(
            &session.id,
            cursor.workspace.as_ref(),
            session.workspace.as_ref(),
        )?;

        if previous_message_count == session.messages.len()
            && cursor.workspace == session.workspace
            && cursor.last_usage == session.last_usage
            && cursor.cumulative_usage == session.cumulative_usage
            && cursor.mode == session.mode
            && cursor.capability_mode == session.capability_mode
            && cursor.valid_len == cursor.file_len
        {
            return Ok(());
        }

        let parsed = ParsedLog {
            snapshot: None,
            valid_len: cursor.valid_len,
            file_len: cursor.file_len,
            ends_with_newline: cursor.ends_with_newline,
        };
        let event = StoredSessionEventRef::Append {
            messages: &session.messages[previous_message_count..],
            workspace: session.workspace.as_ref(),
            last_usage: session.last_usage,
            cumulative_usage: session.cumulative_usage,
            mode: session.mode,
            capability_mode: session.capability_mode,
        };
        let cursor = append_record(
            &path,
            &session.id,
            event,
            &parsed,
            previous_message_count,
            self.inject_post_write_sync_failure(),
        )
        .await?;
        self.cursors.lock().await.insert(session.id.clone(), cursor);
        Ok(())
    }

    async fn save_replacing_from(
        &self,
        session: &SessionSnapshot,
        unchanged_message_count: usize,
    ) -> Result<(), StorageError> {
        validate_session_id(&session.id)?;
        if unchanged_message_count > session.messages.len() {
            return Err(StorageError::InvalidTranscript {
                session_id: session.id.clone(),
                message: format!(
                    "tail replacement starts at message {unchanged_message_count}, but the snapshot contains only {} messages",
                    session.messages.len()
                ),
            });
        }

        let lock = self.session_io_lock(&session.id).await;
        let _guard = lock.lock().await;
        let path = self.session_path(&session.id)?;
        fs::create_dir_all(&self.root)
            .await
            .map_err(|source| io_error(self.root.clone(), source))?;
        let actual_file_len = match fs::metadata(&path).await {
            Ok(metadata) => metadata.len() as usize,
            Err(source) if source.kind() == ErrorKind::NotFound => 0,
            Err(source) => return Err(io_error(path.clone(), source)),
        };
        let cached = self.cursors.lock().await.get(&session.id).cloned();
        let cursor = match cached {
            Some(cursor) if cursor.file_len == actual_file_len => cursor,
            _ => {
                let parsed = read_log(&path, &session.id).await?;
                LogCursor::from_parsed(&parsed)
            }
        };
        validate_workspace_binding(
            &session.id,
            cursor.workspace.as_ref(),
            session.workspace.as_ref(),
        )?;
        if unchanged_message_count > cursor.message_count {
            return Err(StorageError::InvalidTranscript {
                session_id: session.id.clone(),
                message: format!(
                    "tail replacement keeps {unchanged_message_count} messages, but storage contains only {}",
                    cursor.message_count
                ),
            });
        }

        let parsed = ParsedLog {
            snapshot: None,
            valid_len: cursor.valid_len,
            file_len: cursor.file_len,
            ends_with_newline: cursor.ends_with_newline,
        };
        let event = StoredSessionEventRef::ReplaceTail {
            from: unchanged_message_count,
            messages: &session.messages[unchanged_message_count..],
            workspace: session.workspace.as_ref(),
            last_usage: session.last_usage,
            cumulative_usage: session.cumulative_usage,
            mode: session.mode,
            capability_mode: session.capability_mode,
        };
        let cursor = append_record(
            &path,
            &session.id,
            event,
            &parsed,
            cursor.message_count,
            self.inject_post_write_sync_failure(),
        )
        .await?;
        self.cursors.lock().await.insert(session.id.clone(), cursor);
        Ok(())
    }

    async fn delete(&self, session_id: &str) -> Result<(), StorageError> {
        validate_session_id(session_id)?;
        let lock = self.session_io_lock(session_id).await;
        let _guard = lock.lock().await;
        let path = self.session_path(session_id)?;
        let result = match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == ErrorKind::NotFound => Ok(()),
            Err(source) => Err(io_error(path, source)),
        };
        if result.is_ok() {
            self.cursors.lock().await.remove(session_id);
        }
        result
    }
}

async fn append_record(
    path: &Path,
    session_id: &str,
    event: StoredSessionEventRef<'_>,
    parsed: &ParsedLog,
    prior_message_count: usize,
    inject_post_write_sync_failure: bool,
) -> Result<LogCursor, StorageError> {
    let (message_count, workspace, last_usage, cumulative_usage, mode, capability_mode) =
        match &event {
            StoredSessionEventRef::Append {
                messages,
                workspace,
                last_usage,
                cumulative_usage,
                mode,
                capability_mode,
            } => (
                prior_message_count + messages.len(),
                (*workspace).cloned(),
                *last_usage,
                *cumulative_usage,
                *mode,
                *capability_mode,
            ),
            StoredSessionEventRef::Replace {
                messages,
                workspace,
                last_usage,
                cumulative_usage,
                mode,
                capability_mode,
            } => (
                messages.len(),
                (*workspace).cloned(),
                *last_usage,
                *cumulative_usage,
                *mode,
                *capability_mode,
            ),
            StoredSessionEventRef::ReplaceTail {
                from,
                messages,
                workspace,
                last_usage,
                cumulative_usage,
                mode,
                capability_mode,
            } => (
                from + messages.len(),
                (*workspace).cloned(),
                *last_usage,
                *cumulative_usage,
                *mode,
                *capability_mode,
            ),
        };
    let mut bytes = serde_json::to_vec(&StoredSessionRecordRef {
        format_version: DISK_FORMAT_VERSION,
        session_id,
        event,
    })?;
    bytes.push(b'\n');

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .map_err(|source| io_error(path.to_owned(), source))?;
    let file_len = file
        .metadata()
        .await
        .map_err(|source| io_error(path.to_owned(), source))?
        .len() as usize;
    if parsed.valid_len < file_len {
        file.set_len(parsed.valid_len as u64)
            .await
            .map_err(|source| io_error(path.to_owned(), source))?;
    }
    let separator_len = usize::from(parsed.valid_len > 0 && !parsed.ends_with_newline);
    if separator_len > 0 {
        file.write_all(b"\n")
            .await
            .map_err(|source| io_error(path.to_owned(), source))?;
    }
    file.write_all(&bytes)
        .await
        .map_err(|source| io_error(path.to_owned(), source))?;
    let sync_result = if inject_post_write_sync_failure {
        Err(std::io::Error::other(
            "injected session journal sync failure after write",
        ))
    } else {
        file.sync_all().await
    };
    if let Err(source) = sync_result {
        // A sync error is reported after write_all's logical commit point. The
        // complete record may therefore already be visible in the journal. If
        // callers rolled their in-memory checkpoint back unconditionally while
        // the file kept this record, a restart could restore a different mode
        // or tool outcome. Reconcile the exact appended bytes before deciding
        // whether this operation failed. This rare error path may read the
        // complete log; ordinary checkpoints remain append-only.
        drop(file);
        if !appended_record_is_complete(path, parsed.valid_len, separator_len, &bytes).await {
            return Err(io_error(path.to_owned(), source));
        }
    }

    let file_len = parsed.valid_len + separator_len + bytes.len();
    Ok(LogCursor {
        message_count,
        workspace,
        last_usage,
        cumulative_usage,
        mode,
        capability_mode,
        valid_len: file_len,
        file_len,
        ends_with_newline: true,
    })
}

async fn appended_record_is_complete(
    path: &Path,
    valid_len: usize,
    separator_len: usize,
    record: &[u8],
) -> bool {
    let Ok(bytes) = fs::read(path).await else {
        return false;
    };
    let Some(record_start) = valid_len.checked_add(separator_len) else {
        return false;
    };
    let Some(expected_len) = record_start.checked_add(record.len()) else {
        return false;
    };
    if bytes.len() != expected_len {
        return false;
    }
    if separator_len == 1 && bytes.get(valid_len) != Some(&b'\n') {
        return false;
    }
    bytes.get(record_start..expected_len) == Some(record)
}

async fn read_log(path: &Path, session_id: &str) -> Result<ParsedLog, StorageError> {
    let bytes = match fs::read(path).await {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == ErrorKind::NotFound => {
            return Ok(ParsedLog {
                snapshot: None,
                valid_len: 0,
                file_len: 0,
                ends_with_newline: true,
            });
        }
        Err(source) => return Err(io_error(path.to_owned(), source)),
    };
    let mut snapshot: Option<SessionSnapshot> = None;
    let mut valid_len = 0;
    let mut offset = 0;
    let mut ends_with_newline = true;

    for (index, segment) in bytes.split_inclusive(|byte| *byte == b'\n').enumerate() {
        let terminated = segment.last() == Some(&b'\n');
        let mut line = if terminated {
            &segment[..segment.len() - 1]
        } else {
            segment
        };
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        let line_number = index + 1;
        if line.iter().all(u8::is_ascii_whitespace) {
            valid_len = offset + segment.len();
            ends_with_newline = terminated;
            offset += segment.len();
            continue;
        }
        let record: StoredSessionRecord = match serde_json::from_slice(line) {
            Ok(record) => record,
            Err(_) if !terminated && offset + segment.len() == bytes.len() => break,
            Err(source) => {
                return Err(StorageError::InvalidLogRecord {
                    path: path.to_owned(),
                    line: line_number,
                    source,
                });
            }
        };
        if record.format_version != DISK_FORMAT_VERSION {
            return Err(StorageError::UnsupportedFormatVersion(
                record.format_version,
            ));
        }
        if record.session_id != session_id {
            return Err(StorageError::SessionIdMismatch {
                expected: session_id.to_owned(),
                actual: record.session_id,
            });
        }
        apply_record(&mut snapshot, session_id, record.event)?;
        valid_len = offset + segment.len();
        ends_with_newline = terminated;
        offset += segment.len();
    }

    Ok(ParsedLog {
        snapshot,
        valid_len,
        file_len: bytes.len(),
        ends_with_newline,
    })
}

fn apply_record(
    snapshot: &mut Option<SessionSnapshot>,
    session_id: &str,
    event: StoredSessionEvent,
) -> Result<(), StorageError> {
    match event {
        StoredSessionEvent::Append {
            messages,
            workspace,
            last_usage,
            cumulative_usage,
            mode,
            capability_mode,
        } => {
            let session = snapshot.get_or_insert_with(|| SessionSnapshot {
                id: session_id.to_owned(),
                workspace: None,
                messages: Vec::new(),
                last_usage: None,
                cumulative_usage: TokenUsage::default(),
                mode: AgentMode::default(),
                capability_mode: CapabilityMode::default(),
            });
            if workspace.is_some() {
                session.workspace = workspace;
            }
            session.messages.extend(messages);
            session.last_usage = last_usage;
            session.cumulative_usage = cumulative_usage;
            session.mode = mode;
            session.capability_mode = capability_mode;
        }
        StoredSessionEvent::Replace {
            messages,
            workspace,
            last_usage,
            cumulative_usage,
            mode,
            capability_mode,
        } => {
            *snapshot = Some(SessionSnapshot {
                id: session_id.to_owned(),
                workspace,
                messages,
                last_usage,
                cumulative_usage,
                mode,
                capability_mode,
            });
        }
        StoredSessionEvent::ReplaceTail {
            from,
            messages,
            workspace,
            last_usage,
            cumulative_usage,
            mode,
            capability_mode,
        } => {
            let session = snapshot.get_or_insert_with(|| SessionSnapshot {
                id: session_id.to_owned(),
                workspace: None,
                messages: Vec::new(),
                last_usage: None,
                cumulative_usage: TokenUsage::default(),
                mode: AgentMode::default(),
                capability_mode: CapabilityMode::default(),
            });
            if workspace.is_some() {
                session.workspace = workspace;
            }
            if from > session.messages.len() {
                return Err(StorageError::InvalidTranscript {
                    session_id: session_id.to_owned(),
                    message: format!(
                        "tail replacement keeps {from} messages, but only {} have been stored",
                        session.messages.len()
                    ),
                });
            }
            session.messages.truncate(from);
            session.messages.extend(messages);
            session.last_usage = last_usage;
            session.cumulative_usage = cumulative_usage;
            session.mode = mode;
            session.capability_mode = capability_mode;
        }
    }
    Ok(())
}

pub(crate) fn validate_session_id(session_id: &str) -> Result<(), StorageError> {
    if session_id.trim().is_empty() {
        return Err(StorageError::InvalidSessionId(
            "must not be empty".to_owned(),
        ));
    }
    if session_id.len() > MAX_SESSION_ID_BYTES {
        return Err(StorageError::InvalidSessionId(format!(
            "must not exceed {MAX_SESSION_ID_BYTES} UTF-8 bytes"
        )));
    }
    Ok(())
}

pub(crate) fn validate_workspace_binding(
    session_id: &str,
    stored: Option<&Workspace>,
    requested: Option<&Workspace>,
) -> Result<(), StorageError> {
    let Some(stored) = stored else {
        return Ok(());
    };
    if requested == Some(stored) {
        return Ok(());
    }
    Err(StorageError::WorkspaceMismatch {
        session_id: session_id.to_owned(),
        stored: stored.root().to_owned(),
        requested: requested.map(|workspace| workspace.root().to_owned()),
    })
}

fn io_error(path: PathBuf, source: std::io::Error) -> StorageError {
    StorageError::Io { path, source }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;
    use crate::types::{ContentPart, ImageUrl, Message, ProviderState};

    fn snapshot() -> SessionSnapshot {
        SessionSnapshot {
            id: "session/with spaces".to_owned(),
            workspace: Some(Workspace::new("/workspace/project")),
            messages: vec![Message::user_parts([
                ContentPart::text("hello"),
                ContentPart::image(ImageUrl::from_bytes("image/png", &[1, 2, 3])),
            ])],
            last_usage: Some(TokenUsage::new(10, 2, 1)),
            cumulative_usage: TokenUsage::new(20, 5, 2),
            mode: AgentMode::Plan,
            capability_mode: CapabilityMode::WorkspaceEdit,
        }
    }

    #[tokio::test]
    async fn in_memory_storage_round_trips_and_deletes() {
        let storage = InMemorySessionStorage::new();
        let session = snapshot();

        storage.save(&session).await.unwrap();
        assert_eq!(
            storage.load(&session.id).await.unwrap(),
            Some(session.clone())
        );
        storage.delete(&session.id).await.unwrap();
        assert_eq!(storage.load(&session.id).await.unwrap(), None);
    }

    #[test]
    fn legacy_snapshot_without_mode_defaults_to_normal_execution() {
        let snapshot: SessionSnapshot = serde_json::from_value(serde_json::json!({
            "id": "legacy",
            "messages": [],
            "last_usage": null,
            "cumulative_usage": TokenUsage::default()
        }))
        .unwrap();

        assert_eq!(snapshot.mode, AgentMode::Default);
        assert_eq!(snapshot.capability_mode, CapabilityMode::FullAccess);
        assert_eq!(snapshot.workspace, None);
    }

    #[tokio::test]
    async fn session_workspace_can_be_added_but_not_rebound() {
        let storage = InMemorySessionStorage::new();
        let mut session = SessionSnapshot::new("workspace-bound", Vec::new()).unwrap();
        storage.save(&session).await.unwrap();

        session.workspace = Some(Workspace::new("/workspace/one"));
        storage.save(&session).await.unwrap();
        session.workspace = Some(Workspace::new("/workspace/two"));

        assert!(matches!(
            storage.save(&session).await,
            Err(StorageError::WorkspaceMismatch {
                ref session_id,
                ..
            }) if session_id == "workspace-bound"
        ));
    }

    #[tokio::test]
    async fn legacy_disk_record_without_mode_still_loads() {
        let directory = tempfile::tempdir().unwrap();
        let storage = DiskSessionStorage::new(directory.path());
        let path = storage.session_path("legacy-disk").unwrap();
        std::fs::write(
            path,
            format!(
                "{}\n",
                serde_json::json!({
                    "format_version": DISK_FORMAT_VERSION,
                    "session_id": "legacy-disk",
                    "type": "replace",
                    "messages": [],
                    "last_usage": null,
                    "cumulative_usage": TokenUsage::default()
                })
            ),
        )
        .unwrap();

        let snapshot = storage.load("legacy-disk").await.unwrap().unwrap();
        assert_eq!(snapshot.mode, AgentMode::Default);
        assert_eq!(snapshot.capability_mode, CapabilityMode::FullAccess);
        assert_eq!(snapshot.workspace, None);
    }

    #[tokio::test]
    async fn disk_storage_round_trips_opaque_provider_state() {
        let directory = tempfile::tempdir().unwrap();
        let storage = DiskSessionStorage::new(directory.path());
        let mut assistant = Message::assistant(Some(crate::types::Content::text("answer")), vec![]);
        assistant.provider_state = Some(ProviderState::AnthropicMessages {
            content: vec![serde_json::json!({
                "type": "thinking",
                "thinking": "private reasoning",
                "signature": "signature-1"
            })],
        });
        let session = SessionSnapshot::new("provider-state", vec![assistant]).unwrap();

        storage.save(&session).await.unwrap();

        assert_eq!(storage.load(&session.id).await.unwrap(), Some(session));
    }

    #[tokio::test]
    async fn disk_storage_round_trips_multimodal_sessions() {
        let directory = tempfile::tempdir().unwrap();
        let storage = DiskSessionStorage::new(directory.path());
        let session = snapshot();

        storage.save(&session).await.unwrap();
        assert_eq!(
            storage.load(&session.id).await.unwrap(),
            Some(session.clone())
        );
        let mut updated = session.clone();
        updated.messages.push(Message::assistant(
            Some(crate::types::Content::text("updated")),
            Vec::new(),
        ));
        storage.save(&updated).await.unwrap();
        assert_eq!(
            storage.load(&session.id).await.unwrap(),
            Some(updated.clone())
        );

        let path = storage.session_path(&session.id).unwrap();
        let records = std::fs::read_to_string(&path).unwrap();
        let records = records
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["type"], "replace");
        assert_eq!(records[1]["type"], "append");
        assert_eq!(records[1]["messages"].as_array().unwrap().len(), 1);

        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"partial\":")
            .unwrap();
        assert_eq!(
            storage.load(&session.id).await.unwrap(),
            Some(updated.clone())
        );

        let mut repaired = updated;
        repaired.messages.push(Message::assistant(
            Some(crate::types::Content::text("after recovery")),
            Vec::new(),
        ));
        storage.save(&repaired).await.unwrap();
        assert_eq!(storage.load(&session.id).await.unwrap(), Some(repaired));

        let mut replaced = SessionSnapshot::new(&session.id, vec![Message::user("reset")]).unwrap();
        replaced.workspace = session.workspace.clone();
        replaced.cumulative_usage = TokenUsage::new(1, 1, 0);
        storage.save(&replaced).await.unwrap();
        assert_eq!(storage.load(&session.id).await.unwrap(), Some(replaced));

        let records = std::fs::read_to_string(&path).unwrap();
        let records = records
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 4);
        assert_eq!(records[2]["type"], "append");
        assert_eq!(records[3]["type"], "replace");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        storage.delete(&session.id).await.unwrap();
        assert_eq!(storage.load(&session.id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn disk_incremental_save_appends_deltas_and_repairs_partial_tail() {
        let directory = tempfile::tempdir().unwrap();
        let storage = DiskSessionStorage::new(directory.path());
        let mut session = SessionSnapshot::new("incremental", vec![Message::user("one")]).unwrap();

        storage.save_incremental(&session, 0).await.unwrap();
        session.messages.push(Message::assistant(
            Some(crate::types::Content::text("two")),
            Vec::new(),
        ));
        storage.save_incremental(&session, 1).await.unwrap();

        let path = storage.session_path(&session.id).unwrap();
        let records = std::fs::read_to_string(&path).unwrap();
        let records = records
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 2);
        assert!(records.iter().all(|record| record["type"] == "append"));
        assert_eq!(records[0]["messages"].as_array().unwrap().len(), 1);
        assert_eq!(records[1]["messages"].as_array().unwrap().len(), 1);

        // An unchanged checkpoint is a no-op rather than an empty log record.
        storage.save_incremental(&session, 2).await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 2);

        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"partial\":")
            .unwrap();
        session.messages.push(Message::user("three"));
        storage.save_incremental(&session, 2).await.unwrap();

        assert_eq!(storage.load(&session.id).await.unwrap(), Some(session));
        let records = std::fs::read_to_string(path).unwrap();
        assert_eq!(records.lines().count(), 3);
        assert!(
            records
                .lines()
                .all(|line| serde_json::from_str::<serde_json::Value>(line).is_ok())
        );
    }

    #[tokio::test]
    async fn disk_tail_replacement_does_not_repeat_the_unchanged_history() {
        let directory = tempfile::tempdir().unwrap();
        let storage = DiskSessionStorage::new(directory.path());
        let mut session = SessionSnapshot::new(
            "replace-tail",
            vec![
                Message::user("prompt"),
                Message::assistant(
                    None,
                    vec![crate::types::ToolCall::new(
                        "call-1",
                        "side_effect",
                        serde_json::json!({}),
                    )],
                ),
                Message::tool_result("call-1", "outcome unknown", true),
            ],
        )
        .unwrap();
        storage.save_incremental(&session, 0).await.unwrap();

        session.messages[2] = Message::tool_result("call-1", "completed", false);
        storage.save_replacing_from(&session, 1).await.unwrap();

        assert_eq!(storage.load(&session.id).await.unwrap(), Some(session));
        let path = storage.session_path("replace-tail").unwrap();
        let records = std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["type"], "append");
        assert_eq!(records[1]["type"], "replace_tail");
        assert_eq!(records[1]["from"], 1);
        assert_eq!(records[1]["messages"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn post_write_sync_error_keeps_a_complete_tail_replacement_committed() {
        let directory = tempfile::tempdir().unwrap();
        let storage = DiskSessionStorage::new(directory.path());
        let mut session = SessionSnapshot::new(
            "post-write-sync",
            vec![
                Message::user("approve"),
                Message::assistant(
                    None,
                    vec![crate::types::ToolCall::new(
                        "call-exit",
                        "exit_plan_mode",
                        serde_json::json!({}),
                    )],
                ),
                Message::tool_result("call-exit", "outcome unknown", true),
            ],
        )
        .unwrap();
        session.mode = AgentMode::Plan;
        storage.save_incremental(&session, 0).await.unwrap();

        session.messages[2] = Message::tool_result("call-exit", "approved", false);
        session.mode = AgentMode::Default;
        storage.fail_next_post_write_sync();
        storage.save_replacing_from(&session, 1).await.unwrap();

        // Reopen through a new storage instance so the assertion observes only
        // the journal, not the writer's in-memory cursor.
        let reopened = DiskSessionStorage::new(directory.path());
        assert_eq!(
            reopened.load(&session.id).await.unwrap(),
            Some(session),
            "a complete post-write record must not be rolled back in memory while remaining visible after restart"
        );
    }
}

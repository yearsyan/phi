use std::{
    collections::HashMap,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    fs,
    io::AsyncWriteExt,
    sync::{Mutex, RwLock},
};

use crate::types::{Message, TokenUsage};

const DISK_FORMAT_VERSION: u32 = 1;
const MAX_SESSION_ID_BYTES: usize = 180;

/// Conversation state required to resume an agent, including opaque provider
/// replay data attached to assistant messages.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: String,
    pub messages: Vec<Message>,
    pub last_usage: Option<TokenUsage>,
    pub cumulative_usage: TokenUsage,
}

impl SessionSnapshot {
    pub fn new(id: impl Into<String>, messages: Vec<Message>) -> Result<Self, StorageError> {
        let id = id.into();
        validate_session_id(&id)?;
        Ok(Self {
            id,
            messages,
            last_usage: None,
            cumulative_usage: TokenUsage::default(),
        })
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
}

/// Persistence boundary for normalized session snapshots.
#[async_trait]
pub trait SessionStorage: Send + Sync {
    async fn load(&self, session_id: &str) -> Result<Option<SessionSnapshot>, StorageError>;

    async fn save(&self, session: &SessionSnapshot) -> Result<(), StorageError>;

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
        self.sessions
            .write()
            .await
            .insert(session.id.clone(), session.clone());
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
    io_lock: Arc<Mutex<()>>,
}

impl DiskSessionStorage {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            io_lock: Arc::new(Mutex::new(())),
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
        last_usage: Option<TokenUsage>,
        cumulative_usage: TokenUsage,
    },
    Replace {
        messages: &'a [Message],
        last_usage: Option<TokenUsage>,
        cumulative_usage: TokenUsage,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StoredSessionEvent {
    Append {
        messages: Vec<Message>,
        last_usage: Option<TokenUsage>,
        cumulative_usage: TokenUsage,
    },
    Replace {
        messages: Vec<Message>,
        last_usage: Option<TokenUsage>,
        cumulative_usage: TokenUsage,
    },
}

struct ParsedLog {
    snapshot: Option<SessionSnapshot>,
    valid_len: usize,
    ends_with_newline: bool,
}

#[async_trait]
impl SessionStorage for DiskSessionStorage {
    async fn load(&self, session_id: &str) -> Result<Option<SessionSnapshot>, StorageError> {
        let _guard = self.io_lock.lock().await;
        Ok(read_log(&self.session_path(session_id)?, session_id)
            .await?
            .snapshot)
    }

    async fn save(&self, session: &SessionSnapshot) -> Result<(), StorageError> {
        let _guard = self.io_lock.lock().await;
        let path = self.session_path(&session.id)?;
        fs::create_dir_all(&self.root)
            .await
            .map_err(|source| io_error(self.root.clone(), source))?;
        let parsed = read_log(&path, &session.id).await?;
        let event = match parsed.snapshot.as_ref() {
            Some(previous) if session.messages.starts_with(&previous.messages) => {
                StoredSessionEventRef::Append {
                    messages: &session.messages[previous.messages.len()..],
                    last_usage: session.last_usage,
                    cumulative_usage: session.cumulative_usage,
                }
            }
            _ => StoredSessionEventRef::Replace {
                messages: &session.messages,
                last_usage: session.last_usage,
                cumulative_usage: session.cumulative_usage,
            },
        };
        let mut bytes = serde_json::to_vec(&StoredSessionRecordRef {
            format_version: DISK_FORMAT_VERSION,
            session_id: &session.id,
            event,
        })?;
        bytes.push(b'\n');

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|source| io_error(path.clone(), source))?;
        let file_len = file
            .metadata()
            .await
            .map_err(|source| io_error(path.clone(), source))?
            .len() as usize;
        if parsed.valid_len < file_len {
            file.set_len(parsed.valid_len as u64)
                .await
                .map_err(|source| io_error(path.clone(), source))?;
        }
        if parsed.valid_len > 0 && !parsed.ends_with_newline {
            file.write_all(b"\n")
                .await
                .map_err(|source| io_error(path.clone(), source))?;
        }
        file.write_all(&bytes)
            .await
            .map_err(|source| io_error(path.clone(), source))?;
        file.sync_all()
            .await
            .map_err(|source| io_error(path, source))
    }

    async fn delete(&self, session_id: &str) -> Result<(), StorageError> {
        let _guard = self.io_lock.lock().await;
        let path = self.session_path(session_id)?;
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == ErrorKind::NotFound => Ok(()),
            Err(source) => Err(io_error(path, source)),
        }
    }
}

async fn read_log(path: &Path, session_id: &str) -> Result<ParsedLog, StorageError> {
    let bytes = match fs::read(path).await {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == ErrorKind::NotFound => {
            return Ok(ParsedLog {
                snapshot: None,
                valid_len: 0,
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
        apply_record(&mut snapshot, session_id, record.event);
        valid_len = offset + segment.len();
        ends_with_newline = terminated;
        offset += segment.len();
    }

    Ok(ParsedLog {
        snapshot,
        valid_len,
        ends_with_newline,
    })
}

fn apply_record(
    snapshot: &mut Option<SessionSnapshot>,
    session_id: &str,
    event: StoredSessionEvent,
) {
    match event {
        StoredSessionEvent::Append {
            messages,
            last_usage,
            cumulative_usage,
        } => {
            let session = snapshot.get_or_insert_with(|| SessionSnapshot {
                id: session_id.to_owned(),
                messages: Vec::new(),
                last_usage: None,
                cumulative_usage: TokenUsage::default(),
            });
            session.messages.extend(messages);
            session.last_usage = last_usage;
            session.cumulative_usage = cumulative_usage;
        }
        StoredSessionEvent::Replace {
            messages,
            last_usage,
            cumulative_usage,
        } => {
            *snapshot = Some(SessionSnapshot {
                id: session_id.to_owned(),
                messages,
                last_usage,
                cumulative_usage,
            });
        }
    }
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
            messages: vec![Message::user_parts([
                ContentPart::text("hello"),
                ContentPart::image(ImageUrl::from_bytes("image/png", &[1, 2, 3])),
            ])],
            last_usage: Some(TokenUsage::new(10, 2, 1)),
            cumulative_usage: TokenUsage::new(20, 5, 2),
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
}

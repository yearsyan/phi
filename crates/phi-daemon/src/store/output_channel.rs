use std::{
    collections::HashSet,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use thiserror::Error;
use tokio::{
    fs,
    io::{AsyncWriteExt, BufWriter},
    sync::{Mutex, RwLock},
};

use crate::{
    output_channel::{
        OutputChannel, OutputChannelDefinition, OutputChannelValidationError,
        validate_output_channel_id,
    },
    runtime::SessionId,
};

#[async_trait]
pub trait OutputChannelStore: Send + Sync {
    async fn list_output_channels(&self) -> Result<Vec<OutputChannel>, OutputChannelStoreError>;

    async fn get_output_channel(
        &self,
        output_channel_id: &str,
    ) -> Result<Option<OutputChannel>, OutputChannelStoreError>;

    async fn replace_output_channel(
        &self,
        output_channel_id: &str,
        definition: OutputChannelDefinition,
    ) -> Result<OutputChannel, OutputChannelStoreError>;
}

#[derive(Clone, Default)]
pub struct MemoryOutputChannelStore {
    channels: Arc<RwLock<Vec<OutputChannel>>>,
}

impl MemoryOutputChannelStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl OutputChannelStore for MemoryOutputChannelStore {
    async fn list_output_channels(&self) -> Result<Vec<OutputChannel>, OutputChannelStoreError> {
        let mut channels = self.channels.read().await.clone();
        sort_channels(&mut channels);
        Ok(channels)
    }

    async fn get_output_channel(
        &self,
        output_channel_id: &str,
    ) -> Result<Option<OutputChannel>, OutputChannelStoreError> {
        validate_output_channel_id(output_channel_id)?;
        Ok(self
            .channels
            .read()
            .await
            .iter()
            .find(|channel| channel.output_channel_id == output_channel_id)
            .cloned())
    }

    async fn replace_output_channel(
        &self,
        output_channel_id: &str,
        definition: OutputChannelDefinition,
    ) -> Result<OutputChannel, OutputChannelStoreError> {
        validate_output_channel_id(output_channel_id)?;
        let definition = definition.normalized()?;
        let mut channels = self.channels.write().await;
        replace_channel(&mut channels, output_channel_id, definition)
    }
}

/// Atomic secret-bearing storage for daemon output channel configurations.
///
/// Telegram bot tokens are stored in this file, so it is created with
/// owner-only permissions on Unix and must never be exposed through public
/// DTOs or Debug output.
#[derive(Clone, Debug)]
pub struct DiskOutputChannelStore {
    path: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl DiskOutputChannelStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    async fn read_unlocked(&self) -> Result<Vec<OutputChannel>, OutputChannelStoreError> {
        let bytes = match fs::read(&self.path).await {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(OutputChannelStoreError::Io {
                    path: self.path.clone(),
                    source,
                });
            }
        };
        let channels = serde_json::from_slice::<Vec<OutputChannel>>(&bytes).map_err(|source| {
            OutputChannelStoreError::Serialization {
                path: self.path.clone(),
                source,
            }
        })?;
        validate_collection(&self.path, &channels)?;
        Ok(channels)
    }

    async fn write_unlocked(
        &self,
        channels: &[OutputChannel],
    ) -> Result<(), OutputChannelStoreError> {
        let parent = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .await
            .map_err(|source| OutputChannelStoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;

        let mut channels = channels.to_vec();
        sort_channels(&mut channels);
        let mut bytes = serde_json::to_vec_pretty(&channels).map_err(|source| {
            OutputChannelStoreError::Serialization {
                path: self.path.clone(),
                source,
            }
        })?;
        bytes.push(b'\n');

        let temporary = parent.join(format!(".output-channels-{}.tmp", SessionId::new()));
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file =
            options
                .open(&temporary)
                .await
                .map_err(|source| OutputChannelStoreError::Io {
                    path: temporary.clone(),
                    source,
                })?;
        if let Err(source) = write_and_sync(file, &bytes).await {
            let _ = fs::remove_file(&temporary).await;
            return Err(OutputChannelStoreError::Io {
                path: temporary,
                source,
            });
        }
        if let Err(source) = fs::rename(&temporary, &self.path).await {
            let _ = fs::remove_file(&temporary).await;
            return Err(OutputChannelStoreError::Io {
                path: self.path.clone(),
                source,
            });
        }
        if let Err(source) = sync_directory(parent).await {
            tracing::warn!(
                path = %parent.display(),
                error = %source,
                "output channel configuration is visible, but its directory sync failed"
            );
        }
        Ok(())
    }
}

#[async_trait]
impl OutputChannelStore for DiskOutputChannelStore {
    async fn list_output_channels(&self) -> Result<Vec<OutputChannel>, OutputChannelStoreError> {
        let _guard = self.lock.lock().await;
        let mut channels = self.read_unlocked().await?;
        sort_channels(&mut channels);
        Ok(channels)
    }

    async fn get_output_channel(
        &self,
        output_channel_id: &str,
    ) -> Result<Option<OutputChannel>, OutputChannelStoreError> {
        validate_output_channel_id(output_channel_id)?;
        let _guard = self.lock.lock().await;
        Ok(self
            .read_unlocked()
            .await?
            .into_iter()
            .find(|channel| channel.output_channel_id == output_channel_id))
    }

    async fn replace_output_channel(
        &self,
        output_channel_id: &str,
        definition: OutputChannelDefinition,
    ) -> Result<OutputChannel, OutputChannelStoreError> {
        validate_output_channel_id(output_channel_id)?;
        let definition = definition.normalized()?;
        let _guard = self.lock.lock().await;
        let mut channels = self.read_unlocked().await?;
        let channel = replace_channel(&mut channels, output_channel_id, definition)?;
        self.write_unlocked(&channels).await?;
        Ok(channel)
    }
}

fn replace_channel(
    channels: &mut Vec<OutputChannel>,
    output_channel_id: &str,
    definition: OutputChannelDefinition,
) -> Result<OutputChannel, OutputChannelStoreError> {
    let current = channels
        .iter_mut()
        .find(|channel| channel.output_channel_id == output_channel_id);
    let revision = match current.as_ref() {
        Some(current) => current.revision.checked_add(1).ok_or_else(|| {
            OutputChannelStoreError::RevisionExhausted {
                output_channel_id: output_channel_id.to_owned(),
            }
        })?,
        None => 1,
    };
    let channel = OutputChannel {
        output_channel_id: output_channel_id.to_owned(),
        revision,
        definition,
    };
    if let Some(current) = current {
        *current = channel.clone();
    } else {
        channels.push(channel.clone());
    }
    Ok(channel)
}

fn validate_collection(
    path: &Path,
    channels: &[OutputChannel],
) -> Result<(), OutputChannelStoreError> {
    let mut ids = HashSet::with_capacity(channels.len());
    for channel in channels {
        let normalized =
            channel
                .normalized()
                .map_err(|error| OutputChannelStoreError::InvalidCollection {
                    path: path.to_path_buf(),
                    message: format!(
                        "invalid output channel {:?}: {error}",
                        channel.output_channel_id
                    ),
                })?;
        if normalized != *channel {
            return Err(OutputChannelStoreError::InvalidCollection {
                path: path.to_path_buf(),
                message: format!(
                    "output channel {:?} is not in normalized form",
                    channel.output_channel_id
                ),
            });
        }
        if !ids.insert(channel.output_channel_id.as_str()) {
            return Err(OutputChannelStoreError::InvalidCollection {
                path: path.to_path_buf(),
                message: format!(
                    "duplicate output channel ID {:?}",
                    channel.output_channel_id
                ),
            });
        }
    }
    Ok(())
}

fn sort_channels(channels: &mut [OutputChannel]) {
    channels.sort_unstable_by(|left, right| left.output_channel_id.cmp(&right.output_channel_id));
}

async fn write_and_sync(file: fs::File, bytes: &[u8]) -> Result<(), io::Error> {
    let mut writer = BufWriter::new(file);
    writer.write_all(bytes).await?;
    writer.flush().await?;
    writer.get_ref().sync_all().await
}

async fn sync_directory(path: &Path) -> Result<(), io::Error> {
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

#[derive(Debug, Error)]
pub enum OutputChannelStoreError {
    #[error(transparent)]
    Validation(#[from] OutputChannelValidationError),

    #[error("output channel {output_channel_id:?} revision is exhausted")]
    RevisionExhausted { output_channel_id: String },

    #[error("output channel store I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("invalid output channel JSON at {path}: {source}")]
    Serialization {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("invalid output channel collection at {path}: {message}")]
    InvalidCollection { path: PathBuf, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition(token: &str) -> OutputChannelDefinition {
        OutputChannelDefinition::Telegram {
            bot_token: token.to_owned(),
            chat_id: "-1001234567890".to_owned(),
        }
    }

    #[tokio::test]
    async fn memory_store_replaces_channels_and_increments_revision() {
        let store = MemoryOutputChannelStore::new();
        let first = store
            .replace_output_channel(
                "alerts",
                definition("123456789:abcdefghijklmnopqrstuvwxyz_ABCDEFG"),
            )
            .await
            .unwrap();
        let updated = store
            .replace_output_channel(
                "alerts",
                definition("987654321:abcdefghijklmnopqrstuvwxyz_ABCDEFG"),
            )
            .await
            .unwrap();
        assert_eq!(first.revision, 1);
        assert_eq!(updated.revision, 2);
        assert_eq!(
            store.get_output_channel("alerts").await.unwrap(),
            Some(updated)
        );
    }

    #[tokio::test]
    async fn disk_store_round_trips_secrets_with_owner_only_permissions() {
        let root = std::env::temp_dir().join(format!("phi-output-channel-{}", SessionId::new()));
        let path = root.join("output-channels.json");
        let store = DiskOutputChannelStore::new(&path);
        let token = "123456789:abcdefghijklmnopqrstuvwxyz_ABCDEFG";
        let saved = store
            .replace_output_channel("alerts", definition(token))
            .await
            .unwrap();
        assert_eq!(
            store.get_output_channel("alerts").await.unwrap(),
            Some(saved)
        );
        let contents = fs::read_to_string(&path).await.unwrap();
        assert!(contents.contains(token));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).await.unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn revision_exhaustion_preserves_the_existing_channel() {
        let store = MemoryOutputChannelStore::new();
        let existing = OutputChannel {
            output_channel_id: "alerts".to_owned(),
            revision: u64::MAX,
            definition: definition("123456789:abcdefghijklmnopqrstuvwxyz_ABCDEFG")
                .normalized()
                .unwrap(),
        };
        store.channels.write().await.push(existing.clone());

        assert!(matches!(
            store
                .replace_output_channel(
                    "alerts",
                    definition("987654321:abcdefghijklmnopqrstuvwxyz_ABCDEFG")
                )
                .await,
            Err(OutputChannelStoreError::RevisionExhausted { .. })
        ));
        assert_eq!(
            store.get_output_channel("alerts").await.unwrap(),
            Some(existing)
        );
    }
}

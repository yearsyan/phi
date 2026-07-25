use std::{
    collections::{HashMap, HashSet},
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::{
    fs,
    io::{AsyncWriteExt, BufWriter},
    sync::{Mutex, RwLock},
};

use crate::{
    output_channel::{
        BotAccount, BotAccountDefinition, OutputChannel, OutputChannelDefinition,
        OutputChannelValidationError, validate_bot_account_id, validate_output_channel_id,
    },
    runtime::SessionId,
};

const OUTPUT_CHANNEL_COLLECTION_VERSION: u32 = 2;

#[async_trait]
pub trait OutputChannelStore: Send + Sync {
    async fn list_bot_accounts(&self) -> Result<Vec<BotAccount>, OutputChannelStoreError>;

    async fn get_bot_account(
        &self,
        bot_account_id: &str,
    ) -> Result<Option<BotAccount>, OutputChannelStoreError>;

    async fn replace_bot_account(
        &self,
        bot_account_id: &str,
        definition: BotAccountDefinition,
    ) -> Result<BotAccount, OutputChannelStoreError>;

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

    /// Compatibility path for clients that still submit one Telegram token
    /// together with one chat ID. The target ID becomes the implicit bot ID.
    async fn replace_legacy_telegram_channel(
        &self,
        output_channel_id: &str,
        bot_token: String,
        chat_id: String,
    ) -> Result<OutputChannel, OutputChannelStoreError>;
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputChannelCollection {
    version: u32,
    bot_accounts: Vec<BotAccount>,
    output_channels: Vec<OutputChannel>,
}

impl Default for OutputChannelCollection {
    fn default() -> Self {
        Self {
            version: OUTPUT_CHANNEL_COLLECTION_VERSION,
            bot_accounts: Vec::new(),
            output_channels: Vec::new(),
        }
    }
}

#[derive(Clone, Default)]
pub struct MemoryOutputChannelStore {
    collection: Arc<RwLock<OutputChannelCollection>>,
}

impl MemoryOutputChannelStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl OutputChannelStore for MemoryOutputChannelStore {
    async fn list_bot_accounts(&self) -> Result<Vec<BotAccount>, OutputChannelStoreError> {
        let mut accounts = self.collection.read().await.bot_accounts.clone();
        sort_bot_accounts(&mut accounts);
        Ok(accounts)
    }

    async fn get_bot_account(
        &self,
        bot_account_id: &str,
    ) -> Result<Option<BotAccount>, OutputChannelStoreError> {
        validate_bot_account_id(bot_account_id)?;
        Ok(self
            .collection
            .read()
            .await
            .bot_accounts
            .iter()
            .find(|account| account.bot_account_id == bot_account_id)
            .cloned())
    }

    async fn replace_bot_account(
        &self,
        bot_account_id: &str,
        definition: BotAccountDefinition,
    ) -> Result<BotAccount, OutputChannelStoreError> {
        validate_bot_account_id(bot_account_id)?;
        let definition = definition.normalized()?;
        let mut collection = self.collection.write().await;
        upsert_bot_account(&mut collection.bot_accounts, bot_account_id, definition)
    }

    async fn list_output_channels(&self) -> Result<Vec<OutputChannel>, OutputChannelStoreError> {
        let mut channels = self.collection.read().await.output_channels.clone();
        sort_channels(&mut channels);
        Ok(channels)
    }

    async fn get_output_channel(
        &self,
        output_channel_id: &str,
    ) -> Result<Option<OutputChannel>, OutputChannelStoreError> {
        validate_output_channel_id(output_channel_id)?;
        Ok(self
            .collection
            .read()
            .await
            .output_channels
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
        let mut collection = self.collection.write().await;
        ensure_bot_account_exists(&collection, definition.bot_account_id())?;
        upsert_output_channel(
            &mut collection.output_channels,
            output_channel_id,
            definition,
        )
    }

    async fn replace_legacy_telegram_channel(
        &self,
        output_channel_id: &str,
        bot_token: String,
        chat_id: String,
    ) -> Result<OutputChannel, OutputChannelStoreError> {
        let mut collection = self.collection.write().await;
        let mut next = collection.clone();
        let channel =
            upsert_legacy_telegram_channel(&mut next, output_channel_id, bot_token, chat_id)?;
        *collection = next;
        Ok(channel)
    }
}

/// Atomic storage for bot accounts and their recipient targets.
///
/// Bot tokens occur only in `bot_accounts`. The collection is created with
/// owner-only permissions on Unix and public DTOs never expose those tokens.
/// Version 1 was an unversioned array whose channels copied each bot token;
/// `migrate_legacy` rewrites that shape into this normalized collection.
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

    /// Atomically upgrades the legacy unversioned array when it exists.
    pub async fn migrate_legacy(&self) -> Result<bool, OutputChannelStoreError> {
        let _guard = self.lock.lock().await;
        let decoded = self.read_unlocked().await?;
        if decoded.migrated {
            self.write_unlocked(&decoded.collection).await?;
        }
        Ok(decoded.migrated)
    }

    async fn read_unlocked(&self) -> Result<DecodedCollection, OutputChannelStoreError> {
        let bytes = match fs::read(&self.path).await {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == ErrorKind::NotFound => {
                return Ok(DecodedCollection {
                    collection: OutputChannelCollection::default(),
                    migrated: false,
                });
            }
            Err(source) => {
                return Err(OutputChannelStoreError::Io {
                    path: self.path.clone(),
                    source,
                });
            }
        };
        decode_collection(&self.path, &bytes)
    }

    async fn write_unlocked(
        &self,
        collection: &OutputChannelCollection,
    ) -> Result<(), OutputChannelStoreError> {
        validate_collection(&self.path, collection)?;
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

        let mut normalized = collection.clone();
        sort_bot_accounts(&mut normalized.bot_accounts);
        sort_channels(&mut normalized.output_channels);
        let mut bytes = serde_json::to_vec_pretty(&normalized).map_err(|source| {
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
    async fn list_bot_accounts(&self) -> Result<Vec<BotAccount>, OutputChannelStoreError> {
        let _guard = self.lock.lock().await;
        let mut accounts = self.read_unlocked().await?.collection.bot_accounts;
        sort_bot_accounts(&mut accounts);
        Ok(accounts)
    }

    async fn get_bot_account(
        &self,
        bot_account_id: &str,
    ) -> Result<Option<BotAccount>, OutputChannelStoreError> {
        validate_bot_account_id(bot_account_id)?;
        let _guard = self.lock.lock().await;
        Ok(self
            .read_unlocked()
            .await?
            .collection
            .bot_accounts
            .into_iter()
            .find(|account| account.bot_account_id == bot_account_id))
    }

    async fn replace_bot_account(
        &self,
        bot_account_id: &str,
        definition: BotAccountDefinition,
    ) -> Result<BotAccount, OutputChannelStoreError> {
        validate_bot_account_id(bot_account_id)?;
        let definition = definition.normalized()?;
        let _guard = self.lock.lock().await;
        let mut collection = self.read_unlocked().await?.collection;
        let account = upsert_bot_account(&mut collection.bot_accounts, bot_account_id, definition)?;
        self.write_unlocked(&collection).await?;
        Ok(account)
    }

    async fn list_output_channels(&self) -> Result<Vec<OutputChannel>, OutputChannelStoreError> {
        let _guard = self.lock.lock().await;
        let mut channels = self.read_unlocked().await?.collection.output_channels;
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
            .collection
            .output_channels
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
        let mut collection = self.read_unlocked().await?.collection;
        ensure_bot_account_exists(&collection, definition.bot_account_id())?;
        let channel = upsert_output_channel(
            &mut collection.output_channels,
            output_channel_id,
            definition,
        )?;
        self.write_unlocked(&collection).await?;
        Ok(channel)
    }

    async fn replace_legacy_telegram_channel(
        &self,
        output_channel_id: &str,
        bot_token: String,
        chat_id: String,
    ) -> Result<OutputChannel, OutputChannelStoreError> {
        let _guard = self.lock.lock().await;
        let mut collection = self.read_unlocked().await?.collection;
        let channel =
            upsert_legacy_telegram_channel(&mut collection, output_channel_id, bot_token, chat_id)?;
        self.write_unlocked(&collection).await?;
        Ok(channel)
    }
}

fn upsert_legacy_telegram_channel(
    collection: &mut OutputChannelCollection,
    output_channel_id: &str,
    bot_token: String,
    chat_id: String,
) -> Result<OutputChannel, OutputChannelStoreError> {
    validate_output_channel_id(output_channel_id)?;
    let bot_definition = BotAccountDefinition::Telegram { bot_token }.normalized()?;
    let channel_definition = OutputChannelDefinition::Telegram {
        bot_account_id: output_channel_id.to_owned(),
        chat_id,
    }
    .normalized()?;
    upsert_bot_account(
        &mut collection.bot_accounts,
        output_channel_id,
        bot_definition,
    )?;
    upsert_output_channel(
        &mut collection.output_channels,
        output_channel_id,
        channel_definition,
    )
}

fn upsert_bot_account(
    accounts: &mut Vec<BotAccount>,
    bot_account_id: &str,
    definition: BotAccountDefinition,
) -> Result<BotAccount, OutputChannelStoreError> {
    let current = accounts
        .iter_mut()
        .find(|account| account.bot_account_id == bot_account_id);
    let revision = match current.as_ref() {
        Some(current) => current.revision.checked_add(1).ok_or_else(|| {
            OutputChannelStoreError::RevisionExhausted {
                kind: "bot account",
                id: bot_account_id.to_owned(),
            }
        })?,
        None => 1,
    };
    let account = BotAccount {
        bot_account_id: bot_account_id.to_owned(),
        revision,
        definition,
    };
    if let Some(current) = current {
        *current = account.clone();
    } else {
        accounts.push(account.clone());
    }
    Ok(account)
}

fn upsert_output_channel(
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
                kind: "output channel",
                id: output_channel_id.to_owned(),
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

fn ensure_bot_account_exists(
    collection: &OutputChannelCollection,
    bot_account_id: &str,
) -> Result<(), OutputChannelStoreError> {
    if collection
        .bot_accounts
        .iter()
        .any(|account| account.bot_account_id == bot_account_id)
    {
        Ok(())
    } else {
        Err(OutputChannelStoreError::BotAccountNotFound {
            bot_account_id: bot_account_id.to_owned(),
        })
    }
}

struct DecodedCollection {
    collection: OutputChannelCollection,
    migrated: bool,
}

fn decode_collection(
    path: &Path,
    bytes: &[u8],
) -> Result<DecodedCollection, OutputChannelStoreError> {
    let value = serde_json::from_slice::<Value>(bytes).map_err(|source| {
        OutputChannelStoreError::Serialization {
            path: path.to_path_buf(),
            source,
        }
    })?;
    let (collection, migrated) = if value.is_array() {
        let legacy =
            serde_json::from_value::<Vec<LegacyOutputChannel>>(value).map_err(|source| {
                OutputChannelStoreError::Serialization {
                    path: path.to_path_buf(),
                    source,
                }
            })?;
        (migrate_legacy_collection(path, legacy)?, true)
    } else {
        let collection =
            serde_json::from_value::<OutputChannelCollection>(value).map_err(|source| {
                OutputChannelStoreError::Serialization {
                    path: path.to_path_buf(),
                    source,
                }
            })?;
        (collection, false)
    };
    validate_collection(path, &collection)?;
    Ok(DecodedCollection {
        collection,
        migrated,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyOutputChannel {
    output_channel_id: String,
    revision: u64,
    definition: LegacyOutputChannelDefinition,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum LegacyOutputChannelDefinition {
    Telegram { bot_token: String, chat_id: String },
}

fn migrate_legacy_collection(
    path: &Path,
    mut legacy: Vec<LegacyOutputChannel>,
) -> Result<OutputChannelCollection, OutputChannelStoreError> {
    legacy.sort_unstable_by(|left, right| left.output_channel_id.cmp(&right.output_channel_id));
    let mut collection = OutputChannelCollection::default();
    let mut bot_ids_by_token = HashMap::<String, String>::new();
    let mut used_bot_ids = HashSet::<String>::new();
    let mut output_channel_ids = HashSet::<String>::new();

    for channel in legacy {
        validate_output_channel_id(&channel.output_channel_id)?;
        if channel.revision == 0 {
            return Err(invalid_collection(
                path,
                format!(
                    "legacy output channel {:?} has zero revision",
                    channel.output_channel_id
                ),
            ));
        }
        if !output_channel_ids.insert(channel.output_channel_id.clone()) {
            return Err(invalid_collection(
                path,
                format!(
                    "duplicate legacy output channel ID {:?}",
                    channel.output_channel_id
                ),
            ));
        }
        let LegacyOutputChannelDefinition::Telegram { bot_token, chat_id } = channel.definition;
        let bot_definition = BotAccountDefinition::Telegram { bot_token }.normalized()?;
        let BotAccountDefinition::Telegram { bot_token } = &bot_definition;
        let bot_account_id = if let Some(existing) = bot_ids_by_token.get(bot_token) {
            existing.clone()
        } else {
            let id = next_legacy_bot_account_id(bot_token, &used_bot_ids);
            used_bot_ids.insert(id.clone());
            bot_ids_by_token.insert(bot_token.clone(), id.clone());
            collection.bot_accounts.push(BotAccount {
                bot_account_id: id.clone(),
                revision: 1,
                definition: bot_definition,
            });
            id
        };
        let definition = OutputChannelDefinition::Telegram {
            bot_account_id,
            chat_id,
        }
        .normalized()?;
        collection.output_channels.push(OutputChannel {
            output_channel_id: channel.output_channel_id,
            revision: channel.revision,
            definition,
        });
    }
    Ok(collection)
}

fn next_legacy_bot_account_id(token: &str, used: &HashSet<String>) -> String {
    let numeric_id = token.split_once(':').map_or("bot", |(id, _)| id);
    let base = format!("telegram-{numeric_id}");
    if !used.contains(&base) {
        return base;
    }
    for suffix in 2_u32.. {
        let candidate = format!("{base}-{suffix}");
        if !used.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!("the finite set of used bot IDs cannot exhaust u32 suffixes")
}

fn validate_collection(
    path: &Path,
    collection: &OutputChannelCollection,
) -> Result<(), OutputChannelStoreError> {
    if collection.version != OUTPUT_CHANNEL_COLLECTION_VERSION {
        return Err(invalid_collection(
            path,
            format!(
                "unsupported version {}, expected {OUTPUT_CHANNEL_COLLECTION_VERSION}",
                collection.version
            ),
        ));
    }
    let mut bot_ids = HashSet::with_capacity(collection.bot_accounts.len());
    for account in &collection.bot_accounts {
        let normalized =
            account
                .normalized()
                .map_err(|error| OutputChannelStoreError::InvalidCollection {
                    path: path.to_path_buf(),
                    message: format!("invalid bot account {:?}: {error}", account.bot_account_id),
                })?;
        if normalized != *account {
            return Err(invalid_collection(
                path,
                format!(
                    "bot account {:?} is not in normalized form",
                    account.bot_account_id
                ),
            ));
        }
        if !bot_ids.insert(account.bot_account_id.as_str()) {
            return Err(invalid_collection(
                path,
                format!("duplicate bot account ID {:?}", account.bot_account_id),
            ));
        }
    }

    let mut output_ids = HashSet::with_capacity(collection.output_channels.len());
    for channel in &collection.output_channels {
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
            return Err(invalid_collection(
                path,
                format!(
                    "output channel {:?} is not in normalized form",
                    channel.output_channel_id
                ),
            ));
        }
        if !output_ids.insert(channel.output_channel_id.as_str()) {
            return Err(invalid_collection(
                path,
                format!(
                    "duplicate output channel ID {:?}",
                    channel.output_channel_id
                ),
            ));
        }
        if !bot_ids.contains(channel.definition.bot_account_id()) {
            return Err(invalid_collection(
                path,
                format!(
                    "output channel {:?} references missing bot account {:?}",
                    channel.output_channel_id,
                    channel.definition.bot_account_id()
                ),
            ));
        }
    }
    Ok(())
}

fn invalid_collection(path: &Path, message: impl Into<String>) -> OutputChannelStoreError {
    OutputChannelStoreError::InvalidCollection {
        path: path.to_path_buf(),
        message: message.into(),
    }
}

fn sort_bot_accounts(accounts: &mut [BotAccount]) {
    accounts.sort_unstable_by(|left, right| left.bot_account_id.cmp(&right.bot_account_id));
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

    #[error("bot account {bot_account_id:?} does not exist")]
    BotAccountNotFound { bot_account_id: String },

    #[error("{kind} {id:?} revision is exhausted")]
    RevisionExhausted { kind: &'static str, id: String },

    #[error("output-channel store I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("invalid output-channel JSON at {path}: {source}")]
    Serialization {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("invalid output-channel collection at {path}: {message}")]
    InvalidCollection { path: PathBuf, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "123456789:abcdefghijklmnopqrstuvwxyz_ABCDEFG";

    fn bot_definition(token: &str) -> BotAccountDefinition {
        BotAccountDefinition::Telegram {
            bot_token: token.to_owned(),
        }
    }

    fn channel_definition(bot_account_id: &str, chat_id: &str) -> OutputChannelDefinition {
        OutputChannelDefinition::Telegram {
            bot_account_id: bot_account_id.to_owned(),
            chat_id: chat_id.to_owned(),
        }
    }

    #[tokio::test]
    async fn memory_store_shares_one_bot_across_multiple_targets() {
        let store = MemoryOutputChannelStore::new();
        let first = store
            .replace_bot_account("primary", bot_definition(TOKEN))
            .await
            .unwrap();
        let updated = store
            .replace_bot_account(
                "primary",
                bot_definition("987654321:abcdefghijklmnopqrstuvwxyz_ABCDEFG"),
            )
            .await
            .unwrap();
        assert_eq!(first.revision, 1);
        assert_eq!(updated.revision, 2);

        let alice = store
            .replace_output_channel("alice", channel_definition("primary", "111111111"))
            .await
            .unwrap();
        let bob = store
            .replace_output_channel("bob", channel_definition("primary", "222222222"))
            .await
            .unwrap();
        assert_eq!(alice.revision, 1);
        assert_eq!(bob.revision, 1);
        assert_eq!(store.list_bot_accounts().await.unwrap().len(), 1);
        assert_eq!(store.list_output_channels().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn output_target_requires_an_existing_bot_account() {
        let store = MemoryOutputChannelStore::new();
        assert!(matches!(
            store
                .replace_output_channel("alerts", channel_definition("missing", "123"))
                .await,
            Err(OutputChannelStoreError::BotAccountNotFound { .. })
        ));
    }

    #[tokio::test]
    async fn disk_store_round_trips_one_secret_with_owner_only_permissions() {
        let root = std::env::temp_dir().join(format!("phi-output-channel-{}", SessionId::new()));
        let path = root.join("output-channels.json");
        let store = DiskOutputChannelStore::new(&path);
        store
            .replace_bot_account("primary", bot_definition(TOKEN))
            .await
            .unwrap();
        store
            .replace_output_channel("alice", channel_definition("primary", "111111111"))
            .await
            .unwrap();
        store
            .replace_output_channel("bob", channel_definition("primary", "222222222"))
            .await
            .unwrap();

        let contents = fs::read_to_string(&path).await.unwrap();
        assert_eq!(contents.matches(TOKEN).count(), 1);
        assert_eq!(store.list_bot_accounts().await.unwrap().len(), 1);
        assert_eq!(store.list_output_channels().await.unwrap().len(), 2);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).await.unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn legacy_array_migrates_and_deduplicates_bot_tokens() {
        let root = std::env::temp_dir().join(format!("phi-output-migration-{}", SessionId::new()));
        let path = root.join("output-channels.json");
        fs::create_dir_all(&root).await.unwrap();
        fs::write(
            &path,
            serde_json::to_vec_pretty(&serde_json::json!([
                {
                    "output_channel_id": "alice",
                    "revision": 2,
                    "definition": {
                        "type": "telegram",
                        "bot_token": TOKEN,
                        "chat_id": "111111111"
                    }
                },
                {
                    "output_channel_id": "bob",
                    "revision": 4,
                    "definition": {
                        "type": "telegram",
                        "bot_token": TOKEN,
                        "chat_id": "222222222"
                    }
                }
            ]))
            .unwrap(),
        )
        .await
        .unwrap();

        let store = DiskOutputChannelStore::new(&path);
        assert!(store.migrate_legacy().await.unwrap());
        assert!(!store.migrate_legacy().await.unwrap());
        let contents = fs::read_to_string(&path).await.unwrap();
        assert_eq!(contents.matches(TOKEN).count(), 1);
        assert!(contents.contains("\"version\": 2"));
        assert_eq!(store.list_bot_accounts().await.unwrap().len(), 1);
        let channels = store.list_output_channels().await.unwrap();
        assert_eq!(channels.len(), 2);
        assert_eq!(channels[0].revision, 2);
        assert_eq!(channels[1].revision, 4);

        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn unknown_collection_version_is_rejected() {
        let root = std::env::temp_dir().join(format!("phi-output-version-{}", SessionId::new()));
        let path = root.join("output-channels.json");
        fs::create_dir_all(&root).await.unwrap();
        fs::write(
            &path,
            br#"{"version":99,"bot_accounts":[],"output_channels":[]}"#,
        )
        .await
        .unwrap();
        let store = DiskOutputChannelStore::new(&path);
        assert!(matches!(
            store.list_bot_accounts().await,
            Err(OutputChannelStoreError::InvalidCollection { .. })
        ));
        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn revision_exhaustion_preserves_existing_entries() {
        let store = MemoryOutputChannelStore::new();
        store
            .collection
            .write()
            .await
            .bot_accounts
            .push(BotAccount {
                bot_account_id: "primary".to_owned(),
                revision: u64::MAX,
                definition: bot_definition(TOKEN).normalized().unwrap(),
            });
        assert!(matches!(
            store
                .replace_bot_account(
                    "primary",
                    bot_definition("987654321:abcdefghijklmnopqrstuvwxyz_ABCDEFG")
                )
                .await,
            Err(OutputChannelStoreError::RevisionExhausted { .. })
        ));
        let account = store.get_bot_account("primary").await.unwrap().unwrap();
        assert_eq!(account.revision, u64::MAX);
    }
}

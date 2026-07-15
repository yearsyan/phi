use std::{collections::HashSet, io::ErrorKind, path::PathBuf, sync::Arc};

use async_trait::async_trait;
use serde::Deserialize;
use tokio::{
    fs,
    io::{AsyncWriteExt, BufWriter},
    sync::{Mutex, RwLock},
};

use super::{
    DEFAULT_PROFILE_ID, ProviderConfig, ProviderProfile, ProviderStore, ProviderStoreError,
};
use crate::runtime::SessionId;

#[derive(Clone, Default)]
pub struct MemoryProviderStore {
    providers: Arc<RwLock<Vec<ProviderProfile>>>,
}

impl MemoryProviderStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ProviderStore for MemoryProviderStore {
    async fn list_providers(&self) -> Result<Vec<ProviderProfile>, ProviderStoreError> {
        Ok(self.providers.read().await.clone())
    }

    async fn get_provider_by_id(
        &self,
        profile_id: &str,
    ) -> Result<Option<ProviderConfig>, ProviderStoreError> {
        Ok(self
            .providers
            .read()
            .await
            .iter()
            .find(|profile| profile.profile_id == profile_id)
            .map(|profile| profile.config.clone()))
    }

    async fn replace_provider_for(
        &self,
        profile_id: &str,
        provider: ProviderConfig,
    ) -> Result<ProviderConfig, ProviderStoreError> {
        validate_profile_id(profile_id)?;
        let mut providers = self.providers.write().await;
        Ok(replace_profile(&mut providers, profile_id, provider))
    }
}

/// Atomic JSON-array storage for named daemon-wide Provider profiles.
///
/// The file contains the API key and is created with owner-only permissions
/// on Unix. Callers must still protect the containing data directory.
#[derive(Clone, Debug)]
pub struct DiskProviderStore {
    path: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl DiskProviderStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    async fn read_unlocked(&self) -> Result<Vec<ProviderProfile>, ProviderStoreError> {
        let bytes = match fs::read(&self.path).await {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(ProviderStoreError::Io {
                    path: self.path.clone(),
                    source,
                });
            }
        };
        let file = serde_json::from_slice::<ProviderFile>(&bytes).map_err(|source| {
            ProviderStoreError::Serialization {
                path: self.path.clone(),
                source,
            }
        })?;
        let providers = match file {
            ProviderFile::Profiles(providers) => providers,
            ProviderFile::Legacy(config) => {
                vec![ProviderProfile::new(DEFAULT_PROFILE_ID, config)]
            }
        };
        validate_collection(&self.path, &providers)?;
        Ok(providers)
    }

    async fn write_unlocked(
        &self,
        providers: &[ProviderProfile],
    ) -> Result<(), ProviderStoreError> {
        let parent = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        fs::create_dir_all(parent)
            .await
            .map_err(|source| ProviderStoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        let mut bytes = serde_json::to_vec_pretty(providers).map_err(|source| {
            ProviderStoreError::Serialization {
                path: self.path.clone(),
                source,
            }
        })?;
        bytes.push(b'\n');
        let temporary = parent.join(format!(".provider-{}.tmp", SessionId::new()));
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options
            .open(&temporary)
            .await
            .map_err(|source| ProviderStoreError::Io {
                path: temporary.clone(),
                source,
            })?;
        if let Err(source) = write_and_sync(file, &bytes).await {
            let _ = fs::remove_file(&temporary).await;
            return Err(ProviderStoreError::Io {
                path: temporary,
                source,
            });
        }
        if let Err(source) = fs::rename(&temporary, &self.path).await {
            let _ = fs::remove_file(&temporary).await;
            return Err(ProviderStoreError::Io {
                path: self.path.clone(),
                source,
            });
        }

        // Rename is the logical commit point. A later directory sync failure
        // makes crash durability uncertain but must not report a false failed
        // update after the new secret/config is already visible.
        if let Err(source) = sync_directory(parent).await {
            tracing::warn!(
                path = %parent.display(),
                error = %source,
                "provider configuration is visible, but its directory sync failed"
            );
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ProviderFile {
    Profiles(Vec<ProviderProfile>),
    Legacy(ProviderConfig),
}

#[async_trait]
impl ProviderStore for DiskProviderStore {
    async fn list_providers(&self) -> Result<Vec<ProviderProfile>, ProviderStoreError> {
        let _guard = self.lock.lock().await;
        self.read_unlocked().await
    }

    async fn get_provider_by_id(
        &self,
        profile_id: &str,
    ) -> Result<Option<ProviderConfig>, ProviderStoreError> {
        let _guard = self.lock.lock().await;
        Ok(self
            .read_unlocked()
            .await?
            .into_iter()
            .find(|profile| profile.profile_id == profile_id)
            .map(|profile| profile.config))
    }

    async fn replace_provider_for(
        &self,
        profile_id: &str,
        provider: ProviderConfig,
    ) -> Result<ProviderConfig, ProviderStoreError> {
        validate_profile_id(profile_id)?;
        let _guard = self.lock.lock().await;
        let mut providers = self.read_unlocked().await?;
        let provider = replace_profile(&mut providers, profile_id, provider);
        self.write_unlocked(&providers).await?;
        Ok(provider)
    }
}

fn replace_profile(
    providers: &mut Vec<ProviderProfile>,
    profile_id: &str,
    mut provider: ProviderConfig,
) -> ProviderConfig {
    let current = providers
        .iter_mut()
        .find(|profile| profile.profile_id == profile_id);
    provider.revision = current
        .as_ref()
        .map_or(1, |current| current.config.revision.saturating_add(1));
    let profile = ProviderProfile::new(profile_id, provider.clone());
    if let Some(current) = current {
        *current = profile;
    } else {
        providers.push(profile);
    }
    provider
}

fn validate_collection(
    path: &std::path::Path,
    providers: &[ProviderProfile],
) -> Result<(), ProviderStoreError> {
    let mut profile_ids = HashSet::with_capacity(providers.len());
    for profile in providers {
        if let Some(message) = invalid_profile_id_reason(&profile.profile_id) {
            return Err(ProviderStoreError::InvalidCollection {
                path: path.to_path_buf(),
                message: format!("invalid profile_id {:?}: {message}", profile.profile_id),
            });
        }
        if !profile_ids.insert(profile.profile_id.as_str()) {
            return Err(ProviderStoreError::InvalidCollection {
                path: path.to_path_buf(),
                message: format!("duplicate profile_id {:?}", profile.profile_id),
            });
        }
    }
    Ok(())
}

fn validate_profile_id(profile_id: &str) -> Result<(), ProviderStoreError> {
    if let Some(message) = invalid_profile_id_reason(profile_id) {
        return Err(ProviderStoreError::InvalidProfileId {
            profile_id: profile_id.to_owned(),
            message,
        });
    }
    Ok(())
}

fn invalid_profile_id_reason(profile_id: &str) -> Option<String> {
    if profile_id.trim().is_empty() {
        Some("must not be empty".to_owned())
    } else if profile_id != profile_id.trim() {
        Some("must not have surrounding whitespace".to_owned())
    } else if profile_id.len() > 128 {
        Some("must not exceed 128 bytes".to_owned())
    } else if profile_id.chars().any(char::is_control) {
        Some("must not contain control characters".to_owned())
    } else {
        None
    }
}

async fn write_and_sync(file: fs::File, bytes: &[u8]) -> Result<(), std::io::Error> {
    let mut writer = BufWriter::new(file);
    writer.write_all(bytes).await?;
    writer.flush().await?;
    writer.get_ref().sync_all().await
}

async fn sync_directory(path: &std::path::Path) -> Result<(), std::io::Error> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ProviderKind;

    fn config(model: &str) -> ProviderConfig {
        ProviderConfig::new(
            ProviderKind::OpenAiChat,
            "secret",
            "https://example.test/v1",
            model,
            128_000,
        )
    }

    #[tokio::test]
    async fn memory_store_keeps_profiles_and_revisions_independent() {
        let store = MemoryProviderStore::new();
        assert_eq!(store.get_provider().await.unwrap(), None);
        assert_eq!(
            store
                .replace_provider(config("first"))
                .await
                .unwrap()
                .revision,
            1
        );
        let secondary = store
            .replace_provider_for("secondary", config("secondary-first"))
            .await
            .unwrap();
        assert_eq!(secondary.revision, 1);
        let second = store.replace_provider(config("second")).await.unwrap();
        assert_eq!(second.revision, 2);
        assert_eq!(store.get_provider().await.unwrap(), Some(second));
        assert_eq!(
            store
                .get_provider_by_id("secondary")
                .await
                .unwrap()
                .unwrap()
                .model,
            "secondary-first"
        );
        assert_eq!(store.list_providers().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn disk_store_round_trips_and_replaces_atomically() {
        let root = std::env::temp_dir().join(format!("phi-provider-{}", SessionId::new()));
        let path = root.join("provider.json");
        let store = DiskProviderStore::new(&path);
        let first = store.replace_provider(config("first")).await.unwrap();
        assert_eq!(first.revision, 1);
        assert_eq!(store.get_provider().await.unwrap(), Some(first));
        store
            .replace_provider_for("secondary", config("other"))
            .await
            .unwrap();
        let second = store.replace_provider(config("second")).await.unwrap();
        assert_eq!(second.revision, 2);
        assert_eq!(store.get_provider().await.unwrap(), Some(second));
        assert_eq!(store.list_providers().await.unwrap().len(), 2);

        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).await.unwrap()).unwrap();
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 2);
        assert_eq!(json[0]["profile_id"], DEFAULT_PROFILE_ID);
        assert_eq!(json[0]["max_context_tokens"], 128_000);
        assert_eq!(json[1]["profile_id"], "secondary");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).await.unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn disk_store_reads_a_legacy_single_config_and_migrates_on_write() {
        let root = std::env::temp_dir().join(format!("phi-provider-legacy-{}", SessionId::new()));
        let path = root.join("provider.json");
        fs::create_dir_all(&root).await.unwrap();
        fs::write(&path, serde_json::to_vec_pretty(&config("legacy")).unwrap())
            .await
            .unwrap();
        let store = DiskProviderStore::new(&path);

        assert_eq!(store.get_provider().await.unwrap().unwrap().model, "legacy");
        let migrated = store.replace_provider(config("migrated")).await.unwrap();
        assert_eq!(migrated.revision, 1);

        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).await.unwrap()).unwrap();
        assert!(json.is_array());
        assert_eq!(json[0]["profile_id"], DEFAULT_PROFILE_ID);
        assert_eq!(json[0]["model"], "migrated");

        fs::remove_dir_all(root).await.unwrap();
    }
}

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

use crate::runtime::{
    McpProfile, McpProfileDefinition, McpProfileValidationError, SessionId, validate_mcp_profile_id,
};

#[async_trait]
pub trait McpProfileStore: Send + Sync {
    async fn list_mcp_profiles(&self) -> Result<Vec<McpProfile>, McpProfileStoreError>;

    async fn get_mcp_profile(
        &self,
        mcp_profile_id: &str,
    ) -> Result<Option<McpProfile>, McpProfileStoreError>;

    async fn replace_mcp_profile(
        &self,
        mcp_profile_id: &str,
        definition: McpProfileDefinition,
    ) -> Result<McpProfile, McpProfileStoreError>;
}

#[derive(Clone, Default)]
pub struct MemoryMcpProfileStore {
    profiles: Arc<RwLock<Vec<McpProfile>>>,
}

impl MemoryMcpProfileStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl McpProfileStore for MemoryMcpProfileStore {
    async fn list_mcp_profiles(&self) -> Result<Vec<McpProfile>, McpProfileStoreError> {
        let mut profiles = self.profiles.read().await.clone();
        sort_profiles(&mut profiles);
        Ok(profiles)
    }

    async fn get_mcp_profile(
        &self,
        mcp_profile_id: &str,
    ) -> Result<Option<McpProfile>, McpProfileStoreError> {
        validate_mcp_profile_id(mcp_profile_id)?;
        Ok(self
            .profiles
            .read()
            .await
            .iter()
            .find(|profile| profile.mcp_profile_id == mcp_profile_id)
            .cloned())
    }

    async fn replace_mcp_profile(
        &self,
        mcp_profile_id: &str,
        definition: McpProfileDefinition,
    ) -> Result<McpProfile, McpProfileStoreError> {
        validate_mcp_profile_id(mcp_profile_id)?;
        let definition = definition.normalized()?;
        let mut profiles = self.profiles.write().await;
        replace_profile(&mut profiles, mcp_profile_id, definition)
    }
}

/// Atomic secret-bearing storage for daemon-wide MCP connection profiles.
///
/// HTTP bearer tokens, custom header values, and stdio environment values are
/// stored in this file. It is therefore created with owner-only permissions on
/// Unix and must never be exposed through public DTOs or debug output.
#[derive(Clone, Debug)]
pub struct DiskMcpProfileStore {
    path: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl DiskMcpProfileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    async fn read_unlocked(&self) -> Result<Vec<McpProfile>, McpProfileStoreError> {
        let bytes = match fs::read(&self.path).await {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(McpProfileStoreError::Io {
                    path: self.path.clone(),
                    source,
                });
            }
        };
        let profiles = serde_json::from_slice::<Vec<McpProfile>>(&bytes).map_err(|source| {
            McpProfileStoreError::Serialization {
                path: self.path.clone(),
                source,
            }
        })?;
        validate_collection(&self.path, &profiles)?;
        Ok(profiles)
    }

    async fn write_unlocked(&self, profiles: &[McpProfile]) -> Result<(), McpProfileStoreError> {
        let parent = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .await
            .map_err(|source| McpProfileStoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;

        let mut profiles = profiles.to_vec();
        sort_profiles(&mut profiles);
        let mut bytes = serde_json::to_vec_pretty(&profiles).map_err(|source| {
            McpProfileStoreError::Serialization {
                path: self.path.clone(),
                source,
            }
        })?;
        bytes.push(b'\n');

        let temporary = parent.join(format!(".mcp-profiles-{}.tmp", SessionId::new()));
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options
            .open(&temporary)
            .await
            .map_err(|source| McpProfileStoreError::Io {
                path: temporary.clone(),
                source,
            })?;
        if let Err(source) = write_and_sync(file, &bytes).await {
            let _ = fs::remove_file(&temporary).await;
            return Err(McpProfileStoreError::Io {
                path: temporary,
                source,
            });
        }
        if let Err(source) = fs::rename(&temporary, &self.path).await {
            let _ = fs::remove_file(&temporary).await;
            return Err(McpProfileStoreError::Io {
                path: self.path.clone(),
                source,
            });
        }
        if let Err(source) = sync_directory(parent).await {
            tracing::warn!(
                path = %parent.display(),
                error = %source,
                "MCP profile configuration is visible, but its directory sync failed"
            );
        }
        Ok(())
    }
}

#[async_trait]
impl McpProfileStore for DiskMcpProfileStore {
    async fn list_mcp_profiles(&self) -> Result<Vec<McpProfile>, McpProfileStoreError> {
        let _guard = self.lock.lock().await;
        let mut profiles = self.read_unlocked().await?;
        sort_profiles(&mut profiles);
        Ok(profiles)
    }

    async fn get_mcp_profile(
        &self,
        mcp_profile_id: &str,
    ) -> Result<Option<McpProfile>, McpProfileStoreError> {
        validate_mcp_profile_id(mcp_profile_id)?;
        let _guard = self.lock.lock().await;
        Ok(self
            .read_unlocked()
            .await?
            .into_iter()
            .find(|profile| profile.mcp_profile_id == mcp_profile_id))
    }

    async fn replace_mcp_profile(
        &self,
        mcp_profile_id: &str,
        definition: McpProfileDefinition,
    ) -> Result<McpProfile, McpProfileStoreError> {
        validate_mcp_profile_id(mcp_profile_id)?;
        let definition = definition.normalized()?;
        let _guard = self.lock.lock().await;
        let mut profiles = self.read_unlocked().await?;
        let profile = replace_profile(&mut profiles, mcp_profile_id, definition)?;
        self.write_unlocked(&profiles).await?;
        Ok(profile)
    }
}

fn replace_profile(
    profiles: &mut Vec<McpProfile>,
    mcp_profile_id: &str,
    definition: McpProfileDefinition,
) -> Result<McpProfile, McpProfileStoreError> {
    let current = profiles
        .iter_mut()
        .find(|profile| profile.mcp_profile_id == mcp_profile_id);
    let revision = match current.as_ref() {
        Some(current) => current.revision.checked_add(1).ok_or_else(|| {
            McpProfileStoreError::RevisionExhausted {
                mcp_profile_id: mcp_profile_id.to_owned(),
            }
        })?,
        None => 1,
    };
    let profile = McpProfile {
        mcp_profile_id: mcp_profile_id.to_owned(),
        revision,
        definition,
    };
    if let Some(current) = current {
        *current = profile.clone();
    } else {
        profiles.push(profile.clone());
    }
    Ok(profile)
}

fn validate_collection(path: &Path, profiles: &[McpProfile]) -> Result<(), McpProfileStoreError> {
    let mut ids = HashSet::with_capacity(profiles.len());
    for profile in profiles {
        let normalized =
            profile
                .normalized()
                .map_err(|error| McpProfileStoreError::InvalidCollection {
                    path: path.to_path_buf(),
                    message: format!("invalid MCP profile {:?}: {error}", profile.mcp_profile_id),
                })?;
        if normalized != *profile {
            return Err(McpProfileStoreError::InvalidCollection {
                path: path.to_path_buf(),
                message: format!(
                    "MCP profile {:?} is not in normalized form",
                    profile.mcp_profile_id
                ),
            });
        }
        if !ids.insert(profile.mcp_profile_id.as_str()) {
            return Err(McpProfileStoreError::InvalidCollection {
                path: path.to_path_buf(),
                message: format!("duplicate MCP profile ID {:?}", profile.mcp_profile_id),
            });
        }
    }
    Ok(())
}

fn sort_profiles(profiles: &mut [McpProfile]) {
    profiles.sort_unstable_by(|left, right| left.mcp_profile_id.cmp(&right.mcp_profile_id));
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
pub enum McpProfileStoreError {
    #[error(transparent)]
    Validation(#[from] McpProfileValidationError),

    #[error("MCP profile {mcp_profile_id:?} exhausted its revision counter")]
    RevisionExhausted { mcp_profile_id: String },

    #[error("MCP profile store I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("invalid MCP profile JSON at {path}: {source}")]
    Serialization {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("invalid MCP profile collection at {path}: {message}")]
    InvalidCollection { path: PathBuf, message: String },
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::runtime::{
        DEFAULT_MCP_CONNECT_TIMEOUT_SECS, DEFAULT_MCP_OUTPUT_BYTES, DEFAULT_MCP_OUTPUT_LINES,
        DEFAULT_MCP_REQUEST_TIMEOUT_SECS, McpTransportDefinition,
    };

    fn definition(command: &str, secret: &str) -> McpProfileDefinition {
        McpProfileDefinition {
            transport: McpTransportDefinition::Stdio {
                command: command.to_owned(),
                args: vec!["--stdio".to_owned()],
                current_dir: None,
                env: BTreeMap::from([("TOKEN".to_owned(), secret.to_owned())]),
                clear_env: false,
            },
            tool_name_prefix: "test".to_owned(),
            connect_timeout_secs: DEFAULT_MCP_CONNECT_TIMEOUT_SECS,
            request_timeout_secs: Some(DEFAULT_MCP_REQUEST_TIMEOUT_SECS),
            max_output_lines: DEFAULT_MCP_OUTPUT_LINES,
            max_output_bytes: DEFAULT_MCP_OUTPUT_BYTES,
        }
    }

    #[tokio::test]
    async fn memory_store_revisions_profiles_independently() {
        let store = MemoryMcpProfileStore::new();
        let first = store
            .replace_mcp_profile("first", definition("one", "secret-one"))
            .await
            .unwrap();
        let second = store
            .replace_mcp_profile("second", definition("two", "secret-two"))
            .await
            .unwrap();
        let updated = store
            .replace_mcp_profile("first", definition("one-updated", "new-secret"))
            .await
            .unwrap();
        assert_eq!(first.revision, 1);
        assert_eq!(second.revision, 1);
        assert_eq!(updated.revision, 2);
        assert_eq!(
            store.list_mcp_profiles().await.unwrap(),
            vec![updated, second]
        );
    }

    #[tokio::test]
    async fn rejects_invalid_ids_without_mutating_memory_store() {
        let store = MemoryMcpProfileStore::new();
        assert!(matches!(
            store
                .replace_mcp_profile(" bad ", definition("ignored", "secret"))
                .await,
            Err(McpProfileStoreError::Validation(_))
        ));
        assert!(store.list_mcp_profiles().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn disk_store_round_trips_secrets_with_owner_only_permissions() {
        let root = std::env::temp_dir().join(format!("phi-mcp-profile-{}", SessionId::new()));
        let path = root.join("mcp-profiles.json");
        let store = DiskMcpProfileStore::new(&path);
        let saved = store
            .replace_mcp_profile("private", definition("server", "disk-secret"))
            .await
            .unwrap();
        assert_eq!(store.get_mcp_profile("private").await.unwrap(), Some(saved));
        let contents = fs::read_to_string(&path).await.unwrap();
        assert!(contents.contains("disk-secret"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).await.unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn disk_store_rejects_duplicate_or_unnormalized_profiles() {
        let root =
            std::env::temp_dir().join(format!("phi-mcp-profiles-invalid-{}", SessionId::new()));
        let path = root.join("mcp-profiles.json");
        fs::create_dir_all(&root).await.unwrap();
        let mut profile = McpProfile {
            mcp_profile_id: "private".to_owned(),
            revision: 1,
            definition: definition("server", "secret").normalized().unwrap(),
        };

        fs::write(
            &path,
            serde_json::to_vec(&[profile.clone(), profile.clone()]).unwrap(),
        )
        .await
        .unwrap();
        let store = DiskMcpProfileStore::new(&path);
        assert!(matches!(
            store.list_mcp_profiles().await,
            Err(McpProfileStoreError::InvalidCollection { .. })
        ));

        profile.definition.tool_name_prefix = " unnormalized ".to_owned();
        fs::write(&path, serde_json::to_vec(&[profile]).unwrap())
            .await
            .unwrap();
        assert!(matches!(
            store.list_mcp_profiles().await,
            Err(McpProfileStoreError::InvalidCollection { .. })
        ));

        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn revision_exhaustion_does_not_replace_the_visible_profile() {
        let root =
            std::env::temp_dir().join(format!("phi-mcp-profiles-overflow-{}", SessionId::new()));
        let path = root.join("mcp-profiles.json");
        fs::create_dir_all(&root).await.unwrap();
        let profile = McpProfile {
            mcp_profile_id: "private".to_owned(),
            revision: u64::MAX,
            definition: definition("server", "secret").normalized().unwrap(),
        };
        fs::write(
            &path,
            serde_json::to_vec(std::slice::from_ref(&profile)).unwrap(),
        )
        .await
        .unwrap();
        let store = DiskMcpProfileStore::new(&path);

        assert!(matches!(
            store
                .replace_mcp_profile("private", definition("replacement", "new-secret"))
                .await,
            Err(McpProfileStoreError::RevisionExhausted { .. })
        ));
        assert_eq!(
            store.get_mcp_profile("private").await.unwrap(),
            Some(profile)
        );

        fs::remove_dir_all(root).await.unwrap();
    }
}

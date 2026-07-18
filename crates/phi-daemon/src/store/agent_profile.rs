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
    AgentProfile, AgentProfileDefinition, AgentProfileValidationError, DEFAULT_AGENT_PROFILE_ID,
    SessionId, default_agent_profile, validate_agent_profile_id,
};

#[async_trait]
pub trait AgentProfileStore: Send + Sync {
    /// Lists the latest profile revisions.
    ///
    /// The built-in `default@0` profile is returned when no persisted profile
    /// named `default` exists, but it is not written merely by reading the store.
    async fn list_agent_profiles(&self) -> Result<Vec<AgentProfile>, AgentProfileStoreError>;

    /// Returns the latest selected profile. The implicit `default@0` profile is
    /// available even before the first profile file is created.
    async fn get_agent_profile(
        &self,
        agent_profile_id: &str,
    ) -> Result<Option<AgentProfile>, AgentProfileStoreError>;

    /// Atomically replaces one latest profile definition and assigns its next
    /// monotonically increasing revision.
    async fn replace_agent_profile(
        &self,
        agent_profile_id: &str,
        definition: AgentProfileDefinition,
    ) -> Result<AgentProfile, AgentProfileStoreError>;
}

#[derive(Clone, Default)]
pub struct MemoryAgentProfileStore {
    profiles: Arc<RwLock<Vec<AgentProfile>>>,
}

impl MemoryAgentProfileStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl AgentProfileStore for MemoryAgentProfileStore {
    async fn list_agent_profiles(&self) -> Result<Vec<AgentProfile>, AgentProfileStoreError> {
        Ok(with_implicit_default(self.profiles.read().await.clone()))
    }

    async fn get_agent_profile(
        &self,
        agent_profile_id: &str,
    ) -> Result<Option<AgentProfile>, AgentProfileStoreError> {
        validate_agent_profile_id(agent_profile_id)?;
        let profile = self
            .profiles
            .read()
            .await
            .iter()
            .find(|profile| profile.agent_profile_id == agent_profile_id)
            .cloned();
        Ok(profile
            .or_else(|| (agent_profile_id == DEFAULT_AGENT_PROFILE_ID).then(default_agent_profile)))
    }

    async fn replace_agent_profile(
        &self,
        agent_profile_id: &str,
        definition: AgentProfileDefinition,
    ) -> Result<AgentProfile, AgentProfileStoreError> {
        validate_agent_profile_id(agent_profile_id)?;
        let definition = definition.normalized()?;
        let mut profiles = self.profiles.write().await;
        replace_profile(&mut profiles, agent_profile_id, definition)
    }
}

/// Atomic JSON-array storage for the latest daemon-wide Agent profiles.
///
/// Activated sessions persist a complete pinned profile snapshot, so this file
/// does not need to retain historical revisions. It may contain proprietary
/// prompts and is therefore created with owner-only permissions on Unix.
#[derive(Clone, Debug)]
pub struct DiskAgentProfileStore {
    path: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl DiskAgentProfileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    async fn read_unlocked(&self) -> Result<Vec<AgentProfile>, AgentProfileStoreError> {
        let bytes = match fs::read(&self.path).await {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(AgentProfileStoreError::Io {
                    path: self.path.clone(),
                    source,
                });
            }
        };
        let profiles = serde_json::from_slice::<Vec<AgentProfile>>(&bytes).map_err(|source| {
            AgentProfileStoreError::Serialization {
                path: self.path.clone(),
                source,
            }
        })?;
        validate_collection(&self.path, &profiles)?;
        Ok(profiles)
    }

    async fn write_unlocked(
        &self,
        profiles: &[AgentProfile],
    ) -> Result<(), AgentProfileStoreError> {
        let parent = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .await
            .map_err(|source| AgentProfileStoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;

        let mut profiles = profiles.to_vec();
        profiles.sort_unstable_by(|left, right| left.agent_profile_id.cmp(&right.agent_profile_id));
        let mut bytes = serde_json::to_vec_pretty(&profiles).map_err(|source| {
            AgentProfileStoreError::Serialization {
                path: self.path.clone(),
                source,
            }
        })?;
        bytes.push(b'\n');

        let temporary = parent.join(format!(".agent-profiles-{}.tmp", SessionId::new()));
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options
            .open(&temporary)
            .await
            .map_err(|source| AgentProfileStoreError::Io {
                path: temporary.clone(),
                source,
            })?;
        if let Err(source) = write_and_sync(file, &bytes).await {
            let _ = fs::remove_file(&temporary).await;
            return Err(AgentProfileStoreError::Io {
                path: temporary,
                source,
            });
        }
        if let Err(source) = fs::rename(&temporary, &self.path).await {
            let _ = fs::remove_file(&temporary).await;
            return Err(AgentProfileStoreError::Io {
                path: self.path.clone(),
                source,
            });
        }

        // Rename is the logical commit point. Reporting a later directory sync
        // failure as an update failure could invite a duplicate retry after the
        // new revision is already visible.
        if let Err(source) = sync_directory(parent).await {
            tracing::warn!(
                path = %parent.display(),
                error = %source,
                "agent profile configuration is visible, but its directory sync failed"
            );
        }
        Ok(())
    }
}

#[async_trait]
impl AgentProfileStore for DiskAgentProfileStore {
    async fn list_agent_profiles(&self) -> Result<Vec<AgentProfile>, AgentProfileStoreError> {
        let _guard = self.lock.lock().await;
        Ok(with_implicit_default(self.read_unlocked().await?))
    }

    async fn get_agent_profile(
        &self,
        agent_profile_id: &str,
    ) -> Result<Option<AgentProfile>, AgentProfileStoreError> {
        validate_agent_profile_id(agent_profile_id)?;
        let _guard = self.lock.lock().await;
        let profile = self
            .read_unlocked()
            .await?
            .into_iter()
            .find(|profile| profile.agent_profile_id == agent_profile_id);
        Ok(profile
            .or_else(|| (agent_profile_id == DEFAULT_AGENT_PROFILE_ID).then(default_agent_profile)))
    }

    async fn replace_agent_profile(
        &self,
        agent_profile_id: &str,
        definition: AgentProfileDefinition,
    ) -> Result<AgentProfile, AgentProfileStoreError> {
        validate_agent_profile_id(agent_profile_id)?;
        let definition = definition.normalized()?;
        let _guard = self.lock.lock().await;
        let mut profiles = self.read_unlocked().await?;
        let profile = replace_profile(&mut profiles, agent_profile_id, definition)?;
        self.write_unlocked(&profiles).await?;
        Ok(profile)
    }
}

fn with_implicit_default(mut profiles: Vec<AgentProfile>) -> Vec<AgentProfile> {
    if !profiles
        .iter()
        .any(|profile| profile.agent_profile_id == DEFAULT_AGENT_PROFILE_ID)
    {
        profiles.push(default_agent_profile());
    }
    profiles.sort_unstable_by(|left, right| left.agent_profile_id.cmp(&right.agent_profile_id));
    profiles
}

fn replace_profile(
    profiles: &mut Vec<AgentProfile>,
    agent_profile_id: &str,
    definition: AgentProfileDefinition,
) -> Result<AgentProfile, AgentProfileStoreError> {
    let current = profiles
        .iter_mut()
        .find(|profile| profile.agent_profile_id == agent_profile_id);
    let revision = match current.as_ref() {
        Some(current) => current.revision.checked_add(1).ok_or_else(|| {
            AgentProfileStoreError::RevisionExhausted {
                agent_profile_id: agent_profile_id.to_owned(),
            }
        })?,
        None => 1,
    };
    let profile = AgentProfile {
        agent_profile_id: agent_profile_id.to_owned(),
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

fn validate_collection(
    path: &Path,
    profiles: &[AgentProfile],
) -> Result<(), AgentProfileStoreError> {
    let mut ids = HashSet::with_capacity(profiles.len());
    for profile in profiles {
        if profile.revision == 0 {
            return Err(AgentProfileStoreError::InvalidCollection {
                path: path.to_path_buf(),
                message: format!(
                    "persisted profile {:?} has reserved revision 0",
                    profile.agent_profile_id
                ),
            });
        }
        let normalized =
            profile
                .normalized()
                .map_err(|error| AgentProfileStoreError::InvalidCollection {
                    path: path.to_path_buf(),
                    message: format!("invalid profile {:?}: {error}", profile.agent_profile_id),
                })?;
        if normalized != *profile {
            return Err(AgentProfileStoreError::InvalidCollection {
                path: path.to_path_buf(),
                message: format!(
                    "profile {:?} is not in normalized form",
                    profile.agent_profile_id
                ),
            });
        }
        if !ids.insert(profile.agent_profile_id.as_str()) {
            return Err(AgentProfileStoreError::InvalidCollection {
                path: path.to_path_buf(),
                message: format!("duplicate agent_profile_id {:?}", profile.agent_profile_id),
            });
        }
    }
    Ok(())
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
pub enum AgentProfileStoreError {
    #[error(transparent)]
    Validation(#[from] AgentProfileValidationError),

    #[error("agent profile {agent_profile_id:?} exhausted its revision counter")]
    RevisionExhausted { agent_profile_id: String },

    #[error("agent profile store I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("invalid agent profile JSON at {path}: {source}")]
    Serialization {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("invalid agent profile collection at {path}: {message}")]
    InvalidCollection { path: PathBuf, message: String },
}

#[cfg(test)]
mod tests {
    use phi::{ReasoningEffort, tool::CapabilityMode};

    use super::*;
    use crate::runtime::{NamePolicy, PromptDefinition, PromptMode};

    fn definition(text: &str) -> AgentProfileDefinition {
        AgentProfileDefinition {
            prompt: PromptDefinition {
                mode: PromptMode::Extend,
                text: text.to_owned(),
            },
            tools: NamePolicy {
                allow: Some(vec!["write".to_owned(), "read".to_owned()]),
                deny: vec!["bash".to_owned()],
            },
            skills: NamePolicy::default(),
            initial_capability_mode: CapabilityMode::ReadOnly,
            model: Some(" profile-model ".to_owned()),
            reasoning_effort: Some(ReasoningEffort::High),
        }
    }

    #[tokio::test]
    async fn memory_store_exposes_implicit_default_and_independent_revisions() {
        let store = MemoryAgentProfileStore::new();
        let implicit = store
            .get_agent_profile(DEFAULT_AGENT_PROFILE_ID)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(implicit, default_agent_profile());
        assert_eq!(store.list_agent_profiles().await.unwrap(), vec![implicit]);

        let first = store
            .replace_agent_profile(DEFAULT_AGENT_PROFILE_ID, definition("first"))
            .await
            .unwrap();
        assert_eq!(first.revision, 1);
        assert_eq!(first.definition.model.as_deref(), Some("profile-model"));
        assert_eq!(
            first.definition.tools.allow,
            Some(vec!["read".to_owned(), "write".to_owned()])
        );

        let secondary = store
            .replace_agent_profile("reviewer", definition("secondary"))
            .await
            .unwrap();
        assert_eq!(secondary.revision, 1);
        let second = store
            .replace_agent_profile(DEFAULT_AGENT_PROFILE_ID, definition("second"))
            .await
            .unwrap();
        assert_eq!(second.revision, 2);
        assert_eq!(store.list_agent_profiles().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn rejects_invalid_ids_without_mutating_memory_store() {
        let store = MemoryAgentProfileStore::new();
        assert!(matches!(
            store
                .replace_agent_profile(" bad ", definition("ignored"))
                .await,
            Err(AgentProfileStoreError::Validation(
                AgentProfileValidationError::InvalidProfileId { .. }
            ))
        ));
        assert_eq!(
            store.list_agent_profiles().await.unwrap(),
            vec![default_agent_profile()]
        );
    }

    #[tokio::test]
    async fn disk_store_round_trips_atomically_with_owner_only_permissions() {
        let root = std::env::temp_dir().join(format!("phi-agent-profiles-{}", SessionId::new()));
        let path = root.join("agent-profiles.json");
        let store = DiskAgentProfileStore::new(&path);

        // Read-only access must not create the file merely to expose default@0.
        assert_eq!(
            store
                .get_agent_profile(DEFAULT_AGENT_PROFILE_ID)
                .await
                .unwrap(),
            Some(default_agent_profile())
        );
        assert!(!path.exists());

        let first = store
            .replace_agent_profile("reviewer", definition("review carefully"))
            .await
            .unwrap();
        assert_eq!(first.revision, 1);
        let second = store
            .replace_agent_profile("reviewer", definition("review again"))
            .await
            .unwrap();
        assert_eq!(second.revision, 2);
        assert_eq!(
            store.get_agent_profile("reviewer").await.unwrap(),
            Some(second.clone())
        );

        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).await.unwrap()).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 1);
        assert_eq!(json[0]["agent_profile_id"], "reviewer");
        assert_eq!(json[0]["revision"], 2);
        assert_eq!(
            json[0]["definition"]["initial_capability_mode"],
            "read_only"
        );

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
            std::env::temp_dir().join(format!("phi-agent-profiles-invalid-{}", SessionId::new()));
        let path = root.join("agent-profiles.json");
        fs::create_dir_all(&root).await.unwrap();

        let mut profile = AgentProfile {
            agent_profile_id: "reviewer".to_owned(),
            revision: 1,
            definition: definition("review").normalized().unwrap(),
        };
        fs::write(
            &path,
            serde_json::to_vec(&[profile.clone(), profile.clone()]).unwrap(),
        )
        .await
        .unwrap();
        let store = DiskAgentProfileStore::new(&path);
        assert!(matches!(
            store.list_agent_profiles().await,
            Err(AgentProfileStoreError::InvalidCollection { .. })
        ));

        profile.definition.model = Some(" unnormalized ".to_owned());
        fs::write(&path, serde_json::to_vec(&[profile]).unwrap())
            .await
            .unwrap();
        assert!(matches!(
            store.list_agent_profiles().await,
            Err(AgentProfileStoreError::InvalidCollection { .. })
        ));

        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn revision_exhaustion_does_not_replace_the_visible_profile() {
        let root =
            std::env::temp_dir().join(format!("phi-agent-profiles-overflow-{}", SessionId::new()));
        let path = root.join("agent-profiles.json");
        fs::create_dir_all(&root).await.unwrap();
        let profile = AgentProfile {
            agent_profile_id: "reviewer".to_owned(),
            revision: u64::MAX,
            definition: definition("review").normalized().unwrap(),
        };
        fs::write(
            &path,
            serde_json::to_vec(std::slice::from_ref(&profile)).unwrap(),
        )
        .await
        .unwrap();
        let store = DiskAgentProfileStore::new(&path);

        assert!(matches!(
            store
                .replace_agent_profile("reviewer", definition("replacement"))
                .await,
            Err(AgentProfileStoreError::RevisionExhausted { .. })
        ));
        assert_eq!(
            store.get_agent_profile("reviewer").await.unwrap(),
            Some(profile)
        );

        fs::remove_dir_all(root).await.unwrap();
    }
}

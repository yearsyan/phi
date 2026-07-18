mod agent_profile;
mod disk;
mod memory;
mod provider;
mod scheduled_task;

use std::{fmt, io, path::PathBuf, time::Duration};

use async_trait::async_trait;
use phi::{ReasoningEffort, Workspace};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::runtime::{PinnedAgentProfile, SessionId};

pub use agent_profile::{
    AgentProfileStore, AgentProfileStoreError, DiskAgentProfileStore, MemoryAgentProfileStore,
};
pub use disk::DiskControlStore;
pub use memory::MemoryControlStore;
pub use provider::{DiskProviderStore, MemoryProviderStore};
pub use scheduled_task::{
    DiskScheduledTaskStore, MemoryScheduledTaskStore, ScheduledTaskStore, ScheduledTaskStoreError,
};

pub const DEFAULT_PROFILE_ID: &str = "default";
pub const DEFAULT_MAX_RETRIES: usize = 10;
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ProviderKind {
    #[serde(rename = "openai_chat")]
    OpenAiChat,
    #[serde(rename = "openai_responses")]
    OpenAiResponses,
    #[serde(rename = "anthropic")]
    Anthropic,
}

/// Configuration payload for one persisted Provider profile. The API key is
/// intentionally present on disk but omitted from the public HTTP DTO.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider: ProviderKind,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    pub max_context_tokens: u64,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub reasoning_effort: Option<ReasoningEffort>,
    #[serde(default = "default_max_retries")]
    pub max_retries: usize,
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
    #[serde(default = "default_stream_idle_timeout_secs")]
    pub stream_idle_timeout_secs: u64,
    #[serde(default)]
    pub revision: u64,
}

impl ProviderConfig {
    pub fn new(
        provider: ProviderKind,
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        model: impl Into<String>,
        max_context_tokens: u64,
    ) -> Self {
        Self {
            provider,
            api_key: api_key.into(),
            base_url: base_url.into(),
            model: model.into(),
            system_prompt: None,
            max_output_tokens: None,
            max_context_tokens,
            temperature: None,
            reasoning_effort: None,
            max_retries: DEFAULT_MAX_RETRIES,
            request_timeout_secs: DEFAULT_REQUEST_TIMEOUT.as_secs(),
            stream_idle_timeout_secs: DEFAULT_STREAM_IDLE_TIMEOUT.as_secs(),
            revision: 0,
        }
    }
}

impl fmt::Debug for ProviderConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderConfig")
            .field("provider", &self.provider)
            .field("api_key", &"[REDACTED]")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("system_prompt", &self.system_prompt)
            .field("max_output_tokens", &self.max_output_tokens)
            .field("max_context_tokens", &self.max_context_tokens)
            .field("temperature", &self.temperature)
            .field("reasoning_effort", &self.reasoning_effort)
            .field("max_retries", &self.max_retries)
            .field("request_timeout_secs", &self.request_timeout_secs)
            .field("stream_idle_timeout_secs", &self.stream_idle_timeout_secs)
            .field("revision", &self.revision)
            .finish()
    }
}

/// A named Provider configuration selectable by a session's `profile_id`.
///
/// Profiles are serialized as flat objects so the on-disk representation is
/// a straightforward JSON array rather than a nested implementation detail.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderProfile {
    pub profile_id: String,
    #[serde(flatten)]
    pub config: ProviderConfig,
}

impl ProviderProfile {
    pub fn new(profile_id: impl Into<String>, config: ProviderConfig) -> Self {
        Self {
            profile_id: profile_id.into(),
            config,
        }
    }
}

impl fmt::Debug for ProviderProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderProfile")
            .field("profile_id", &self.profile_id)
            .field("config", &self.config)
            .finish()
    }
}

fn default_max_retries() -> usize {
    DEFAULT_MAX_RETRIES
}

fn default_request_timeout_secs() -> u64 {
    DEFAULT_REQUEST_TIMEOUT.as_secs()
}

fn default_stream_idle_timeout_secs() -> u64 {
    DEFAULT_STREAM_IDLE_TIMEOUT.as_secs()
}

/// Metadata needed to find and rebuild a stateful agent. Conversation messages
/// remain behind `phi::SessionStorage`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: SessionId,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    pub profile_id: String,
    /// Complete Agent Profile snapshot selected before this session was
    /// activated. Old metadata omits it and resumes with the built-in
    /// `default@0` profile.
    #[serde(default)]
    pub agent_profile: Option<PinnedAgentProfile>,
    pub model: String,
    #[serde(default)]
    pub workspace: Option<Workspace>,
    #[serde(default)]
    pub reasoning_effort: Option<ReasoningEffort>,
    #[serde(default)]
    pub config_revision: u64,
}

impl SessionRecord {
    pub fn new(
        id: SessionId,
        profile_id: impl Into<String>,
        model: impl Into<String>,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Self {
        Self {
            id,
            title: None,
            pinned: false,
            profile_id: profile_id.into(),
            agent_profile: None,
            model: model.into(),
            workspace: None,
            reasoning_effort,
            config_revision: 0,
        }
    }

    pub fn with_workspace(mut self, workspace: Workspace) -> Self {
        self.workspace = Some(workspace);
        self
    }
}

#[async_trait]
pub trait ControlStore: Send + Sync {
    async fn create_session(&self, session: SessionRecord) -> Result<(), ControlStoreError>;

    /// Replaces metadata for an existing session. Implementations must not
    /// create a missing session as a side effect of an update.
    async fn update_session(&self, session: SessionRecord) -> Result<(), ControlStoreError>;

    async fn get_session(
        &self,
        session_id: SessionId,
    ) -> Result<Option<SessionRecord>, ControlStoreError>;

    async fn list_sessions(&self) -> Result<Vec<SessionRecord>, ControlStoreError>;

    async fn delete_session(&self, session_id: SessionId) -> Result<bool, ControlStoreError>;
}

#[async_trait]
pub trait ProviderStore: Send + Sync {
    async fn list_providers(&self) -> Result<Vec<ProviderProfile>, ProviderStoreError>;

    async fn get_provider_by_id(
        &self,
        profile_id: &str,
    ) -> Result<Option<ProviderConfig>, ProviderStoreError>;

    /// Atomically inserts or replaces one named Provider profile and assigns
    /// that profile's next monotonically increasing revision.
    async fn replace_provider_for(
        &self,
        profile_id: &str,
        provider: ProviderConfig,
    ) -> Result<ProviderConfig, ProviderStoreError>;

    /// Compatibility helper for the conventional `default` profile.
    async fn get_provider(&self) -> Result<Option<ProviderConfig>, ProviderStoreError> {
        self.get_provider_by_id(DEFAULT_PROFILE_ID).await
    }

    /// Compatibility helper for the conventional `default` profile.
    async fn replace_provider(
        &self,
        provider: ProviderConfig,
    ) -> Result<ProviderConfig, ProviderStoreError> {
        self.replace_provider_for(DEFAULT_PROFILE_ID, provider)
            .await
    }
}

#[derive(Debug, Error)]
pub enum ControlStoreError {
    #[error("session {session_id} already exists")]
    AlreadyExists { session_id: SessionId },

    #[error("session {session_id} does not exist")]
    NotFound { session_id: SessionId },

    #[error("control store I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("invalid control store JSON at {path}: {source}")]
    Serialization {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("session record at {path} has ID {actual}, expected {expected}")]
    SessionIdMismatch {
        path: PathBuf,
        expected: SessionId,
        actual: SessionId,
    },

    #[error("control store failed: {0}")]
    Backend(String),
}

#[derive(Debug, Error)]
pub enum ProviderStoreError {
    #[error("invalid provider profile ID {profile_id:?}: {message}")]
    InvalidProfileId { profile_id: String, message: String },

    #[error("provider store I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("invalid provider configuration JSON at {path}: {source}")]
    Serialization {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("invalid provider profile collection at {path}: {message}")]
    InvalidCollection { path: PathBuf, message: String },
}

use std::{fmt, sync::Arc, time::Duration};

use async_trait::async_trait;
use phi::{
    Agent, AnthropicMessagesProvider, ContextManager, ContextManagerRegistry,
    DEFAULT_CONTEXT_MANAGEMENT_THRESHOLD_PERCENT, OpenAiChatProvider, OpenAiResponsesProvider,
    ProviderError, ReasoningEffort, RetryConfig, SkillCatalog, SkillError, SkillsConfig,
};
use thiserror::Error;

use super::SessionId;
use crate::store::{ProviderConfig, ProviderKind, ProviderStore, ProviderStoreError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentBuildRequest {
    pub session_id: SessionId,
    pub profile_id: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub reasoning_effort_is_override: bool,
}

impl AgentBuildRequest {
    pub fn new(session_id: SessionId, profile_id: impl Into<String>) -> Self {
        Self {
            session_id,
            profile_id: profile_id.into(),
            model: None,
            reasoning_effort: None,
            reasoning_effort_is_override: false,
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_reasoning_effort(mut self, reasoning_effort: ReasoningEffort) -> Self {
        self.reasoning_effort = Some(reasoning_effort);
        self.reasoning_effort_is_override = true;
        self
    }

    pub fn without_reasoning_effort(mut self) -> Self {
        self.reasoning_effort = None;
        self.reasoning_effort_is_override = true;
        self
    }
}

/// A newly built agent and the effective, persistable profile selection used
/// to construct it.
pub struct BuiltAgent {
    pub agent: Agent,
    pub skills: SkillCatalog,
    pub profile_id: String,
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
}

/// Builds a fresh in-process agent for a persisted session.
#[async_trait]
pub trait AgentFactory: Send + Sync {
    async fn build(&self, request: &AgentBuildRequest) -> Result<BuiltAgent, AgentFactoryError>;
}

/// Factory backed by the Provider configuration managed through the daemon's
/// HTTP API. Every build reads the latest committed configuration, so new and
/// restart-restored actors do not require process environment variables.
#[derive(Clone)]
pub struct ConfiguredAgentFactory {
    providers: Arc<dyn ProviderStore>,
    http_client: reqwest::Client,
    skills: SkillsConfig,
    context_management_threshold_percent: u8,
    context_managers: ContextManagerRegistry,
}

impl ConfiguredAgentFactory {
    pub fn new(providers: Arc<dyn ProviderStore>) -> Self {
        Self {
            providers,
            http_client: reqwest::Client::new(),
            skills: SkillsConfig::disabled(),
            context_management_threshold_percent: DEFAULT_CONTEXT_MANAGEMENT_THRESHOLD_PERCENT,
            context_managers: ContextManagerRegistry::default(),
        }
    }

    /// Replaces the HTTP client shared by every provider built by this
    /// factory. A single client preserves connection pools across sessions.
    pub fn http_client(mut self, http_client: reqwest::Client) -> Self {
        self.http_client = http_client;
        self
    }

    pub fn skills_config(mut self, skills: SkillsConfig) -> Self {
        self.skills = skills;
        self
    }

    /// Sets the context-usage percentage that activates registered context
    /// managers for every Agent built by this factory.
    pub fn context_management_threshold_percent(mut self, threshold_percent: u8) -> Self {
        self.context_management_threshold_percent = threshold_percent.clamp(1, 100);
        self
    }

    /// Registers one context manager for every Agent built by this factory.
    pub fn context_manager(mut self, manager: impl ContextManager + 'static) -> Self {
        self.context_managers.register(manager);
        self
    }

    pub fn context_managers(mut self, managers: ContextManagerRegistry) -> Self {
        self.context_managers.extend(managers);
        self
    }
}

impl fmt::Debug for ConfiguredAgentFactory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfiguredAgentFactory")
            .field("skills_enabled", &self.skills.enabled)
            .field("skill_directories", &self.skills.directories.len())
            .field(
                "context_management_threshold_percent",
                &self.context_management_threshold_percent,
            )
            .field("context_manager_count", &self.context_managers.len())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl AgentFactory for ConfiguredAgentFactory {
    async fn build(&self, request: &AgentBuildRequest) -> Result<BuiltAgent, AgentFactoryError> {
        let config = self
            .providers
            .get_provider_by_id(&request.profile_id)
            .await?
            .ok_or_else(|| AgentFactoryError::ProfileUnavailable {
                profile_id: request.profile_id.clone(),
            })?;
        let config = normalize_provider_config(config)?;
        let model = request.model.as_deref().unwrap_or(&config.model).trim();
        if model.is_empty() {
            return Err(AgentFactoryError::InvalidBuildRequest {
                field: "model",
                message: "must not be empty".to_owned(),
            });
        }
        let model = model.to_owned();
        let reasoning_effort = if request.reasoning_effort_is_override {
            request.reasoning_effort
        } else {
            config.reasoning_effort
        };
        let retry_config = RetryConfig::default()
            .with_max_retries(config.max_retries)
            .with_request_timeout(Duration::from_secs(config.request_timeout_secs))
            .with_stream_idle_timeout(Duration::from_secs(config.stream_idle_timeout_secs));

        let mut builder = match config.provider {
            ProviderKind::OpenAiChat => Agent::builder(
                OpenAiChatProvider::new_with_client(
                    self.http_client.clone(),
                    config.api_key.clone(),
                    config.base_url.clone(),
                    model.clone(),
                )?
                .retry_config(retry_config),
            ),
            ProviderKind::OpenAiResponses => Agent::builder(
                OpenAiResponsesProvider::new_with_client(
                    self.http_client.clone(),
                    config.api_key.clone(),
                    config.base_url.clone(),
                    model.clone(),
                )?
                .retry_config(retry_config),
            ),
            ProviderKind::Anthropic => Agent::builder(
                AnthropicMessagesProvider::with_base_url_and_client(
                    self.http_client.clone(),
                    config.api_key.clone(),
                    config.base_url.clone(),
                    model.clone(),
                )?
                .retry_config(retry_config),
            ),
        };
        builder = builder.model(model.clone());

        if let Some(system_prompt) = &config.system_prompt {
            builder = builder.system_prompt(system_prompt.clone());
        }
        if let Some(max_output_tokens) = config.max_output_tokens {
            builder = builder.max_tokens(max_output_tokens);
        }
        builder = builder.max_context_tokens(config.max_context_tokens);
        if let Some(temperature) = config.temperature {
            builder = builder.temperature(temperature);
        }
        if let Some(reasoning_effort) = reasoning_effort {
            builder = builder.reasoning_effort(reasoning_effort);
        }
        let skills = SkillCatalog::load(&self.skills).await?;
        builder = builder
            .skills(skills.clone())
            .context_management_threshold_percent(self.context_management_threshold_percent)
            .context_managers(self.context_managers.clone());

        Ok(BuiltAgent {
            agent: builder.build(),
            skills,
            profile_id: request.profile_id.clone(),
            model,
            reasoning_effort,
        })
    }
}

/// Validates and normalizes a configuration before it becomes visible to new
/// Agent builds. This performs no network request.
pub fn normalize_provider_config(
    mut config: ProviderConfig,
) -> Result<ProviderConfig, AgentFactoryError> {
    config.api_key = required("api_key", config.api_key)?;
    config.base_url = required("base_url", config.base_url)?;
    config.model = required("model", config.model)?;
    if config.max_output_tokens == Some(0) {
        return Err(invalid("max_output_tokens", "must be greater than zero"));
    }
    if config.max_context_tokens == 0 {
        return Err(invalid("max_context_tokens", "must be greater than zero"));
    }
    if config.temperature.is_some_and(|value| !value.is_finite()) {
        return Err(invalid("temperature", "must be a finite number"));
    }
    if config.request_timeout_secs == 0 {
        return Err(invalid("request_timeout_secs", "must be greater than zero"));
    }
    if config.stream_idle_timeout_secs == 0 {
        return Err(invalid(
            "stream_idle_timeout_secs",
            "must be greater than zero",
        ));
    }
    if config
        .system_prompt
        .as_ref()
        .is_some_and(|value| value.is_empty())
    {
        config.system_prompt = None;
    }

    Ok(config)
}

fn required(field: &'static str, value: String) -> Result<String, AgentFactoryError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid(field, "must not be empty"));
    }
    Ok(value.to_owned())
}

fn invalid(field: &'static str, message: impl Into<String>) -> AgentFactoryError {
    AgentFactoryError::InvalidProviderConfig {
        field,
        message: message.into(),
    }
}

/// Placeholder used by tests/embedders that provide no managed Provider.
#[derive(Clone, Debug, Default)]
pub struct UnconfiguredAgentFactory;

#[async_trait]
impl AgentFactory for UnconfiguredAgentFactory {
    async fn build(&self, request: &AgentBuildRequest) -> Result<BuiltAgent, AgentFactoryError> {
        Err(AgentFactoryError::ProfileUnavailable {
            profile_id: request.profile_id.clone(),
        })
    }
}

#[derive(Debug, Error)]
pub enum AgentFactoryError {
    #[error("agent profile {profile_id:?} is not configured")]
    ProfileUnavailable { profile_id: String },

    #[error("invalid Provider configuration field {field}: {message}")]
    InvalidProviderConfig {
        field: &'static str,
        message: String,
    },

    #[error("invalid agent build request field {field}: {message}")]
    InvalidBuildRequest {
        field: &'static str,
        message: String,
    },

    #[error("could not load Provider configuration: {0}")]
    ProviderStore(#[from] ProviderStoreError),

    #[error("could not build provider: {0}")]
    Provider(#[from] ProviderError),

    #[error("could not load skills: {0}")]
    Skills(#[from] SkillError),

    #[error("could not build agent: {0}")]
    Build(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{DEFAULT_PROFILE_ID, MemoryProviderStore, ProviderStore};

    async fn configured_factory() -> ConfiguredAgentFactory {
        let store = Arc::new(MemoryProviderStore::new());
        let mut config = ProviderConfig::new(
            ProviderKind::OpenAiChat,
            "test-key",
            "https://example.test/v1",
            "default-model",
            8_192,
        );
        config.system_prompt = Some("Test prompt".to_owned());
        config.max_output_tokens = Some(512);
        config.temperature = Some(0.25);
        config.reasoning_effort = Some(ReasoningEffort::Medium);
        config.max_retries = 2;
        config.request_timeout_secs = 5;
        store.replace_provider(config).await.unwrap();
        ConfiguredAgentFactory::new(store)
    }

    #[tokio::test]
    async fn request_overrides_model_and_reasoning_effort() {
        let factory = configured_factory().await;
        let request = AgentBuildRequest::new(SessionId::new(), DEFAULT_PROFILE_ID)
            .with_model("override-model")
            .with_reasoning_effort(ReasoningEffort::High);

        let built = factory.build(&request).await.unwrap();

        assert_eq!(built.profile_id, DEFAULT_PROFILE_ID);
        assert_eq!(built.model, "override-model");
        assert_eq!(built.reasoning_effort, Some(ReasoningEffort::High));
        assert!(built.agent.messages().is_empty());

        let cleared = factory
            .build(
                &AgentBuildRequest::new(SessionId::new(), DEFAULT_PROFILE_ID)
                    .without_reasoning_effort(),
            )
            .await
            .unwrap();
        assert_eq!(cleared.reasoning_effort, None);
    }

    #[tokio::test]
    async fn builds_the_provider_selected_by_profile_id() {
        let store = Arc::new(MemoryProviderStore::new());
        store
            .replace_provider_for(
                "secondary",
                ProviderConfig::new(
                    ProviderKind::Anthropic,
                    "secondary-key",
                    "https://example.test/v1",
                    "secondary-model",
                    200_000,
                ),
            )
            .await
            .unwrap();
        let factory = ConfiguredAgentFactory::new(store);

        let built = factory
            .build(&AgentBuildRequest::new(SessionId::new(), "secondary"))
            .await
            .unwrap();

        assert_eq!(built.profile_id, "secondary");
        assert_eq!(built.model, "secondary-model");
    }

    #[tokio::test]
    async fn rejects_unknown_or_unconfigured_profiles() {
        let store = Arc::new(MemoryProviderStore::new());
        let factory = ConfiguredAgentFactory::new(store);
        assert!(matches!(
            factory
                .build(&AgentBuildRequest::new(
                    SessionId::new(),
                    DEFAULT_PROFILE_ID
                ))
                .await,
            Err(AgentFactoryError::ProfileUnavailable { .. })
        ));

        let configured = configured_factory().await;
        assert!(matches!(
            configured
                .build(&AgentBuildRequest::new(SessionId::new(), "unknown"))
                .await,
            Err(AgentFactoryError::ProfileUnavailable { profile_id }) if profile_id == "unknown"
        ));
    }

    #[test]
    fn validates_and_redacts_provider_configuration() {
        let config = ProviderConfig::new(
            ProviderKind::OpenAiChat,
            "test-key",
            "https://example.test/v1",
            "model",
            0,
        );
        assert!(matches!(
            normalize_provider_config(config),
            Err(AgentFactoryError::InvalidProviderConfig {
                field: "max_context_tokens",
                ..
            })
        ));

        let mut config = ProviderConfig::new(
            ProviderKind::OpenAiResponses,
            "test-key",
            "https://example.test/v1",
            "model",
            128_000,
        );
        config.temperature = Some(f64::NAN);
        assert!(normalize_provider_config(config).is_err());

        let mut config = ProviderConfig::new(
            ProviderKind::OpenAiChat,
            "test-key",
            "https://example.test/v1",
            "model",
            128_000,
        );
        config.stream_idle_timeout_secs = 0;
        assert!(matches!(
            normalize_provider_config(config),
            Err(AgentFactoryError::InvalidProviderConfig {
                field: "stream_idle_timeout_secs",
                ..
            })
        ));

        let config = ProviderConfig::new(
            ProviderKind::Anthropic,
            "test-key",
            "https://example.test/v1",
            "model",
            200_000,
        );
        let output = format!("{config:?}");
        assert!(!output.contains("test-key"));
        assert!(output.contains("model"));
    }
}

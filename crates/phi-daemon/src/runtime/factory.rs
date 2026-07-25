use std::{collections::HashSet, fmt, sync::Arc, time::Duration};

use async_trait::async_trait;
use phi::{
    Agent, AnthropicMessagesProvider, BuiltinTools, CapabilityMode, ContextCompactor,
    DefaultContextCompactor, HookRegistry, LlmProvider, McpClient, McpClientOptions, McpError,
    McpHttpConfig, McpStdioConfig, OpenAiChatProvider, OpenAiResponsesProvider, ProviderError,
    ProviderEventStream, ProviderRequest, ReasoningEffort, RetryConfig, SkillCatalog, SkillError,
    SkillsConfig, Workspace,
};
use thiserror::Error;

use super::SessionId;
use super::{
    AgentProfileValidationError, DEFAULT_AGENT_PROFILE_ID, McpProfile, McpProfileDefinition,
    McpProfileValidationError, McpTransportDefinition, PinnedAgentProfile, compile_agent_profile,
};
use crate::store::{
    AgentProfileStore, AgentProfileStoreError, McpProfileStore, McpProfileStoreError,
    MemoryAgentProfileStore, MemoryMcpProfileStore, ProviderConfig, ProviderKind, ProviderStore,
    ProviderStoreError,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentBuildRequest {
    pub session_id: SessionId,
    pub profile_id: String,
    pub agent_profile_id: String,
    pub pinned_agent_profile: Option<PinnedAgentProfile>,
    /// Compatibility path for metadata written before Agent Profiles existed.
    /// Such sessions were created with the daemon's built-in prompt and must
    /// not silently adopt a later user replacement of the `default` profile.
    pub legacy_builtin_agent_profile: bool,
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub reasoning_effort_is_override: bool,
    pub workspace: Option<Workspace>,
    pub capability_mode: Option<CapabilityMode>,
    pub prompt_overlay: Option<String>,
}

impl AgentBuildRequest {
    pub fn new(session_id: SessionId, profile_id: impl Into<String>) -> Self {
        Self {
            session_id,
            profile_id: profile_id.into(),
            agent_profile_id: DEFAULT_AGENT_PROFILE_ID.to_owned(),
            pinned_agent_profile: None,
            legacy_builtin_agent_profile: false,
            model: None,
            reasoning_effort: None,
            reasoning_effort_is_override: false,
            workspace: None,
            capability_mode: None,
            prompt_overlay: None,
        }
    }

    pub fn with_agent_profile_id(mut self, agent_profile_id: impl Into<String>) -> Self {
        self.agent_profile_id = agent_profile_id.into();
        self.pinned_agent_profile = None;
        self.legacy_builtin_agent_profile = false;
        self
    }

    pub fn with_pinned_agent_profile(mut self, profile: PinnedAgentProfile) -> Self {
        self.agent_profile_id.clone_from(&profile.agent_profile_id);
        self.pinned_agent_profile = Some(profile);
        self.legacy_builtin_agent_profile = false;
        self
    }

    pub fn with_legacy_builtin_agent_profile(mut self) -> Self {
        self.agent_profile_id = DEFAULT_AGENT_PROFILE_ID.to_owned();
        self.pinned_agent_profile = None;
        self.legacy_builtin_agent_profile = true;
        self
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

    pub fn with_workspace(mut self, workspace: Workspace) -> Self {
        self.workspace = Some(workspace);
        self
    }

    pub fn with_capability_mode(mut self, capability_mode: CapabilityMode) -> Self {
        self.capability_mode = Some(capability_mode);
        self
    }

    pub fn with_prompt_overlay(mut self, prompt_overlay: impl Into<String>) -> Self {
        self.prompt_overlay = Some(prompt_overlay.into());
        self
    }
}

/// A newly built agent and the effective, persistable profile selection used
/// to construct it.
pub struct BuiltAgent {
    pub agent: Agent,
    pub skills: SkillCatalog,
    pub profile_id: String,
    pub agent_profile: PinnedAgentProfile,
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
}

/// Builds a fresh in-process agent for a persisted session.
///
/// Implementations must preserve `request.workspace` on the returned
/// [`BuiltAgent::agent`]. `ApplicationService` rejects explicit or restored
/// session workspaces that a factory silently changes.
#[async_trait]
pub trait AgentFactory: Send + Sync {
    async fn build(&self, request: &AgentBuildRequest) -> Result<BuiltAgent, AgentFactoryError>;
}

/// Factory backed by the Provider configuration managed through the daemon's
/// HTTP API. Every build reads the latest committed configuration, so new and
/// restart-restored actors do not require process environment variables.
type ContextCompactorFactory =
    dyn Fn(&AgentBuildRequest) -> Arc<dyn ContextCompactor> + Send + Sync + 'static;

pub(crate) enum ConfiguredProvider {
    OpenAiChat(OpenAiChatProvider),
    OpenAiResponses(OpenAiResponsesProvider),
    Anthropic(AnthropicMessagesProvider),
}

impl LlmProvider for ConfiguredProvider {
    fn stream(&self, request: ProviderRequest) -> ProviderEventStream {
        match self {
            Self::OpenAiChat(provider) => provider.stream(request),
            Self::OpenAiResponses(provider) => provider.stream(request),
            Self::Anthropic(provider) => provider.stream(request),
        }
    }

    fn extend_hooks(&mut self, hooks: HookRegistry) {
        match self {
            Self::OpenAiChat(provider) => provider.extend_hooks(hooks),
            Self::OpenAiResponses(provider) => provider.extend_hooks(hooks),
            Self::Anthropic(provider) => provider.extend_hooks(hooks),
        }
    }
}

#[derive(Clone)]
pub struct ConfiguredAgentFactory {
    providers: Arc<dyn ProviderStore>,
    agent_profiles: Arc<dyn AgentProfileStore>,
    mcp_profiles: Arc<dyn McpProfileStore>,
    http_client: reqwest::Client,
    skills: SkillsConfig,
    builtin_tools: Option<BuiltinTools>,
    default_workspace: Workspace,
    context_compactor_factory: Arc<ContextCompactorFactory>,
}

impl ConfiguredAgentFactory {
    pub fn new(providers: Arc<dyn ProviderStore>) -> Self {
        Self {
            providers,
            agent_profiles: Arc::new(MemoryAgentProfileStore::new()),
            mcp_profiles: Arc::new(MemoryMcpProfileStore::new()),
            http_client: reqwest::Client::new(),
            skills: SkillsConfig::disabled(),
            builtin_tools: None,
            default_workspace: Workspace::new("."),
            // Construct one strategy per Agent. The default implementation is
            // stateless today, but this boundary also supports future
            // session-scoped compactors without accidentally sharing state.
            context_compactor_factory: Arc::new(|_request| {
                Arc::new(DefaultContextCompactor::default())
            }),
        }
    }

    pub fn agent_profile_store(mut self, agent_profiles: Arc<dyn AgentProfileStore>) -> Self {
        self.agent_profiles = agent_profiles;
        self
    }

    pub fn mcp_profile_store(mut self, mcp_profiles: Arc<dyn McpProfileStore>) -> Self {
        self.mcp_profiles = mcp_profiles;
        self
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

    /// Installs the selected local coding tools on every subsequently built
    /// parent or child Agent.
    pub fn builtin_tools(mut self, tools: BuiltinTools) -> Self {
        self.default_workspace = tools.workspace().clone();
        self.builtin_tools = Some(tools);
        self
    }

    /// Replaces the context compactor selected for every Agent subsequently
    /// built by this factory. The standalone daemon uses the library's default
    /// strategy, while embedders retain an explicit seam for a session-specific
    /// resolver in the future.
    pub fn context_compactor<C>(mut self, compactor: C) -> Self
    where
        C: ContextCompactor + Clone + 'static,
    {
        self.context_compactor_factory = Arc::new(move |_request| Arc::new(compactor.clone()));
        self
    }

    /// Selects a fresh context compactor from the complete Agent build
    /// request. This keeps session-creation policy outside the runtime actor
    /// and permits different strategies per profile, model, or session.
    pub fn context_compactor_factory<F>(mut self, factory: F) -> Self
    where
        F: Fn(&AgentBuildRequest) -> Arc<dyn ContextCompactor> + Send + Sync + 'static,
    {
        self.context_compactor_factory = Arc::new(factory);
        self
    }

    /// Explicitly shares one compactor object across Agents. Prefer
    /// [`Self::context_compactor_factory`] for stateful implementations.
    pub fn shared_context_compactor(mut self, compactor: Arc<dyn ContextCompactor>) -> Self {
        self.context_compactor_factory = Arc::new(move |_request| Arc::clone(&compactor));
        self
    }
}

impl fmt::Debug for ConfiguredAgentFactory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfiguredAgentFactory")
            .field("skills_enabled", &self.skills.enabled)
            .field("skill_directories", &self.skills.directories.len())
            .field("builtin_tools_enabled", &self.builtin_tools.is_some())
            .field("agent_profile_store", &"configured")
            .field("mcp_profile_store", &"configured")
            .field("default_workspace", &self.default_workspace)
            .field("context_compactor_factory", &"configured")
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
        let workspace = request
            .workspace
            .clone()
            .unwrap_or_else(|| self.default_workspace.clone());
        let mut agent_profile = match &request.pinned_agent_profile {
            Some(profile) => {
                profile.validate()?;
                profile.clone()
            }
            None if request.legacy_builtin_agent_profile => {
                compile_agent_profile(&super::default_agent_profile(), &workspace)?
            }
            None => {
                let profile = self
                    .agent_profiles
                    .get_agent_profile(&request.agent_profile_id)
                    .await?
                    .ok_or_else(|| AgentFactoryError::AgentProfileUnavailable {
                        agent_profile_id: request.agent_profile_id.clone(),
                    })?;
                compile_agent_profile(&profile, &workspace)?
            }
        };
        if let Some(overlay) = request.prompt_overlay.as_deref().map(str::trim)
            && !overlay.is_empty()
        {
            agent_profile.compiled_system_prompt.push_str("\n\n");
            agent_profile.compiled_system_prompt.push_str(overlay);
        }
        let model = request
            .model
            .as_deref()
            .or(agent_profile.definition.model.as_deref())
            .unwrap_or(&config.model)
            .trim();
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
            agent_profile
                .definition
                .reasoning_effort
                .or(config.reasoning_effort)
        };
        let mcp_profiles = self
            .resolve_mcp_profiles(&agent_profile.definition.mcp_profile_ids)
            .await?;

        let provider = build_configured_provider(&config, &model, self.http_client.clone())?;
        let mut builder = Agent::builder(provider);
        let capability_mode = request
            .capability_mode
            .unwrap_or(agent_profile.definition.initial_capability_mode);
        let tool_policy = agent_profile.definition.tools.to_tool_policy();
        builder = builder
            .model(model.clone())
            .workspace(workspace.clone())
            .system_prompt(agent_profile.compiled_system_prompt.clone())
            .capability_mode(capability_mode)
            .tool_policy(tool_policy);

        if let Some(tools) = &self.builtin_tools {
            builder = builder.builtin_tools(tools.clone().in_workspace(workspace.clone()));
        }
        let mut mcp_tool_names = HashSet::new();
        for profile in mcp_profiles {
            let client = connect_mcp_profile(&profile, &workspace).await?;
            for definition in client.tool_definitions() {
                if !mcp_tool_names.insert(definition.name.clone()) {
                    return Err(AgentFactoryError::DuplicateMcpToolName {
                        tool_name: definition.name,
                    });
                }
            }
            builder = builder.mcp_client(client);
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
        let skills = SkillCatalog::load(&self.skills.resolve_against(&workspace)).await?;
        let skills = skills.select(
            agent_profile.definition.skills.allow.as_deref(),
            &agent_profile.definition.skills.deny,
        )?;
        let context_compactor = (self.context_compactor_factory)(request);
        builder = builder
            .skills(skills.clone())
            .shared_context_compactor(context_compactor);

        Ok(BuiltAgent {
            agent: builder.build(),
            skills,
            profile_id: request.profile_id.clone(),
            agent_profile,
            model,
            reasoning_effort,
        })
    }
}

impl ConfiguredAgentFactory {
    async fn resolve_mcp_profiles(
        &self,
        profile_ids: &[String],
    ) -> Result<Vec<McpProfile>, AgentFactoryError> {
        let mut profiles = Vec::with_capacity(profile_ids.len());
        let mut prefixes = HashSet::with_capacity(profile_ids.len());
        for profile_id in profile_ids {
            let profile = self
                .mcp_profiles
                .get_mcp_profile(profile_id)
                .await?
                .ok_or_else(|| AgentFactoryError::McpProfileUnavailable {
                    mcp_profile_id: profile_id.clone(),
                })?;
            let profile = profile.normalized()?;
            if !prefixes.insert(profile.definition.tool_name_prefix.clone()) {
                return Err(AgentFactoryError::DuplicateMcpToolPrefix {
                    tool_name_prefix: profile.definition.tool_name_prefix,
                });
            }
            profiles.push(profile);
        }
        Ok(profiles)
    }
}

async fn connect_mcp_profile(
    profile: &McpProfile,
    workspace: &Workspace,
) -> Result<McpClient, AgentFactoryError> {
    let McpProfileDefinition {
        transport,
        tool_name_prefix,
        connect_timeout_secs,
        request_timeout_secs,
        max_output_lines,
        max_output_bytes,
    } = &profile.definition;
    let mut options = McpClientOptions::default()
        .connect_timeout(Duration::from_secs(*connect_timeout_secs))
        .tool_name_prefix(tool_name_prefix.clone())
        .output_limits(*max_output_lines, *max_output_bytes);
    options = match request_timeout_secs {
        Some(timeout) => options.request_timeout(Duration::from_secs(*timeout)),
        None => options.without_request_timeout(),
    };

    let result = match transport {
        McpTransportDefinition::Stdio {
            command,
            args,
            current_dir,
            env,
            clear_env,
        } => {
            let current_dir = current_dir.as_deref().map_or_else(
                || workspace.root().to_path_buf(),
                |path| workspace.resolve(path),
            );
            let mut config = McpStdioConfig::new(command)
                .args(args)
                .current_dir(current_dir)
                .envs(env)
                .options(options);
            if *clear_env {
                config = config.clear_env();
            }
            McpClient::connect_stdio(config).await
        }
        McpTransportDefinition::Http {
            url,
            bearer_token,
            headers,
            allow_stateless,
            reinitialize_on_expired_session,
        } => {
            let mut config = McpHttpConfig::new(url)
                .allow_stateless(*allow_stateless)
                .reinitialize_on_expired_session(*reinitialize_on_expired_session)
                .options(options);
            if let Some(token) = bearer_token {
                config = config.bearer_token(token);
            }
            for (name, value) in headers {
                config = config.header(name, value);
            }
            McpClient::connect_http(config).await
        }
    };
    result.map_err(|source| AgentFactoryError::McpConnection {
        mcp_profile_id: profile.mcp_profile_id.clone(),
        source,
    })
}

pub(crate) fn build_configured_provider(
    config: &ProviderConfig,
    model: &str,
    http_client: reqwest::Client,
) -> Result<ConfiguredProvider, ProviderError> {
    let retry_config = RetryConfig::default()
        .with_max_retries(config.max_retries)
        .with_request_timeout(Duration::from_secs(config.request_timeout_secs))
        .with_stream_idle_timeout(Duration::from_secs(config.stream_idle_timeout_secs));
    match config.provider {
        ProviderKind::OpenAiChat => Ok(ConfiguredProvider::OpenAiChat(
            OpenAiChatProvider::new_with_client(
                http_client,
                config.api_key.clone(),
                config.base_url.clone(),
                model.to_owned(),
            )?
            .retry_config(retry_config),
        )),
        ProviderKind::OpenAiResponses => Ok(ConfiguredProvider::OpenAiResponses(
            OpenAiResponsesProvider::new_with_client(
                http_client,
                config.api_key.clone(),
                config.base_url.clone(),
                model.to_owned(),
            )?
            .retry_config(retry_config),
        )),
        ProviderKind::Anthropic => Ok(ConfiguredProvider::Anthropic(
            AnthropicMessagesProvider::with_base_url_and_client(
                http_client,
                config.api_key.clone(),
                config.base_url.clone(),
                model.to_owned(),
            )?
            .retry_config(retry_config),
        )),
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
    #[error("Provider profile {profile_id:?} is not configured")]
    ProfileUnavailable { profile_id: String },

    #[error("Agent profile {agent_profile_id:?} is not configured")]
    AgentProfileUnavailable { agent_profile_id: String },

    #[error("MCP profile {mcp_profile_id:?} is not configured")]
    McpProfileUnavailable { mcp_profile_id: String },

    #[error("multiple MCP profiles use tool name prefix {tool_name_prefix:?}")]
    DuplicateMcpToolPrefix { tool_name_prefix: String },

    #[error("multiple MCP profiles expose tool name {tool_name:?}")]
    DuplicateMcpToolName { tool_name: String },

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

    #[error("could not load Agent Profile configuration: {0}")]
    AgentProfileStore(#[from] AgentProfileStoreError),

    #[error("could not load MCP Profile configuration: {0}")]
    McpProfileStore(#[from] McpProfileStoreError),

    #[error("invalid Agent Profile: {0}")]
    AgentProfile(#[from] AgentProfileValidationError),

    #[error("invalid MCP Profile: {0}")]
    McpProfile(#[from] McpProfileValidationError),

    #[error("could not connect MCP profile {mcp_profile_id:?}: {source}")]
    McpConnection {
        mcp_profile_id: String,
        #[source]
        source: McpError,
    },

    #[error("could not build provider: {0}")]
    Provider(#[from] ProviderError),

    #[error("could not load skills: {0}")]
    Skills(#[from] SkillError),

    #[error("could not build agent: {0}")]
    Build(String),
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, HashSet},
        sync::atomic::{AtomicUsize, Ordering},
    };

    use axum::{Json, Router, extract::State, http::header, routing::post};
    use serde_json::{Value, json};
    use tokio::{
        net::TcpListener,
        sync::{Mutex, oneshot},
        task::JoinHandle,
    };

    use super::*;
    use crate::runtime::{
        DEFAULT_MCP_CONNECT_TIMEOUT_SECS, DEFAULT_MCP_OUTPUT_BYTES, DEFAULT_MCP_OUTPUT_LINES,
        DEFAULT_MCP_REQUEST_TIMEOUT_SECS,
    };
    use crate::store::{
        DEFAULT_PROFILE_ID, McpProfileStore, MemoryMcpProfileStore, MemoryProviderStore,
        ProviderStore,
    };

    type RequestCapture = Arc<Mutex<Option<oneshot::Sender<Value>>>>;

    async fn capture_chat_request(
        State(capture): State<RequestCapture>,
        Json(request): Json<Value>,
    ) -> ([(header::HeaderName, &'static str); 1], String) {
        if let Some(sender) = capture.lock().await.take() {
            let _ = sender.send(request);
        }
        let event = json!({
            "choices": [{
                "index": 0,
                "delta": { "content": "ok" },
                "finish_reason": "stop"
            }]
        });
        (
            [(header::CONTENT_TYPE, "text/event-stream")],
            format!("data: {event}\n\n"),
        )
    }

    async fn chat_server() -> (String, oneshot::Receiver<Value>, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (request, captured) = oneshot::channel();
        let app = Router::new()
            .route("/chat/completions", post(capture_chat_request))
            .with_state(Arc::new(Mutex::new(Some(request))));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}"), captured, server)
    }

    struct TestCompactor {
        name: &'static str,
    }

    #[async_trait]
    impl ContextCompactor for TestCompactor {
        fn name(&self) -> &'static str {
            self.name
        }

        fn should_compact(&self, _request: &phi::ContextCompactionRequest) -> bool {
            true
        }

        fn prompt(&self, _request: &phi::ContextCompactionRequest) -> String {
            "test prompt".to_owned()
        }

        async fn compact(
            &self,
            _provider: &dyn phi::LlmProvider,
            _request: phi::ContextCompactionRequest,
            _prompt: String,
        ) -> Result<phi::ContextCompactionPlan, phi::ContextCompactionError> {
            Err(phi::ContextCompactionError::new(
                "not used by this factory test",
            ))
        }
    }

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

    fn mcp_definition(prefix: &str) -> McpProfileDefinition {
        McpProfileDefinition {
            transport: McpTransportDefinition::Stdio {
                command: "test-mcp".to_owned(),
                args: Vec::new(),
                current_dir: None,
                env: BTreeMap::new(),
                clear_env: false,
            },
            tool_name_prefix: prefix.to_owned(),
            connect_timeout_secs: DEFAULT_MCP_CONNECT_TIMEOUT_SECS,
            request_timeout_secs: Some(DEFAULT_MCP_REQUEST_TIMEOUT_SECS),
            max_output_lines: DEFAULT_MCP_OUTPUT_LINES,
            max_output_bytes: DEFAULT_MCP_OUTPUT_BYTES,
        }
    }

    #[tokio::test]
    async fn request_overrides_model_and_reasoning_effort() {
        let factory = configured_factory().await;
        let workspace = Workspace::new("/workspace/request-specific");
        let request = AgentBuildRequest::new(SessionId::new(), DEFAULT_PROFILE_ID)
            .with_model("override-model")
            .with_reasoning_effort(ReasoningEffort::High)
            .with_workspace(workspace.clone());

        let built = factory.build(&request).await.unwrap();

        assert_eq!(built.profile_id, DEFAULT_PROFILE_ID);
        assert_eq!(built.model, "override-model");
        assert_eq!(built.reasoning_effort, Some(ReasoningEffort::High));
        assert_eq!(built.agent.workspace(), Some(&workspace));
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
    async fn installs_coding_tools_and_uses_the_fixed_system_prompt() {
        let (base_url, captured, server) = chat_server().await;
        let store = Arc::new(MemoryProviderStore::new());
        let mut config = ProviderConfig::new(
            ProviderKind::OpenAiChat,
            "test-key",
            base_url,
            "test-model",
            8_192,
        );
        config.system_prompt = Some("profile prompt must be ignored".to_owned());
        config.max_retries = 0;
        store.replace_provider(config).await.unwrap();
        let factory = ConfiguredAgentFactory::new(store).builtin_tools(BuiltinTools::all("."));
        let expected_system_prompt = crate::runtime::compile_agent_profile(
            &crate::runtime::default_agent_profile(),
            &factory.default_workspace,
        )
        .unwrap()
        .compiled_system_prompt;
        let mut built = factory
            .build(&AgentBuildRequest::new(
                SessionId::new(),
                DEFAULT_PROFILE_ID,
            ))
            .await
            .unwrap();

        built.agent.prompt("inspect the workspace").await.unwrap();
        let request = captured.await.unwrap();
        server.abort();
        let _ = server.await;

        assert_eq!(request["messages"][0]["role"], "system");
        assert_eq!(request["messages"][0]["content"], expected_system_prompt);
        assert!(
            request["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains("Workspace root:")
        );
        assert!(
            !request
                .to_string()
                .contains("profile prompt must be ignored")
        );

        let tool_names = request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap())
            .collect::<HashSet<_>>();
        assert_eq!(
            tool_names,
            HashSet::from(["bash", "bash_task_stop", "edit", "read", "write",])
        );
    }

    #[tokio::test]
    async fn builds_a_fresh_default_or_selected_context_compactor_per_agent() {
        let factory = configured_factory().await;
        let default = factory
            .build(&AgentBuildRequest::new(
                SessionId::new(),
                DEFAULT_PROFILE_ID,
            ))
            .await
            .unwrap();
        assert_eq!(default.agent.context_compactor_name(), Some("default"));

        let builds = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&builds);
        let factory = factory.context_compactor_factory(move |request| {
            observed.fetch_add(1, Ordering::SeqCst);
            let name = match request.model.as_deref() {
                Some("compact-model") => "model_compactor",
                _ => "profile_compactor",
            };
            Arc::new(TestCompactor { name })
        });
        let built = factory
            .build(&AgentBuildRequest::new(
                SessionId::new(),
                DEFAULT_PROFILE_ID,
            ))
            .await
            .unwrap();
        assert_eq!(
            built.agent.context_compactor_name(),
            Some("profile_compactor")
        );

        let built = factory
            .build(
                &AgentBuildRequest::new(SessionId::new(), DEFAULT_PROFILE_ID)
                    .with_model("compact-model"),
            )
            .await
            .unwrap();
        assert_eq!(
            built.agent.context_compactor_name(),
            Some("model_compactor")
        );
        assert_eq!(builds.load(Ordering::SeqCst), 2);
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

    #[tokio::test]
    async fn resolves_referenced_mcp_profiles_and_rejects_missing_or_duplicate_prefixes() {
        let mcp_profiles = Arc::new(MemoryMcpProfileStore::new());
        mcp_profiles
            .replace_mcp_profile("first", mcp_definition("first"))
            .await
            .unwrap();
        mcp_profiles
            .replace_mcp_profile("second", mcp_definition("second"))
            .await
            .unwrap();
        let factory = configured_factory()
            .await
            .mcp_profile_store(mcp_profiles.clone());

        let resolved = factory
            .resolve_mcp_profiles(&["second".to_owned(), "first".to_owned()])
            .await
            .unwrap();
        assert_eq!(
            resolved
                .iter()
                .map(|profile| profile.mcp_profile_id.as_str())
                .collect::<Vec<_>>(),
            vec!["second", "first"]
        );
        assert!(matches!(
            factory
                .resolve_mcp_profiles(&["missing".to_owned()])
                .await,
            Err(AgentFactoryError::McpProfileUnavailable { mcp_profile_id })
                if mcp_profile_id == "missing"
        ));

        mcp_profiles
            .replace_mcp_profile("second", mcp_definition("first"))
            .await
            .unwrap();
        assert!(matches!(
            factory
                .resolve_mcp_profiles(&["first".to_owned(), "second".to_owned()])
                .await,
            Err(AgentFactoryError::DuplicateMcpToolPrefix {
                tool_name_prefix
            }) if tool_name_prefix == "first"
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

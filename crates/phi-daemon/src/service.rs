use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use futures_util::{StreamExt, stream::FuturesUnordered};
use phi::{
    Agent, BuiltSubagent, BuiltinTools, CapabilityMode, ConfiguredSubagentBuildRequest, Content,
    GenerationConfig, InMemorySessionStorage, Message, ReasoningEffort, Role, SessionSnapshot,
    SessionStorage, SkillCatalog, SkillsConfig, StorageError, SubagentBuildRequest, SubagentConfig,
    SubagentFactory, SubagentFactoryError, SubagentIsolation, SubagentOutputContract,
    SubagentResource, SubagentResourceDisposition, SubagentResourceFinalization,
    SubagentResourceInfo, SubagentRuntime, SubagentTools, SubagentType, Workspace,
};
use thiserror::Error;
use tokio::{
    sync::{Mutex, RwLock, RwLockReadGuard, Semaphore},
    task::JoinSet,
};

use crate::{
    runtime::{
        AgentBuildRequest, AgentFactory, AgentFactoryError, AgentHandle, AgentHandleError,
        AgentProfile, AgentProfileDefinition, AgentRegistry, AgentSummary, BuiltAgent,
        ConfiguredAgentFactory, DEFAULT_AGENT_PROFILE_ID, QueuedRun, RegistryError, SessionId,
        ShutdownFailure, UnconfiguredAgentFactory, WorktreeManager, compile_agent_profile,
        normalize_provider_config,
    },
    session_title::{ProviderSessionTitleGenerator, SessionTitleGenerator, SessionTitleRequest},
    store::{
        AgentProfileStore, AgentProfileStoreError, ControlStore, ControlStoreError,
        DEFAULT_PROFILE_ID, MemoryAgentProfileStore, MemoryControlStore, ProviderConfig,
        ProviderProfile, ProviderStore, ProviderStoreError, SessionRecord,
    },
};

const MAX_CONCURRENT_SESSION_TITLE_TASKS: usize = 8;

#[derive(Clone)]
pub struct ApplicationService {
    registry: AgentRegistry,
    store: Arc<dyn ControlStore>,
    session_storage: Arc<dyn SessionStorage>,
    factory: Arc<dyn AgentFactory>,
    provider_store: Option<Arc<dyn ProviderStore>>,
    agent_profile_store: Option<Arc<dyn AgentProfileStore>>,
    title_generator: Option<Arc<dyn SessionTitleGenerator>>,
    title_tasks: Arc<Mutex<JoinSet<()>>>,
    title_slots: Arc<Semaphore>,
    subagents_enabled: bool,
    subagent_worktrees: Option<WorktreeManager>,
    prepared: Arc<Mutex<HashMap<SessionId, AgentHandle>>>,
    lifecycle: Arc<RwLock<LifecycleState>>,
}

#[derive(Default)]
struct LifecycleState {
    closing: bool,
}

/// Adapts the daemon's provider-backed root factory to the library subagent
/// runtime. Children inherit the parent's effective profile and generation
/// settings, but are not registered as independently writable daemon sessions.
#[derive(Clone)]
struct DaemonSubagentFactory {
    factory: Arc<dyn AgentFactory>,
    parent_id: String,
    profile_id: String,
    agent_profile: crate::runtime::PinnedAgentProfile,
    model: String,
    reasoning_effort: Option<ReasoningEffort>,
    capability_ceiling: CapabilityMode,
    workspace: Option<Workspace>,
    worktrees: Option<WorktreeManager>,
}

#[derive(Debug)]
struct RecoveredWorktreeResource {
    location: String,
}

#[async_trait]
impl SubagentResource for RecoveredWorktreeResource {
    fn info(&self) -> SubagentResourceInfo {
        SubagentResourceInfo {
            kind: "git_worktree".to_owned(),
            location: Some(self.location.clone()),
        }
    }

    async fn finalize(
        &self,
        _disposition: SubagentResourceDisposition,
    ) -> Result<SubagentResourceFinalization, SubagentFactoryError> {
        Ok(SubagentResourceFinalization {
            preserved: true,
            location: Some(self.location.clone()),
            message: Some(
                "recovered worktree was preserved because its original base revision is unavailable"
                    .to_owned(),
            ),
        })
    }
}

impl DaemonSubagentFactory {
    async fn build_child(
        &self,
        request: ConfiguredSubagentBuildRequest,
        persisted_workspace: Option<Workspace>,
    ) -> Result<BuiltSubagent, SubagentFactoryError> {
        if request.base.allow_nested_subagents {
            return Err(SubagentFactoryError::new(
                "nested subagents are disabled by the daemon",
            ));
        }
        let model = request
            .effective_config
            .generation_config
            .model
            .clone()
            .unwrap_or_else(|| self.model.clone());
        let reasoning_effort = request
            .effective_config
            .generation_config
            .reasoning_effort
            .or(self.reasoning_effort);
        let resumed_worktree = if request.effective_config.isolation == SubagentIsolation::Worktree
        {
            persisted_workspace
                .as_ref()
                .map(|workspace| workspace.root().display().to_string())
        } else {
            None
        };
        let mut prepared_worktree = None;
        let workspace = match persisted_workspace {
            Some(workspace) => Some(workspace),
            None => match request.effective_config.isolation {
                SubagentIsolation::Shared => self.workspace.clone(),
                SubagentIsolation::Worktree => {
                    let workspace = self.workspace.as_ref().ok_or_else(|| {
                        SubagentFactoryError::new(
                            "worktree isolation requires a parent session workspace",
                        )
                    })?;
                    let manager = self.worktrees.as_ref().ok_or_else(|| {
                        SubagentFactoryError::new(
                            "worktree isolation is not configured for this daemon",
                        )
                    })?;
                    let prepared = manager
                        .create(workspace, &self.parent_id, &request.base.agent_id)
                        .await?;
                    let child_workspace = prepared.workspace().clone();
                    prepared_worktree = Some(prepared);
                    Some(child_workspace)
                }
            },
        };
        let profile = AgentProfile {
            agent_profile_id: self.agent_profile.agent_profile_id.clone(),
            revision: self.agent_profile.revision,
            definition: self.agent_profile.definition.clone(),
        };
        let child_profile = match workspace.as_ref() {
            Some(workspace) => compile_agent_profile(&profile, workspace)
                .map_err(|error| SubagentFactoryError::new(error.to_string()))?,
            None => self.agent_profile.clone(),
        };
        let overlay = subagent_prompt_overlay(&request.effective_config, workspace.as_ref());
        let mut build = AgentBuildRequest::new(SessionId::new(), self.profile_id.clone())
            .with_pinned_agent_profile(child_profile)
            .with_model(model)
            .with_capability_mode(request.effective_config.capability_mode)
            .with_prompt_overlay(overlay);
        if let Some(workspace) = workspace {
            build = build.with_workspace(workspace);
        }
        let build = match reasoning_effort {
            Some(reasoning_effort) => build.with_reasoning_effort(reasoning_effort),
            None => build.without_reasoning_effort(),
        };
        match self.factory.build(&build).await {
            Ok(built) => {
                let mut child = BuiltSubagent::new(built.agent);
                if let Some(prepared) = prepared_worktree {
                    child = child.with_resource(prepared.adopt_resource());
                } else if let Some(location) = resumed_worktree {
                    child = child.with_resource(Arc::new(RecoveredWorktreeResource { location }));
                }
                Ok(child)
            }
            Err(error) => {
                if let Some(prepared) = prepared_worktree {
                    let _ = prepared.finalize_unadopted().await;
                }
                Err(SubagentFactoryError::new(error.to_string()))
            }
        }
    }
}

#[async_trait]
impl SubagentFactory for DaemonSubagentFactory {
    async fn build(&self, request: SubagentBuildRequest) -> Result<Agent, SubagentFactoryError> {
        let configured = ConfiguredSubagentBuildRequest {
            effective_config: phi::EffectiveSubagentConfig {
                agent_type: SubagentType::General,
                capability_mode: self.capability_ceiling,
                generation_config: request.generation_config.clone(),
                output_contract: SubagentOutputContract::Text,
                isolation: SubagentIsolation::Shared,
                run_in_background: false,
            },
            base: request,
        };
        self.build_configured(configured)
            .await
            .map(|built| built.agent)
    }

    async fn build_configured(
        &self,
        request: ConfiguredSubagentBuildRequest,
    ) -> Result<BuiltSubagent, SubagentFactoryError> {
        self.build_child(request, None).await
    }

    async fn resume_configured(
        &self,
        request: ConfiguredSubagentBuildRequest,
        persisted_workspace: Option<Workspace>,
    ) -> Result<BuiltSubagent, SubagentFactoryError> {
        if request.effective_config.isolation == SubagentIsolation::Worktree
            && persisted_workspace.is_none()
        {
            return Err(SubagentFactoryError::new(
                "persisted worktree child has no workspace binding",
            ));
        }
        self.build_child(request, persisted_workspace).await
    }
}

impl ApplicationService {
    pub fn new(
        registry: AgentRegistry,
        store: Arc<dyn ControlStore>,
        session_storage: Arc<dyn SessionStorage>,
        factory: Arc<dyn AgentFactory>,
    ) -> Self {
        Self {
            registry,
            store,
            session_storage,
            factory,
            provider_store: None,
            agent_profile_store: None,
            title_generator: None,
            title_tasks: Arc::new(Mutex::new(JoinSet::new())),
            title_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_SESSION_TITLE_TASKS)),
            // Embedders opt in by installing the daemon integration. The
            // standalone daemon enables it from DaemonConfig by default.
            subagents_enabled: false,
            subagent_worktrees: None,
            prepared: Arc::new(Mutex::new(HashMap::new())),
            lifecycle: Arc::new(RwLock::new(LifecycleState::default())),
        }
    }

    /// Constructs the standard daemon service whose AgentFactory reads the
    /// Provider configuration managed through HTTP.
    pub fn managed(
        registry: AgentRegistry,
        store: Arc<dyn ControlStore>,
        session_storage: Arc<dyn SessionStorage>,
        provider_store: Arc<dyn ProviderStore>,
    ) -> Self {
        Self::managed_with_skills(
            registry,
            store,
            session_storage,
            provider_store,
            SkillsConfig::disabled(),
        )
    }

    pub fn managed_with_skills(
        registry: AgentRegistry,
        store: Arc<dyn ControlStore>,
        session_storage: Arc<dyn SessionStorage>,
        provider_store: Arc<dyn ProviderStore>,
        skills: SkillsConfig,
    ) -> Self {
        let http_client = reqwest::Client::new();
        let agent_profile_store: Arc<dyn AgentProfileStore> =
            Arc::new(MemoryAgentProfileStore::new());
        let factory = ConfiguredAgentFactory::new(Arc::clone(&provider_store))
            .agent_profile_store(Arc::clone(&agent_profile_store))
            .http_client(http_client.clone())
            .skills_config(skills);
        Self::managed_with_configured_factory(
            registry,
            store,
            session_storage,
            provider_store,
            agent_profile_store,
            factory,
            http_client,
        )
    }

    pub fn managed_with_skills_and_builtin_tools(
        registry: AgentRegistry,
        store: Arc<dyn ControlStore>,
        session_storage: Arc<dyn SessionStorage>,
        provider_store: Arc<dyn ProviderStore>,
        skills: SkillsConfig,
        builtin_tools: BuiltinTools,
    ) -> Self {
        let http_client = reqwest::Client::new();
        let agent_profile_store: Arc<dyn AgentProfileStore> =
            Arc::new(MemoryAgentProfileStore::new());
        let factory = ConfiguredAgentFactory::new(Arc::clone(&provider_store))
            .agent_profile_store(Arc::clone(&agent_profile_store))
            .http_client(http_client.clone())
            .skills_config(skills)
            .builtin_tools(builtin_tools);
        Self::managed_with_configured_factory(
            registry,
            store,
            session_storage,
            provider_store,
            agent_profile_store,
            factory,
            http_client,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn managed_with_profiles_skills_and_builtin_tools(
        registry: AgentRegistry,
        store: Arc<dyn ControlStore>,
        session_storage: Arc<dyn SessionStorage>,
        provider_store: Arc<dyn ProviderStore>,
        agent_profile_store: Arc<dyn AgentProfileStore>,
        skills: SkillsConfig,
        builtin_tools: BuiltinTools,
    ) -> Self {
        Self::managed_with_profiles_skills_and_builtin_tools_http_client(
            registry,
            store,
            session_storage,
            provider_store,
            agent_profile_store,
            skills,
            builtin_tools,
            reqwest::Client::new(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn managed_with_profiles_skills_and_builtin_tools_http_client(
        registry: AgentRegistry,
        store: Arc<dyn ControlStore>,
        session_storage: Arc<dyn SessionStorage>,
        provider_store: Arc<dyn ProviderStore>,
        agent_profile_store: Arc<dyn AgentProfileStore>,
        skills: SkillsConfig,
        builtin_tools: BuiltinTools,
        http_client: reqwest::Client,
    ) -> Self {
        let factory = ConfiguredAgentFactory::new(Arc::clone(&provider_store))
            .agent_profile_store(Arc::clone(&agent_profile_store))
            .http_client(http_client.clone())
            .skills_config(skills)
            .builtin_tools(builtin_tools);
        Self::managed_with_configured_factory(
            registry,
            store,
            session_storage,
            provider_store,
            agent_profile_store,
            factory,
            http_client,
        )
    }

    fn managed_with_configured_factory(
        registry: AgentRegistry,
        store: Arc<dyn ControlStore>,
        session_storage: Arc<dyn SessionStorage>,
        provider_store: Arc<dyn ProviderStore>,
        agent_profile_store: Arc<dyn AgentProfileStore>,
        factory: ConfiguredAgentFactory,
        http_client: reqwest::Client,
    ) -> Self {
        let factory: Arc<dyn AgentFactory> = Arc::new(factory);
        let mut service = Self::new(registry, store, session_storage, factory);
        service.title_generator = Some(Arc::new(
            ProviderSessionTitleGenerator::new(Arc::clone(&provider_store))
                .http_client(http_client),
        ));
        service.provider_store = Some(provider_store);
        service.agent_profile_store = Some(agent_profile_store);
        service
    }

    pub fn unconfigured() -> Self {
        Self::new(
            AgentRegistry::new(),
            Arc::new(MemoryControlStore::new()),
            Arc::new(InMemorySessionStorage::new()),
            Arc::new(UnconfiguredAgentFactory),
        )
    }

    pub fn with_subagents_enabled(mut self, enabled: bool) -> Self {
        self.subagents_enabled = enabled;
        self
    }

    pub fn with_session_title_generator<G>(mut self, generator: G) -> Self
    where
        G: SessionTitleGenerator + 'static,
    {
        self.title_generator = Some(Arc::new(generator));
        self
    }

    pub fn subagents_enabled(&self) -> bool {
        self.subagents_enabled
    }

    pub fn with_subagent_worktree_root(mut self, root: impl Into<std::path::PathBuf>) -> Self {
        self.subagent_worktrees = Some(WorktreeManager::new(root));
        self
    }

    /// Builds provider, tools and future MCP resources, but deliberately does
    /// not create persistent session metadata yet.
    pub async fn prepare_session(
        &self,
        profile_id: impl Into<String>,
    ) -> Result<PreparedSession, ServiceError> {
        self.prepare_session_with_options(
            profile_id.into(),
            DEFAULT_AGENT_PROFILE_ID.to_owned(),
            None,
            None,
        )
        .await
    }

    pub async fn prepare_session_configured(
        &self,
        profile_id: impl Into<String>,
        agent_profile_id: impl Into<String>,
        capability_mode: Option<CapabilityMode>,
    ) -> Result<PreparedSession, ServiceError> {
        self.prepare_session_with_options(
            profile_id.into(),
            agent_profile_id.into(),
            capability_mode,
            None,
        )
        .await
    }

    pub async fn prepare_session_configured_in_workspace(
        &self,
        profile_id: impl Into<String>,
        agent_profile_id: impl Into<String>,
        capability_mode: Option<CapabilityMode>,
        workspace: Workspace,
    ) -> Result<PreparedSession, ServiceError> {
        self.prepare_session_with_options(
            profile_id.into(),
            agent_profile_id.into(),
            capability_mode,
            Some(workspace),
        )
        .await
    }

    /// Prepares a session bound to an explicit library workspace with the
    /// default Agent Profile and capability mode.
    pub async fn prepare_session_in_workspace(
        &self,
        profile_id: impl Into<String>,
        workspace: Workspace,
    ) -> Result<PreparedSession, ServiceError> {
        self.prepare_session_with_options(
            profile_id.into(),
            DEFAULT_AGENT_PROFILE_ID.to_owned(),
            None,
            Some(workspace),
        )
        .await
    }

    async fn prepare_session_with_options(
        &self,
        profile_id: String,
        agent_profile_id: String,
        capability_mode: Option<CapabilityMode>,
        workspace: Option<Workspace>,
    ) -> Result<PreparedSession, ServiceError> {
        let _lifecycle = self.enter().await?;
        let session_id = SessionId::new();
        let profile_id = normalize_profile_id(&profile_id)?;
        let request =
            AgentBuildRequest::new(session_id, profile_id).with_agent_profile_id(agent_profile_id);
        let request = match workspace {
            Some(workspace) => request.with_workspace(workspace),
            None => request,
        };
        let request = match capability_mode {
            Some(capability_mode) => request.with_capability_mode(capability_mode),
            None => request,
        };
        let built = self.factory.build(&request).await?;
        if let Some(requested_workspace) = request.workspace.as_ref()
            && built.agent.workspace() != Some(requested_workspace)
        {
            return Err(AgentFactoryError::InvalidBuildRequest {
                field: "workspace",
                message: format!(
                    "factory built workspace {:?}, expected {:?}",
                    built.agent.workspace().map(|workspace| workspace.root()),
                    requested_workspace.root()
                ),
            }
            .into());
        }
        let handle = self.spawn_handle(session_id, built);
        self.prepared
            .lock()
            .await
            .insert(session_id, handle.clone());
        Ok(PreparedSession { handle })
    }

    /// Activates a `/new` connection on its first prompt. Only activated
    /// sessions are persisted, registered and returned by the list endpoint.
    pub async fn activate_session(
        &self,
        prepared: &PreparedSession,
    ) -> Result<AgentHandle, ServiceError> {
        let _lifecycle = self.enter().await?;
        let session_id = prepared.handle.session_id();
        // Activation and restart-time attach use the same per-session lock.
        // Once metadata becomes visible, an attaching client therefore either
        // observes this exact actor or waits for activation to fail cleanly.
        let _activation_guard = self.registry.lock_session(session_id).await;
        self.activate_session_locked(prepared).await
    }

    /// Atomically exposes a freshly prepared session and admits the prompt
    /// that caused activation. An attach racing through the list endpoint
    /// cannot submit work ahead of this first prompt.
    pub async fn activate_and_enqueue(
        &self,
        prepared: &PreparedSession,
        content: Content,
    ) -> Result<(AgentHandle, QueuedRun), ServiceError> {
        self.activate_and_enqueue_with_title_content(prepared, content.clone(), content)
            .await
    }

    /// Variant used by transports that expand an explicitly invoked skill
    /// before enqueueing. The title remains based on the user's unexpanded
    /// request rather than the injected skill instructions.
    pub async fn activate_and_enqueue_with_title_content(
        &self,
        prepared: &PreparedSession,
        content: Content,
        title_content: Content,
    ) -> Result<(AgentHandle, QueuedRun), ServiceError> {
        let _lifecycle = self.enter().await?;
        let session_id = prepared.handle.session_id();
        let _activation_guard = self.registry.lock_session(session_id).await;
        let handle = self.activate_session_locked(prepared).await?;
        let queued = handle.enqueue_prompt(content).await?;
        self.schedule_session_title(&handle, title_content).await;
        Ok((handle, queued))
    }

    async fn activate_session_locked(
        &self,
        prepared: &PreparedSession,
    ) -> Result<AgentHandle, ServiceError> {
        let session_id = prepared.handle.session_id();
        let view = prepared.handle.summary();
        if view.initialized {
            return Ok(prepared.handle.clone());
        }
        if self.registry.get(session_id).await.is_some() {
            return Err(RegistryError::AlreadyRegistered { session_id }.into());
        }

        let mut record = SessionRecord::new(
            view.session_id,
            view.profile_id,
            view.model,
            view.reasoning_effort,
        );
        record.agent_profile = Some(prepared.handle.agent_profile().clone());
        record.workspace = view.workspace;
        record.config_revision = view.config_revision;
        self.store.create_session(record.clone()).await?;

        if let Err(error) = prepared
            .handle
            .initialize(
                record,
                Arc::clone(&self.session_storage),
                Arc::clone(&self.store),
            )
            .await
        {
            let _ = self.store.delete_session(session_id).await;
            return Err(error.into());
        }

        if let Err(error) = self.registry.register(prepared.handle.clone()).await {
            let _ = self.store.delete_session(session_id).await;
            return Err(error.into());
        }
        self.prepared.lock().await.remove(&session_id);
        Ok(prepared.handle.clone())
    }

    /// Returns the single live actor for a session, constructing and restoring
    /// it once when the first connection attaches after process restart.
    pub async fn attach_session(&self, session_id: SessionId) -> Result<AgentHandle, ServiceError> {
        let _lifecycle = self.enter().await?;
        if let Some(handle) = self.registry.get(session_id).await
            && handle.status() != crate::runtime::AgentStatus::Closed
        {
            return Ok(handle);
        }

        // Reject random/unknown IDs before joining the keyed load lock. Re-read
        // after acquiring it because activation may still be publishing or
        // rolling back this record.
        if self.store.get_session(session_id).await?.is_none() {
            return Err(ServiceError::SessionNotFound { session_id });
        }
        let _load_guard = self.registry.lock_session(session_id).await;
        if let Some(handle) = self.registry.get(session_id).await {
            if handle.status() != crate::runtime::AgentStatus::Closed {
                return Ok(handle);
            }
            self.registry.remove(session_id).await;
        }

        let mut record = self
            .store
            .get_session(session_id)
            .await?
            .ok_or(ServiceError::SessionNotFound { session_id })?;
        let request = build_request_for_record(session_id, &record);
        let request = match record.reasoning_effort {
            Some(reasoning_effort) => request.with_reasoning_effort(reasoning_effort),
            None => request.without_reasoning_effort(),
        };
        let built = self.factory.build(&request).await?;
        let effective_workspace = built.agent.workspace().cloned();
        if let Some(stored_workspace) = record.workspace.as_ref()
            && effective_workspace.as_ref() != Some(stored_workspace)
        {
            return Err(AgentFactoryError::InvalidBuildRequest {
                field: "workspace",
                message: format!(
                    "factory built workspace {:?}, expected {:?}",
                    effective_workspace
                        .as_ref()
                        .map(|workspace| workspace.root()),
                    stored_workspace.root()
                ),
            }
            .into());
        }
        let workspace_migrated = record.workspace.is_none() && effective_workspace.is_some();
        if workspace_migrated {
            record.workspace = effective_workspace;
        }
        let agent_profile_migrated = record.agent_profile.is_none();
        if agent_profile_migrated {
            record.agent_profile = Some(built.agent_profile.clone());
        }
        let handle = self.spawn_handle(session_id, built);
        if let Err(error) = handle
            .initialize(
                record.clone(),
                Arc::clone(&self.session_storage),
                Arc::clone(&self.store),
            )
            .await
        {
            let _ = handle.shutdown().await;
            return Err(error.into());
        }
        if (workspace_migrated || agent_profile_migrated)
            && let Err(error) = self.store.update_session(record.clone()).await
        {
            let _ = handle.shutdown().await;
            return Err(error.into());
        }
        self.registry.register(handle.clone()).await?;
        if record.title.is_none()
            && let Some(content) = handle
                .snapshot()
                .messages
                .iter()
                .find(|message| message.role == Role::User)
                .and_then(|message| message.content.clone())
        {
            self.schedule_session_title(&handle, content).await;
        }
        Ok(handle)
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionListing>, ServiceError> {
        let mut records = self.store.list_sessions().await?;
        records.sort_unstable_by(|left, right| {
            right
                .pinned
                .cmp(&left.pinned)
                .then_with(|| right.id.as_uuid().cmp(&left.id.as_uuid()))
        });
        let live = self.registry.summaries().await;
        let mut sessions = Vec::with_capacity(records.len());
        for record in records {
            let state = live.get(&record.id).cloned();
            sessions.push(SessionListing { record, state });
        }
        Ok(sessions)
    }

    pub async fn get_session(&self, session_id: SessionId) -> Result<SessionListing, ServiceError> {
        let record = self
            .store
            .get_session(session_id)
            .await?
            .ok_or(ServiceError::SessionNotFound { session_id })?;
        let state = self
            .registry
            .get(session_id)
            .await
            .map(|handle| handle.summary());
        Ok(SessionListing { record, state })
    }

    /// Creates a durable, offline session at one protocol-safe point in the
    /// source session.
    ///
    /// Forks read the latest durable storage checkpoint, including while the
    /// source actor is running. Streaming drafts are therefore never copied.
    /// An after-response fork keeps a selected tool-call batch indivisible;
    /// a before-tool-calls fork keeps only its visible assistant preamble.
    pub async fn fork_session(
        &self,
        session_id: SessionId,
        message_index: usize,
        position: ForkPosition,
    ) -> Result<SessionListing, ServiceError> {
        let _lifecycle = self.enter().await?;
        let _session_guard = self.registry.lock_session(session_id).await;
        let source_record = self
            .store
            .get_session(session_id)
            .await?
            .ok_or(ServiceError::SessionNotFound { session_id })?;

        // Manual compaction replaces active transcript indexes. Share its
        // admission permit while resolving this fork so the client-supplied
        // index can never race either side of that replacement. Running turns
        // may continue appending protocol-complete durable checkpoints.
        let _fork_guard = self
            .registry
            .get(session_id)
            .await
            .map(|handle| handle.acquire_fork_guard())
            .transpose()?;

        let source = self
            .session_storage
            .load(&session_id.to_string())
            .await?
            .ok_or_else(|| StorageError::InvalidTranscript {
                session_id: session_id.to_string(),
                message: "session metadata exists without a transcript".to_owned(),
            })?;
        let messages = fork_messages_at(&source.messages, session_id, message_index, position)?;

        let fork_id = SessionId::new();
        let mut snapshot = SessionSnapshot::new(fork_id.to_string(), messages)?;
        snapshot.workspace = source_record
            .workspace
            .clone()
            .or_else(|| source.workspace.clone());
        snapshot.capability_mode = source.capability_mode;

        let mut record = source_record;
        record.id = fork_id;
        record.pinned = false;
        record.workspace.clone_from(&snapshot.workspace);

        self.session_storage.save(&snapshot).await?;
        if let Err(error) = self.store.create_session(record.clone()).await {
            if let Err(rollback_error) = self.session_storage.delete(&fork_id.to_string()).await {
                tracing::error!(
                    %fork_id,
                    error = %rollback_error,
                    "could not remove fork transcript after metadata creation failed"
                );
            }
            return Err(error.into());
        }

        Ok(SessionListing {
            record,
            state: None,
        })
    }

    pub async fn set_session_pinned(
        &self,
        session_id: SessionId,
        pinned: bool,
    ) -> Result<SessionListing, ServiceError> {
        let _lifecycle = self.enter().await?;
        let _session_guard = self.registry.lock_session(session_id).await;
        let mut record = self
            .store
            .get_session(session_id)
            .await?
            .ok_or(ServiceError::SessionNotFound { session_id })?;

        if record.pinned != pinned {
            if let Some(handle) = self.registry.get(session_id).await
                && handle.status() != crate::runtime::AgentStatus::Closed
            {
                handle.set_pinned(pinned).await?;
            } else {
                record.pinned = pinned;
                self.store.update_session(record).await?;
            }
        }

        self.get_session(session_id).await
    }

    pub async fn delete_session(&self, session_id: SessionId) -> Result<(), ServiceError> {
        let _lifecycle = self.enter().await?;
        let _session_guard = self.registry.lock_session(session_id).await;
        if self.store.get_session(session_id).await?.is_none() {
            return Err(ServiceError::SessionNotFound { session_id });
        }

        if let Some(handle) = self.registry.get(session_id).await {
            if handle.status() != crate::runtime::AgentStatus::Closed {
                handle.shutdown().await?;
            }
            self.registry.remove(session_id).await;
        }

        let record = self
            .store
            .get_session(session_id)
            .await?
            .ok_or(ServiceError::SessionNotFound { session_id })?;

        if !self.store.delete_session(session_id).await? {
            return Err(ServiceError::SessionNotFound { session_id });
        }
        if let Err(error) = self.session_storage.delete(&session_id.to_string()).await {
            if let Err(rollback_error) = self.store.create_session(record).await {
                tracing::error!(
                    %session_id,
                    error = %rollback_error,
                    "could not restore session metadata after transcript deletion failed"
                );
            }
            return Err(error.into());
        }
        Ok(())
    }

    /// Returns the immutable skill snapshot used by a live session. For an
    /// offline session this builds, but does not register, the runtime that a
    /// subsequent attach would receive.
    pub async fn session_skills(
        &self,
        session_id: SessionId,
    ) -> Result<SkillCatalog, ServiceError> {
        let _lifecycle = self.enter().await?;
        if let Some(handle) = self.registry.get(session_id).await
            && handle.status() != crate::runtime::AgentStatus::Closed
        {
            return Ok(handle.skill_catalog().clone());
        }
        if self.store.get_session(session_id).await?.is_none() {
            return Err(ServiceError::SessionNotFound { session_id });
        }
        let _load_guard = self.registry.lock_session(session_id).await;
        if let Some(handle) = self.registry.get(session_id).await
            && handle.status() != crate::runtime::AgentStatus::Closed
        {
            return Ok(handle.skill_catalog().clone());
        }
        let record = self
            .store
            .get_session(session_id)
            .await?
            .ok_or(ServiceError::SessionNotFound { session_id })?;
        let request = build_request_for_record(session_id, &record);
        let request = match record.reasoning_effort {
            Some(reasoning_effort) => request.with_reasoning_effort(reasoning_effort),
            None => request.without_reasoning_effort(),
        };
        Ok(self.factory.build(&request).await?.skills)
    }

    pub async fn provider_config(&self) -> Result<Option<ProviderConfig>, ServiceError> {
        self.provider_config_for(DEFAULT_PROFILE_ID).await
    }

    pub async fn agent_profiles(&self) -> Result<Vec<AgentProfile>, ServiceError> {
        let store = self
            .agent_profile_store
            .as_ref()
            .ok_or(ServiceError::AgentProfileManagementUnavailable)?;
        Ok(store.list_agent_profiles().await?)
    }

    pub async fn agent_profile(
        &self,
        agent_profile_id: &str,
    ) -> Result<Option<AgentProfile>, ServiceError> {
        let store = self
            .agent_profile_store
            .as_ref()
            .ok_or(ServiceError::AgentProfileManagementUnavailable)?;
        Ok(store.get_agent_profile(agent_profile_id.trim()).await?)
    }

    pub async fn configure_agent_profile(
        &self,
        agent_profile_id: &str,
        definition: AgentProfileDefinition,
    ) -> Result<AgentProfile, ServiceError> {
        let _lifecycle = self.enter().await?;
        let store = self
            .agent_profile_store
            .as_ref()
            .ok_or(ServiceError::AgentProfileManagementUnavailable)?;
        Ok(store
            .replace_agent_profile(agent_profile_id.trim(), definition)
            .await?)
    }

    pub async fn provider_configs(&self) -> Result<Vec<ProviderProfile>, ServiceError> {
        let store = self
            .provider_store
            .as_ref()
            .ok_or(ServiceError::ProviderManagementUnavailable)?;
        Ok(store.list_providers().await?)
    }

    pub async fn provider_config_for(
        &self,
        profile_id: &str,
    ) -> Result<Option<ProviderConfig>, ServiceError> {
        let store = self
            .provider_store
            .as_ref()
            .ok_or(ServiceError::ProviderManagementUnavailable)?;
        let profile_id = normalize_profile_id(profile_id)?;
        Ok(store.get_provider_by_id(&profile_id).await?)
    }

    pub async fn configure_provider(
        &self,
        provider: ProviderConfig,
    ) -> Result<ProviderConfig, ServiceError> {
        self.configure_provider_for(DEFAULT_PROFILE_ID, provider)
            .await
    }

    pub async fn configure_provider_for(
        &self,
        profile_id: &str,
        provider: ProviderConfig,
    ) -> Result<ProviderConfig, ServiceError> {
        let _lifecycle = self.enter().await?;
        let store = self
            .provider_store
            .as_ref()
            .ok_or(ServiceError::ProviderManagementUnavailable)?;
        let profile_id = normalize_profile_id(profile_id)?;
        let provider = normalize_provider_config(provider)?;
        Ok(store.replace_provider_for(&profile_id, provider).await?)
    }

    pub fn registry(&self) -> &AgentRegistry {
        &self.registry
    }

    fn spawn_handle(&self, session_id: SessionId, mut built: BuiltAgent) -> AgentHandle {
        let subagents = self
            .subagents_enabled
            .then(|| self.install_subagents(session_id, &mut built));
        AgentHandle::spawn_configured_with_skills_and_subagents(
            session_id,
            built.agent,
            built.profile_id,
            built.agent_profile,
            built.model,
            built.reasoning_effort,
            built.skills,
            subagents,
        )
    }

    fn install_subagents(&self, session_id: SessionId, built: &mut BuiltAgent) -> SubagentRuntime {
        let config = SubagentConfig {
            capability_ceiling: built.agent.capability_mode(),
            generation_config: GenerationConfig {
                model: Some(built.model.clone()),
                reasoning_effort: built.reasoning_effort,
                ..GenerationConfig::default()
            },
            ..SubagentConfig::default()
        };
        let child_factory: Arc<dyn SubagentFactory> = Arc::new(DaemonSubagentFactory {
            factory: Arc::clone(&self.factory),
            parent_id: session_id.to_string(),
            profile_id: built.profile_id.clone(),
            agent_profile: built.agent_profile.clone(),
            model: built.model.clone(),
            reasoning_effort: built.reasoning_effort,
            capability_ceiling: built.agent.capability_mode(),
            workspace: built.agent.workspace().cloned(),
            worktrees: self.subagent_worktrees.clone(),
        });
        let runtime = SubagentRuntime::with_storage(
            session_id.to_string(),
            child_factory,
            config,
            Arc::clone(&self.session_storage),
        );
        let SubagentTools {
            spawn_agent,
            send_agent_message,
            close_agent,
        } = SubagentTools::new(runtime.clone());
        built.agent.add_tool(spawn_agent);
        built.agent.add_tool(send_agent_message);
        built.agent.add_mandatory_tool(close_agent);
        runtime
    }

    async fn schedule_session_title(&self, handle: &AgentHandle, initial_content: Content) {
        let Some(generator) = self.title_generator.as_ref().map(Arc::clone) else {
            return;
        };
        let summary = handle.summary();
        if summary.title.is_some() {
            return;
        }
        let request = SessionTitleRequest {
            session_id: summary.session_id,
            profile_id: summary.profile_id,
            model: summary.model,
            reasoning_effort: summary.reasoning_effort,
            initial_content,
        };
        let title_permit = match Arc::clone(&self.title_slots).try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                tracing::warn!(
                    session_id = %summary.session_id,
                    capacity = MAX_CONCURRENT_SESSION_TITLE_TASKS,
                    "session title generation concurrency is full"
                );
                return;
            }
        };
        let handle = handle.clone();
        let session_id = handle.session_id();
        let mut tasks = self.title_tasks.lock().await;
        while let Some(result) = tasks.try_join_next() {
            if let Err(error) = result
                && !error.is_cancelled()
            {
                tracing::warn!(error = %error, "session title task failed");
            }
        }
        tasks.spawn(async move {
            let _title_permit = title_permit;
            match generator.generate_title(request).await {
                Ok(title) => {
                    if let Err(error) = handle.set_title(title).await {
                        tracing::warn!(
                            %session_id,
                            error = %error,
                            "could not persist generated session title"
                        );
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        %session_id,
                        error = %error,
                        "could not generate session title"
                    );
                }
            }
        });
    }

    pub async fn discard_prepared(&self, prepared: &PreparedSession) {
        let session_id = prepared.handle.session_id();
        let removed = self.prepared.lock().await.remove(&session_id);
        if let Some(handle) = removed {
            let _ = handle.shutdown().await;
        }
    }

    pub async fn shutdown(&self) -> Vec<ShutdownFailure> {
        let prepared = {
            let mut lifecycle = self.lifecycle.write().await;
            lifecycle.closing = true;
            self.prepared
                .lock()
                .await
                .drain()
                .map(|(_, handle)| handle)
                .collect::<Vec<_>>()
        };

        {
            let mut title_tasks = self.title_tasks.lock().await;
            title_tasks.abort_all();
            while let Some(result) = title_tasks.join_next().await {
                if let Err(error) = result
                    && !error.is_cancelled()
                {
                    tracing::warn!(error = %error, "session title task failed during shutdown");
                }
            }
        }

        let mut failures = self.registry.shutdown_all().await;
        let mut pending = FuturesUnordered::new();
        for handle in prepared {
            pending.push(async move {
                let session_id = handle.session_id();
                (session_id, handle.shutdown().await)
            });
        }
        while let Some((session_id, result)) = pending.next().await {
            if let Err(error) = result {
                failures.push(ShutdownFailure { session_id, error });
            }
        }
        failures
    }

    async fn enter(&self) -> Result<RwLockReadGuard<'_, LifecycleState>, ServiceError> {
        let lifecycle = self.lifecycle.read().await;
        if lifecycle.closing {
            return Err(ServiceError::ShuttingDown);
        }
        Ok(lifecycle)
    }
}

fn normalize_profile_id(profile_id: &str) -> Result<String, AgentFactoryError> {
    let profile_id = profile_id.trim();
    if profile_id.is_empty() {
        return Err(AgentFactoryError::InvalidProviderConfig {
            field: "profile_id",
            message: "must not be empty".to_owned(),
        });
    }
    if profile_id.len() > 128 {
        return Err(AgentFactoryError::InvalidProviderConfig {
            field: "profile_id",
            message: "must not exceed 128 bytes".to_owned(),
        });
    }
    if profile_id.chars().any(char::is_control) {
        return Err(AgentFactoryError::InvalidProviderConfig {
            field: "profile_id",
            message: "must not contain control characters".to_owned(),
        });
    }
    Ok(profile_id.to_owned())
}

fn build_request_for_record(session_id: SessionId, record: &SessionRecord) -> AgentBuildRequest {
    let mut request = AgentBuildRequest::new(session_id, record.profile_id.clone())
        .with_model(record.model.clone());
    if let Some(agent_profile) = &record.agent_profile {
        request = request.with_pinned_agent_profile(agent_profile.clone());
    } else {
        request = request.with_legacy_builtin_agent_profile();
    }
    match &record.workspace {
        Some(workspace) => request.with_workspace(workspace.clone()),
        None => request,
    }
}

impl Default for ApplicationService {
    fn default() -> Self {
        Self::unconfigured()
    }
}

fn subagent_prompt_overlay(
    config: &phi::EffectiveSubagentConfig,
    workspace: Option<&Workspace>,
) -> String {
    let role = match config.agent_type {
        SubagentType::General => {
            "You are a general-purpose child agent. Complete the delegated task independently and report a concise, evidence-backed result to the parent."
        }
        SubagentType::Explore => {
            "You are an exploration child agent. Inspect, search, and reason about the workspace without modifying it. Report concrete findings with file paths and relevant details."
        }
        SubagentType::Plan => {
            "You are a planning child agent. Explore the workspace and return a structured implementation plan. Do not implement the plan or modify project files."
        }
    };
    let output = match &config.output_contract {
        SubagentOutputContract::Text => {
            "Return a textual result as your final assistant response.".to_owned()
        }
        SubagentOutputContract::Json { required_fields } if required_fields.is_empty() => {
            "Return valid JSON as the final response.".to_owned()
        }
        SubagentOutputContract::Json { required_fields } => format!(
            "Return valid JSON as the final response. The top-level object must contain these fields: {}.",
            required_fields.join(", ")
        ),
    };
    let delivery = if config.run_in_background {
        "This task is running in the background. You may use notify_parent for progress or a blocker; the runtime automatically delivers your final response."
    } else {
        "This task is running in the foreground. Return the result directly; do not send a duplicate final notify_parent message."
    };
    let workspace = workspace
        .map(|workspace| format!("{:?}", workspace.root()))
        .unwrap_or_else(|| "not configured".to_owned());
    format!(
        "# Subagent Role\n{role}\n\n# Effective Child Policy\n- Capability mode: {:?}\n- Isolation: {:?}\n- Child workspace: {workspace}\n- The capability and workspace boundaries are runtime-enforced and cannot be widened by instructions.\n\n# Delivery\n{delivery}\n\n# Output Contract\n{output}",
        config.capability_mode, config.isolation,
    )
}

#[derive(Clone)]
pub struct PreparedSession {
    handle: AgentHandle,
}

impl PreparedSession {
    pub fn handle(&self) -> &AgentHandle {
        &self.handle
    }
}

#[derive(Clone, Debug)]
pub struct SessionListing {
    pub record: SessionRecord,
    pub state: Option<AgentSummary>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForkPosition {
    After,
    BeforeToolCalls,
}

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("daemon is shutting down")]
    ShuttingDown,

    #[error("session {session_id} was not found")]
    SessionNotFound { session_id: SessionId },

    #[error(
        "message index {message_index} in session {session_id} is not a forkable boundary: {message}"
    )]
    InvalidForkPoint {
        session_id: SessionId,
        message_index: usize,
        message: String,
    },

    #[error("Provider management is unavailable for this embedded service")]
    ProviderManagementUnavailable,

    #[error("Agent Profile management is unavailable for this embedded service")]
    AgentProfileManagementUnavailable,

    #[error(transparent)]
    Factory(#[from] AgentFactoryError),

    #[error(transparent)]
    Store(#[from] ControlStoreError),

    #[error(transparent)]
    Storage(#[from] StorageError),

    #[error(transparent)]
    ProviderStore(#[from] ProviderStoreError),

    #[error(transparent)]
    AgentProfileStore(#[from] AgentProfileStoreError),

    #[error(transparent)]
    Agent(#[from] AgentHandleError),

    #[error(transparent)]
    Registry(#[from] RegistryError),
}

fn fork_messages_at(
    messages: &[Message],
    session_id: SessionId,
    message_index: usize,
    position: ForkPosition,
) -> Result<Vec<Message>, ServiceError> {
    let invalid = |message: &str| ServiceError::InvalidForkPoint {
        session_id,
        message_index,
        message: message.to_owned(),
    };
    let assistant = messages
        .get(message_index)
        .ok_or_else(|| invalid("message index is out of range"))?;
    if assistant.role != Role::Assistant || !assistant.visibility.is_public() {
        return Err(invalid(
            "selected message is not a public assistant response",
        ));
    }

    let end = message_index
        .checked_add(1 + assistant.tool_calls.len())
        .ok_or_else(|| invalid("message index overflowed"))?;
    if end > messages.len() {
        return Err(invalid("assistant tool-call batch has missing results"));
    }
    for (offset, call) in assistant.tool_calls.iter().enumerate() {
        let result = &messages[message_index + 1 + offset];
        if result.role != Role::Tool || result.tool_call_id.as_deref() != Some(call.id.as_str()) {
            return Err(invalid(
                "assistant tool-call batch is not followed by its ordered results",
            ));
        }
    }

    match position {
        ForkPosition::After => Ok(messages[..end].to_vec()),
        ForkPosition::BeforeToolCalls => {
            if assistant.tool_calls.is_empty() {
                return Err(invalid(
                    "selected assistant response does not contain tool calls",
                ));
            }
            let mut prefix = messages[..message_index].to_vec();
            if assistant.content.as_ref().is_some_and(content_has_payload) {
                // The provider-specific replay state can still contain the
                // removed tool calls, so rebuild a normalized text-only
                // assistant message instead of mutating the original.
                prefix.push(Message::assistant(assistant.content.clone(), Vec::new()));
            }
            Ok(prefix)
        }
    }
}

fn content_has_payload(content: &Content) -> bool {
    match content {
        Content::Text(text) => !text.trim().is_empty(),
        Content::Parts(parts) => !parts.is_empty(),
    }
}

#[cfg(test)]
mod fork_tests {
    use phi::{MessageVisibility, ToolCall};
    use serde_json::json;

    use super::*;

    #[test]
    fn fork_boundary_keeps_the_complete_selected_tool_batch() {
        let messages = vec![
            Message::user("inspect"),
            Message::assistant(
                Some(Content::text("checking")),
                vec![ToolCall::new("call-1", "read", json!({}))],
            ),
            Message::tool("call-1", "result"),
            Message::assistant(Some(Content::text("done")), Vec::new()),
        ];

        let prefix = fork_messages_at(&messages, SessionId::new(), 1, ForkPosition::After).unwrap();

        assert_eq!(prefix, messages[..3]);
    }

    #[test]
    fn fork_boundary_rejects_non_public_or_incomplete_assistant_nodes() {
        let session_id = SessionId::new();
        let internal = Message::assistant(Some(Content::text("hidden")), Vec::new())
            .with_visibility(MessageVisibility::Internal);
        assert!(matches!(
            fork_messages_at(
                &[Message::user("hello"), internal],
                session_id,
                1,
                ForkPosition::After,
            ),
            Err(ServiceError::InvalidForkPoint { .. })
        ));

        let incomplete = Message::assistant(None, vec![ToolCall::new("call-1", "read", json!({}))]);
        assert!(matches!(
            fork_messages_at(
                &[Message::user("hello"), incomplete],
                session_id,
                1,
                ForkPosition::After,
            ),
            Err(ServiceError::InvalidForkPoint { .. })
        ));
    }

    #[test]
    fn fork_boundary_before_tool_calls_keeps_only_the_visible_preamble() {
        let mut assistant = Message::assistant(
            Some(Content::text("I will inspect the workspace.")),
            vec![ToolCall::new("call-1", "read", json!({}))],
        );
        assistant.reasoning = Some("private normalized reasoning".to_owned());
        assistant.provider_state = Some(phi::ProviderState::OpenAiResponses {
            output: vec![json!({ "type": "function_call" })],
        });
        let messages = vec![
            Message::user("inspect"),
            assistant,
            Message::tool("call-1", "result"),
            Message::assistant(Some(Content::text("done")), Vec::new()),
        ];

        let prefix = fork_messages_at(
            &messages,
            SessionId::new(),
            1,
            ForkPosition::BeforeToolCalls,
        )
        .unwrap();

        assert_eq!(prefix.len(), 2);
        assert_eq!(
            prefix[1].text_content(),
            Some("I will inspect the workspace.")
        );
        assert!(prefix[1].tool_calls.is_empty());
        assert_eq!(prefix[1].reasoning, None);
        assert_eq!(prefix[1].provider_state, None);
    }

    #[test]
    fn fork_boundary_before_tool_only_response_ends_at_the_prior_message() {
        let messages = vec![
            Message::user("inspect"),
            Message::assistant(None, vec![ToolCall::new("call-1", "read", json!({}))]),
            Message::tool("call-1", "result"),
        ];

        let prefix = fork_messages_at(
            &messages,
            SessionId::new(),
            1,
            ForkPosition::BeforeToolCalls,
        )
        .unwrap();

        assert_eq!(prefix, messages[..1]);
    }
}

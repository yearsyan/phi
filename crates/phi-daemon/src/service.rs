use std::{collections::HashMap, sync::Arc};

use futures_util::{StreamExt, stream::FuturesUnordered};
use phi::{
    Content, InMemoryPlanStore, InMemorySessionStorage, PlanStore, SessionStorage, SkillCatalog,
    SkillsConfig,
};
use thiserror::Error;
use tokio::sync::{Mutex, RwLock, RwLockReadGuard};

use crate::{
    runtime::{
        AgentBuildRequest, AgentFactory, AgentFactoryError, AgentHandle, AgentHandleError,
        AgentRegistry, AgentSummary, ConfiguredAgentFactory, QueuedRun, RegistryError, SessionId,
        ShutdownFailure, UnconfiguredAgentFactory, normalize_provider_config,
    },
    store::{
        ControlStore, ControlStoreError, DEFAULT_PROFILE_ID, MemoryControlStore, ProviderConfig,
        ProviderProfile, ProviderStore, ProviderStoreError, SessionRecord,
    },
};

#[derive(Clone)]
pub struct ApplicationService {
    registry: AgentRegistry,
    store: Arc<dyn ControlStore>,
    session_storage: Arc<dyn SessionStorage>,
    plan_store: Arc<dyn PlanStore>,
    factory: Arc<dyn AgentFactory>,
    provider_store: Option<Arc<dyn ProviderStore>>,
    prepared: Arc<Mutex<HashMap<SessionId, AgentHandle>>>,
    lifecycle: Arc<RwLock<LifecycleState>>,
}

#[derive(Default)]
struct LifecycleState {
    closing: bool,
}

impl ApplicationService {
    pub fn new(
        registry: AgentRegistry,
        store: Arc<dyn ControlStore>,
        session_storage: Arc<dyn SessionStorage>,
        factory: Arc<dyn AgentFactory>,
    ) -> Self {
        Self::new_with_plan_store(
            registry,
            store,
            session_storage,
            Arc::new(InMemoryPlanStore::new()),
            factory,
        )
    }

    pub fn new_with_plan_store(
        registry: AgentRegistry,
        store: Arc<dyn ControlStore>,
        session_storage: Arc<dyn SessionStorage>,
        plan_store: Arc<dyn PlanStore>,
        factory: Arc<dyn AgentFactory>,
    ) -> Self {
        Self {
            registry,
            store,
            session_storage,
            plan_store,
            factory,
            provider_store: None,
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
        Self::managed_with_plan_store(
            registry,
            store,
            session_storage,
            Arc::new(InMemoryPlanStore::new()),
            provider_store,
        )
    }

    pub fn managed_with_plan_store(
        registry: AgentRegistry,
        store: Arc<dyn ControlStore>,
        session_storage: Arc<dyn SessionStorage>,
        plan_store: Arc<dyn PlanStore>,
        provider_store: Arc<dyn ProviderStore>,
    ) -> Self {
        Self::managed_with_plan_store_and_skills(
            registry,
            store,
            session_storage,
            plan_store,
            provider_store,
            SkillsConfig::disabled(),
        )
    }

    pub fn managed_with_plan_store_and_skills(
        registry: AgentRegistry,
        store: Arc<dyn ControlStore>,
        session_storage: Arc<dyn SessionStorage>,
        plan_store: Arc<dyn PlanStore>,
        provider_store: Arc<dyn ProviderStore>,
        skills: SkillsConfig,
    ) -> Self {
        let factory: Arc<dyn AgentFactory> = Arc::new(
            ConfiguredAgentFactory::new(Arc::clone(&provider_store)).skills_config(skills),
        );
        let mut service =
            Self::new_with_plan_store(registry, store, session_storage, plan_store, factory);
        service.provider_store = Some(provider_store);
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

    /// Builds provider, tools and future MCP resources, but deliberately does
    /// not create persistent session metadata yet.
    pub async fn prepare_session(
        &self,
        profile_id: impl Into<String>,
    ) -> Result<PreparedSession, ServiceError> {
        let _lifecycle = self.enter().await?;
        let session_id = SessionId::new();
        let profile_id = profile_id.into();
        let profile_id = normalize_profile_id(&profile_id)?;
        let request = AgentBuildRequest::new(session_id, profile_id);
        let built = self.factory.build(&request).await?;
        let handle = AgentHandle::spawn_with_plan_store_and_skills(
            session_id,
            built.agent,
            built.profile_id,
            built.model,
            built.reasoning_effort,
            Arc::clone(&self.plan_store),
            built.skills,
        );
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
        let _lifecycle = self.enter().await?;
        let session_id = prepared.handle.session_id();
        let _activation_guard = self.registry.lock_session(session_id).await;
        let handle = self.activate_session_locked(prepared).await?;
        let queued = handle.enqueue_prompt(content).await?;
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

        let record = self
            .store
            .get_session(session_id)
            .await?
            .ok_or(ServiceError::SessionNotFound { session_id })?;
        let request = AgentBuildRequest::new(session_id, record.profile_id.clone())
            .with_model(record.model.clone());
        let request = match record.reasoning_effort {
            Some(reasoning_effort) => request.with_reasoning_effort(reasoning_effort),
            None => request.without_reasoning_effort(),
        };
        let built = self.factory.build(&request).await?;
        let handle = AgentHandle::spawn_with_plan_store_and_skills(
            session_id,
            built.agent,
            record.profile_id.clone(),
            record.model.clone(),
            record.reasoning_effort,
            Arc::clone(&self.plan_store),
            built.skills,
        );
        if let Err(error) = handle
            .initialize(
                record,
                Arc::clone(&self.session_storage),
                Arc::clone(&self.store),
            )
            .await
        {
            let _ = handle.shutdown().await;
            return Err(error.into());
        }
        self.registry.register(handle.clone()).await?;
        Ok(handle)
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionListing>, ServiceError> {
        let records = self.store.list_sessions().await?;
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
        let request =
            AgentBuildRequest::new(session_id, record.profile_id).with_model(record.model);
        let request = match record.reasoning_effort {
            Some(reasoning_effort) => request.with_reasoning_effort(reasoning_effort),
            None => request.without_reasoning_effort(),
        };
        Ok(self.factory.build(&request).await?.skills)
    }

    pub async fn provider_config(&self) -> Result<Option<ProviderConfig>, ServiceError> {
        self.provider_config_for(DEFAULT_PROFILE_ID).await
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

impl Default for ApplicationService {
    fn default() -> Self {
        Self::unconfigured()
    }
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

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("daemon is shutting down")]
    ShuttingDown,

    #[error("session {session_id} was not found")]
    SessionNotFound { session_id: SessionId },

    #[error("Provider management is unavailable for this embedded service")]
    ProviderManagementUnavailable,

    #[error(transparent)]
    Factory(#[from] AgentFactoryError),

    #[error(transparent)]
    Store(#[from] ControlStoreError),

    #[error(transparent)]
    ProviderStore(#[from] ProviderStoreError),

    #[error(transparent)]
    Agent(#[from] AgentHandleError),

    #[error(transparent)]
    Registry(#[from] RegistryError),
}

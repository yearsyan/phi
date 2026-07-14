use std::sync::Arc;

use crate::{
    runtime::{AgentFactory, AgentRegistry, ShutdownFailure, UnconfiguredAgentFactory},
    store::{ControlStore, MemoryControlStore},
};

/// Transport-independent application boundary shared by HTTP and WebSocket
/// handlers. Concrete operations are added here before either transport exposes
/// them.
#[derive(Clone)]
pub struct ApplicationService {
    registry: AgentRegistry,
    store: Arc<dyn ControlStore>,
    factory: Arc<dyn AgentFactory>,
}

impl ApplicationService {
    pub fn new(
        registry: AgentRegistry,
        store: Arc<dyn ControlStore>,
        factory: Arc<dyn AgentFactory>,
    ) -> Self {
        Self {
            registry,
            store,
            factory,
        }
    }

    /// Constructs a bootable shell without provider profiles or durable
    /// metadata. Public API routes remain disabled in this configuration.
    pub fn unconfigured() -> Self {
        Self::new(
            AgentRegistry::new(),
            Arc::new(MemoryControlStore::new()),
            Arc::new(UnconfiguredAgentFactory),
        )
    }

    pub fn registry(&self) -> &AgentRegistry {
        &self.registry
    }

    pub fn store(&self) -> Arc<dyn ControlStore> {
        Arc::clone(&self.store)
    }

    pub fn factory(&self) -> Arc<dyn AgentFactory> {
        Arc::clone(&self.factory)
    }

    pub async fn shutdown(&self) -> Vec<ShutdownFailure> {
        self.registry.shutdown_all().await
    }
}

impl Default for ApplicationService {
    fn default() -> Self {
        Self::unconfigured()
    }
}

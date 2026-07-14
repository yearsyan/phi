use std::{collections::HashMap, sync::Arc};

use thiserror::Error;
use tokio::sync::RwLock;

use super::{AgentHandle, AgentHandleError, SessionId};

#[derive(Clone, Default)]
pub struct AgentRegistry {
    agents: Arc<RwLock<HashMap<SessionId, AgentHandle>>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn register(&self, handle: AgentHandle) -> Result<(), RegistryError> {
        let session_id = handle.session_id();
        let mut agents = self.agents.write().await;
        if agents.contains_key(&session_id) {
            return Err(RegistryError::AlreadyRegistered { session_id });
        }
        agents.insert(session_id, handle);
        Ok(())
    }

    pub async fn get(&self, session_id: SessionId) -> Option<AgentHandle> {
        self.agents.read().await.get(&session_id).cloned()
    }

    pub async fn remove(&self, session_id: SessionId) -> Option<AgentHandle> {
        self.agents.write().await.remove(&session_id)
    }

    pub async fn len(&self) -> usize {
        self.agents.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.agents.read().await.is_empty()
    }

    pub async fn shutdown_all(&self) -> Vec<ShutdownFailure> {
        let handles = {
            let mut agents = self.agents.write().await;
            agents.drain().map(|(_, handle)| handle).collect::<Vec<_>>()
        };

        let mut failures = Vec::new();
        for handle in handles {
            if let Err(error) = handle.shutdown().await {
                failures.push(ShutdownFailure {
                    session_id: handle.session_id(),
                    error,
                });
            }
        }
        failures
    }
}

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("session {session_id} already has a running agent actor")]
    AlreadyRegistered { session_id: SessionId },
}

#[derive(Debug)]
pub struct ShutdownFailure {
    pub session_id: SessionId,
    pub error: AgentHandleError,
}

use async_trait::async_trait;
use phi::Agent;
use thiserror::Error;

use super::SessionId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentBuildRequest {
    pub session_id: SessionId,
    pub profile_id: String,
}

/// Builds a fresh in-process agent for a persisted session.
#[async_trait]
pub trait AgentFactory: Send + Sync {
    async fn build(&self, request: &AgentBuildRequest) -> Result<Agent, AgentFactoryError>;
}

/// Boot-time placeholder until profile/provider configuration is wired in.
#[derive(Clone, Debug, Default)]
pub struct UnconfiguredAgentFactory;

#[async_trait]
impl AgentFactory for UnconfiguredAgentFactory {
    async fn build(&self, request: &AgentBuildRequest) -> Result<Agent, AgentFactoryError> {
        Err(AgentFactoryError::ProfileUnavailable {
            profile_id: request.profile_id.clone(),
        })
    }
}

#[derive(Debug, Error)]
pub enum AgentFactoryError {
    #[error("agent profile {profile_id:?} is not configured")]
    ProfileUnavailable { profile_id: String },

    #[error("could not build agent: {0}")]
    Build(String),
}

mod memory;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::runtime::SessionId;

pub use memory::MemoryControlStore;

/// Metadata needed to find and rebuild a stateful agent. Conversation messages
/// remain behind `phi::SessionStorage`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: SessionId,
    pub profile_id: String,
}

#[async_trait]
pub trait ControlStore: Send + Sync {
    async fn create_session(&self, session: SessionRecord) -> Result<(), ControlStoreError>;

    async fn get_session(
        &self,
        session_id: SessionId,
    ) -> Result<Option<SessionRecord>, ControlStoreError>;

    async fn list_sessions(&self) -> Result<Vec<SessionRecord>, ControlStoreError>;

    async fn delete_session(&self, session_id: SessionId) -> Result<bool, ControlStoreError>;
}

#[derive(Debug, Error)]
pub enum ControlStoreError {
    #[error("session {session_id} already exists")]
    AlreadyExists { session_id: SessionId },

    #[error("control store failed: {0}")]
    Backend(String),
}

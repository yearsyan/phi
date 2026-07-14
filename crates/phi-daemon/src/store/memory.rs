use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::{ControlStore, ControlStoreError, SessionRecord};
use crate::runtime::SessionId;

#[derive(Clone, Default)]
pub struct MemoryControlStore {
    sessions: Arc<RwLock<HashMap<SessionId, SessionRecord>>>,
}

impl MemoryControlStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ControlStore for MemoryControlStore {
    async fn create_session(&self, session: SessionRecord) -> Result<(), ControlStoreError> {
        let mut sessions = self.sessions.write().await;
        if sessions.contains_key(&session.id) {
            return Err(ControlStoreError::AlreadyExists {
                session_id: session.id,
            });
        }
        sessions.insert(session.id, session);
        Ok(())
    }

    async fn get_session(
        &self,
        session_id: SessionId,
    ) -> Result<Option<SessionRecord>, ControlStoreError> {
        Ok(self.sessions.read().await.get(&session_id).cloned())
    }

    async fn list_sessions(&self) -> Result<Vec<SessionRecord>, ControlStoreError> {
        let mut sessions = self
            .sessions
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        sessions.sort_unstable_by_key(|session| session.id.as_uuid());
        Ok(sessions)
    }

    async fn delete_session(&self, session_id: SessionId) -> Result<bool, ControlStoreError> {
        Ok(self.sessions.write().await.remove(&session_id).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stores_session_metadata() {
        let store = MemoryControlStore::new();
        let record = SessionRecord {
            id: SessionId::new(),
            profile_id: "default".to_owned(),
        };

        store.create_session(record.clone()).await.unwrap();
        assert_eq!(
            store.get_session(record.id).await.unwrap(),
            Some(record.clone())
        );
        assert_eq!(store.list_sessions().await.unwrap(), vec![record.clone()]);
        assert!(store.delete_session(record.id).await.unwrap());
        assert_eq!(store.get_session(record.id).await.unwrap(), None);
    }
}

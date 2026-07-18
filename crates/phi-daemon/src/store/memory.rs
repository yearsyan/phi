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

    async fn update_session(&self, session: SessionRecord) -> Result<(), ControlStoreError> {
        let mut sessions = self.sessions.write().await;
        let Some(current) = sessions.get_mut(&session.id) else {
            return Err(ControlStoreError::NotFound {
                session_id: session.id,
            });
        };
        *current = session;
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
    use phi::{ReasoningEffort, Workspace};

    use super::*;

    fn record() -> SessionRecord {
        SessionRecord::new(
            SessionId::new(),
            "default",
            "model-1",
            Some(ReasoningEffort::Medium),
        )
        .with_workspace(Workspace::new("/workspace/project"))
    }

    #[tokio::test]
    async fn stores_session_metadata() {
        let store = MemoryControlStore::new();
        let record = record();

        store.create_session(record.clone()).await.unwrap();
        assert_eq!(
            store.get_session(record.id).await.unwrap(),
            Some(record.clone())
        );
        assert_eq!(store.list_sessions().await.unwrap(), vec![record.clone()]);
        assert!(store.delete_session(record.id).await.unwrap());
        assert_eq!(store.get_session(record.id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn updates_only_existing_sessions() {
        let store = MemoryControlStore::new();
        let mut record = record();

        assert!(matches!(
            store.update_session(record.clone()).await,
            Err(ControlStoreError::NotFound { session_id }) if session_id == record.id
        ));

        store.create_session(record.clone()).await.unwrap();
        record.title = Some("Storage session".to_owned());
        record.pinned = true;
        record.model = "model-2".to_owned();
        record.reasoning_effort = Some(ReasoningEffort::High);
        record.config_revision = 1;
        store.update_session(record.clone()).await.unwrap();

        assert_eq!(store.get_session(record.id).await.unwrap(), Some(record));
    }

    #[tokio::test]
    async fn rejects_duplicate_creation_without_overwriting() {
        let store = MemoryControlStore::new();
        let record = record();
        store.create_session(record.clone()).await.unwrap();

        let mut duplicate = record.clone();
        duplicate.model = "different".to_owned();
        assert!(matches!(
            store.create_session(duplicate).await,
            Err(ControlStoreError::AlreadyExists { session_id }) if session_id == record.id
        ));
        assert_eq!(store.get_session(record.id).await.unwrap(), Some(record));
    }
}

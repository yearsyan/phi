use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex},
};

use futures_util::{StreamExt, stream::FuturesUnordered};
use thiserror::Error;
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};

use super::{AgentHandle, AgentHandleError, AgentSummary, SessionId};

#[derive(Clone, Default)]
pub struct AgentRegistry {
    agents: Arc<RwLock<HashMap<SessionId, AgentHandle>>>,
    load_locks: Arc<StdMutex<HashMap<SessionId, LoadLockEntry>>>,
}

struct LoadLockEntry {
    lock: Arc<Mutex<()>>,
    users: usize,
}

/// Owns one keyed lock request. The lease count includes both acquired guards
/// and tasks waiting for the mutex, so an entry is removed only after the last
/// user completes or cancels.
struct LoadLockLease {
    session_id: SessionId,
    lock: Arc<Mutex<()>>,
    load_locks: Arc<StdMutex<HashMap<SessionId, LoadLockEntry>>>,
}

impl Drop for LoadLockLease {
    fn drop(&mut self) {
        let mut locks = self
            .load_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let remove = locks.get_mut(&self.session_id).is_some_and(|entry| {
            if !Arc::ptr_eq(&entry.lock, &self.lock) {
                return false;
            }
            entry.users = entry
                .users
                .checked_sub(1)
                .expect("session load-lock lease count underflowed");
            entry.users == 0
        });
        if remove {
            locks.remove(&self.session_id);
        }
    }
}

pub(crate) struct SessionLoadGuard {
    guard: Option<OwnedMutexGuard<()>>,
    lease: Option<LoadLockLease>,
}

impl Drop for SessionLoadGuard {
    fn drop(&mut self) {
        // Release the per-session mutex before making the entry eligible for
        // removal. A racing request either joins this entry or creates a new
        // one after the old critical section has ended.
        self.guard.take();
        self.lease.take();
    }
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

    pub async fn summaries(&self) -> HashMap<SessionId, AgentSummary> {
        self.agents
            .read()
            .await
            .iter()
            .map(|(session_id, handle)| (*session_id, handle.summary()))
            .collect()
    }

    /// Serializes lazy construction for one session without holding the global
    /// registry lock while provider and MCP resources are initialized.
    pub(crate) async fn lock_session(&self, session_id: SessionId) -> SessionLoadGuard {
        let (lock, lease) = {
            let mut locks = self
                .load_locks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry = locks.entry(session_id).or_insert_with(|| LoadLockEntry {
                lock: Arc::new(Mutex::new(())),
                users: 0,
            });
            entry.users = entry
                .users
                .checked_add(1)
                .expect("session load-lock lease count overflowed");
            let lock = Arc::clone(&entry.lock);
            let lease = LoadLockLease {
                session_id,
                lock: Arc::clone(&lock),
                load_locks: Arc::clone(&self.load_locks),
            };
            (lock, lease)
        };
        let guard = lock.lock_owned().await;
        SessionLoadGuard {
            guard: Some(guard),
            lease: Some(lease),
        }
    }

    pub async fn shutdown_all(&self) -> Vec<ShutdownFailure> {
        let handles = {
            let mut agents = self.agents.write().await;
            agents.drain().map(|(_, handle)| handle).collect::<Vec<_>>()
        };
        self.load_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();

        let mut pending = FuturesUnordered::new();
        for handle in handles {
            pending.push(async move {
                let session_id = handle.session_id();
                (session_id, handle.shutdown().await)
            });
        }

        let mut failures = Vec::new();
        while let Some((session_id, result)) = pending.next().await {
            if let Err(error) = result {
                failures.push(ShutdownFailure { session_id, error });
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

#[cfg(test)]
mod tests {
    use tokio::sync::{Notify, oneshot};

    use super::*;

    fn lock_state(registry: &AgentRegistry, session_id: SessionId) -> (usize, usize) {
        let locks = registry
            .load_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (
            locks.len(),
            locks.get(&session_id).map_or(0, |entry| entry.users),
        )
    }

    async fn wait_for_users(registry: &AgentRegistry, session_id: SessionId, users: usize) {
        for _ in 0..100 {
            if lock_state(registry, session_id).1 == users {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("session load lock did not reach {users} users");
    }

    #[tokio::test]
    async fn keyed_lock_serializes_waiters_and_reclaims_after_last_user() {
        let registry = AgentRegistry::new();
        let session_id = SessionId::new();
        let first = registry.lock_session(session_id).await;
        assert_eq!(lock_state(&registry, session_id), (1, 1));

        let waiting_registry = registry.clone();
        let release = Arc::new(Notify::new());
        let waiting_release = Arc::clone(&release);
        let (acquired, acquired_rx) = oneshot::channel();
        let waiter = tokio::spawn(async move {
            let _guard = waiting_registry.lock_session(session_id).await;
            let _ = acquired.send(());
            waiting_release.notified().await;
        });

        wait_for_users(&registry, session_id, 2).await;
        assert!(!waiter.is_finished(), "the waiter bypassed the keyed lock");
        drop(first);
        acquired_rx.await.unwrap();
        assert_eq!(lock_state(&registry, session_id), (1, 1));

        release.notify_one();
        waiter.await.unwrap();
        assert_eq!(lock_state(&registry, session_id), (0, 0));
    }

    #[tokio::test]
    async fn cancelled_waiter_releases_its_lease_and_does_not_pin_the_entry() {
        let registry = AgentRegistry::new();
        let session_id = SessionId::new();
        let first = registry.lock_session(session_id).await;

        let waiting_registry = registry.clone();
        let waiter = tokio::spawn(async move { waiting_registry.lock_session(session_id).await });
        wait_for_users(&registry, session_id, 2).await;
        waiter.abort();
        match waiter.await {
            Err(error) => assert!(error.is_cancelled()),
            Ok(_) => panic!("the aborted waiter unexpectedly acquired the lock"),
        }
        assert_eq!(lock_state(&registry, session_id), (1, 1));

        drop(first);
        assert_eq!(lock_state(&registry, session_id), (0, 0));
    }
}

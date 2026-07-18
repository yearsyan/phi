use std::{
    collections::{HashMap, HashSet},
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    fs,
    io::{AsyncWriteExt, BufWriter},
    sync::{Mutex, RwLock},
};

use crate::scheduled_task::{
    MAX_SCHEDULED_TASKS, ScheduledTask, ScheduledTaskId, validate_persisted_task,
};

const COLLECTION_VERSION: u32 = 1;

#[async_trait]
pub trait ScheduledTaskStore: Send + Sync {
    async fn create_task(&self, task: ScheduledTask) -> Result<(), ScheduledTaskStoreError>;

    async fn update_task(&self, task: ScheduledTask) -> Result<(), ScheduledTaskStoreError>;

    async fn get_task(
        &self,
        task_id: ScheduledTaskId,
    ) -> Result<Option<ScheduledTask>, ScheduledTaskStoreError>;

    async fn list_tasks(&self) -> Result<Vec<ScheduledTask>, ScheduledTaskStoreError>;

    async fn delete_task(&self, task_id: ScheduledTaskId) -> Result<bool, ScheduledTaskStoreError>;
}

#[derive(Clone, Default)]
pub struct MemoryScheduledTaskStore {
    tasks: Arc<RwLock<HashMap<ScheduledTaskId, ScheduledTask>>>,
}

impl MemoryScheduledTaskStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ScheduledTaskStore for MemoryScheduledTaskStore {
    async fn create_task(&self, task: ScheduledTask) -> Result<(), ScheduledTaskStoreError> {
        validate_for_store(&task)?;
        let mut tasks = self.tasks.write().await;
        if tasks.contains_key(&task.id) {
            return Err(ScheduledTaskStoreError::AlreadyExists { task_id: task.id });
        }
        tasks.insert(task.id, task);
        Ok(())
    }

    async fn update_task(&self, task: ScheduledTask) -> Result<(), ScheduledTaskStoreError> {
        validate_for_store(&task)?;
        let mut tasks = self.tasks.write().await;
        let Some(current) = tasks.get_mut(&task.id) else {
            return Err(ScheduledTaskStoreError::NotFound { task_id: task.id });
        };
        *current = task;
        Ok(())
    }

    async fn get_task(
        &self,
        task_id: ScheduledTaskId,
    ) -> Result<Option<ScheduledTask>, ScheduledTaskStoreError> {
        Ok(self.tasks.read().await.get(&task_id).cloned())
    }

    async fn list_tasks(&self) -> Result<Vec<ScheduledTask>, ScheduledTaskStoreError> {
        let mut tasks = self
            .tasks
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        tasks.sort_unstable_by_key(|task| task.id);
        Ok(tasks)
    }

    async fn delete_task(&self, task_id: ScheduledTaskId) -> Result<bool, ScheduledTaskStoreError> {
        Ok(self.tasks.write().await.remove(&task_id).is_some())
    }
}

#[derive(Clone, Debug)]
pub struct DiskScheduledTaskStore {
    path: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl DiskScheduledTaskStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    async fn read_unlocked(&self) -> Result<Vec<ScheduledTask>, ScheduledTaskStoreError> {
        let bytes = match fs::read(&self.path).await {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(ScheduledTaskStoreError::Io {
                    path: self.path.clone(),
                    source,
                });
            }
        };
        let collection =
            serde_json::from_slice::<ScheduledTaskCollection>(&bytes).map_err(|source| {
                ScheduledTaskStoreError::Serialization {
                    path: self.path.clone(),
                    source,
                }
            })?;
        validate_collection(&self.path, &collection)?;
        Ok(collection.tasks)
    }

    async fn write_unlocked(&self, tasks: &[ScheduledTask]) -> Result<(), ScheduledTaskStoreError> {
        let mut tasks = tasks.to_vec();
        tasks.sort_unstable_by_key(|task| task.id);
        let collection = ScheduledTaskCollection {
            version: COLLECTION_VERSION,
            tasks,
        };
        validate_collection(&self.path, &collection)?;

        let parent = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .await
            .map_err(|source| ScheduledTaskStoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        let mut bytes = serde_json::to_vec_pretty(&collection).map_err(|source| {
            ScheduledTaskStoreError::Serialization {
                path: self.path.clone(),
                source,
            }
        })?;
        bytes.push(b'\n');

        let temporary = parent.join(format!(".scheduled-tasks-{}.tmp", ScheduledTaskId::new()));
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file =
            options
                .open(&temporary)
                .await
                .map_err(|source| ScheduledTaskStoreError::Io {
                    path: temporary.clone(),
                    source,
                })?;
        if let Err(source) = write_and_sync(file, &bytes).await {
            let _ = fs::remove_file(&temporary).await;
            return Err(ScheduledTaskStoreError::Io {
                path: temporary,
                source,
            });
        }
        if let Err(source) = fs::rename(&temporary, &self.path).await {
            let _ = fs::remove_file(&temporary).await;
            return Err(ScheduledTaskStoreError::Io {
                path: self.path.clone(),
                source,
            });
        }
        if let Err(source) = sync_directory(parent).await {
            tracing::warn!(
                path = %parent.display(),
                error = %source,
                "scheduled-task configuration is visible, but its directory sync failed"
            );
        }
        Ok(())
    }
}

#[async_trait]
impl ScheduledTaskStore for DiskScheduledTaskStore {
    async fn create_task(&self, task: ScheduledTask) -> Result<(), ScheduledTaskStoreError> {
        validate_for_store(&task)?;
        let _guard = self.lock.lock().await;
        let mut tasks = self.read_unlocked().await?;
        if tasks.iter().any(|current| current.id == task.id) {
            return Err(ScheduledTaskStoreError::AlreadyExists { task_id: task.id });
        }
        tasks.push(task);
        self.write_unlocked(&tasks).await
    }

    async fn update_task(&self, task: ScheduledTask) -> Result<(), ScheduledTaskStoreError> {
        validate_for_store(&task)?;
        let _guard = self.lock.lock().await;
        let mut tasks = self.read_unlocked().await?;
        let Some(current) = tasks.iter_mut().find(|current| current.id == task.id) else {
            return Err(ScheduledTaskStoreError::NotFound { task_id: task.id });
        };
        *current = task;
        self.write_unlocked(&tasks).await
    }

    async fn get_task(
        &self,
        task_id: ScheduledTaskId,
    ) -> Result<Option<ScheduledTask>, ScheduledTaskStoreError> {
        let _guard = self.lock.lock().await;
        Ok(self
            .read_unlocked()
            .await?
            .into_iter()
            .find(|task| task.id == task_id))
    }

    async fn list_tasks(&self) -> Result<Vec<ScheduledTask>, ScheduledTaskStoreError> {
        let _guard = self.lock.lock().await;
        let mut tasks = self.read_unlocked().await?;
        tasks.sort_unstable_by_key(|task| task.id);
        Ok(tasks)
    }

    async fn delete_task(&self, task_id: ScheduledTaskId) -> Result<bool, ScheduledTaskStoreError> {
        let _guard = self.lock.lock().await;
        let mut tasks = self.read_unlocked().await?;
        let original_len = tasks.len();
        tasks.retain(|task| task.id != task_id);
        if tasks.len() == original_len {
            return Ok(false);
        }
        self.write_unlocked(&tasks).await?;
        Ok(true)
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScheduledTaskCollection {
    version: u32,
    tasks: Vec<ScheduledTask>,
}

fn validate_collection(
    path: &Path,
    collection: &ScheduledTaskCollection,
) -> Result<(), ScheduledTaskStoreError> {
    if collection.version != COLLECTION_VERSION {
        return Err(ScheduledTaskStoreError::InvalidCollection {
            path: path.to_path_buf(),
            message: format!(
                "unsupported version {}; expected {COLLECTION_VERSION}",
                collection.version
            ),
        });
    }
    if collection.tasks.len() > MAX_SCHEDULED_TASKS {
        return Err(ScheduledTaskStoreError::InvalidCollection {
            path: path.to_path_buf(),
            message: format!(
                "task count {} exceeds the supported capacity {MAX_SCHEDULED_TASKS}",
                collection.tasks.len()
            ),
        });
    }
    let mut ids = HashSet::with_capacity(collection.tasks.len());
    for task in &collection.tasks {
        validate_persisted_task(task).map_err(|message| {
            ScheduledTaskStoreError::InvalidCollection {
                path: path.to_path_buf(),
                message: format!("invalid task {}: {message}", task.id),
            }
        })?;
        if !ids.insert(task.id) {
            return Err(ScheduledTaskStoreError::InvalidCollection {
                path: path.to_path_buf(),
                message: format!("duplicate task ID {}", task.id),
            });
        }
    }
    Ok(())
}

fn validate_for_store(task: &ScheduledTask) -> Result<(), ScheduledTaskStoreError> {
    validate_persisted_task(task).map_err(|message| ScheduledTaskStoreError::InvalidTask {
        task_id: task.id,
        message,
    })
}

async fn write_and_sync(file: fs::File, bytes: &[u8]) -> Result<(), io::Error> {
    let mut writer = BufWriter::new(file);
    writer.write_all(bytes).await?;
    writer.flush().await?;
    writer.get_ref().sync_all().await
}

async fn sync_directory(path: &Path) -> Result<(), io::Error> {
    #[cfg(unix)]
    {
        fs::File::open(path).await?.sync_all().await
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum ScheduledTaskStoreError {
    #[error("scheduled task {task_id} already exists")]
    AlreadyExists { task_id: ScheduledTaskId },

    #[error("scheduled task {task_id} does not exist")]
    NotFound { task_id: ScheduledTaskId },

    #[error("invalid scheduled task {task_id}: {message}")]
    InvalidTask {
        task_id: ScheduledTaskId,
        message: String,
    },

    #[error("scheduled-task store I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("invalid scheduled-task JSON at {path}: {source}")]
    Serialization {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("invalid scheduled-task collection at {path}: {message}")]
    InvalidCollection { path: PathBuf, message: String },

    #[error("scheduled-task store failed: {0}")]
    Backend(String),
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use phi::Workspace;

    use super::*;
    use crate::scheduled_task::{ScheduledIntervalUnit, ScheduledTaskRun, ScheduledTaskSchedule};

    fn task() -> ScheduledTask {
        let now = Utc.with_ymd_and_hms(2026, 7, 17, 1, 0, 0).unwrap();
        ScheduledTask {
            id: ScheduledTaskId::new(),
            name: "Morning review".to_owned(),
            prompt: "Review the latest changes".to_owned(),
            workspace: Workspace::new("/workspace/phi"),
            profile_id: "default".to_owned(),
            agent_profile_id: "default".to_owned(),
            capability_mode: None,
            schedule: ScheduledTaskSchedule::Interval {
                every: 1,
                unit: ScheduledIntervalUnit::Hours,
            },
            enabled: true,
            created_at: now,
            updated_at: now,
            next_run_at: Some(now + chrono::Duration::hours(1)),
            last_run: None::<ScheduledTaskRun>,
            skipped_runs: 0,
            revision: 1,
        }
    }

    #[tokio::test]
    async fn memory_store_round_trips_and_requires_existing_updates() {
        let store = MemoryScheduledTaskStore::new();
        let mut task = task();
        assert!(matches!(
            store.update_task(task.clone()).await,
            Err(ScheduledTaskStoreError::NotFound { task_id }) if task_id == task.id
        ));

        store.create_task(task.clone()).await.unwrap();
        task.enabled = false;
        task.next_run_at = None;
        task.revision = 2;
        store.update_task(task.clone()).await.unwrap();
        assert_eq!(store.get_task(task.id).await.unwrap(), Some(task.clone()));
        assert!(store.delete_task(task.id).await.unwrap());
        assert!(!store.delete_task(task.id).await.unwrap());
    }

    #[tokio::test]
    async fn store_rejects_impossible_run_timestamps() {
        let store = MemoryScheduledTaskStore::new();
        let mut task = task();
        let started_at = task.created_at + chrono::Duration::minutes(1);
        task.last_run = Some(ScheduledTaskRun {
            scheduled_for: started_at + chrono::Duration::minutes(1),
            started_at,
            finished_at: None,
            outcome: crate::scheduled_task::ScheduledRunOutcome::Running,
            session_id: None,
            error: None,
        });

        assert!(matches!(
            store.create_task(task).await,
            Err(ScheduledTaskStoreError::InvalidTask { .. })
        ));
    }

    #[tokio::test]
    async fn disk_store_uses_private_atomic_collection_file() {
        let root = std::env::temp_dir().join(format!(
            "phi-daemon-scheduled-task-store-{}",
            ScheduledTaskId::new()
        ));
        let path = root.join("scheduled-tasks.json");
        let store = DiskScheduledTaskStore::new(&path);
        let mut first = task();
        let second = task();

        store.create_task(second.clone()).await.unwrap();
        store.create_task(first.clone()).await.unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(&path).await.unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        first.enabled = false;
        first.next_run_at = None;
        first.revision = 2;
        store.update_task(first.clone()).await.unwrap();

        let mut expected = vec![first.clone(), second];
        expected.sort_unstable_by_key(|task| task.id);
        assert_eq!(store.list_tasks().await.unwrap(), expected);
        assert_eq!(
            DiskScheduledTaskStore::new(&path)
                .get_task(first.id)
                .await
                .unwrap(),
            Some(first)
        );

        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn disk_store_rejects_unknown_collection_versions() {
        let root = std::env::temp_dir().join(format!(
            "phi-daemon-scheduled-task-version-{}",
            ScheduledTaskId::new()
        ));
        let path = root.join("scheduled-tasks.json");
        fs::create_dir_all(&root).await.unwrap();
        fs::write(&path, br#"{"version":2,"tasks":[]}"#)
            .await
            .unwrap();
        let store = DiskScheduledTaskStore::new(&path);

        assert!(matches!(
            store.list_tasks().await,
            Err(ScheduledTaskStoreError::InvalidCollection { .. })
        ));

        fs::remove_dir_all(root).await.unwrap();
    }
}

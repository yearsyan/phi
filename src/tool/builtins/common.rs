use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use crate::error::ToolError;

pub(super) fn normalize_cwd(cwd: impl Into<PathBuf>) -> PathBuf {
    let cwd = cwd.into();
    if cwd.is_absolute() {
        cwd
    } else {
        env::current_dir().map_or(cwd.clone(), |current| current.join(cwd))
    }
}

pub(super) fn resolve_path(cwd: &Path, raw_path: &str) -> Result<PathBuf, ToolError> {
    let raw_path = raw_path.strip_prefix('@').unwrap_or(raw_path);
    if raw_path.trim().is_empty() {
        return Err(ToolError::new("path must not be empty"));
    }

    let expanded = if raw_path == "~" {
        home_dir().ok_or_else(|| ToolError::new("cannot expand ~: home directory is unknown"))?
    } else if let Some(rest) = raw_path
        .strip_prefix("~/")
        .or_else(|| raw_path.strip_prefix("~\\"))
    {
        home_dir()
            .ok_or_else(|| ToolError::new("cannot expand ~: home directory is unknown"))?
            .join(rest)
    } else {
        PathBuf::from(raw_path)
    };

    Ok(if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    })
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

type FileLockMap = Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>>;

fn file_locks() -> &'static FileLockMap {
    static FILE_LOCKS: OnceLock<FileLockMap> = OnceLock::new();
    FILE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) async fn mutation_guard(path: &Path) -> OwnedMutexGuard<()> {
    let key = tokio::fs::canonicalize(path)
        .await
        .unwrap_or_else(|_| path.to_path_buf());
    let lock = {
        let mut locks = file_locks().lock().expect("file mutation lock poisoned");
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
            lock
        } else {
            let lock = Arc::new(AsyncMutex::new(()));
            locks.insert(key, Arc::downgrade(&lock));
            lock
        }
    };
    lock.lock_owned().await
}

pub(super) fn invalid_arguments(tool: &str, error: serde_json::Error) -> ToolError {
    ToolError::new(format!("invalid {tool} arguments: {error}"))
}

pub(super) fn io_error(action: &str, path: &Path, error: std::io::Error) -> ToolError {
    ToolError::new(format!("{action} {}: {error}", path.display()))
}

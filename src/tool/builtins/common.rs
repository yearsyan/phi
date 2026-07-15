use std::{
    collections::HashMap,
    env,
    path::{Component, Path, PathBuf},
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
    let key = canonical_mutation_key(path).await;
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

/// Produces a stable lock key even when the target and some of its parents do
/// not exist yet. Canonicalizing the deepest existing ancestor resolves
/// symlinks, while lexical normalization of the unresolved suffix folds `.`
/// and `..` aliases before the file is created.
async fn canonical_mutation_key(path: &Path) -> PathBuf {
    for ancestor in path.ancestors() {
        if let Ok(canonical) = tokio::fs::canonicalize(ancestor).await {
            let suffix = path
                .strip_prefix(ancestor)
                .unwrap_or_else(|_| Path::new(""));
            return lexical_normalize(&canonical.join(suffix));
        }
    }
    lexical_normalize(path)
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() && !path.is_absolute() {
                    normalized.push(component.as_os_str());
                }
            }
        }
    }
    normalized
}

pub(super) fn invalid_arguments(tool: &str, error: serde_json::Error) -> ToolError {
    ToolError::new(format!("invalid {tool} arguments: {error}"))
}

pub(super) fn io_error(action: &str, path: &Path, error: std::io::Error) -> ToolError {
    ToolError::new(format!("{action} {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn leading_at_is_a_literal_file_name() {
        let cwd = Path::new("/tmp/work");
        assert_eq!(
            resolve_path(cwd, "@notes.txt").unwrap(),
            cwd.join("@notes.txt")
        );
    }

    #[tokio::test]
    async fn missing_path_aliases_share_a_mutation_key() {
        let directory = tempdir().unwrap();
        let direct = directory.path().join("file.txt");
        let aliased = directory.path().join("missing/../file.txt");

        assert_eq!(
            canonical_mutation_key(&direct).await,
            canonical_mutation_key(&aliased).await
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_parent_aliases_share_a_mutation_key() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let real = directory.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = directory.path().join("link");
        symlink(&real, &link).unwrap();

        assert_eq!(
            canonical_mutation_key(&real.join("new/file.txt")).await,
            canonical_mutation_key(&link.join("new/file.txt")).await
        );
    }
}

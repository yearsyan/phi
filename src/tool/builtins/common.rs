use std::{
    collections::HashMap,
    env,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use crate::{
    Workspace,
    error::ToolError,
    tool::{CapabilityMode, ToolExecutionContext},
};

pub(super) fn normalize_cwd(cwd: impl Into<PathBuf>) -> PathBuf {
    Workspace::new(cwd).root().to_owned()
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

pub(super) async fn resolve_path_for_context(
    cwd: &Path,
    raw_path: &str,
    context: Option<&ToolExecutionContext>,
) -> Result<PathBuf, ToolError> {
    let path = resolve_path(cwd, raw_path)?;
    let Some(context) = context else {
        return Ok(path);
    };
    if context.capability_mode() == CapabilityMode::FullAccess {
        return Ok(path);
    }

    let workspace = context.workspace().ok_or_else(|| {
        ToolError::new("restricted capability mode requires a configured workspace")
    })?;
    let canonical_workspace = tokio::fs::canonicalize(workspace.root())
        .await
        .map_err(|error| io_error("could not canonicalize workspace", workspace.root(), error))?;
    let canonical_target = canonical_mutation_key(&path).await;
    if !canonical_target.starts_with(&canonical_workspace) {
        return Err(ToolError::new(format!(
            "path {} is outside the configured workspace {}",
            path.display(),
            workspace.root().display()
        )));
    }
    Ok(path)
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
    use crate::tool::{CapabilityMode, ToolExecutionContext};

    fn restricted_context(
        workspace: &Path,
        capability_mode: CapabilityMode,
    ) -> ToolExecutionContext {
        ToolExecutionContext::detached("test")
            .with_workspace_policy(Some(Workspace::new(workspace)), capability_mode)
    }

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

    #[tokio::test]
    async fn restricted_capabilities_confine_paths_to_the_workspace() {
        let parent = tempdir().unwrap();
        let workspace = parent.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let outside = parent.path().join("outside.txt");
        let context = restricted_context(&workspace, CapabilityMode::WorkspaceEdit);

        assert!(
            resolve_path_for_context(&workspace, outside.to_str().unwrap(), Some(&context))
                .await
                .unwrap_err()
                .to_string()
                .contains("outside the configured workspace")
        );
        assert!(
            resolve_path_for_context(&workspace, "../outside.txt", Some(&context))
                .await
                .is_err()
        );
        assert_eq!(
            resolve_path_for_context(&workspace, "nested/file.txt", Some(&context))
                .await
                .unwrap(),
            workspace.join("nested/file.txt")
        );
    }

    #[tokio::test]
    async fn full_access_preserves_absolute_path_compatibility() {
        let parent = tempdir().unwrap();
        let workspace = parent.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let outside = parent.path().join("outside.txt");
        let context = restricted_context(&workspace, CapabilityMode::FullAccess);

        assert_eq!(
            resolve_path_for_context(&workspace, outside.to_str().unwrap(), Some(&context))
                .await
                .unwrap(),
            outside
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn restricted_capabilities_reject_a_symlinked_parent_escape() {
        use std::os::unix::fs::symlink;

        let parent = tempdir().unwrap();
        let workspace = parent.path().join("workspace");
        let outside = parent.path().join("outside");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&outside).unwrap();
        symlink(&outside, workspace.join("link")).unwrap();
        let context = restricted_context(&workspace, CapabilityMode::WorkspaceEdit);

        assert!(
            resolve_path_for_context(&workspace, "link/new.txt", Some(&context))
                .await
                .is_err()
        );
    }
}

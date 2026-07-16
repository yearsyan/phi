use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use phi::{
    SubagentFactoryError, SubagentResource, SubagentResourceDisposition,
    SubagentResourceFinalization, SubagentResourceInfo, Workspace,
};
use tokio::{
    fs,
    process::Command,
    sync::{Mutex, oneshot},
};

#[derive(Clone, Debug)]
pub struct WorktreeManager {
    root: PathBuf,
    #[cfg(test)]
    pre_add_hook: Option<CreatePauseHook>,
    #[cfg(test)]
    post_create_hook: Option<CreatePauseHook>,
}

impl WorktreeManager {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            #[cfg(test)]
            pre_add_hook: None,
            #[cfg(test)]
            post_create_hook: None,
        }
    }

    /// Runs creation in an owned task so cancellation of the requesting build
    /// cannot abort `git worktree add` halfway through. The caller acknowledges
    /// receipt synchronously before this future returns; a dropped receiver
    /// causes the owner task to finalize any created worktree.
    pub async fn create(
        &self,
        parent_workspace: &Workspace,
        parent_id: &str,
        agent_id: &str,
    ) -> Result<PreparedWorktree, SubagentFactoryError> {
        let manager = self.clone();
        let parent_workspace = parent_workspace.clone();
        let parent_id = parent_id.to_owned();
        let agent_id = agent_id.to_owned();
        let (result_sender, result_receiver) = oneshot::channel();
        let (accepted_sender, accepted_receiver) = oneshot::channel();
        std::mem::drop(tokio::spawn(async move {
            let result = manager
                .create_owned(&parent_workspace, &parent_id, &agent_id)
                .await;
            let cleanup_resource = result
                .as_ref()
                .ok()
                .map(|prepared| Arc::clone(&prepared.resource));
            match result_sender.send(result) {
                Ok(()) => {
                    if accepted_receiver.await.is_err()
                        && let Some(resource) = cleanup_resource
                    {
                        finalize_abandoned_resource(resource).await;
                    }
                }
                Err(Ok(prepared)) => {
                    if let Err(error) = prepared.finalize_unadopted().await {
                        tracing::error!(
                            error = %error,
                            "failed to finalize a subagent worktree whose creation receiver was dropped"
                        );
                    }
                }
                Err(Err(_)) => {}
            }
        }));

        let result = result_receiver.await.map_err(|_| {
            SubagentFactoryError::new("subagent worktree creation task stopped unexpectedly")
        })?;
        let _ = accepted_sender.send(());
        result
    }

    async fn create_owned(
        &self,
        parent_workspace: &Workspace,
        parent_id: &str,
        agent_id: &str,
    ) -> Result<PreparedWorktree, SubagentFactoryError> {
        validate_component(parent_id, "parent session ID")?;
        validate_component(agent_id, "subagent ID")?;

        let parent_root = fs::canonicalize(parent_workspace.root())
            .await
            .map_err(|error| {
                SubagentFactoryError::new(format!(
                    "could not resolve parent workspace {:?}: {error}",
                    parent_workspace.root()
                ))
            })?;
        let repository_root = git_stdout(&parent_root, ["rev-parse", "--show-toplevel"]).await?;
        let repository_root = fs::canonicalize(repository_root.trim())
            .await
            .map_err(|error| {
                SubagentFactoryError::new(format!(
                    "could not resolve Git repository root for {:?}: {error}",
                    parent_workspace.root()
                ))
            })?;
        let relative_workspace = parent_root
            .strip_prefix(&repository_root)
            .map_err(|_| {
                SubagentFactoryError::new(format!(
                    "workspace {:?} is not contained by Git repository {:?}",
                    parent_root, repository_root
                ))
            })?
            .to_path_buf();
        let base_revision = git_stdout(&repository_root, ["rev-parse", "HEAD"]).await?;
        let base_revision = base_revision.trim().to_owned();

        let parent_directory = self.root.join(parent_id);
        fs::create_dir_all(&parent_directory)
            .await
            .map_err(|error| {
                SubagentFactoryError::new(format!(
                    "could not create worktree directory {:?}: {error}",
                    parent_directory
                ))
            })?;
        set_owner_only_directory(&self.root).await?;
        set_owner_only_directory(&parent_directory).await?;

        let worktree_path = parent_directory.join(agent_id);
        if fs::try_exists(&worktree_path).await.map_err(|error| {
            SubagentFactoryError::new(format!(
                "could not inspect worktree path {:?}: {error}",
                worktree_path
            ))
        })? {
            return Err(SubagentFactoryError::new(format!(
                "worktree path {:?} already exists",
                worktree_path
            )));
        }

        let mut created = CreatedWorktreeGuard::new(repository_root.clone(), worktree_path.clone());
        #[cfg(test)]
        if let Some(hook) = &self.pre_add_hook {
            hook.reached.notify_one();
            hook.release.notified().await;
        }
        if let Err(error) = run_git(
            &repository_root,
            [
                "worktree",
                "add",
                "--detach",
                path_argument(&worktree_path)?,
                base_revision.as_str(),
            ],
        )
        .await
        {
            if let Err(cleanup_error) = created.cleanup_now().await {
                tracing::warn!(
                    worktree_path = %worktree_path.display(),
                    error = %cleanup_error,
                    "failed to clean up after git worktree add failed"
                );
            }
            return Err(error);
        }

        #[cfg(test)]
        if let Some(hook) = &self.post_create_hook {
            hook.reached.notify_one();
            hook.release.notified().await;
        }

        let child_workspace_path = worktree_path.join(relative_workspace);
        if let Err(error) = fs::metadata(&child_workspace_path).await {
            if let Err(cleanup_error) = created.cleanup_now().await {
                tracing::warn!(
                    worktree_path = %worktree_path.display(),
                    error = %cleanup_error,
                    "failed to clean up an invalid newly-created subagent worktree"
                );
            }
            return Err(SubagentFactoryError::new(format!(
                "created worktree does not contain child workspace {:?}: {error}",
                child_workspace_path
            )));
        }

        let resource = Arc::new(WorktreeResource {
            repository_root,
            worktree_path: worktree_path.clone(),
            base_revision,
            finalization: Mutex::new(None),
        });
        created.disarm();
        Ok(PreparedWorktree {
            workspace: Workspace::new(child_workspace_path),
            resource,
            cleanup_on_drop: true,
        })
    }

    #[cfg(test)]
    fn with_pre_add_hook(mut self, hook: CreatePauseHook) -> Self {
        self.pre_add_hook = Some(hook);
        self
    }

    #[cfg(test)]
    fn with_post_create_hook(mut self, hook: CreatePauseHook) -> Self {
        self.post_create_hook = Some(hook);
        self
    }
}

#[derive(Debug)]
pub struct PreparedWorktree {
    workspace: Workspace,
    resource: Arc<WorktreeResource>,
    cleanup_on_drop: bool,
}

impl PreparedWorktree {
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn adopt_resource(mut self) -> Arc<dyn SubagentResource> {
        self.cleanup_on_drop = false;
        let resource: Arc<dyn SubagentResource> = self.resource.clone();
        resource
    }

    pub async fn finalize_unadopted(
        mut self,
    ) -> Result<SubagentResourceFinalization, SubagentFactoryError> {
        let result = self
            .resource
            .finalize(SubagentResourceDisposition::RuntimeFailed)
            .await;
        if result.is_ok() {
            self.cleanup_on_drop = false;
        }
        result
    }
}

impl Drop for PreparedWorktree {
    fn drop(&mut self) {
        if self.cleanup_on_drop {
            schedule_resource_cleanup(Arc::clone(&self.resource));
        }
    }
}

#[derive(Debug)]
struct CreatedWorktreeGuard {
    repository_root: PathBuf,
    worktree_path: PathBuf,
    armed: bool,
}

impl CreatedWorktreeGuard {
    fn new(repository_root: PathBuf, worktree_path: PathBuf) -> Self {
        Self {
            repository_root,
            worktree_path,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }

    async fn cleanup_now(mut self) -> Result<(), SubagentFactoryError> {
        let result = cleanup_created_worktree(&self.repository_root, &self.worktree_path).await;
        if result.is_ok() {
            self.armed = false;
        }
        result
    }
}

impl Drop for CreatedWorktreeGuard {
    fn drop(&mut self) {
        if self.armed {
            schedule_created_worktree_cleanup(
                self.repository_root.clone(),
                self.worktree_path.clone(),
            );
        }
    }
}

fn schedule_created_worktree_cleanup(repository_root: PathBuf, worktree_path: PathBuf) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::error!(
            worktree_path = %worktree_path.display(),
            "could not schedule cancelled worktree creation cleanup because no Tokio runtime is active; preserving the worktree"
        );
        return;
    };
    std::mem::drop(handle.spawn(async move {
        if let Err(error) = cleanup_created_worktree(&repository_root, &worktree_path).await {
            tracing::error!(
                worktree_path = %worktree_path.display(),
                error = %error,
                "failed to clean up a cancelled subagent worktree creation"
            );
        }
    }));
}

fn schedule_resource_cleanup(resource: Arc<WorktreeResource>) {
    let location = resource.worktree_path.clone();
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::error!(
            worktree_path = %location.display(),
            "could not schedule unadopted worktree cleanup because no Tokio runtime is active; preserving the worktree"
        );
        return;
    };
    std::mem::drop(handle.spawn(finalize_abandoned_resource(resource)));
}

async fn finalize_abandoned_resource(resource: Arc<WorktreeResource>) {
    let location = resource.worktree_path.clone();
    match resource
        .finalize(SubagentResourceDisposition::RuntimeFailed)
        .await
    {
        Ok(finalization) if finalization.preserved => {
            tracing::warn!(
                worktree_path = %location.display(),
                "unadopted subagent worktree was dirty and has been preserved"
            );
        }
        Ok(_) => {}
        Err(error) => {
            tracing::error!(
                worktree_path = %location.display(),
                error = %error,
                "failed to finalize an unadopted subagent worktree"
            );
        }
    }
}

async fn cleanup_created_worktree(
    repository_root: &Path,
    worktree_path: &Path,
) -> Result<(), SubagentFactoryError> {
    if !fs::try_exists(worktree_path).await.map_err(|error| {
        SubagentFactoryError::new(format!(
            "could not inspect cancelled worktree {:?}: {error}",
            worktree_path
        ))
    })? {
        let _ = run_git(repository_root, ["worktree", "prune"]).await;
        return Ok(());
    }
    run_git(
        repository_root,
        [
            "worktree",
            "remove",
            "--force",
            path_argument(worktree_path)?,
        ],
    )
    .await?;
    let _ = run_git(repository_root, ["worktree", "prune"]).await;
    Ok(())
}

#[cfg(test)]
#[derive(Clone, Debug)]
struct CreatePauseHook {
    reached: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[derive(Debug)]
struct WorktreeResource {
    repository_root: PathBuf,
    worktree_path: PathBuf,
    base_revision: String,
    finalization: Mutex<Option<SubagentResourceFinalization>>,
}

#[async_trait]
impl SubagentResource for WorktreeResource {
    fn info(&self) -> SubagentResourceInfo {
        SubagentResourceInfo {
            kind: "git_worktree".to_owned(),
            location: Some(self.worktree_path.display().to_string()),
        }
    }

    async fn finalize(
        &self,
        _disposition: SubagentResourceDisposition,
    ) -> Result<SubagentResourceFinalization, SubagentFactoryError> {
        let mut finalization = self.finalization.lock().await;
        if let Some(result) = finalization.clone() {
            return Ok(result);
        }

        if !fs::try_exists(&self.worktree_path).await.map_err(|error| {
            SubagentFactoryError::new(format!(
                "could not inspect worktree {:?}: {error}",
                self.worktree_path
            ))
        })? {
            let result = SubagentResourceFinalization {
                preserved: false,
                location: None,
                message: Some("worktree was already absent".to_owned()),
            };
            *finalization = Some(result.clone());
            return Ok(result);
        }

        let current_revision = git_stdout(&self.worktree_path, ["rev-parse", "HEAD"]).await;
        let current_revision = match current_revision {
            Ok(revision) => revision.trim().to_owned(),
            Err(error) => {
                let result = SubagentResourceFinalization {
                    preserved: true,
                    location: Some(self.worktree_path.display().to_string()),
                    message: Some(format!(
                        "could not inspect worktree revision; preserving it: {error}"
                    )),
                };
                *finalization = Some(result.clone());
                return Ok(result);
            }
        };
        if current_revision != self.base_revision {
            let result = SubagentResourceFinalization {
                preserved: true,
                location: Some(self.worktree_path.display().to_string()),
                message: Some(format!(
                    "worktree HEAD changed from {} to {}; preserving commit/checkout state",
                    self.base_revision, current_revision
                )),
            };
            *finalization = Some(result.clone());
            return Ok(result);
        }

        let status = git_stdout(
            &self.worktree_path,
            ["status", "--porcelain=v1", "--untracked-files=normal"],
        )
        .await;
        let status = match status {
            Ok(status) => status,
            Err(error) => {
                let result = SubagentResourceFinalization {
                    preserved: true,
                    location: Some(self.worktree_path.display().to_string()),
                    message: Some(format!(
                        "could not inspect worktree changes; preserving it: {error}"
                    )),
                };
                *finalization = Some(result.clone());
                return Ok(result);
            }
        };

        if !status.trim().is_empty() {
            let result = SubagentResourceFinalization {
                preserved: true,
                location: Some(self.worktree_path.display().to_string()),
                message: Some(format!(
                    "dirty worktree based on {} was preserved",
                    self.base_revision
                )),
            };
            *finalization = Some(result.clone());
            return Ok(result);
        }

        run_git(
            &self.repository_root,
            ["worktree", "remove", path_argument(&self.worktree_path)?],
        )
        .await?;
        let _ = run_git(&self.repository_root, ["worktree", "prune"]).await;
        let result = SubagentResourceFinalization {
            preserved: false,
            location: None,
            message: Some("clean worktree removed".to_owned()),
        };
        *finalization = Some(result.clone());
        Ok(result)
    }
}

async fn git_stdout<const N: usize>(
    cwd: &Path,
    args: [&str; N],
) -> Result<String, SubagentFactoryError> {
    let output = git_command(cwd, args).await?;
    String::from_utf8(output.stdout)
        .map_err(|error| SubagentFactoryError::new(format!("git output was not UTF-8: {error}")))
}

async fn run_git<const N: usize>(cwd: &Path, args: [&str; N]) -> Result<(), SubagentFactoryError> {
    git_command(cwd, args).await.map(|_| ())
}

async fn git_command<const N: usize>(
    cwd: &Path,
    args: [&str; N],
) -> Result<std::process::Output, SubagentFactoryError> {
    let mut command = Command::new("git");
    command.current_dir(cwd).args(args).kill_on_drop(true);
    let output = command.output().await.map_err(|error| {
        SubagentFactoryError::new(format!("could not start git in {:?}: {error}", cwd))
    })?;
    if output.status.success() {
        return Ok(output);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(SubagentFactoryError::new(format!(
        "git command failed in {:?} with status {}: {}",
        cwd,
        output.status,
        stderr.trim()
    )))
}

fn path_argument(path: &Path) -> Result<&str, SubagentFactoryError> {
    path.to_str().ok_or_else(|| {
        SubagentFactoryError::new(format!("worktree path is not valid UTF-8: {path:?}"))
    })
}

fn validate_component(value: &str, label: &str) -> Result<(), SubagentFactoryError> {
    if value.is_empty()
        || value.len() > 180
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(SubagentFactoryError::new(format!(
            "invalid {label} for worktree path: {value:?}"
        )));
    }
    Ok(())
}

async fn set_owner_only_directory(path: &Path) -> Result<(), SubagentFactoryError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path)
            .await
            .map_err(|error| {
                SubagentFactoryError::new(format!(
                    "could not inspect worktree directory {:?}: {error}",
                    path
                ))
            })?
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions)
            .await
            .map_err(|error| {
                SubagentFactoryError::new(format!(
                    "could not secure worktree directory {:?}: {error}",
                    path
                ))
            })?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs as std_fs, time::Duration};

    use tokio::sync::Notify;
    use uuid::Uuid;

    use super::*;

    struct TestDirectory {
        root: PathBuf,
    }

    impl TestDirectory {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("phi-daemon-worktree-test-{}", Uuid::now_v7()));
            std_fs::create_dir_all(&root).unwrap();
            Self { root }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std_fs::remove_dir_all(&self.root);
        }
    }

    async fn test_repository() -> (TestDirectory, PathBuf, Workspace) {
        let directory = TestDirectory::new();
        let repository = directory.root.join("repository");
        let project = repository.join("project");
        fs::create_dir_all(&project).await.unwrap();
        run_git(&repository, ["init", "--quiet"]).await.unwrap();
        run_git(
            &repository,
            ["config", "user.email", "phi-tests@example.invalid"],
        )
        .await
        .unwrap();
        run_git(&repository, ["config", "user.name", "Phi Tests"])
            .await
            .unwrap();
        fs::write(project.join("tracked.txt"), "initial\n")
            .await
            .unwrap();
        run_git(&repository, ["add", "."]).await.unwrap();
        run_git(&repository, ["commit", "--quiet", "-m", "initial"])
            .await
            .unwrap();
        let workspace = Workspace::new(project);
        (directory, repository, workspace)
    }

    async fn wait_until_absent(path: &Path) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !fs::try_exists(path).await.unwrap() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("worktree was not removed: {path:?}"));
    }

    async fn assert_not_registered(repository: &Path, worktree: &Path) {
        let listing = git_stdout(repository, ["worktree", "list", "--porcelain"])
            .await
            .unwrap();
        assert!(
            !listing.contains(worktree.to_str().unwrap()),
            "worktree remained registered:\n{listing}"
        );
    }

    #[tokio::test]
    async fn clean_adopted_worktree_is_removed_on_finalize() {
        let (directory, repository, workspace) = test_repository().await;
        let manager = WorktreeManager::new(directory.root.join("worktrees"));
        let prepared = manager
            .create(&workspace, "parent_clean", "agent_clean")
            .await
            .unwrap();
        let child_workspace = prepared.workspace().clone();
        let worktree = child_workspace.root().parent().unwrap().to_owned();
        assert_eq!(
            child_workspace.root().file_name().unwrap(),
            workspace.root().file_name().unwrap()
        );
        let resource = prepared.adopt_resource();

        let finalization = resource
            .finalize(SubagentResourceDisposition::Closed)
            .await
            .unwrap();
        assert!(!finalization.preserved);
        assert!(finalization.location.is_none());
        wait_until_absent(&worktree).await;
        assert_not_registered(&repository, &worktree).await;
    }

    #[tokio::test]
    async fn dirty_worktree_is_preserved_and_finalization_is_idempotent() {
        let (directory, repository, workspace) = test_repository().await;
        let manager = WorktreeManager::new(directory.root.join("worktrees"));
        let prepared = manager
            .create(&workspace, "parent_dirty", "agent_dirty")
            .await
            .unwrap();
        let child_workspace = prepared.workspace().clone();
        let worktree = child_workspace.root().parent().unwrap().to_owned();
        fs::write(child_workspace.root().join("new.txt"), "uncommitted\n")
            .await
            .unwrap();
        let resource = prepared.adopt_resource();

        let first = resource
            .finalize(SubagentResourceDisposition::Closed)
            .await
            .unwrap();
        let second = resource
            .finalize(SubagentResourceDisposition::RuntimeFailed)
            .await
            .unwrap();
        assert_eq!(first, second);
        assert!(first.preserved);
        assert_eq!(first.location.as_deref(), Some(worktree.to_str().unwrap()));
        assert!(fs::try_exists(&worktree).await.unwrap());

        cleanup_created_worktree(&repository, &worktree)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn committed_child_changes_are_preserved_even_when_status_is_clean() {
        let (directory, repository, workspace) = test_repository().await;
        let manager = WorktreeManager::new(directory.root.join("worktrees"));
        let prepared = manager
            .create(&workspace, "parent_commit", "agent_commit")
            .await
            .unwrap();
        let child_workspace = prepared.workspace().clone();
        let worktree = child_workspace.root().parent().unwrap().to_owned();
        fs::write(child_workspace.root().join("tracked.txt"), "child commit\n")
            .await
            .unwrap();
        run_git(&worktree, ["add", "."]).await.unwrap();
        run_git(&worktree, ["commit", "--quiet", "-m", "child change"])
            .await
            .unwrap();
        assert!(
            git_stdout(
                &worktree,
                ["status", "--porcelain=v1", "--untracked-files=normal"],
            )
            .await
            .unwrap()
            .trim()
            .is_empty()
        );
        let resource = prepared.adopt_resource();

        let finalization = resource
            .finalize(SubagentResourceDisposition::Closed)
            .await
            .unwrap();
        assert!(finalization.preserved);
        assert_eq!(
            finalization.location.as_deref(),
            Some(worktree.to_str().unwrap())
        );
        assert!(
            finalization
                .message
                .as_deref()
                .is_some_and(|message| message.contains("HEAD changed"))
        );
        assert!(fs::try_exists(&worktree).await.unwrap());

        cleanup_created_worktree(&repository, &worktree)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn non_git_workspace_fails_without_creating_a_worktree() {
        let directory = TestDirectory::new();
        let workspace_path = directory.root.join("not-a-repository");
        fs::create_dir_all(&workspace_path).await.unwrap();
        let worktree_root = directory.root.join("worktrees");
        let manager = WorktreeManager::new(&worktree_root);

        let error = manager
            .create(
                &Workspace::new(workspace_path),
                "parent_non_git",
                "agent_non_git",
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("git command failed"));
        assert!(!fs::try_exists(worktree_root).await.unwrap());
    }

    #[tokio::test]
    async fn invalid_components_and_existing_paths_fail_closed() {
        let (directory, repository, workspace) = test_repository().await;
        let worktree_root = directory.root.join("worktrees");
        let manager = WorktreeManager::new(&worktree_root);

        let invalid = manager
            .create(&workspace, "../escape", "agent")
            .await
            .unwrap_err();
        assert!(invalid.to_string().contains("invalid parent session ID"));
        assert!(!fs::try_exists(&worktree_root).await.unwrap());

        let prepared = manager
            .create(&workspace, "parent_collision", "agent_collision")
            .await
            .unwrap();
        let worktree = prepared.workspace().root().parent().unwrap().to_owned();
        let collision = manager
            .create(&workspace, "parent_collision", "agent_collision")
            .await
            .unwrap_err();
        assert!(collision.to_string().contains("already exists"));

        prepared.finalize_unadopted().await.unwrap();
        wait_until_absent(&worktree).await;
        assert_not_registered(&repository, &worktree).await;
    }

    #[tokio::test]
    async fn dropping_an_unadopted_worktree_removes_it() {
        let (directory, repository, workspace) = test_repository().await;
        let manager = WorktreeManager::new(directory.root.join("worktrees"));
        let prepared = manager
            .create(&workspace, "parent_drop", "agent_drop")
            .await
            .unwrap();
        let worktree = prepared.workspace().root().parent().unwrap().to_owned();
        drop(prepared);

        wait_until_absent(&worktree).await;
        assert_not_registered(&repository, &worktree).await;
    }

    #[tokio::test]
    async fn cancelling_after_create_before_adopt_cleans_the_prepared_worktree() {
        let (directory, repository, workspace) = test_repository().await;
        let manager = WorktreeManager::new(directory.root.join("worktrees"));
        let ready = Arc::new(Notify::new());
        let task_manager = manager.clone();
        let task_workspace = workspace.clone();
        let task_ready = Arc::clone(&ready);
        let worktree = directory
            .root
            .join("worktrees")
            .join("parent_build_cancel")
            .join("agent_build_cancel");
        let task = tokio::spawn(async move {
            let _prepared = task_manager
                .create(&task_workspace, "parent_build_cancel", "agent_build_cancel")
                .await
                .unwrap();
            task_ready.notify_one();
            std::future::pending::<()>().await;
        });
        ready.notified().await;
        assert!(fs::try_exists(&worktree).await.unwrap());

        task.abort();
        let _ = task.await;
        wait_until_absent(&worktree).await;
        assert_not_registered(&repository, &worktree).await;
    }

    #[tokio::test]
    async fn cancelling_receiver_does_not_cancel_git_add_and_cleans_the_created_worktree() {
        let (directory, repository, workspace) = test_repository().await;
        let pre_add_reached = Arc::new(Notify::new());
        let pre_add_release = Arc::new(Notify::new());
        let post_create_reached = Arc::new(Notify::new());
        let post_create_release = Arc::new(Notify::new());
        let manager = WorktreeManager::new(directory.root.join("worktrees"))
            .with_pre_add_hook(CreatePauseHook {
                reached: Arc::clone(&pre_add_reached),
                release: Arc::clone(&pre_add_release),
            })
            .with_post_create_hook(CreatePauseHook {
                reached: Arc::clone(&post_create_reached),
                release: Arc::clone(&post_create_release),
            });
        let worktree = directory
            .root
            .join("worktrees")
            .join("parent_cancel")
            .join("agent_cancel");
        let create_manager = manager.clone();
        let create_workspace = workspace.clone();
        let task = tokio::spawn(async move {
            create_manager
                .create(&create_workspace, "parent_cancel", "agent_cancel")
                .await
        });
        pre_add_reached.notified().await;

        // Abort only the receiver-facing future. The owned creation task must
        // still run git add to completion, observe the missing receiver, and
        // clean up its result.
        task.abort();
        let _ = task.await;
        pre_add_release.notify_one();
        post_create_reached.notified().await;
        assert!(fs::try_exists(&worktree).await.unwrap());
        post_create_release.notify_one();
        wait_until_absent(&worktree).await;
        assert_not_registered(&repository, &worktree).await;
    }
}

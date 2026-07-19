mod bash;
mod bash_classifier;
mod bash_task;
mod common;
mod edit;
mod read;
pub(crate) mod truncate;
mod write;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub use bash::{BashTool, DEFAULT_BASH_TIMEOUT};
pub use bash_classifier::{classify_bash_arguments_concurrency, classify_bash_concurrency};
pub use edit::{DEFAULT_MAX_EDIT_BYTES, EditTool};
pub use read::ReadTool;
pub use truncate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES};
pub use write::WriteTool;

use self::bash_task::{BashTaskOutputTool, BashTaskRegistry, BashTaskStopTool};
use super::Tool;
use crate::Workspace;

/// Selection of built-in local tools to install on an [`AgentBuilder`](crate::AgentBuilder).
///
/// Merely constructing an agent does not enable any built-in tools. Use
/// [`BuiltinTools::all`] or start from [`BuiltinTools::none`] and opt into
/// individual capabilities.
#[derive(Clone, Debug)]
pub struct BuiltinTools {
    workspace: Workspace,
    read: bool,
    bash: bool,
    edit: bool,
    write: bool,
}

impl BuiltinTools {
    pub fn all(cwd: impl Into<PathBuf>) -> Self {
        Self::all_in(Workspace::new(cwd))
    }

    pub fn all_in(workspace: Workspace) -> Self {
        Self {
            workspace,
            read: true,
            bash: true,
            edit: true,
            write: true,
        }
    }

    pub fn none(cwd: impl Into<PathBuf>) -> Self {
        Self::none_in(Workspace::new(cwd))
    }

    pub fn none_in(workspace: Workspace) -> Self {
        Self {
            workspace,
            read: false,
            bash: false,
            edit: false,
            write: false,
        }
    }

    /// Returns the normalized working directory shared by the selected tools.
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn workspace_dir(&self) -> &Path {
        self.workspace.root()
    }

    /// Retargets the selected capabilities to another session workspace.
    pub fn in_workspace(mut self, workspace: Workspace) -> Self {
        self.workspace = workspace;
        self
    }

    pub fn with_read(mut self) -> Self {
        self.read = true;
        self
    }

    pub fn with_bash(mut self) -> Self {
        self.bash = true;
        self
    }

    pub fn with_edit(mut self) -> Self {
        self.edit = true;
        self
    }

    pub fn with_write(mut self) -> Self {
        self.write = true;
        self
    }

    pub(crate) fn into_tools(self) -> Vec<Arc<dyn Tool>> {
        let mut tools: Vec<Arc<dyn Tool>> = Vec::with_capacity(6);
        let bash_registry = self
            .bash
            .then(|| BashTaskRegistry::new(DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES));
        if self.read {
            let read = bash_registry.as_ref().map_or_else(
                || ReadTool::new(self.workspace.root()),
                |registry| {
                    ReadTool::new(self.workspace.root())
                        .with_internal_read_root(registry.output_root())
                },
            );
            tools.push(Arc::new(read));
        }
        if let Some(registry) = bash_registry {
            tools.push(Arc::new(
                BashTool::new(self.workspace.root()).task_registry(registry.clone()),
            ));
            tools.push(Arc::new(BashTaskOutputTool::new(registry.clone())));
            tools.push(Arc::new(BashTaskStopTool::new(registry)));
        }
        if self.edit {
            tools.push(Arc::new(EditTool::new(self.workspace.root())));
        }
        if self.write {
            tools.push(Arc::new(WriteTool::new(self.workspace.root())));
        }
        tools
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CapabilityMode, ToolEffect, ToolExecutionContext};
    use serde_json::json;

    #[test]
    fn supports_selective_tool_sets() {
        let tools = BuiltinTools::none(".").with_read().with_edit().into_tools();
        let names = tools
            .iter()
            .map(|tool| tool.definition().name)
            .collect::<Vec<_>>();
        assert_eq!(names, ["read", "edit"]);
    }

    #[test]
    fn exposes_the_normalized_workspace_directory() {
        let current = std::env::current_dir().unwrap();
        assert_eq!(
            BuiltinTools::none("relative-workspace").workspace_dir(),
            current.join("relative-workspace")
        );
    }

    #[test]
    fn declares_conservative_builtin_effects() {
        let effects = BuiltinTools::all(".")
            .into_tools()
            .into_iter()
            .map(|tool| (tool.definition().name, tool.effect()))
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(effects["read"], ToolEffect::ReadOnly);
        assert_eq!(effects["edit"], ToolEffect::WorkspaceWrite);
        assert_eq!(effects["write"], ToolEffect::WorkspaceWrite);
        assert_eq!(effects["bash"], ToolEffect::ExternalSideEffect);
        assert_eq!(effects["bash_task_output"], ToolEffect::ReadOnly);
        assert_eq!(effects["bash_task_stop"], ToolEffect::ExternalSideEffect);
    }

    #[test]
    fn enabling_bash_installs_shared_task_management_tools() {
        let names = BuiltinTools::none(".")
            .with_bash()
            .into_tools()
            .into_iter()
            .map(|tool| tool.definition().name)
            .collect::<Vec<_>>();
        assert_eq!(names, ["bash", "bash_task_output", "bash_task_stop"]);
    }

    #[tokio::test]
    async fn read_can_access_the_shared_internal_background_output_root() {
        let workspace = tempfile::tempdir().unwrap();
        let tools = BuiltinTools::none(workspace.path())
            .with_read()
            .with_bash()
            .into_tools();
        let bash = tools
            .iter()
            .find(|tool| tool.definition().name == "bash")
            .unwrap();
        let task_output = tools
            .iter()
            .find(|tool| tool.definition().name == "bash_task_output")
            .unwrap();
        let read = tools
            .iter()
            .find(|tool| tool.definition().name == "read")
            .unwrap();

        let started = bash
            .execute(json!({
                "command": "printf 'background file\\n'",
                "run_in_background": true
            }))
            .await
            .unwrap();
        let metadata = started.metadata.as_ref().unwrap();
        let task_id = metadata["task_id"].as_str().unwrap();
        let output_file = metadata["output_file"].as_str().unwrap();
        task_output
            .execute(json!({ "task_id": task_id, "timeout": 2_000 }))
            .await
            .unwrap();

        let context = ToolExecutionContext::detached("read-background-output")
            .with_workspace_policy(
                Some(Workspace::new(workspace.path())),
                CapabilityMode::ReadOnly,
            );
        let output = read
            .execute_with_context(json!({ "path": output_file }), context)
            .await
            .unwrap();
        assert!(output.content.contains("background file"));
    }
}

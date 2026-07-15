mod bash;
mod bash_classifier;
mod bash_task;
mod common;
mod edit;
mod read;
pub(crate) mod truncate;
mod write;

use std::{path::PathBuf, sync::Arc};

pub use bash::{BashTool, DEFAULT_BASH_TIMEOUT};
pub use bash_classifier::{classify_bash_arguments_concurrency, classify_bash_concurrency};
pub use edit::{DEFAULT_MAX_EDIT_BYTES, EditTool};
pub use read::ReadTool;
pub use truncate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES};
pub use write::WriteTool;

use self::bash_task::{BashTaskOutputTool, BashTaskRegistry, BashTaskStopTool};
use self::common::normalize_cwd;
use super::Tool;

/// Selection of built-in local tools to install on an [`AgentBuilder`](crate::AgentBuilder).
///
/// Merely constructing an agent does not enable any built-in tools. Use
/// [`BuiltinTools::all`] or start from [`BuiltinTools::none`] and opt into
/// individual capabilities.
#[derive(Clone, Debug)]
pub struct BuiltinTools {
    cwd: PathBuf,
    read: bool,
    bash: bool,
    edit: bool,
    write: bool,
}

impl BuiltinTools {
    pub fn all(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: normalize_cwd(cwd),
            read: true,
            bash: true,
            edit: true,
            write: true,
        }
    }

    pub fn none(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: normalize_cwd(cwd),
            read: false,
            bash: false,
            edit: false,
            write: false,
        }
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
        if self.read {
            tools.push(Arc::new(ReadTool::new(self.cwd.clone())));
        }
        if self.bash {
            let registry = BashTaskRegistry::new(DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
            tools.push(Arc::new(
                BashTool::new(self.cwd.clone()).task_registry(registry.clone()),
            ));
            tools.push(Arc::new(BashTaskOutputTool::new(registry.clone())));
            tools.push(Arc::new(BashTaskStopTool::new(registry)));
        }
        if self.edit {
            tools.push(Arc::new(EditTool::new(self.cwd.clone())));
        }
        if self.write {
            tools.push(Arc::new(WriteTool::new(self.cwd)));
        }
        tools
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolEffect;

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
}

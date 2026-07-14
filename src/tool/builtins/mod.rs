mod bash;
mod common;
mod edit;
mod read;
pub(crate) mod truncate;
mod write;

use std::{path::PathBuf, sync::Arc};

pub use bash::BashTool;
pub use edit::EditTool;
pub use read::ReadTool;
pub use truncate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES};
pub use write::WriteTool;

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
        let mut tools: Vec<Arc<dyn Tool>> = Vec::with_capacity(4);
        if self.read {
            tools.push(Arc::new(ReadTool::new(self.cwd.clone())));
        }
        if self.bash {
            tools.push(Arc::new(BashTool::new(self.cwd.clone())));
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

    #[test]
    fn supports_selective_tool_sets() {
        let tools = BuiltinTools::none(".").with_read().with_edit().into_tools();
        let names = tools
            .iter()
            .map(|tool| tool.definition().name)
            .collect::<Vec<_>>();
        assert_eq!(names, ["read", "edit"]);
    }
}

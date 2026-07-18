use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use super::common::{
    invalid_arguments, io_error, mutation_guard, normalize_cwd, resolve_path_for_context,
};
use crate::{
    error::ToolError,
    tool::{Tool, ToolEffect, ToolExecutionContext, ToolOutput},
    types::ToolDefinition,
};

#[derive(Clone, Debug)]
pub struct WriteTool {
    cwd: PathBuf,
}

impl WriteTool {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: normalize_cwd(cwd),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteArguments {
    path: String,
    content: String,
}

#[async_trait]
impl Tool for WriteTool {
    fn effect(&self) -> ToolEffect {
        ToolEffect::WorkspaceWrite
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "write",
            "Write UTF-8 content to a file. Creates parent directories and the file when needed, and overwrites an existing file completely.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to write (relative to the configured working directory or absolute)"
                    },
                    "content": {
                        "type": "string",
                        "description": "Complete content to write to the file"
                    }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        self.execute_inner(arguments, None).await
    }

    async fn execute_with_context(
        &self,
        arguments: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        self.execute_inner(arguments, Some(&context)).await
    }
}

impl WriteTool {
    async fn execute_inner(
        &self,
        arguments: serde_json::Value,
        context: Option<&ToolExecutionContext>,
    ) -> Result<ToolOutput, ToolError> {
        let arguments: WriteArguments =
            serde_json::from_value(arguments).map_err(|error| invalid_arguments("write", error))?;
        let path = resolve_path_for_context(&self.cwd, &arguments.path, context).await?;
        let _guard = mutation_guard(&path).await;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| io_error("could not create parent directory for", &path, error))?;
        }
        tokio::fs::write(&path, arguments.content.as_bytes())
            .await
            .map_err(|error| io_error("could not write", &path, error))?;

        Ok(ToolOutput::success(format!(
            "Successfully wrote {} bytes to {}",
            arguments.content.len(),
            arguments.path
        )))
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::{
        Workspace,
        tool::{CapabilityMode, ToolExecutionContext},
    };

    #[tokio::test]
    async fn creates_parent_directories_and_writes_content() {
        let directory = tempdir().unwrap();
        let output = WriteTool::new(directory.path())
            .execute(json!({ "path": "nested/file.txt", "content": "hello" }))
            .await
            .unwrap();

        assert!(!output.is_error);
        assert_eq!(
            tokio::fs::read_to_string(directory.path().join("nested/file.txt"))
                .await
                .unwrap(),
            "hello"
        );
    }

    #[tokio::test]
    async fn treats_a_leading_at_as_part_of_the_file_name() {
        let directory = tempdir().unwrap();
        WriteTool::new(directory.path())
            .execute(json!({ "path": "@notes.txt", "content": "literal" }))
            .await
            .unwrap();

        assert_eq!(
            tokio::fs::read_to_string(directory.path().join("@notes.txt"))
                .await
                .unwrap(),
            "literal"
        );
        assert!(!directory.path().join("notes.txt").exists());
    }

    #[tokio::test]
    async fn workspace_edit_context_rejects_an_outside_write() {
        let parent = tempdir().unwrap();
        let workspace = parent.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let outside = parent.path().join("outside.txt");
        let context = ToolExecutionContext::detached("write").with_workspace_policy(
            Some(Workspace::new(&workspace)),
            CapabilityMode::WorkspaceEdit,
        );

        let error = WriteTool::new(&workspace)
            .execute_with_context(json!({ "path": outside, "content": "blocked" }), context)
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("outside the configured workspace")
        );
        assert!(!outside.exists());
    }
}

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use super::common::{invalid_arguments, io_error, mutation_guard, normalize_cwd, resolve_path};
use crate::{
    error::ToolError,
    tool::{Tool, ToolOutput},
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
        let arguments: WriteArguments =
            serde_json::from_value(arguments).map_err(|error| invalid_arguments("write", error))?;
        let path = resolve_path(&self.cwd, &arguments.path)?;
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
}

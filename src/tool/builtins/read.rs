use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use super::{
    common::{invalid_arguments, io_error, normalize_cwd, resolve_path},
    truncate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, TruncatedBy, truncate_head},
};
use crate::{
    error::ToolError,
    tool::{Tool, ToolOutput},
    types::ToolDefinition,
};

#[derive(Clone, Debug)]
pub struct ReadTool {
    cwd: PathBuf,
    max_lines: usize,
    max_bytes: usize,
}

impl ReadTool {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: normalize_cwd(cwd),
            max_lines: DEFAULT_MAX_LINES,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }

    pub fn output_limits(mut self, max_lines: usize, max_bytes: usize) -> Self {
        self.max_lines = max_lines.max(1);
        self.max_bytes = max_bytes.max(1);
        self
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArguments {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[async_trait]
impl Tool for ReadTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "read",
            format!(
                "Read a text file. Output is truncated to {} lines or {} bytes, whichever is reached first. Use offset and limit to continue reading large files.",
                self.max_lines, self.max_bytes
            ),
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to read (relative to the configured working directory or absolute)"
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "1-indexed line number to start reading from"
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Maximum number of lines to read"
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let arguments: ReadArguments =
            serde_json::from_value(arguments).map_err(|error| invalid_arguments("read", error))?;
        let offset = arguments.offset.unwrap_or(1);
        if offset == 0 {
            return Err(ToolError::new("read offset must be at least 1"));
        }
        if arguments.limit == Some(0) {
            return Err(ToolError::new("read limit must be at least 1"));
        }

        let path = resolve_path(&self.cwd, &arguments.path)?;
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|error| io_error("could not read", &path, error))?;
        let text = String::from_utf8_lossy(&bytes);
        let lines = text.split('\n').collect::<Vec<_>>();
        let total_file_lines = lines.len();
        let start = offset - 1;
        if start >= total_file_lines {
            return Err(ToolError::new(format!(
                "offset {offset} is beyond end of file ({total_file_lines} lines total)"
            )));
        }

        let end = arguments.limit.map_or(total_file_lines, |limit| {
            start.saturating_add(limit).min(total_file_lines)
        });
        let selected = lines[start..end].join("\n");
        let truncated = truncate_head(&selected, self.max_lines, self.max_bytes);
        let start_line = start + 1;

        let output = if truncated.first_line_exceeds_limit {
            let first_line_bytes = lines[start].len();
            format!(
                "[Line {start_line} is {first_line_bytes} bytes and exceeds the {} byte limit. Use bash with sed/head to inspect it.]",
                self.max_bytes
            )
        } else if truncated.truncated {
            let end_line = start_line + truncated.output_lines.saturating_sub(1);
            let next_offset = end_line + 1;
            let qualifier = if truncated.truncated_by == Some(TruncatedBy::Bytes) {
                format!(" ({} byte limit)", self.max_bytes)
            } else {
                String::new()
            };
            format!(
                "{}\n\n[Showing lines {start_line}-{end_line} of {total_file_lines}{qualifier}. Use offset={next_offset} to continue.]",
                truncated.content
            )
        } else if end < total_file_lines {
            let remaining = total_file_lines - end;
            format!(
                "{}\n\n[{remaining} more lines in file. Use offset={} to continue.]",
                truncated.content,
                end + 1
            )
        } else {
            truncated.content
        };

        Ok(ToolOutput::success(output))
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn reads_ranges_and_reports_continuation() {
        let directory = tempdir().unwrap();
        tokio::fs::write(directory.path().join("notes.txt"), "one\ntwo\nthree")
            .await
            .unwrap();
        let output = ReadTool::new(directory.path())
            .execute(json!({ "path": "notes.txt", "offset": 2, "limit": 1 }))
            .await
            .unwrap();

        assert_eq!(
            output.content,
            "two\n\n[1 more lines in file. Use offset=3 to continue.]"
        );
    }

    #[tokio::test]
    async fn truncates_at_complete_lines() {
        let directory = tempdir().unwrap();
        tokio::fs::write(directory.path().join("notes.txt"), "one\ntwo\nthree")
            .await
            .unwrap();
        let output = ReadTool::new(directory.path())
            .output_limits(2, 100)
            .execute(json!({ "path": "notes.txt" }))
            .await
            .unwrap();

        assert!(output.content.starts_with("one\ntwo\n\n[Showing lines 1-2"));
    }
}

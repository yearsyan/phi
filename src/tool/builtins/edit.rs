use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use super::common::{io_error, mutation_guard, normalize_cwd, resolve_path};
use crate::{
    error::ToolError,
    tool::{Tool, ToolOutput},
    types::ToolDefinition,
};

#[derive(Clone, Debug)]
pub struct EditTool {
    cwd: PathBuf,
}

impl EditTool {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: normalize_cwd(cwd),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Edit {
    old_text: String,
    new_text: String,
}

struct EditArguments {
    path: String,
    edits: Vec<Edit>,
}

impl EditArguments {
    fn parse(mut value: Value) -> Result<Self, ToolError> {
        let object = value
            .as_object_mut()
            .ok_or_else(|| ToolError::new("invalid edit arguments: expected an object"))?;
        let path = object
            .remove("path")
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| ToolError::new("invalid edit arguments: path must be a string"))?;

        let old_text = object
            .remove("oldText")
            .map(|value| {
                value.as_str().map(str::to_owned).ok_or_else(|| {
                    ToolError::new("invalid edit arguments: oldText must be a string")
                })
            })
            .transpose()?;
        let new_text = object
            .remove("newText")
            .map(|value| {
                value.as_str().map(str::to_owned).ok_or_else(|| {
                    ToolError::new("invalid edit arguments: newText must be a string")
                })
            })
            .transpose()?;
        let mut edits = match object.remove("edits") {
            Some(Value::String(encoded)) => serde_json::from_str(&encoded).map_err(|error| {
                ToolError::new(format!(
                    "invalid edit arguments: edits is not valid JSON: {error}"
                ))
            })?,
            Some(value) => serde_json::from_value(value)
                .map_err(|error| ToolError::new(format!("invalid edit arguments: {error}")))?,
            None => Vec::new(),
        };
        match (old_text, new_text) {
            (Some(old_text), Some(new_text)) => edits.push(Edit { old_text, new_text }),
            (None, None) => {}
            _ => {
                return Err(ToolError::new(
                    "invalid edit arguments: oldText and newText must be provided together",
                ));
            }
        }
        if !object.is_empty() {
            return Err(ToolError::new(format!(
                "invalid edit arguments: unknown field(s): {}",
                object.keys().cloned().collect::<Vec<_>>().join(", ")
            )));
        }
        if edits.is_empty() {
            return Err(ToolError::new(
                "invalid edit arguments: edits must contain at least one replacement",
            ));
        }
        Ok(Self { path, edits })
    }
}

#[derive(Clone, Debug)]
struct MatchedEdit {
    edit_index: usize,
    start: usize,
    length: usize,
    replacement: String,
}

#[async_trait]
impl Tool for EditTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "edit",
            "Edit one UTF-8 file using exact text replacement. Every edits[].oldText must match exactly once in the original file, and replacements must not overlap.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to edit (relative to the configured working directory or absolute)"
                    },
                    "edits": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "oldText": {
                                    "type": "string",
                                    "description": "Exact, unique text from the original file"
                                },
                                "newText": {
                                    "type": "string",
                                    "description": "Replacement text"
                                }
                            },
                            "required": ["oldText", "newText"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["path", "edits"],
                "additionalProperties": false
            }),
        )
    }

    async fn execute(&self, arguments: Value) -> Result<ToolOutput, ToolError> {
        let arguments = EditArguments::parse(arguments)?;
        let path = resolve_path(&self.cwd, &arguments.path)?;
        let _guard = mutation_guard(&path).await;
        let raw_content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|error| io_error("could not edit", &path, error))?;
        let (bom, content) = raw_content
            .strip_prefix('\u{feff}')
            .map_or(("", raw_content.as_str()), |content| ("\u{feff}", content));
        let line_ending = detect_line_ending(content);
        let original = normalize_to_lf(content);
        let mut matches = match_edits(&original, &arguments.edits, &arguments.path)?;
        matches.sort_by_key(|edit| edit.start);

        for pair in matches.windows(2) {
            let previous = &pair[0];
            let current = &pair[1];
            if previous.start + previous.length > current.start {
                return Err(ToolError::new(format!(
                    "edits[{}] and edits[{}] overlap in {}",
                    previous.edit_index, current.edit_index, arguments.path
                )));
            }
        }

        let mut edited = original.clone();
        for edit in matches.iter().rev() {
            edited.replace_range(edit.start..edit.start + edit.length, &edit.replacement);
        }
        if edited == original {
            return Err(ToolError::new(format!(
                "no changes made to {}: replacements produced identical content",
                arguments.path
            )));
        }

        let final_content = format!("{bom}{}", restore_line_endings(&edited, line_ending));
        tokio::fs::write(&path, final_content.as_bytes())
            .await
            .map_err(|error| io_error("could not edit", &path, error))?;
        Ok(ToolOutput::success(format!(
            "Successfully replaced {} block(s) in {}.",
            arguments.edits.len(),
            arguments.path
        )))
    }
}

fn match_edits(content: &str, edits: &[Edit], path: &str) -> Result<Vec<MatchedEdit>, ToolError> {
    edits
        .iter()
        .enumerate()
        .map(|(index, edit)| {
            let old_text = normalize_to_lf(&edit.old_text);
            if old_text.is_empty() {
                return Err(ToolError::new(format!(
                    "edits[{index}].oldText must not be empty in {path}"
                )));
            }
            let occurrences = content.match_indices(&old_text).collect::<Vec<_>>();
            match occurrences.as_slice() {
                [] => Err(ToolError::new(format!(
                    "could not find edits[{index}] in {path}; oldText must match exactly including whitespace and newlines"
                ))),
                [(start, _)] => Ok(MatchedEdit {
                    edit_index: index,
                    start: *start,
                    length: old_text.len(),
                    replacement: normalize_to_lf(&edit.new_text),
                }),
                _ => Err(ToolError::new(format!(
                    "found {} occurrences of edits[{index}] in {path}; oldText must be unique",
                    occurrences.len()
                ))),
            }
        })
        .collect()
}

fn detect_line_ending(content: &str) -> &'static str {
    let crlf = content.find("\r\n");
    let lf = content.find('\n');
    if matches!((crlf, lf), (Some(crlf), Some(lf)) if crlf < lf) {
        "\r\n"
    } else {
        "\n"
    }
}

fn normalize_to_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn restore_line_endings(text: &str, ending: &str) -> String {
    if ending == "\r\n" {
        text.replace('\n', "\r\n")
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn applies_multiple_edits_against_the_original_file() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("file.txt");
        tokio::fs::write(&path, "alpha\nbeta\ngamma\n")
            .await
            .unwrap();
        EditTool::new(directory.path())
            .execute(json!({
                "path": "file.txt",
                "edits": [
                    { "oldText": "alpha", "newText": "one" },
                    { "oldText": "gamma", "newText": "three" }
                ]
            }))
            .await
            .unwrap();

        assert_eq!(
            tokio::fs::read_to_string(path).await.unwrap(),
            "one\nbeta\nthree\n"
        );
    }

    #[tokio::test]
    async fn rejects_non_unique_matches() {
        let directory = tempdir().unwrap();
        tokio::fs::write(directory.path().join("file.txt"), "same same")
            .await
            .unwrap();
        let error = EditTool::new(directory.path())
            .execute(json!({
                "path": "file.txt",
                "edits": [{ "oldText": "same", "newText": "new" }]
            }))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("must be unique"));
    }

    #[tokio::test]
    async fn accepts_legacy_single_edit_arguments() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("file.txt");
        tokio::fs::write(&path, "before").await.unwrap();
        EditTool::new(directory.path())
            .execute(json!({
                "path": "file.txt",
                "oldText": "before",
                "newText": "after"
            }))
            .await
            .unwrap();

        assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), "after");
    }

    #[tokio::test]
    async fn preserves_bom_and_crlf() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("file.txt");
        tokio::fs::write(&path, "\u{feff}one\r\ntwo\r\n")
            .await
            .unwrap();
        EditTool::new(directory.path())
            .execute(json!({
                "path": "file.txt",
                "edits": [{ "oldText": "one\ntwo", "newText": "first\nsecond" }]
            }))
            .await
            .unwrap();

        assert_eq!(
            tokio::fs::read_to_string(path).await.unwrap(),
            "\u{feff}first\r\nsecond\r\n"
        );
    }
}

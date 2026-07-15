use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::{
    error::ToolError,
    tool::{Tool, ToolEffect, ToolOutput},
    types::ToolDefinition,
};

use super::SkillCatalog;

const SKILL_TOOL_NAME: &str = "skill";

#[derive(Clone, Debug)]
pub struct SkillTool {
    catalog: SkillCatalog,
}

impl SkillTool {
    pub fn new(catalog: SkillCatalog) -> Self {
        Self { catalog }
    }

    pub fn catalog(&self) -> &SkillCatalog {
        &self.catalog
    }
}

#[derive(Deserialize)]
struct SkillToolInput {
    skill: String,
    #[serde(default, alias = "args")]
    arguments: Option<String>,
}

#[async_trait]
impl Tool for SkillTool {
    fn definition(&self) -> ToolDefinition {
        let listing = self.catalog.model_listing();
        ToolDefinition::new(
            SKILL_TOOL_NAME,
            format!(
                "Load a skill's complete instructions into this conversation. When a listed skill matches the user's request, call this tool before acting on the task. Do not call a skill that is already loaded or explicitly marked as selected in the current prompt.\n\nAvailable skills:\n{listing}"
            ),
            json!({
                "type": "object",
                "properties": {
                    "skill": {
                        "type": "string",
                        "description": "The exact skill name from the available-skills list"
                    },
                    "arguments": {
                        "type": "string",
                        "description": "Optional arguments or additional instructions for the skill"
                    }
                },
                "required": ["skill"],
                "additionalProperties": false
            }),
        )
    }

    fn effect(&self) -> ToolEffect {
        ToolEffect::ReadOnly
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let input: SkillToolInput = serde_json::from_value(arguments)
            .map_err(|error| ToolError::new(format!("invalid skill arguments: {error}")))?;
        let rendered = self
            .catalog
            .render_for_model(&input.skill, input.arguments.as_deref())
            .map_err(|error| ToolError::new(error.to_string()))?;
        let metadata = self
            .catalog
            .skills()
            .iter()
            .find(|skill| skill.name == rendered.name)
            .map(|skill| {
                json!({
                    "skill": skill.name,
                    "version": skill.version,
                })
            })
            .unwrap_or_else(|| json!({ "skill": rendered.name }));
        Ok(ToolOutput::success(rendered.content).with_metadata(metadata))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use crate::skills::SkillsConfig;

    use super::*;

    #[tokio::test]
    async fn tool_lists_summaries_and_returns_full_body_only_on_execute() {
        let root = TempDir::new().unwrap();
        let directory = root.path().join("audit");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("SKILL.md"),
            "---\ndescription: Find security issues\n---\nSECRET FULL BODY",
        )
        .unwrap();
        let catalog = SkillCatalog::load(&SkillsConfig::new().directory(root.path()))
            .await
            .unwrap();
        let tool = SkillTool::new(catalog);

        let definition = tool.definition();
        assert!(definition.description.contains("Find security issues"));
        assert!(!definition.description.contains("SECRET FULL BODY"));
        let output = tool.execute(json!({ "skill": "audit" })).await.unwrap();
        assert!(output.content.contains("SECRET FULL BODY"));
        assert_eq!(output.metadata.unwrap()["skill"], "audit");
    }
}

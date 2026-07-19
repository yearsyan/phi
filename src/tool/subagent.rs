//! Model-facing tools for [`crate::subagent::SubagentRuntime`].

use std::time::Instant;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    error::ToolError,
    subagent::{
        ConfiguredSpawnAgentRequest, MAX_SUBAGENT_OUTPUT_FIELD_BYTES, MAX_SUBAGENT_OUTPUT_FIELDS,
        SubagentIsolation, SubagentNotificationKind, SubagentNotificationSource,
        SubagentOutputContract, SubagentRunOutcome, SubagentRuntime, SubagentType,
    },
    tool::{CapabilityMode, Tool, ToolConcurrency, ToolEffect, ToolExecutionContext, ToolOutput},
    types::{GenerationConfig, ReasoningEffort, ToolDefinition},
};

pub const SPAWN_AGENT_TOOL_NAME: &str = "spawn_agent";
pub const SEND_AGENT_MESSAGE_TOOL_NAME: &str = "send_agent_message";
pub const CLOSE_AGENT_TOOL_NAME: &str = "close_agent";
pub const NOTIFY_PARENT_TOOL_NAME: &str = "notify_parent";

/// The three tools installed on a parent agent. Omitting this set is the
/// library-level off switch for subagents.
#[derive(Clone)]
pub struct SubagentTools {
    pub spawn_agent: SpawnAgentTool,
    pub send_agent_message: SendAgentMessageTool,
    pub close_agent: CloseAgentTool,
}

impl SubagentTools {
    pub fn new(runtime: SubagentRuntime) -> Self {
        Self {
            spawn_agent: SpawnAgentTool::new(runtime.clone()),
            send_agent_message: SendAgentMessageTool::new(runtime.clone()),
            close_agent: CloseAgentTool::new(runtime),
        }
    }
}

#[derive(Clone)]
pub struct SpawnAgentTool {
    runtime: SubagentRuntime,
}

impl SpawnAgentTool {
    pub fn new(runtime: SubagentRuntime) -> Self {
        Self { runtime }
    }

    pub fn runtime(&self) -> &SubagentRuntime {
        &self.runtime
    }
}

#[derive(Deserialize)]
struct SpawnAgentInput {
    description: String,
    prompt: String,
    #[serde(default)]
    subagent_type: SubagentType,
    #[serde(default)]
    capability_mode: Option<CapabilityMode>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    reasoning_effort: Option<ReasoningEffort>,
    #[serde(default)]
    output_contract: Option<SubagentOutputContract>,
    #[serde(default)]
    isolation: Option<SubagentIsolation>,
    #[serde(default)]
    run_in_background: bool,
}

impl SpawnAgentTool {
    async fn execute_spawn(
        &self,
        arguments: Value,
        context: Option<ToolExecutionContext>,
    ) -> Result<ToolOutput, ToolError> {
        let input: SpawnAgentInput = parse(arguments, SPAWN_AGENT_TOOL_NAME)?;
        let generation_config =
            (input.model.is_some() || input.reasoning_effort.is_some()).then(|| GenerationConfig {
                model: input.model,
                reasoning_effort: input.reasoning_effort,
                ..GenerationConfig::default()
            });
        let description = input.description;
        let run_in_background = input.run_in_background;
        let started_at = Instant::now();
        let spawned = self
            .runtime
            .spawn_configured(ConfiguredSpawnAgentRequest {
                description: description.clone(),
                prompt: input.prompt,
                agent_type: input.subagent_type,
                capability_mode: input.capability_mode,
                generation_config,
                output_contract: input.output_contract,
                isolation: input.isolation,
                run_in_background,
            })
            .await
            .map_err(|error| ToolError::new(error.to_string()))?;

        if run_in_background {
            return Ok(ToolOutput::success(format!(
                "Agent launched successfully.\nagent_id: {}\nThe task is running in the background; its terminal notification will be delivered automatically.",
                spawned.agent_id
            ))
            .with_metadata(json!({
                "subagent_tool": SPAWN_AGENT_TOOL_NAME,
                "version": 1,
                "parent_id": self.runtime.parent_id(),
                "agent_id": spawned.agent_id,
                "delivery_id": spawned.delivery_id,
                "description": description,
                "status": "async_launched",
                "effective_config": spawned.effective_config,
            })));
        }

        let completion = match context {
            Some(context) => {
                tokio::select! {
                    result = self.runtime.wait_for_completion(
                        &spawned.agent_id,
                        &spawned.delivery_id,
                    ) => result,
                    _ = context.cancelled() => {
                        let _ = self.runtime.close(
                            &spawned.agent_id,
                            "foreground subagent cancelled with parent tool call",
                        ).await;
                        return Err(ToolError::new("foreground subagent execution was cancelled"));
                    }
                }
            }
            None => {
                self.runtime
                    .wait_for_completion(&spawned.agent_id, &spawned.delivery_id)
                    .await
            }
        }
        .map_err(|error| ToolError::new(error.to_string()))?;

        let duration_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        let total_tool_uses = self
            .runtime
            .snapshot(&spawned.agent_id)
            .map(|snapshot| {
                snapshot
                    .messages
                    .iter()
                    .map(|message| message.tool_calls.len())
                    .sum::<usize>()
            })
            .unwrap_or_default();

        let metadata = json!({
            "subagent_tool": SPAWN_AGENT_TOOL_NAME,
            "version": 1,
            "parent_id": self.runtime.parent_id(),
            "agent_id": completion.agent_id,
            "delivery_id": completion.delivery_id,
            "description": description,
            "status": match &completion.outcome {
                SubagentRunOutcome::Completed { .. } => "completed",
                SubagentRunOutcome::Stopped => "stopped",
                SubagentRunOutcome::Failed { .. } => "failed",
            },
            "effective_config": &spawned.effective_config,
            "validated_output": &completion.validated_output,
            "outcome": &completion.outcome,
            "total_tool_uses": total_tool_uses,
            "duration_ms": duration_ms,
        });
        Ok(match completion.outcome {
            SubagentRunOutcome::Completed { text, usage, .. } => {
                let text = if text.is_empty() {
                    "(Subagent completed but returned no output.)".to_owned()
                } else {
                    text
                };
                let content = if matches!(
                    spawned.effective_config.agent_type,
                    SubagentType::Explore | SubagentType::Plan
                ) {
                    text
                } else {
                    format!(
                        "{text}\n\nagent_id: {} (use send_agent_message with this ID to continue the agent)\n<usage>total_tokens: {}\ntool_uses: {total_tool_uses}\nduration_ms: {duration_ms}</usage>",
                        spawned.agent_id, usage.total_tokens,
                    )
                };
                ToolOutput::success(content).with_metadata(metadata)
            }
            SubagentRunOutcome::Stopped => {
                ToolOutput::error("Subagent stopped.").with_metadata(metadata)
            }
            SubagentRunOutcome::Failed { error } => {
                ToolOutput::error(format!("Subagent failed: {error}")).with_metadata(metadata)
            }
        })
    }
}

#[async_trait]
impl Tool for SpawnAgentTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            SPAWN_AGENT_TOOL_NAME,
            "Run a configured child agent for an independent, self-contained task. By default this waits and returns the child's final result. Set run_in_background=true to return immediately with a stable agent_id; a terminal notification will be delivered automatically. explore and plan are read-only, one-shot roles; capability_mode can further restrict but never widen a role or the parent runtime ceiling. Completed general children release their live runtime resources and can later be resumed by agent_id from their sidechain transcript.",
            json!({
                "type": "object",
                "properties": {
                    "description": {
                        "type": "string",
                        "description": "A short human-readable description of the delegated task"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Complete instructions and context for the child agent"
                    },
                    "subagent_type": {
                        "type": "string",
                        "enum": ["general", "explore", "plan"],
                        "default": "general"
                    },
                    "capability_mode": {
                        "type": "string",
                        "enum": ["read_only", "workspace_edit", "full_access"],
                        "description": "Optional tool-capability restriction. explore and plan remain read_only even if a broader value is requested."
                    },
                    "model": {
                        "type": "string",
                        "description": "Optional model override accepted by the configured child factory"
                    },
                    "reasoning_effort": {
                        "type": "string",
                        "enum": ["none", "minimal", "low", "medium", "high", "xhigh", "max"]
                    },
                    "output_contract": {
                        "oneOf": [
                            {
                                "type": "object",
                                "properties": {
                                    "type": { "const": "text" }
                                },
                                "required": ["type"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "type": { "const": "json" },
                                    "required_fields": {
                                        "type": "array",
                                        "maxItems": MAX_SUBAGENT_OUTPUT_FIELDS,
                                        "items": {
                                            "type": "string",
                                            "minLength": 1,
                                            "maxLength": MAX_SUBAGENT_OUTPUT_FIELD_BYTES
                                        }
                                    }
                                },
                                "required": ["type"],
                                "additionalProperties": false
                            }
                        ]
                    },
                    "isolation": {
                        "type": "string",
                        "enum": ["shared", "worktree"],
                        "default": "shared"
                    },
                    "run_in_background": {
                        "type": "boolean",
                        "default": false,
                        "description": "Return immediately and deliver the terminal result asynchronously"
                    }
                },
                "required": ["description", "prompt"],
                "additionalProperties": false
            }),
        )
    }

    fn effect(&self) -> ToolEffect {
        // The tool stays visible under restrictive modes so a read-only
        // explore/plan child can be delegated. Each invocation is classified
        // again from its requested child authority below.
        ToolEffect::Internal
    }

    fn effect_for(&self, arguments: &Value) -> ToolEffect {
        let Ok(input) = serde_json::from_value::<SpawnAgentInput>(arguments.clone()) else {
            return ToolEffect::ExternalSideEffect;
        };
        if input.isolation == Some(SubagentIsolation::Worktree) {
            return ToolEffect::ExternalSideEffect;
        }
        let type_ceiling = match input.subagent_type {
            SubagentType::General => CapabilityMode::FullAccess,
            SubagentType::Explore | SubagentType::Plan => CapabilityMode::ReadOnly,
        };
        match input
            .capability_mode
            .unwrap_or(type_ceiling)
            .safest(type_ceiling)
        {
            CapabilityMode::ReadOnly => ToolEffect::Internal,
            CapabilityMode::WorkspaceEdit => ToolEffect::WorkspaceWrite,
            CapabilityMode::FullAccess => ToolEffect::ExternalSideEffect,
        }
    }

    fn concurrency(&self, _arguments: &Value) -> ToolConcurrency {
        ToolConcurrency::Safe
    }

    async fn execute(&self, arguments: Value) -> Result<ToolOutput, ToolError> {
        self.execute_spawn(arguments, None).await
    }

    async fn execute_with_context(
        &self,
        arguments: Value,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        self.execute_spawn(arguments, Some(context)).await
    }
}

#[derive(Clone)]
pub struct SendAgentMessageTool {
    runtime: SubagentRuntime,
}

impl SendAgentMessageTool {
    pub fn new(runtime: SubagentRuntime) -> Self {
        Self { runtime }
    }
}

#[derive(Deserialize)]
struct SendAgentMessageInput {
    agent_id: String,
    message: String,
}

#[async_trait]
impl Tool for SendAgentMessageTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            SEND_AGENT_MESSAGE_TOOL_NAME,
            "Send a follow-up or steering message to an existing general child agent. Delivery is queued and occurs at a model/tool protocol-safe boundary; a completed child resumes from its sidechain transcript. One-shot explore and plan children cannot be continued.",
            json!({
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string" },
                    "message": { "type": "string" }
                },
                "required": ["agent_id", "message"],
                "additionalProperties": false
            }),
        )
    }

    fn effect(&self) -> ToolEffect {
        // Argument-aware classification below preserves the authority of the
        // target child instead of pessimistically hiding every steering call.
        ToolEffect::Internal
    }

    fn effect_for(&self, arguments: &Value) -> ToolEffect {
        let Ok(input) = serde_json::from_value::<SendAgentMessageInput>(arguments.clone()) else {
            return ToolEffect::ExternalSideEffect;
        };
        match self
            .runtime
            .snapshot(&input.agent_id)
            .map(|snapshot| snapshot.effective_config.capability_mode)
        {
            Some(CapabilityMode::ReadOnly) => ToolEffect::Internal,
            Some(CapabilityMode::WorkspaceEdit) => ToolEffect::WorkspaceWrite,
            Some(CapabilityMode::FullAccess) | None => ToolEffect::ExternalSideEffect,
        }
    }

    fn concurrency(&self, _arguments: &Value) -> ToolConcurrency {
        ToolConcurrency::Exclusive
    }

    async fn execute(&self, arguments: Value) -> Result<ToolOutput, ToolError> {
        let input: SendAgentMessageInput = parse(arguments, SEND_AGENT_MESSAGE_TOOL_NAME)?;
        let queued = self
            .runtime
            .send_message(&input.agent_id, input.message)
            .map_err(|error| ToolError::new(error.to_string()))?;
        Ok(ToolOutput::success(format!(
            "Queued message {} for subagent {}.",
            queued.delivery_id, queued.agent_id
        ))
        .with_metadata(json!({
            "agent_id": queued.agent_id,
            "delivery_id": queued.delivery_id,
            "status": "queued"
        })))
    }
}

#[derive(Clone)]
pub struct CloseAgentTool {
    runtime: SubagentRuntime,
}

impl CloseAgentTool {
    pub fn new(runtime: SubagentRuntime) -> Self {
        Self { runtime }
    }
}

#[derive(Deserialize)]
struct CloseAgentInput {
    agent_id: String,
    #[serde(default)]
    reason: Option<String>,
}

#[async_trait]
impl Tool for CloseAgentTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            CLOSE_AGENT_TOOL_NAME,
            "Permanently close a child agent. This is idempotent, cancels its active run, discards queued messages, and prevents future messages. A closed child cannot be resumed.",
            json!({
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string" },
                    "reason": {
                        "type": "string",
                        "description": "Optional reason recorded in the terminal event"
                    }
                },
                "required": ["agent_id"],
                "additionalProperties": false
            }),
        )
    }

    fn effect(&self) -> ToolEffect {
        // Closing only mutates agent-local coordination state and remains
        // available at every capability level as a safety action.
        ToolEffect::Internal
    }

    async fn execute(&self, arguments: Value) -> Result<ToolOutput, ToolError> {
        let input: CloseAgentInput = parse(arguments, CLOSE_AGENT_TOOL_NAME)?;
        let closed = self
            .runtime
            .close(
                &input.agent_id,
                input
                    .reason
                    .unwrap_or_else(|| "closed by parent".to_owned()),
            )
            .await
            .map_err(|error| ToolError::new(error.to_string()))?;
        Ok(ToolOutput::success(if closed.already_closed {
            format!("Subagent {} was already closed.", closed.agent_id)
        } else {
            format!("Closed subagent {}.", closed.agent_id)
        })
        .with_metadata(json!({
            "parent_id": self.runtime.parent_id(),
            "agent_id": closed.agent_id,
            "state": "closed",
            "already_closed": closed.already_closed
        })))
    }
}

/// Installed automatically on children; it is intentionally not included in
/// [`SubagentTools`] and cannot target an arbitrary parent or child.
#[derive(Clone)]
pub struct NotifyParentTool {
    runtime: SubagentRuntime,
    agent_id: String,
}

impl NotifyParentTool {
    pub(crate) fn new(runtime: SubagentRuntime, agent_id: String) -> Self {
        Self { runtime, agent_id }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum NotifyKindInput {
    Progress,
    Blocker,
    Result,
}

impl From<NotifyKindInput> for SubagentNotificationKind {
    fn from(value: NotifyKindInput) -> Self {
        match value {
            NotifyKindInput::Progress => Self::Progress,
            NotifyKindInput::Blocker => Self::Blocker,
            NotifyKindInput::Result => Self::Result,
        }
    }
}

#[derive(Deserialize)]
struct NotifyParentInput {
    kind: NotifyKindInput,
    message: String,
}

#[async_trait]
impl Tool for NotifyParentTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            NOTIFY_PARENT_TOOL_NAME,
            "Notify the parent agent about progress, a blocker that needs attention, or a result. Progress is observable but does not wake an idle parent; blocker and result notifications do.",
            json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["progress", "blocker", "result"]
                    },
                    "message": { "type": "string" }
                },
                "required": ["kind", "message"],
                "additionalProperties": false
            }),
        )
    }

    fn effect(&self) -> ToolEffect {
        ToolEffect::Internal
    }

    async fn execute(&self, arguments: Value) -> Result<ToolOutput, ToolError> {
        let input: NotifyParentInput = parse(arguments, NOTIFY_PARENT_TOOL_NAME)?;
        let kind = SubagentNotificationKind::from(input.kind);
        let wake_parent = self
            .runtime
            .notification_wakes_parent(&self.agent_id, &kind)
            .map_err(|error| ToolError::new(error.to_string()))?;
        let delivery_id = self
            .runtime
            .notify(
                &self.agent_id,
                kind,
                input.message,
                SubagentNotificationSource::Child,
            )
            .map_err(|error| ToolError::new(error.to_string()))?;
        Ok(ToolOutput::success(format!(
            "Notification {delivery_id} delivered to the parent."
        ))
        .with_metadata(json!({
            "delivery_id": delivery_id,
            "wake_parent": wake_parent
        })))
    }
}

fn parse<T: for<'de> Deserialize<'de>>(arguments: Value, tool: &str) -> Result<T, ToolError> {
    serde_json::from_value(arguments)
        .map_err(|error| ToolError::new(format!("invalid {tool} arguments: {error}")))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;

    use super::*;
    use crate::{
        Agent,
        provider::{LlmProvider, ProviderEventStream},
        subagent::{SubagentBuildRequest, SubagentConfig, SubagentFactory, SubagentFactoryError},
        types::{AssistantMessage, ProviderEvent, ProviderResponse},
    };

    struct PendingFactory;

    struct ImmediateProvider;

    impl LlmProvider for ImmediateProvider {
        fn stream(&self, _request: crate::ProviderRequest) -> ProviderEventStream {
            Box::pin(futures_util::stream::iter([Ok(ProviderEvent::Done(
                ProviderResponse {
                    message: AssistantMessage::text("foreground child result"),
                    usage: None,
                },
            ))]))
        }
    }

    struct ImmediateFactory;

    #[async_trait]
    impl SubagentFactory for ImmediateFactory {
        async fn build(
            &self,
            _request: SubagentBuildRequest,
        ) -> Result<Agent, SubagentFactoryError> {
            Ok(Agent::builder(ImmediateProvider).build())
        }
    }

    #[async_trait]
    impl SubagentFactory for PendingFactory {
        async fn build(
            &self,
            _request: SubagentBuildRequest,
        ) -> Result<Agent, SubagentFactoryError> {
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn spawn_tool_exposes_configured_schema_and_effective_metadata() {
        let runtime = SubagentRuntime::new(
            "tool-parent",
            Arc::new(PendingFactory),
            SubagentConfig::default(),
        );
        let tool = SpawnAgentTool::new(runtime.clone());
        let definition = tool.definition();
        assert_eq!(
            definition.parameters["properties"]["subagent_type"]["enum"],
            json!(["general", "explore", "plan"])
        );
        assert_eq!(
            definition.parameters["properties"]["capability_mode"]["enum"],
            json!(["read_only", "workspace_edit", "full_access"])
        );
        assert_eq!(
            definition.parameters["properties"]["run_in_background"]["default"],
            Value::Bool(false)
        );
        assert_eq!(tool.concurrency(&json!({})), ToolConcurrency::Safe);

        let output = tool
            .execute(json!({
                "description": "inspect",
                "prompt": "inspect the repository",
                "subagent_type": "explore",
                "capability_mode": "full_access",
                "model": "child-model",
                "reasoning_effort": "high",
                "output_contract": {
                    "type": "json",
                    "required_fields": ["summary"]
                },
                "isolation": "worktree",
                "run_in_background": true
            }))
            .await
            .unwrap();
        let metadata = output.metadata.unwrap();
        assert_eq!(
            metadata["effective_config"]["agent_type"],
            Value::String("explore".to_owned())
        );
        assert_eq!(
            metadata["effective_config"]["capability_mode"],
            Value::String("read_only".to_owned())
        );
        assert_eq!(
            metadata["effective_config"]["generation_config"]["model"],
            Value::String("child-model".to_owned())
        );
        assert_eq!(
            metadata["effective_config"]["output_contract"]["type"],
            Value::String("json".to_owned())
        );
        assert_eq!(
            metadata["effective_config"]["isolation"],
            Value::String("worktree".to_owned())
        );
        assert_eq!(
            metadata["status"],
            Value::String("async_launched".to_owned())
        );
        assert_eq!(metadata["parent_id"], "tool-parent");
        runtime.shutdown("test complete").await;
    }

    #[tokio::test]
    async fn spawn_tool_is_foreground_by_default_and_returns_the_child_result() {
        let runtime = SubagentRuntime::new(
            "foreground-tool-parent",
            Arc::new(ImmediateFactory),
            SubagentConfig::default(),
        );
        let tool = SpawnAgentTool::new(runtime.clone());
        let output = tool
            .execute(json!({
                "description": "answer directly",
                "prompt": "produce the result"
            }))
            .await
            .unwrap();
        assert!(!output.is_error);
        assert!(output.content.starts_with("foreground child result"));
        assert!(output.content.contains("use send_agent_message"));
        assert!(output.content.contains("tool_uses: 0"));
        assert!(output.content.contains("duration_ms:"));
        let metadata = output.metadata.unwrap();
        assert_eq!(metadata["status"], "completed");
        assert_eq!(metadata["parent_id"], "foreground-tool-parent");
        assert_eq!(
            metadata["effective_config"]["run_in_background"],
            Value::Bool(false)
        );
        let agent_id = metadata["agent_id"].as_str().unwrap();
        assert_eq!(
            runtime.snapshot(agent_id).unwrap().state,
            crate::SubagentState::Idle
        );
        runtime.shutdown("test complete").await;
    }
}

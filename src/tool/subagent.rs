//! Model-facing tools for [`crate::subagent::SubagentRuntime`].

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    error::ToolError,
    subagent::{
        SpawnAgentRequest, SubagentNotificationKind, SubagentNotificationSource, SubagentRuntime,
    },
    tool::{Tool, ToolConcurrency, ToolEffect, ToolOutput},
    types::ToolDefinition,
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
}

#[async_trait]
impl Tool for SpawnAgentTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            SPAWN_AGENT_TOOL_NAME,
            "Start a child agent for an independent, self-contained task. Returns immediately with a stable agent_id. The child persists after its first result so you may send follow-up messages; close it explicitly when no longer needed.",
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
                    }
                },
                "required": ["description", "prompt"],
                "additionalProperties": false
            }),
        )
    }

    fn effect(&self) -> ToolEffect {
        // A factory may give the child externally-effecting tools. The parent
        // tool must therefore keep the conservative upper bound.
        ToolEffect::ExternalSideEffect
    }

    fn concurrency(&self, _arguments: &Value) -> ToolConcurrency {
        ToolConcurrency::Exclusive
    }

    async fn execute(&self, arguments: Value) -> Result<ToolOutput, ToolError> {
        let input: SpawnAgentInput = parse(arguments, SPAWN_AGENT_TOOL_NAME)?;
        let spawned = self
            .runtime
            .spawn(SpawnAgentRequest::new(input.description, input.prompt))
            .await
            .map_err(|error| ToolError::new(error.to_string()))?;
        Ok(ToolOutput::success(format!(
            "Started subagent {} (delivery {}).",
            spawned.agent_id, spawned.delivery_id
        ))
        .with_metadata(json!({
            "agent_id": spawned.agent_id,
            "delivery_id": spawned.delivery_id,
            "state": "starting"
        })))
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
            "Send a follow-up or steering message to an existing child agent. Delivery is queued and occurs at a model/tool protocol-safe boundary; it does not corrupt an active provider stream.",
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
        // Steering can cause the already-running child to perform effects.
        ToolEffect::ExternalSideEffect
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
        // available as a safety action in plan mode.
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
        let wake_parent = kind.wakes_parent();
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

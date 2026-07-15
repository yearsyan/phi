use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use phi::{
    Tool, ToolDefinition, ToolEffect, ToolError, ToolOutput,
    plan::{PlanArtifact, PlanStore},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

use super::{PlanApprovalId, SessionId};

pub const READ_PLAN_TOOL_NAME: &str = "read_plan";
pub const WRITE_PLAN_TOOL_NAME: &str = "write_plan";
pub const EXIT_PLAN_MODE_TOOL_NAME: &str = "exit_plan_mode";

const READ_PLAN_DESCRIPTION: &str = "Read the current session's persisted implementation plan and revision. This tool is available only in plan mode.";
const WRITE_PLAN_DESCRIPTION: &str = "Create or replace the current session's persisted implementation plan. Pass the revision returned by read_plan/write_plan; use revision 0 only when creating the first plan. If this call is cancelled or times out, its outcome may be uncertain: call read_plan to reconcile before retrying and never blindly replay the old expected_revision. This tool is available only in plan mode.";
const EXIT_PLAN_MODE_DESCRIPTION: &str = "Submit the current persisted plan for explicit user approval. Call this only after the plan is complete. The tool waits for the user to approve or reject the exact persisted revision; do not ask for approval in ordinary text.";

/// An immutable plan revision presented to the user for approval.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlanApprovalRequest {
    pub approval_id: PlanApprovalId,
    pub plan: PlanArtifact,
}

/// The user's decision for one exact plan revision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlanApprovalDecision {
    Approve {
        revision: u64,
    },
    Reject {
        revision: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        feedback: Option<String>,
    },
}

impl PlanApprovalDecision {
    pub fn revision(&self) -> u64 {
        match self {
            Self::Approve { revision } | Self::Reject { revision, .. } => *revision,
        }
    }
}

pub(super) struct PendingPlanApprovalRequest {
    pub request: PlanApprovalRequest,
    pub reply: oneshot::Sender<Result<PlanApprovalDecision, String>>,
}

pub(super) enum PlanApprovalMessage {
    Request(PendingPlanApprovalRequest),
    Cancel { approval_id: PlanApprovalId },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WritePlanArguments {
    expected_revision: u64,
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyArguments {}

#[derive(Clone)]
pub(super) struct ReadPlanTool {
    session_id: SessionId,
    store: Arc<dyn PlanStore>,
}

impl ReadPlanTool {
    pub(super) fn new(session_id: SessionId, store: Arc<dyn PlanStore>) -> Self {
        Self { session_id, store }
    }
}

#[async_trait]
impl Tool for ReadPlanTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            READ_PLAN_TOOL_NAME,
            READ_PLAN_DESCRIPTION,
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {}
            }),
        )
    }

    fn effect(&self) -> ToolEffect {
        ToolEffect::PlanOnly
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        serde_json::from_value::<EmptyArguments>(arguments)
            .map_err(|error| ToolError::new(format!("invalid read_plan arguments: {error}")))?;
        let plan = self
            .store
            .current(&self.session_id.to_string())
            .await
            .map_err(|error| ToolError::new(format!("could not read plan: {error}")))?;
        let content = serde_json::to_string(&json!({ "plan": plan }))
            .map_err(|error| ToolError::new(format!("could not serialize plan: {error}")))?;
        Ok(ToolOutput::success(content))
    }
}

#[derive(Clone)]
pub(super) struct WritePlanTool {
    session_id: SessionId,
    store: Arc<dyn PlanStore>,
}

impl WritePlanTool {
    pub(super) fn new(session_id: SessionId, store: Arc<dyn PlanStore>) -> Self {
        Self { session_id, store }
    }
}

#[async_trait]
impl Tool for WritePlanTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            WRITE_PLAN_TOOL_NAME,
            WRITE_PLAN_DESCRIPTION,
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "expected_revision": { "type": "integer", "minimum": 0 },
                    "content": { "type": "string" }
                },
                "required": ["expected_revision", "content"]
            }),
        )
    }

    fn effect(&self) -> ToolEffect {
        ToolEffect::PlanOnly
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let arguments: WritePlanArguments = serde_json::from_value(arguments)
            .map_err(|error| ToolError::new(format!("invalid write_plan arguments: {error}")))?;
        let plan = self
            .store
            .update(
                &self.session_id.to_string(),
                arguments.expected_revision,
                arguments.content,
            )
            .await
            .map_err(|error| ToolError::new(format!("could not write plan: {error}")))?;
        let content = serde_json::to_string(&plan)
            .map_err(|error| ToolError::new(format!("could not serialize plan: {error}")))?;
        Ok(ToolOutput::success(content))
    }
}

#[derive(Clone)]
pub(super) struct ExitPlanModeTool {
    session_id: SessionId,
    store: Arc<dyn PlanStore>,
    messages: mpsc::UnboundedSender<PlanApprovalMessage>,
    request_in_flight: Arc<AtomicBool>,
}

impl ExitPlanModeTool {
    pub(super) fn channel(
        session_id: SessionId,
        store: Arc<dyn PlanStore>,
    ) -> (Self, mpsc::UnboundedReceiver<PlanApprovalMessage>) {
        let (messages, receiver) = mpsc::unbounded_channel();
        (
            Self {
                session_id,
                store,
                messages,
                request_in_flight: Arc::new(AtomicBool::new(false)),
            },
            receiver,
        )
    }
}

struct InFlightGuard(Arc<AtomicBool>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

struct ApprovalCancellationGuard {
    messages: mpsc::UnboundedSender<PlanApprovalMessage>,
    approval_id: PlanApprovalId,
    armed: bool,
}

impl ApprovalCancellationGuard {
    fn new(
        messages: mpsc::UnboundedSender<PlanApprovalMessage>,
        approval_id: PlanApprovalId,
    ) -> Self {
        Self {
            messages,
            approval_id,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ApprovalCancellationGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.messages.send(PlanApprovalMessage::Cancel {
                approval_id: self.approval_id,
            });
        }
    }
}

#[async_trait]
impl Tool for ExitPlanModeTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            EXIT_PLAN_MODE_TOOL_NAME,
            EXIT_PLAN_MODE_DESCRIPTION,
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {}
            }),
        )
    }

    fn effect(&self) -> ToolEffect {
        ToolEffect::PlanOnly
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        serde_json::from_value::<EmptyArguments>(arguments).map_err(|error| {
            ToolError::new(format!("invalid exit_plan_mode arguments: {error}"))
        })?;
        if self
            .request_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ToolError::new(
                "a plan approval request is already pending for this session",
            ));
        }
        let _in_flight = InFlightGuard(Arc::clone(&self.request_in_flight));

        let plan = self
            .store
            .current(&self.session_id.to_string())
            .await
            .map_err(|error| ToolError::new(format!("could not read plan for approval: {error}")))?
            .ok_or_else(|| ToolError::new("cannot exit plan mode before a plan is written"))?;
        if plan.content.trim().is_empty() {
            return Err(ToolError::new("cannot submit an empty plan for approval"));
        }

        let request = PlanApprovalRequest {
            approval_id: PlanApprovalId::new(),
            plan,
        };
        let (reply, response) = oneshot::channel();
        let mut cancellation =
            ApprovalCancellationGuard::new(self.messages.clone(), request.approval_id);
        if self
            .messages
            .send(PlanApprovalMessage::Request(PendingPlanApprovalRequest {
                request: request.clone(),
                reply,
            }))
            .is_err()
        {
            cancellation.disarm();
            return Err(ToolError::new("plan approval runtime is unavailable"));
        }
        // A tool timeout/cancellation drops this future. The guard emits a
        // cancellation on the same FIFO channel as the request, so the actor
        // observes Request before Cancel even if it has not registered the
        // request yet.
        let response = match response.await {
            Ok(response) => response,
            Err(_) => return Err(ToolError::new("plan approval request was cancelled")),
        };
        let decision = match response {
            Ok(decision) => {
                cancellation.disarm();
                decision
            }
            Err(error) => {
                // The actor has authoritatively removed/cancelled the request.
                cancellation.disarm();
                return Err(ToolError::new(error));
            }
        };

        let content = serde_json::to_string(&decision).map_err(|error| {
            ToolError::new(format!(
                "could not serialize plan approval decision: {error}"
            ))
        })?;
        match decision {
            PlanApprovalDecision::Approve { .. } => Ok(ToolOutput::success(content)),
            PlanApprovalDecision::Reject { .. } => Ok(ToolOutput::error(content)),
        }
    }
}

#[cfg(test)]
mod tests {
    use phi::plan::{EMPTY_PLAN_REVISION, InMemoryPlanStore};

    use super::*;

    #[tokio::test]
    async fn read_and_write_tools_share_the_versioned_artifact() {
        let session_id = SessionId::new();
        let store: Arc<dyn PlanStore> = Arc::new(InMemoryPlanStore::new());
        let write = WritePlanTool::new(session_id, Arc::clone(&store));
        let read = ReadPlanTool::new(session_id, store);

        let output = write
            .execute(json!({
                "expected_revision": EMPTY_PLAN_REVISION,
                "content": "# Plan\n\nShip it."
            }))
            .await
            .unwrap();
        let written: PlanArtifact = serde_json::from_str(&output.content).unwrap();
        assert_eq!(written.revision, 1);

        let output = read.execute(json!({})).await.unwrap();
        let value: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(value["plan"]["revision"], 1);
        assert_eq!(value["plan"]["content"], "# Plan\n\nShip it.");
    }

    #[tokio::test]
    async fn exit_waits_for_a_typed_decision_about_the_persisted_revision() {
        let session_id = SessionId::new();
        let store: Arc<dyn PlanStore> = Arc::new(InMemoryPlanStore::new());
        store
            .update(
                &session_id.to_string(),
                EMPTY_PLAN_REVISION,
                "Do it".to_owned(),
            )
            .await
            .unwrap();
        let (tool, mut requests) = ExitPlanModeTool::channel(session_id, store);

        let execution = tokio::spawn(async move { tool.execute(json!({})).await });
        let PlanApprovalMessage::Request(pending) = requests.recv().await.unwrap() else {
            panic!("request must precede cancellation");
        };
        assert_eq!(pending.request.plan.revision, 1);
        pending
            .reply
            .send(Ok(PlanApprovalDecision::Approve { revision: 1 }))
            .unwrap();
        let output = execution.await.unwrap().unwrap();
        assert!(!output.is_error);
        assert_eq!(
            serde_json::from_str::<PlanApprovalDecision>(&output.content).unwrap(),
            PlanApprovalDecision::Approve { revision: 1 }
        );
        assert!(requests.try_recv().is_err());
    }

    #[tokio::test]
    async fn dropping_exit_emits_an_ordered_cancellation_for_its_request() {
        let session_id = SessionId::new();
        let store: Arc<dyn PlanStore> = Arc::new(InMemoryPlanStore::new());
        store
            .update(
                &session_id.to_string(),
                EMPTY_PLAN_REVISION,
                "Do it".to_owned(),
            )
            .await
            .unwrap();
        let (tool, mut messages) = ExitPlanModeTool::channel(session_id, store);
        let execution = tokio::spawn(async move { tool.execute(json!({})).await });

        let PlanApprovalMessage::Request(pending) = messages.recv().await.unwrap() else {
            panic!("request must be the first message");
        };
        let approval_id = pending.request.approval_id;
        execution.abort();
        assert!(execution.await.unwrap_err().is_cancelled());
        assert!(matches!(
            messages.recv().await,
            Some(PlanApprovalMessage::Cancel {
                approval_id: current
            }) if current == approval_id
        ));
    }

    #[tokio::test]
    async fn exit_rejects_missing_and_empty_plans_without_publishing_a_request() {
        let session_id = SessionId::new();
        let store: Arc<dyn PlanStore> = Arc::new(InMemoryPlanStore::new());
        let (tool, mut requests) = ExitPlanModeTool::channel(session_id, Arc::clone(&store));
        assert!(tool.execute(json!({})).await.is_err());
        assert!(requests.try_recv().is_err());

        store
            .update(
                &session_id.to_string(),
                EMPTY_PLAN_REVISION,
                "  ".to_owned(),
            )
            .await
            .unwrap();
        assert!(tool.execute(json!({})).await.is_err());
        assert!(requests.try_recv().is_err());
    }
}

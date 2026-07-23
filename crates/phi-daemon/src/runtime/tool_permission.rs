use async_trait::async_trait;
use phi::{
    CapabilityMode, ToolCall, ToolEffect, ToolPermissionApprover, ToolPermissionDecision,
    ToolPermissionRequest, ToolPermissionRule,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

use super::ToolPermissionId;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolPermissionPrompt {
    pub permission_id: ToolPermissionId,
    pub call: ToolCall,
    pub effect: ToolEffect,
    pub capability_mode: CapabilityMode,
    pub suggestions: Vec<ToolPermissionRule>,
}

pub(super) struct PendingToolPermissionRequest {
    pub prompt: ToolPermissionPrompt,
    pub reply: oneshot::Sender<ToolPermissionDecision>,
}

#[derive(Clone)]
pub(super) struct DaemonToolPermissionApprover {
    requests: mpsc::Sender<PendingToolPermissionRequest>,
}

impl DaemonToolPermissionApprover {
    pub(super) fn channel(capacity: usize) -> (Self, mpsc::Receiver<PendingToolPermissionRequest>) {
        let (requests, receiver) = mpsc::channel(capacity.max(1));
        (Self { requests }, receiver)
    }
}

#[async_trait]
impl ToolPermissionApprover for DaemonToolPermissionApprover {
    async fn decide(&self, request: ToolPermissionRequest) -> ToolPermissionDecision {
        let cancellation = request.cancellation;
        let prompt = ToolPermissionPrompt {
            permission_id: ToolPermissionId::new(),
            call: request.call,
            effect: request.effect,
            capability_mode: request.capability_mode,
            suggestions: request.suggestions,
        };
        let (reply, response) = oneshot::channel();
        let pending = PendingToolPermissionRequest { prompt, reply };
        let sent = tokio::select! {
            biased;
            _ = cancellation.cancelled() => false,
            result = self.requests.send(pending) => result.is_ok(),
        };
        if !sent {
            return ToolPermissionDecision::Deny {
                message: "tool permission request was cancelled".to_owned(),
            };
        }
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => ToolPermissionDecision::Deny {
                message: "tool permission request was cancelled".to_owned(),
            },
            decision = response => decision.unwrap_or_else(|_| ToolPermissionDecision::Deny {
                message: "tool permission runtime is unavailable".to_owned(),
            }),
        }
    }
}

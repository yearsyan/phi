use std::{
    collections::HashSet,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::watch;

use crate::{
    Workspace,
    error::ToolError,
    types::{Content, ContentPart, ToolDefinition},
};

pub mod builtins;
pub mod subagent;

/// The maximum tool capability granted to an agent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityMode {
    /// Read-only and agent-local coordination tools.
    ReadOnly,
    /// Read-only, coordination, and workspace-scoped file mutation tools.
    WorkspaceEdit,
    /// Every registered tool.
    #[default]
    FullAccess,
}

impl CapabilityMode {
    pub fn allows(self, effect: ToolEffect) -> bool {
        match self {
            Self::ReadOnly => matches!(effect, ToolEffect::ReadOnly | ToolEffect::Internal),
            Self::WorkspaceEdit => matches!(
                effect,
                ToolEffect::ReadOnly | ToolEffect::Internal | ToolEffect::WorkspaceWrite
            ),
            Self::FullAccess => true,
        }
    }

    pub fn is_subset_of(self, other: Self) -> bool {
        self.rank() <= other.rank()
    }

    pub(crate) fn safest(self, other: Self) -> Self {
        if self.rank() <= other.rank() {
            self
        } else {
            other
        }
    }

    const fn rank(self) -> u8 {
        match self {
            Self::ReadOnly => 0,
            Self::WorkspaceEdit => 1,
            Self::FullAccess => 2,
        }
    }
}

/// Name-based tool selection applied before execution-mode boundaries.
///
/// A configured deny takes precedence over the allow-list. Mandatory harness
/// tools bypass only this name policy; their effects remain subject to
/// [`CapabilityMode`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    allowed_tools: Option<HashSet<String>>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    denied_tools: HashSet<String>,
}

impl ToolPolicy {
    pub fn allow_only<I, S>(tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            allowed_tools: Some(tools.into_iter().map(Into::into).collect()),
            denied_tools: HashSet::new(),
        }
    }

    pub fn with_allowed_tools<I, S>(mut self, tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.allowed_tools = Some(tools.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_denied_tools<I, S>(mut self, tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.denied_tools.extend(tools.into_iter().map(Into::into));
        self
    }

    pub fn allowed_tools(&self) -> Option<&HashSet<String>> {
        self.allowed_tools.as_ref()
    }

    pub fn denied_tools(&self) -> &HashSet<String> {
        &self.denied_tools
    }

    pub fn allows(&self, tool_name: &str, mandatory: bool) -> bool {
        mandatory
            || (!self.denied_tools.contains(tool_name)
                && self
                    .allowed_tools
                    .as_ref()
                    .is_none_or(|allowed| allowed.contains(tool_name)))
    }
}

/// The externally observable effect a tool may have.
///
/// Custom and dynamically discovered tools default to
/// [`ToolEffect::ExternalSideEffect`]. Authors must explicitly opt into a less
/// privileged classification after auditing the implementation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolEffect {
    /// Observes state without modifying it.
    ReadOnly,
    /// Mutates only agent-local coordination state.
    Internal,
    /// Writes to the configured workspace.
    WorkspaceWrite,
    /// May run commands, use the network, or otherwise affect external state.
    #[default]
    ExternalSideEffect,
}

/// Whether calls to a tool may overlap other calls in the same model batch.
///
/// `Safe` is deliberately opt-in for argument-dependent tools. The default is
/// derived from [`ToolEffect`], so unknown and side-effecting tools remain
/// exclusive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolConcurrency {
    Safe,
    #[default]
    Exclusive,
}

/// A cooperative cancellation signal for one tool invocation.
#[derive(Clone, Debug)]
pub struct ToolCancellation {
    local: watch::Sender<bool>,
    parent: Option<watch::Receiver<bool>>,
}

impl ToolCancellation {
    pub fn new() -> Self {
        let (local, _) = watch::channel(false);
        Self {
            local,
            parent: None,
        }
    }

    pub(crate) fn from_sender(stopped: &watch::Sender<bool>) -> Self {
        let (local, _) = watch::channel(false);
        Self {
            local,
            parent: Some(stopped.subscribe()),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        *self.local.borrow() || self.parent.as_ref().is_some_and(|parent| *parent.borrow())
    }

    pub(crate) fn cancel(&self) {
        self.local.send_replace(true);
    }

    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }

        let mut local = self.local.subscribe();
        let mut parent = self.parent.clone();
        loop {
            if *local.borrow_and_update()
                || parent
                    .as_mut()
                    .is_some_and(|parent| *parent.borrow_and_update())
            {
                return;
            }
            match parent.as_mut() {
                Some(parent) => {
                    tokio::select! {
                        changed = local.changed() => {
                            if changed.is_err() {
                                return;
                            }
                        }
                        changed = parent.changed() => {
                            if changed.is_err() {
                                return;
                            }
                        }
                    }
                }
                None => {
                    if local.changed().await.is_err() {
                        return;
                    }
                }
            }
        }
    }
}

impl Default for ToolCancellation {
    fn default() -> Self {
        Self::new()
    }
}

/// An incremental status update emitted while a tool is running.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolProgress {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl ToolProgress {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            metadata: None,
        }
    }

    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

pub(crate) type ProgressReporter = Arc<dyn Fn(ToolProgress) + Send + Sync>;
pub(crate) type AgentNotificationReporter =
    Arc<dyn Fn(Content) -> Result<(), ToolError> + Send + Sync>;

/// Per-invocation services available to every tool.
///
/// Tools can cooperatively observe cancellation, emit progress, and determine
/// whether a prior tool result they reference is still present in the active
/// transcript (used by transcript-aware output de-duplication).
#[derive(Clone)]
pub struct ToolExecutionContext {
    call_id: String,
    cancellation: ToolCancellation,
    visible_tool_results: Arc<HashSet<String>>,
    progress: Option<ProgressReporter>,
    progress_active: Arc<AtomicBool>,
    agent_notification: Option<AgentNotificationReporter>,
    workspace: Option<Workspace>,
    capability_mode: CapabilityMode,
}

impl ToolExecutionContext {
    pub fn detached(call_id: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            cancellation: ToolCancellation::new(),
            visible_tool_results: Arc::new(HashSet::new()),
            progress: None,
            progress_active: Arc::new(AtomicBool::new(true)),
            agent_notification: None,
            workspace: None,
            capability_mode: CapabilityMode::default(),
        }
    }

    pub(crate) fn new(
        call_id: impl Into<String>,
        cancellation: ToolCancellation,
        visible_tool_results: Arc<HashSet<String>>,
        progress: Option<ProgressReporter>,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            cancellation,
            visible_tool_results,
            progress,
            progress_active: Arc::new(AtomicBool::new(true)),
            agent_notification: None,
            workspace: None,
            capability_mode: CapabilityMode::default(),
        }
    }

    pub(crate) fn with_workspace_policy(
        mut self,
        workspace: Option<Workspace>,
        capability_mode: CapabilityMode,
    ) -> Self {
        self.workspace = workspace;
        self.capability_mode = capability_mode;
        self
    }

    pub(crate) fn with_agent_notification(
        mut self,
        agent_notification: Option<AgentNotificationReporter>,
    ) -> Self {
        self.agent_notification = agent_notification;
        self
    }

    pub fn call_id(&self) -> &str {
        &self.call_id
    }

    pub fn cancellation(&self) -> &ToolCancellation {
        &self.cancellation
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    pub async fn cancelled(&self) {
        self.cancellation.cancelled().await;
    }

    pub fn has_visible_tool_result(&self, call_id: &str) -> bool {
        self.visible_tool_results.contains(call_id)
    }

    pub fn workspace(&self) -> Option<&Workspace> {
        self.workspace.as_ref()
    }

    pub fn capability_mode(&self) -> CapabilityMode {
        self.capability_mode
    }

    pub fn report_progress(&self, progress: impl Into<ToolProgress>) {
        if !self.is_cancelled()
            && self.progress_active.load(Ordering::Acquire)
            && let Some(report) = &self.progress
        {
            report(progress.into());
        }
    }

    /// Queues an internal follow-up message for the owning Agent.
    ///
    /// Managed background work may call this after the originating tool call
    /// has returned. Delivery is bounded by the Agent mailbox and joins an
    /// active run only at a provider/tool protocol-safe boundary. When the
    /// Agent is idle, its host must supervise the mailbox wake signal and start
    /// a mailbox-driven run.
    pub fn notify_agent(&self, content: impl Into<Content>) -> Result<(), ToolError> {
        let notify = self.agent_notification.as_ref().ok_or_else(|| {
            ToolError::new("the owning Agent does not have a notification mailbox")
        })?;
        notify(content.into())
    }

    /// Returns whether the owning Agent installed a background notification
    /// mailbox for this invocation.
    pub fn can_notify_agent(&self) -> bool {
        self.agent_notification.is_some()
    }

    pub(crate) fn finish(&self) {
        self.progress_active.store(false, Ordering::Release);
        // Context clones may have been handed to helper tasks. Once the tool
        // invocation returns, cancel ordinary cooperative work and suppress
        // late progress. The bounded Agent notification reporter deliberately
        // remains usable by explicitly managed background work.
        self.cancellation.cancel();
    }
}

impl fmt::Debug for ToolExecutionContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolExecutionContext")
            .field("call_id", &self.call_id)
            .field("cancelled", &self.is_cancelled())
            .field("visible_tool_results", &self.visible_tool_results.len())
            .field("progress_enabled", &self.progress.is_some())
            .field("workspace", &self.workspace)
            .field("capability_mode", &self.capability_mode)
            .finish()
    }
}

impl From<String> for ToolProgress {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for ToolProgress {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
    /// Additional provider-neutral content blocks (for example images or
    /// documents). `content` remains the textual fallback and summary.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_parts: Vec<ContentPart>,
    /// Machine-readable result data retained in the transcript and daemon API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl ToolOutput {
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            content_parts: Vec::new(),
            metadata: None,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            content_parts: Vec::new(),
            metadata: None,
        }
    }

    pub fn with_content_part(mut self, part: ContentPart) -> Self {
        self.content_parts.push(part);
        self
    }

    pub fn with_content_parts(mut self, parts: impl IntoIterator<Item = ContentPart>) -> Self {
        self.content_parts.extend(parts);
        self
    }

    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    pub fn message_content(&self) -> Content {
        if self.content_parts.is_empty() {
            return Content::text(self.content.clone());
        }

        let mut parts = Vec::with_capacity(self.content_parts.len() + 1);
        if !self.content.is_empty() {
            parts.push(ContentPart::text(self.content.clone()));
        }
        parts.extend(self.content_parts.clone());
        Content::parts(parts)
    }

    pub fn into_message_parts(self) -> (Content, bool, Option<Value>) {
        let Self {
            content,
            is_error,
            content_parts,
            metadata,
        } = self;
        let content = if content_parts.is_empty() {
            Content::text(content)
        } else {
            let mut parts = Vec::with_capacity(content_parts.len() + 1);
            if !content.is_empty() {
                parts.push(ContentPart::text(content));
            }
            parts.extend(content_parts);
            Content::parts(parts)
        };
        (content, is_error, metadata)
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDefinition;

    /// Declares the tool's maximum possible side effect.
    ///
    /// The conservative default restricts unknown, custom, and MCP tools to
    /// [`CapabilityMode::FullAccess`] unless their implementation explicitly
    /// opts into a safer classification.
    fn effect(&self) -> ToolEffect {
        ToolEffect::ExternalSideEffect
    }

    /// Declares the maximum possible effect for one invocation.
    ///
    /// Definitions are filtered with [`Tool::effect`], while scheduling and
    /// execution use this argument-aware value.
    fn effect_for(&self, _arguments: &Value) -> ToolEffect {
        self.effect()
    }

    /// Classifies this invocation for in-batch scheduling.
    ///
    /// Tools such as a shell may conservatively inspect `arguments` and return
    /// [`ToolConcurrency::Safe`] only for commands proven read-only.
    fn concurrency(&self, _arguments: &Value) -> ToolConcurrency {
        match self.effect_for(_arguments) {
            ToolEffect::ReadOnly => ToolConcurrency::Safe,
            ToolEffect::Internal | ToolEffect::WorkspaceWrite | ToolEffect::ExternalSideEffect => {
                ToolConcurrency::Exclusive
            }
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError>;

    /// Executes with invocation-scoped cancellation, progress, and transcript
    /// visibility. Existing tools can continue implementing only `execute`.
    async fn execute_with_context(
        &self,
        arguments: Value,
        _context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        self.execute(arguments).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_modes_form_a_conservative_linear_boundary() {
        assert!(CapabilityMode::ReadOnly.is_subset_of(CapabilityMode::WorkspaceEdit));
        assert!(CapabilityMode::WorkspaceEdit.is_subset_of(CapabilityMode::FullAccess));
        assert!(!CapabilityMode::FullAccess.is_subset_of(CapabilityMode::WorkspaceEdit));

        assert!(CapabilityMode::ReadOnly.allows(ToolEffect::ReadOnly));
        assert!(!CapabilityMode::ReadOnly.allows(ToolEffect::WorkspaceWrite));
        assert!(CapabilityMode::WorkspaceEdit.allows(ToolEffect::WorkspaceWrite));
        assert!(!CapabilityMode::WorkspaceEdit.allows(ToolEffect::ExternalSideEffect));
        assert!(CapabilityMode::FullAccess.allows(ToolEffect::ExternalSideEffect));
    }

    #[test]
    fn mandatory_tools_bypass_name_policy_but_denies_win_for_ordinary_tools() {
        let policy = ToolPolicy::allow_only(["read", "write"]).with_denied_tools(["write"]);
        assert!(policy.allows("read", false));
        assert!(!policy.allows("write", false));
        assert!(!policy.allows("bash", false));
        assert!(policy.allows("bash", true));

        let serialized = serde_json::to_value(&policy).unwrap();
        assert_eq!(
            serde_json::from_value::<ToolPolicy>(serialized).unwrap(),
            policy
        );
    }
}

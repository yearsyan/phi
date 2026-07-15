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
    error::ToolError,
    types::{Content, ContentPart, ToolDefinition},
};

pub mod builtins;

/// The agent's execution mode.
///
/// Plan mode is a capability boundary, not just a prompting convention: only
/// read-only, internal, and plan-only tools are exposed to the provider or
/// allowed to execute.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    #[default]
    Default,
    Plan,
}

impl AgentMode {
    pub fn allows(self, effect: ToolEffect) -> bool {
        match self {
            Self::Default => effect != ToolEffect::PlanOnly,
            Self::Plan => matches!(
                effect,
                ToolEffect::ReadOnly | ToolEffect::Internal | ToolEffect::PlanOnly
            ),
        }
    }

    pub(crate) fn safest(self, other: Self) -> Self {
        if self == Self::Plan || other == Self::Plan {
            Self::Plan
        } else {
            Self::Default
        }
    }
}

/// A clonable, in-memory mode switch for mode-transition tools.
///
/// Changing this control is intentionally not a persistence operation. Code
/// that owns an [`crate::Agent`] should use `Agent::set_mode` so idle mode
/// changes are durable. A tool can use this control during execution; the
/// agent checkpoints and persists the resulting mode with the tool outcome.
#[derive(Clone, Debug)]
pub struct AgentModeControl {
    mode: watch::Sender<AgentMode>,
}

impl AgentModeControl {
    pub fn new(mode: AgentMode) -> Self {
        let (mode, _) = watch::channel(mode);
        Self { mode }
    }

    pub fn mode(&self) -> AgentMode {
        *self.mode.borrow()
    }

    pub fn set_mode(&self, mode: AgentMode) {
        self.mode.send_replace(mode);
    }

    pub(crate) fn restore_safely(&self, checkpoint: AgentMode) {
        self.set_mode(self.mode().safest(checkpoint));
    }
}

impl Default for AgentModeControl {
    fn default() -> Self {
        Self::new(AgentMode::default())
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
    /// A narrowly scoped tool that is available only while planning.
    PlanOnly,
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
}

impl ToolExecutionContext {
    pub fn detached(call_id: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            cancellation: ToolCancellation::new(),
            visible_tool_results: Arc::new(HashSet::new()),
            progress: None,
            progress_active: Arc::new(AtomicBool::new(true)),
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
        }
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

    pub fn report_progress(&self, progress: impl Into<ToolProgress>) {
        if !self.is_cancelled()
            && self.progress_active.load(Ordering::Acquire)
            && let Some(report) = &self.progress
        {
            report(progress.into());
        }
    }

    pub(crate) fn finish(&self) {
        self.progress_active.store(false, Ordering::Release);
        // Context clones may have been handed to helper tasks. Once the tool
        // invocation returns, tell those helpers to stop and suppress any
        // late progress events.
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
    /// The conservative default keeps unknown, custom, and MCP tools out of
    /// plan mode unless their implementation explicitly opts into a safer
    /// classification.
    fn effect(&self) -> ToolEffect {
        ToolEffect::ExternalSideEffect
    }

    /// Classifies this invocation for in-batch scheduling.
    ///
    /// Tools such as a shell may conservatively inspect `arguments` and return
    /// [`ToolConcurrency::Safe`] only for commands proven read-only.
    fn concurrency(&self, _arguments: &Value) -> ToolConcurrency {
        match self.effect() {
            ToolEffect::ReadOnly => ToolConcurrency::Safe,
            ToolEffect::Internal
            | ToolEffect::PlanOnly
            | ToolEffect::WorkspaceWrite
            | ToolEffect::ExternalSideEffect => ToolConcurrency::Exclusive,
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

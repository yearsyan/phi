use async_trait::async_trait;

use crate::{
    error::ContextCompactionError,
    provider::LlmProvider,
    types::{ContextUsage, GenerationConfig, Message, TokenUsage},
};

mod default_compactor;

pub(crate) use default_compactor::estimate_messages_tokens;
pub use default_compactor::{
    DEFAULT_CONTEXT_COMPACTION_BOUNDARY_MESSAGE, DEFAULT_CONTEXT_COMPACTION_BUFFER_TOKENS,
    DEFAULT_CONTEXT_COMPACTION_MAX_RETRIES, DEFAULT_CONTEXT_COMPACTION_MAX_SUMMARY_TOKENS,
    DefaultContextCompactor, default_context_compaction_threshold,
};

/// Why context compaction was activated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextCompactionTrigger {
    /// The selected compactor is evaluating usage before an LLM request. Its
    /// strategy decides the exact threshold (the default implementation uses
    /// fixed token headroom rather than a percentage).
    Automatic { usage: ContextUsage },
    /// Context compaction was explicitly requested by the Agent owner.
    Manual {
        /// Optional instructions appended to the compaction prompt.
        instructions: Option<String>,
    },
    /// The provider rejected a request because its context window was exceeded
    /// before emitting any assistant output.
    ContextLengthExceeded { error: String },
}

impl ContextCompactionTrigger {
    pub fn is_automatic(&self) -> bool {
        !matches!(self, Self::Manual { .. })
    }

    pub fn instructions(&self) -> Option<&str> {
        match self {
            Self::Manual { instructions } => instructions.as_deref(),
            Self::Automatic { .. } | Self::ContextLengthExceeded { .. } => None,
        }
    }
}

/// Immutable input used by a selected context-compaction implementation.
///
/// The request owns a transcript snapshot. Implementations return a complete
/// replacement plan, which the Agent validates and persists atomically only
/// after the compaction model call succeeds.
#[derive(Clone, Debug)]
pub struct ContextCompactionRequest {
    pub trigger: ContextCompactionTrigger,
    pub system_prompt: String,
    pub messages: Vec<Message>,
    pub max_context_tokens: Option<u64>,
    pub generation_config: GenerationConfig,
}

impl ContextCompactionRequest {
    pub fn context_usage(&self) -> Option<ContextUsage> {
        match &self.trigger {
            ContextCompactionTrigger::Automatic { usage } => Some(*usage),
            ContextCompactionTrigger::Manual { .. }
            | ContextCompactionTrigger::ContextLengthExceeded { .. } => None,
        }
    }
}

/// A fully generated, not-yet-applied transcript replacement.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextCompactionPlan {
    pub messages: Vec<Message>,
    pub summary: String,
    pub usage: Option<TokenUsage>,
    pub estimated_context_tokens: u64,
}

/// Result of an atomically applied context compaction.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextCompactionOutcome {
    pub compactor: String,
    pub trigger: ContextCompactionTrigger,
    pub before_message_count: usize,
    pub after_message_count: usize,
    pub changed_from: usize,
    pub replacement: Vec<Message>,
    pub summary: String,
    pub usage: Option<TokenUsage>,
    pub estimated_context_tokens: u64,
}

/// Outcome of a cancellable, explicitly requested compaction.
#[derive(Clone, Debug, PartialEq)]
pub enum ContextCompactionRunOutcome {
    Completed(ContextCompactionOutcome),
    Stopped,
}

/// One selectable context-compaction strategy.
///
/// One compactor is selected per Agent. The Agent owner may replace it while
/// the Agent is idle, allowing session-scoped policy without global state.
#[async_trait]
pub trait ContextCompactor: Send + Sync {
    /// Stable implementation name exposed in progress events.
    fn name(&self) -> &'static str;

    /// Decides whether an automatic usage observation requires compaction.
    /// Manual and overflow requests should normally return `true`.
    fn should_compact(&self, request: &ContextCompactionRequest) -> bool;

    /// Returns the exact prompt that will be sent to the compaction model.
    /// The Agent emits this before awaiting [`ContextCompactor::compact`].
    fn prompt(&self, request: &ContextCompactionRequest) -> String;

    /// Generates a replacement plan using the Agent's provider.
    ///
    /// Implementations must not mutate live Agent state. They may issue
    /// provider requests, but must not recursively invoke the Agent runtime.
    async fn compact(
        &self,
        provider: &dyn LlmProvider,
        request: ContextCompactionRequest,
        prompt: String,
    ) -> Result<ContextCompactionPlan, ContextCompactionError>;
}

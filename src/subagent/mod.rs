//! Long-lived child-agent coordination.
//!
//! A [`SubagentRuntime`] is scoped to one parent agent/session. It owns child
//! lifecycles independently from any individual tool invocation. The
//! model-facing `spawn_agent` tool chooses foreground waiting or background
//! delivery, while completed general children remain resumable from their
//! sidechain transcript. Model-facing tools live in [`crate::tool::subagent`].

mod runtime;

pub use runtime::{
    ActiveSubagentRun, BuiltSubagent, CloseSubagentResult, ConfiguredSpawnAgentRequest,
    ConfiguredSubagentBuildRequest, DEFAULT_MAX_SUBAGENT_MESSAGE_BYTES, DEFAULT_MAX_SUBAGENTS,
    DEFAULT_SUBAGENT_EVENT_CAPACITY, DEFAULT_SUBAGENT_MAILBOX_CAPACITY, EffectiveSubagentConfig,
    MAX_SUBAGENT_OUTPUT_FIELD_BYTES, MAX_SUBAGENT_OUTPUT_FIELDS, QueuedSubagentMessage,
    SpawnAgentRequest, SpawnedSubagent, SubagentBuildRequest, SubagentCompletion, SubagentConfig,
    SubagentError, SubagentEvent, SubagentEventKind, SubagentFactory, SubagentFactoryError,
    SubagentIsolation, SubagentNotification, SubagentNotificationKind, SubagentNotificationSource,
    SubagentOutputContract, SubagentResource, SubagentResourceDisposition,
    SubagentResourceFinalization, SubagentResourceInfo, SubagentRestoreFailure,
    SubagentRestoreReport, SubagentRunOutcome, SubagentRuntime, SubagentSnapshot, SubagentState,
    SubagentType, ValidatedSubagentOutput,
};

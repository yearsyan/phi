//! Long-lived child-agent coordination.
//!
//! A [`SubagentRuntime`] is scoped to one parent agent/session. It owns child
//! lifecycles independently from any individual tool invocation, so returning
//! from `spawn_agent` does not cancel the child. Model-facing tools live in
//! [`crate::tool::subagent`].

mod runtime;

pub use runtime::{
    ActiveSubagentRun, BuiltSubagent, CloseSubagentResult, ConfiguredSpawnAgentRequest,
    ConfiguredSubagentBuildRequest, DEFAULT_MAX_SUBAGENT_MESSAGE_BYTES, DEFAULT_MAX_SUBAGENTS,
    DEFAULT_SUBAGENT_EVENT_CAPACITY, DEFAULT_SUBAGENT_MAILBOX_CAPACITY, EffectiveSubagentConfig,
    MAX_SUBAGENT_OUTPUT_FIELD_BYTES, MAX_SUBAGENT_OUTPUT_FIELDS, QueuedSubagentMessage,
    SpawnAgentRequest, SpawnedSubagent, SubagentBuildRequest, SubagentConfig, SubagentError,
    SubagentEvent, SubagentEventKind, SubagentFactory, SubagentFactoryError, SubagentIsolation,
    SubagentNotification, SubagentNotificationKind, SubagentNotificationSource,
    SubagentOutputContract, SubagentResource, SubagentResourceDisposition,
    SubagentResourceFinalization, SubagentResourceInfo, SubagentRunOutcome, SubagentRuntime,
    SubagentSnapshot, SubagentState, SubagentType, ValidatedSubagentOutput,
};

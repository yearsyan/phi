//! Long-lived child-agent coordination.
//!
//! A [`SubagentRuntime`] is scoped to one parent agent/session. It owns child
//! lifecycles independently from any individual tool invocation, so returning
//! from `spawn_agent` does not cancel the child. Model-facing tools live in
//! [`crate::tool::subagent`].

mod runtime;

pub use runtime::{
    ActiveSubagentRun, CloseSubagentResult, DEFAULT_MAX_SUBAGENT_MESSAGE_BYTES,
    DEFAULT_MAX_SUBAGENTS, DEFAULT_SUBAGENT_EVENT_CAPACITY, DEFAULT_SUBAGENT_MAILBOX_CAPACITY,
    QueuedSubagentMessage, SpawnAgentRequest, SpawnedSubagent, SubagentBuildRequest,
    SubagentConfig, SubagentError, SubagentEvent, SubagentEventKind, SubagentFactory,
    SubagentFactoryError, SubagentNotification, SubagentNotificationKind,
    SubagentNotificationSource, SubagentRunOutcome, SubagentRuntime, SubagentSnapshot,
    SubagentState,
};

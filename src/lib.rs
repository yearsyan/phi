//! A small, event-driven Rust agent SDK inspired by Pi's `agent-core`.

pub mod agent;
pub mod context;
pub mod error;
pub mod hook;
pub mod mcp;
pub mod plan;
pub mod provider;
pub mod skills;
pub mod storage;
pub mod subagent;
pub mod tool;
pub mod types;

pub use agent::{
    Agent, AgentBuilder, AgentMailbox, AgentMailboxDelivery, AgentMailboxSendError,
    AgentMailboxSender, AgentRunControl, DEFAULT_AGENT_MAILBOX_CAPACITY,
    DEFAULT_MAX_PARALLEL_TOOLS, DEFAULT_TOOL_CALL_TIMEOUT,
};
pub use context::{
    ContextCompactionOutcome, ContextCompactionPlan, ContextCompactionRequest,
    ContextCompactionRunOutcome, ContextCompactionTrigger, ContextCompactor,
    DEFAULT_CONTEXT_COMPACTION_BUFFER_TOKENS, DEFAULT_CONTEXT_COMPACTION_MAX_RETRIES,
    DEFAULT_CONTEXT_COMPACTION_MAX_SUMMARY_TOKENS, DefaultContextCompactor,
    default_context_compaction_threshold,
};
pub use error::{
    AgentError, ContextCompactionError, HookError, McpError, ProviderError, ToolError,
};
pub use hook::{
    BeforeRequestContext, Hook, HookRegistry, LlmResponseContext, ProviderApi, TurnEndContext,
    TurnStartContext,
};
pub use mcp::{
    DEFAULT_MCP_CONNECT_TIMEOUT, DEFAULT_MCP_REQUEST_TIMEOUT, McpClient, McpClientOptions,
    McpHttpConfig, McpServerInfo, McpStdioConfig,
};
pub use plan::{
    DiskPlanStore, EMPTY_PLAN_REVISION, InMemoryPlanStore, LockedPlan, MAX_PLAN_BYTES,
    PlanArtifact, PlanStore, PlanStoreError,
};
pub use provider::{
    AnthropicMessagesProvider, DEFAULT_MAX_RETRIES, DEFAULT_REQUEST_TIMEOUT,
    DEFAULT_STREAM_IDLE_TIMEOUT, LlmProvider, OpenAiChatProvider, OpenAiResponsesProvider,
    ProviderEventStream, RetryConfig,
};
pub use skills::{
    DEFAULT_MAX_SKILL_BYTES, DEFAULT_MAX_SKILLS, DEFAULT_SKILL_LISTING_BUDGET, DiagnosticLevel,
    DuplicateSkillPolicy, RenderedSkill, SkillCatalog, SkillDiagnostic, SkillDirectory, SkillError,
    SkillInvocation, SkillMetadata, SkillTool, SkillsConfig,
};
pub use storage::{
    DiskSessionStorage, InMemorySessionStorage, SessionSnapshot, SessionStorage, StorageError,
};
pub use subagent::{
    ActiveSubagentRun, CloseSubagentResult, DEFAULT_MAX_SUBAGENT_MESSAGE_BYTES,
    DEFAULT_MAX_SUBAGENTS, DEFAULT_SUBAGENT_EVENT_CAPACITY, DEFAULT_SUBAGENT_MAILBOX_CAPACITY,
    QueuedSubagentMessage, SpawnAgentRequest, SpawnedSubagent, SubagentBuildRequest,
    SubagentConfig, SubagentError, SubagentEvent, SubagentEventKind, SubagentFactory,
    SubagentFactoryError, SubagentNotification, SubagentNotificationKind,
    SubagentNotificationSource, SubagentRunOutcome, SubagentRuntime, SubagentSnapshot,
    SubagentState,
};
pub use tool::builtins::{
    BashTool, BuiltinTools, DEFAULT_BASH_TIMEOUT, DEFAULT_MAX_BYTES, DEFAULT_MAX_EDIT_BYTES,
    DEFAULT_MAX_LINES, EditTool, ReadTool, WriteTool,
};
pub use tool::subagent::{
    CLOSE_AGENT_TOOL_NAME, CloseAgentTool, NOTIFY_PARENT_TOOL_NAME, NotifyParentTool,
    SEND_AGENT_MESSAGE_TOOL_NAME, SPAWN_AGENT_TOOL_NAME, SendAgentMessageTool, SpawnAgentTool,
    SubagentTools,
};
pub use tool::{
    AgentMode, AgentModeControl, Tool, ToolCancellation, ToolConcurrency, ToolEffect,
    ToolExecutionContext, ToolOutput, ToolProgress,
};
pub use types::{
    AgentEvent, AgentRun, AgentRunOutcome, AssistantDelta, AssistantMessage, Content, ContentPart,
    ContextUsage, Document, GenerationConfig, ImageDetail, ImageUrl, Message,
    ParseReasoningEffortError, ProviderEvent, ProviderRequest, ProviderResponse,
    ProviderRetryEvent, ProviderRetryReason, ProviderState, ReasoningEffort, Role, TokenUsage,
    ToolCall, ToolDefinition, ToolExecutionMode,
};

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
pub mod tool;
pub mod types;

pub use agent::{
    Agent, AgentBuilder, AgentRunControl, DEFAULT_MAX_PARALLEL_TOOLS, DEFAULT_TOOL_CALL_TIMEOUT,
};
pub use context::{
    ContextManagementContext, ContextManagementTrigger, ContextManager, ContextManagerRegistry,
    DEFAULT_CONTEXT_MANAGEMENT_THRESHOLD_PERCENT,
};
pub use error::{AgentError, HookError, McpError, ProviderError, ToolError};
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
pub use tool::builtins::{
    BashTool, BuiltinTools, DEFAULT_BASH_TIMEOUT, DEFAULT_MAX_BYTES, DEFAULT_MAX_EDIT_BYTES,
    DEFAULT_MAX_LINES, EditTool, ReadTool, WriteTool,
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

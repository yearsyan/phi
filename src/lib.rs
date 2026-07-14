//! A small, event-driven Rust agent SDK inspired by Pi's `agent-core`.

pub mod agent;
pub mod error;
pub mod hook;
pub mod mcp;
pub mod provider;
pub mod storage;
pub mod tool;
pub mod types;

pub use agent::{Agent, AgentBuilder};
pub use error::{AgentError, HookError, McpError, ProviderError, ToolError};
pub use hook::{
    BeforeRequestContext, Hook, HookRegistry, LlmResponseContext, ProviderApi, TurnEndContext,
    TurnStartContext,
};
pub use mcp::{
    DEFAULT_MCP_CONNECT_TIMEOUT, DEFAULT_MCP_REQUEST_TIMEOUT, McpClient, McpClientOptions,
    McpHttpConfig, McpServerInfo, McpStdioConfig,
};
pub use provider::{
    AnthropicMessagesProvider, DEFAULT_MAX_RETRIES, LlmProvider, OpenAiChatProvider,
    OpenAiResponsesProvider, ProviderEventStream, RetryConfig,
};
pub use storage::{
    DiskSessionStorage, InMemorySessionStorage, SessionSnapshot, SessionStorage, StorageError,
};
pub use tool::builtins::{
    BashTool, BuiltinTools, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, EditTool, ReadTool, WriteTool,
};
pub use tool::{Tool, ToolOutput};
pub use types::{
    AgentEvent, AgentRun, AssistantDelta, AssistantMessage, Content, ContentPart, ContextUsage,
    GenerationConfig, ImageDetail, ImageUrl, Message, ProviderEvent, ProviderRequest,
    ProviderResponse, ProviderRetryEvent, ProviderRetryReason, ProviderState, ReasoningEffort,
    Role, TokenUsage, ToolCall, ToolDefinition, ToolExecutionMode,
};

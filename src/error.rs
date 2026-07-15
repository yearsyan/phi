use std::time::Duration;

use reqwest::StatusCode;
use thiserror::Error;

use crate::storage::StorageError;

#[derive(Debug, Error)]
#[error("{message}")]
pub struct HookError {
    message: String,
}

impl HookError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Debug, Error)]
pub enum McpError {
    #[error("failed to start MCP stdio server: {0}")]
    Spawn(#[source] std::io::Error),

    #[error("MCP connection timed out after {timeout:?}")]
    ConnectTimeout { timeout: Duration },

    #[error("MCP initialization failed: {message}")]
    Initialize { message: String },

    #[error("MCP server did not provide initialization metadata")]
    MissingServerInfo,

    #[error("MCP {operation} timed out after {timeout:?}")]
    RequestTimeout {
        operation: &'static str,
        timeout: Duration,
    },

    #[error("MCP {operation} failed: {message}")]
    Request {
        operation: &'static str,
        message: String,
    },

    #[error("invalid MCP HTTP header `{name}`: {message}")]
    InvalidHttpHeader { name: String, message: String },

    #[error("MCP server exposed duplicate tool name `{0}`")]
    DuplicateToolName(String),

    #[error("failed to close MCP connection: {0}")]
    Close(String),
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("API key is empty")]
    MissingApiKey,

    #[error("invalid provider configuration: {0}")]
    InvalidConfiguration(String),

    #[error("request cannot be represented by this provider: {0}")]
    InvalidRequest(String),

    #[error("provider HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("provider HTTP request timed out after {timeout:?}")]
    RequestTimeout { timeout: Duration },

    #[error("provider stream failed: {0}")]
    Stream(String),

    #[error("provider stream was idle for {timeout:?}")]
    StreamIdleTimeout { timeout: Duration },

    #[error("provider API returned {status}: {body}")]
    Api { status: StatusCode, body: String },

    /// The provider rejected a request because the model context window was
    /// exceeded. Custom providers should return this variant when they can
    /// classify the condition without relying on message matching.
    #[error("provider context length exceeded: {message}")]
    ContextLengthExceeded { message: String },

    #[error("invalid provider response: {0}")]
    InvalidResponse(String),

    #[error("provider hook failed: {0}")]
    Hook(#[from] HookError),
}

impl ProviderError {
    pub fn context_length_exceeded(message: impl Into<String>) -> Self {
        Self::ContextLengthExceeded {
            message: message.into(),
        }
    }

    /// Returns whether this error represents a provider context-window
    /// rejection. Explicitly typed errors are preferred; API and stream error
    /// bodies are also recognized for compatibility with common gateways.
    pub fn is_context_length_exceeded(&self) -> bool {
        match self {
            Self::ContextLengthExceeded { .. } => true,
            Self::Api { body, .. }
            | Self::InvalidRequest(body)
            | Self::Stream(body)
            | Self::InvalidResponse(body) => looks_like_context_length_error(body),
            Self::MissingApiKey
            | Self::InvalidConfiguration(_)
            | Self::Http(_)
            | Self::RequestTimeout { .. }
            | Self::StreamIdleTimeout { .. }
            | Self::Hook(_) => false,
        }
    }

    pub(crate) fn from_api_response(status: StatusCode, body: String) -> Self {
        if looks_like_context_length_error(&body) {
            Self::context_length_exceeded(format!("HTTP {status}: {body}"))
        } else {
            Self::Api { status, body }
        }
    }

    pub(crate) fn from_stream_response_error(message: String) -> Self {
        if looks_like_context_length_error(&message) {
            Self::context_length_exceeded(message)
        } else {
            Self::InvalidResponse(message)
        }
    }
}

fn looks_like_context_length_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "context_length_exceeded",
        "context_window_exceeded",
        "context length exceeded",
        "maximum context length",
        "context window exceeded",
        "exceeds the context window",
        "exceeded the context window",
        "prompt is too long",
        "prompt_too_long",
        "input is too long",
        "input_too_long",
        "input tokens exceed",
        "too many tokens for",
    ]
    .iter()
    .any(|pattern| message.contains(pattern))
}

#[derive(Debug, Error)]
#[error("{message}")]
pub struct ToolError {
    message: String,
}

impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Debug, Error)]
pub enum AgentError {
    #[error(transparent)]
    Provider(#[from] ProviderError),

    #[error(transparent)]
    Storage(#[from] StorageError),

    #[error("agent hook failed: {0}")]
    Hook(#[from] HookError),
}

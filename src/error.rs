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

    #[error("provider API returned {status}: {body}")]
    Api { status: StatusCode, body: String },

    #[error("invalid provider response: {0}")]
    InvalidResponse(String),

    #[error("provider hook failed: {0}")]
    Hook(#[from] HookError),
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

    #[error("agent exceeded its limit of {max_turns} turns")]
    MaxTurnsExceeded { max_turns: usize },

    #[error("agent hook failed: {0}")]
    Hook(#[from] HookError),
}

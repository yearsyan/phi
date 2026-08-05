use std::time::Duration;

use reqwest::StatusCode;
use thiserror::Error;

use crate::{storage::StorageError, types::ProviderState};

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
#[error("{message}")]
pub struct ContextCompactionError {
    message: String,
}

impl ContextCompactionError {
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

    #[error("MCP client has no captured configuration and cannot be reconnected")]
    NoReconnectSpec,

    #[error(
        "MCP server `{server}` changed its tool set after reconnect (was {previous}, now {current})"
    )]
    ToolSetChanged {
        server: String,
        previous: usize,
        current: usize,
    },
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

    /// The provider completed a protocol-complete response whose tool-call
    /// arguments are not valid JSON. The call identifiers and replay state
    /// are preserved so the agent can pair the rejected call with a
    /// synthetic error result and ask the model to re-emit it instead of
    /// failing the run.
    #[error(
        "invalid tool arguments for `{name}`: {parse_error}",
        name = .0.name,
        parse_error = .0.parse_error
    )]
    InvalidToolArguments(Box<InvalidToolCallDetails>),

    #[error("provider hook failed: {0}")]
    Hook(#[from] HookError),
}

/// A tool call whose streamed arguments could not be parsed as JSON. The
/// provider's response was protocol-complete, so no tool ran and nothing
/// about the call entered the durable transcript yet.
#[derive(Debug)]
pub struct InvalidToolCallDetails {
    /// Provider-issued tool call id needed for tool-result pairing.
    pub id: String,
    pub name: String,
    pub parse_error: String,
    /// Truncated malformed payload kept for diagnostics and model feedback.
    /// Never contains more than a few hundred characters.
    pub raw_arguments: String,
    pub provider_state: Option<ProviderState>,
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
            | Self::InvalidToolArguments { .. }
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

    /// Builds an [`ProviderError::InvalidToolArguments`] with the malformed
    /// payload truncated to a bounded, UTF-8-safe diagnostic snippet.
    pub(crate) fn invalid_tool_arguments(
        id: impl Into<String>,
        name: impl Into<String>,
        raw_arguments: &str,
        parse_error: impl Into<String>,
        provider_state: Option<ProviderState>,
    ) -> Self {
        Self::InvalidToolArguments(Box::new(InvalidToolCallDetails {
            id: id.into(),
            name: name.into(),
            parse_error: parse_error.into(),
            raw_arguments: truncate_raw_arguments(raw_arguments),
            provider_state,
        }))
    }
}

const MAX_RAW_ARGUMENT_SNIPPET_CHARS: usize = 500;

fn truncate_raw_arguments(raw_arguments: &str) -> String {
    if raw_arguments.chars().count() <= MAX_RAW_ARGUMENT_SNIPPET_CHARS {
        return raw_arguments.to_owned();
    }
    let mut truncated: String = raw_arguments
        .chars()
        .take(MAX_RAW_ARGUMENT_SNIPPET_CHARS.saturating_sub(1))
        .collect();
    truncated.push('…');
    truncated
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

    #[error("context compaction failed: {0}")]
    ContextCompaction(#[from] ContextCompactionError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_tool_arguments_truncates_long_raw_payloads() {
        let raw = "x".repeat(1_000);
        let error =
            ProviderError::invalid_tool_arguments("id-1", "tool", &raw, "parse error", None);
        let ProviderError::InvalidToolArguments(details) = &error else {
            panic!("expected InvalidToolArguments");
        };
        assert_eq!(
            details.raw_arguments.chars().count(),
            MAX_RAW_ARGUMENT_SNIPPET_CHARS
        );
        assert!(details.raw_arguments.ends_with('…'));
        // The Display used for logs and run-failure events never includes the
        // malformed payload itself.
        assert!(!error.to_string().contains("xxxx"));
        assert_eq!(
            error.to_string(),
            "invalid tool arguments for `tool`: parse error"
        );
    }

    #[test]
    fn invalid_tool_arguments_is_never_a_context_length_error() {
        let error = ProviderError::invalid_tool_arguments(
            "id-1",
            "tool",
            "{}",
            "unexpected token: maximum context length reached",
            None,
        );
        assert!(!error.is_context_length_exceeded());
    }
}

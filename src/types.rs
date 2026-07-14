use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, fmt, ops::AddAssign, time::Duration};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// Provider-neutral text or multimodal message content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Content {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl Content {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    pub fn parts(parts: impl IntoIterator<Item = ContentPart>) -> Self {
        Self::Parts(parts.into_iter().collect())
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Parts(parts) => parts.iter().find_map(ContentPart::as_text),
        }
    }

    pub fn into_text(self) -> Option<String> {
        match self {
            Self::Text(text) => Some(text),
            Self::Parts(parts) => {
                let text = parts
                    .into_iter()
                    .filter_map(ContentPart::into_text)
                    .collect::<Vec<_>>()
                    .join("");
                (!text.is_empty()).then_some(text)
            }
        }
    }
}

impl From<String> for Content {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for Content {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
}

impl ContentPart {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    pub fn image_url(url: impl Into<String>) -> Self {
        Self::image(ImageUrl::new(url))
    }

    pub fn image(image_url: ImageUrl) -> Self {
        Self::ImageUrl { image_url }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            Self::ImageUrl { .. } => None,
        }
    }

    fn into_text(self) -> Option<String> {
        match self {
            Self::Text { text } => Some(text),
            Self::ImageUrl { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
    pub detail: Option<ImageDetail>,
}

impl ImageUrl {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: ImageDetail) -> Self {
        self.detail = Some(detail);
        self
    }

    pub fn from_bytes(mime_type: &str, bytes: &[u8]) -> Self {
        Self::new(format!(
            "data:{mime_type};base64,{}",
            STANDARD.encode(bytes)
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageDetail {
    Auto,
    Low,
    High,
}

impl ImageDetail {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Low => "low",
            Self::High => "high",
        }
    }
}

/// A provider-neutral conversation message retained in agent state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Option<Content>,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
    pub tool_result_is_error: bool,
    /// Opaque response state that must be replayed unmodified to the same API
    /// adapter. Applications should retain this value when copying messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_state: Option<ProviderState>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self::text(Role::System, content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::text(Role::User, content)
    }

    pub fn user_content(content: Content) -> Self {
        Self {
            role: Role::User,
            content: Some(content),
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_result_is_error: false,
            provider_state: None,
        }
    }

    pub fn user_parts(parts: impl IntoIterator<Item = ContentPart>) -> Self {
        Self::user_content(Content::parts(parts))
    }

    pub fn assistant(content: Option<Content>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content,
            tool_calls,
            tool_call_id: None,
            tool_result_is_error: false,
            provider_state: None,
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::tool_result(tool_call_id, content, false)
    }

    pub fn tool_result(
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            role: Role::Tool,
            content: Some(Content::text(content)),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            tool_result_is_error: is_error,
            provider_state: None,
        }
    }

    pub fn text_content(&self) -> Option<&str> {
        self.content.as_ref().and_then(Content::as_text)
    }

    fn text(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: Some(Content::text(content)),
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_result_is_error: false,
            provider_state: None,
        }
    }
}

/// Provider-specific assistant response data needed for lossless multi-turn
/// replay. The payload is serialized with sessions but redacted from `Debug`
/// output so reasoning text is not printed accidentally.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum ProviderState {
    OpenAiChat { fields: BTreeMap<String, Value> },
    OpenAiResponses { output: Vec<Value> },
    AnthropicMessages { content: Vec<Value> },
}

impl fmt::Debug for ProviderState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpenAiChat { fields } => formatter
                .debug_struct("OpenAiChat")
                .field("fields", &fields.keys().collect::<Vec<_>>())
                .finish(),
            Self::OpenAiResponses { output } => formatter
                .debug_struct("OpenAiResponses")
                .field("output_items", &output.len())
                .finish(),
            Self::AnthropicMessages { content } => formatter
                .debug_struct("AnthropicMessages")
                .field("content_blocks", &content.len())
                .finish(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

impl ToolCall {
    pub fn new(id: impl Into<String>, name: impl Into<String>, arguments: Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AssistantMessage {
    pub content: Option<Content>,
    pub tool_calls: Vec<ToolCall>,
    pub provider_state: Option<ProviderState>,
}

/// Normalized token accounting returned by a provider adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Total input context processed, including cached input tokens.
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    /// Cached tokens are included in `input_tokens` and exposed as a breakdown.
    pub cached_input_tokens: u64,
}

impl TokenUsage {
    pub fn new(input_tokens: u64, output_tokens: u64, cached_input_tokens: u64) -> Self {
        Self {
            input_tokens,
            output_tokens,
            total_tokens: input_tokens.saturating_add(output_tokens),
            cached_input_tokens,
        }
    }

    pub fn with_total(
        input_tokens: u64,
        output_tokens: u64,
        total_tokens: u64,
        cached_input_tokens: u64,
    ) -> Self {
        Self {
            input_tokens,
            output_tokens,
            total_tokens,
            cached_input_tokens,
        }
    }
}

impl AddAssign for TokenUsage {
    fn add_assign(&mut self, rhs: Self) {
        self.input_tokens = self.input_tokens.saturating_add(rhs.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(rhs.output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(rhs.total_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(rhs.cached_input_tokens);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderResponse {
    pub message: AssistantMessage,
    /// `None` is allowed for compatible gateways that omit usage data.
    pub usage: Option<TokenUsage>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssistantDelta {
    Text {
        delta: String,
    },
    ToolCall {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments_delta: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProviderEvent {
    Delta(AssistantDelta),
    Retry(ProviderRetryEvent),
    Done(ProviderResponse),
}

/// A failed provider HTTP attempt that will be retried after `delay`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderRetryEvent {
    /// One-based retry number about to be attempted.
    pub retry_number: usize,
    pub max_retries: usize,
    pub delay: Duration,
    pub reason: ProviderRetryReason,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderRetryReason {
    RequestTimeout { timeout: Duration },
    Transport { message: String },
    HttpStatus { status: u16, body: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextUsage {
    pub max_tokens: u64,
    pub used_tokens: u64,
    pub remaining_tokens: u64,
}

impl ContextUsage {
    pub fn from_usage(max_tokens: u64, usage: TokenUsage) -> Self {
        Self {
            max_tokens,
            used_tokens: usage.total_tokens,
            remaining_tokens: max_tokens.saturating_sub(usage.total_tokens),
        }
    }
}

impl AssistantMessage {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: Some(Content::text(content)),
            tool_calls: Vec::new(),
            provider_state: None,
        }
    }

    pub fn tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            content: None,
            tool_calls,
            provider_state: None,
        }
    }

    pub fn into_message(self) -> Message {
        Message {
            role: Role::Assistant,
            content: self.content,
            tool_calls: self.tool_calls,
            tool_call_id: None,
            tool_result_is_error: false,
            provider_state: self.provider_state,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

impl ToolDefinition {
    pub fn new(name: impl Into<String>, description: impl Into<String>, parameters: Value) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }
}

/// Generation settings understood by the agent core and mapped by each adapter.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GenerationConfig {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub reasoning_effort: Option<ReasoningEffort>,
}

/// Provider-neutral reasoning intensity.
///
/// Provider and model support varies. Adapters map levels that their target
/// protocol cannot represent to the closest supported intensity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// The complete provider-neutral boundary between the agent and an API adapter.
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderRequest {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub config: GenerationConfig,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolExecutionMode {
    Sequential,
    #[default]
    Parallel,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgentEvent {
    AgentStart,
    AgentEnd {
        messages: Vec<Message>,
    },
    TurnStart {
        turn: usize,
    },
    TurnEnd {
        turn: usize,
        message: Message,
        tool_results: Vec<Message>,
    },
    MessageStart {
        message: Message,
    },
    MessageEnd {
        message: Message,
    },
    MessageUpdate {
        delta: AssistantDelta,
    },
    ToolExecutionStart {
        call: ToolCall,
    },
    ToolExecutionEnd {
        call: ToolCall,
        content: String,
        is_error: bool,
    },
    UsageUpdate {
        usage: TokenUsage,
        context_usage: Option<ContextUsage>,
    },
    ProviderRetry {
        event: ProviderRetryEvent,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentRun {
    pub final_message: Message,
    pub new_messages: Vec<Message>,
    pub turns: usize,
    /// Sum of all provider requests made during this run (billing-oriented).
    pub run_usage: TokenUsage,
    /// Current conversation occupancy measured by the final provider response.
    pub context_usage: Option<ContextUsage>,
}

impl AgentRun {
    pub fn text(&self) -> Option<&str> {
        self.final_message.text_content()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_reasoning_effort_wire_names() {
        let cases = [
            (ReasoningEffort::None, "none"),
            (ReasoningEffort::Minimal, "minimal"),
            (ReasoningEffort::Low, "low"),
            (ReasoningEffort::Medium, "medium"),
            (ReasoningEffort::High, "high"),
            (ReasoningEffort::XHigh, "xhigh"),
            (ReasoningEffort::Max, "max"),
        ];

        for (effort, expected) in cases {
            assert_eq!(effort.as_str(), expected);
        }
    }

    #[test]
    fn retains_provider_neutral_multimodal_content() {
        let message = Message::user_parts([
            ContentPart::text("Describe this image"),
            ContentPart::image(
                ImageUrl::new("https://example.com/cat.png").with_detail(ImageDetail::High),
            ),
        ]);

        assert_eq!(message.text_content(), Some("Describe this image"));
        let Content::Parts(parts) = message.content.unwrap() else {
            panic!("expected multimodal parts");
        };
        assert!(matches!(parts[1], ContentPart::ImageUrl { .. }));
    }

    #[test]
    fn creates_a_base64_data_url() {
        let image = ImageUrl::from_bytes("image/png", &[1, 2, 3]);
        assert_eq!(image.url, "data:image/png;base64,AQID");
    }

    #[test]
    fn context_remaining_saturates_at_zero() {
        let context = ContextUsage::from_usage(100, TokenUsage::new(120, 10, 0));
        assert_eq!(context.used_tokens, 130);
        assert_eq!(context.remaining_tokens, 0);
    }

    #[test]
    fn provider_state_round_trips_without_exposing_reasoning_in_debug_output() {
        let mut message = Message::assistant(
            None,
            vec![ToolCall::new("tool-1", "echo", serde_json::json!({}))],
        );
        message.provider_state = Some(ProviderState::AnthropicMessages {
            content: vec![serde_json::json!({
                "type": "thinking",
                "thinking": "private reasoning",
                "signature": "signature-1"
            })],
        });

        let encoded = serde_json::to_string(&message).unwrap();
        let decoded: Message = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, message);
        assert!(encoded.contains("private reasoning"));
        assert!(!format!("{message:?}").contains("private reasoning"));
    }
}

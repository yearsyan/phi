mod anthropic;
mod openai_chat;
mod openai_responses;
mod retry;

use std::pin::Pin;

use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Map, Value};

use crate::{
    error::ProviderError,
    hook::HookRegistry,
    types::{Content, ContentPart, Message, ProviderEvent, ProviderRequest, ProviderResponse},
};

pub use anthropic::AnthropicMessagesProvider;
pub use openai_chat::OpenAiChatProvider;
pub use openai_responses::OpenAiResponsesProvider;
pub use retry::{DEFAULT_MAX_RETRIES, RetryConfig};
pub(crate) use retry::{HttpRequestEvent, send_with_retry};

pub(crate) type ExtraBody = Map<String, Value>;

pub(crate) fn json_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers
}

pub(crate) fn header_value(name: &str, value: &str) -> Result<HeaderValue, ProviderError> {
    HeaderValue::from_str(value).map_err(|error| {
        ProviderError::InvalidConfiguration(format!(
            "invalid value for HTTP header `{name}`: {error}"
        ))
    })
}

/// Validates the user-supplied JSON members that are appended to each provider
/// request body.
pub(crate) fn parse_extra_body(extra_body: Value) -> Result<ExtraBody, ProviderError> {
    match extra_body {
        Value::Object(extra_body) => Ok(extra_body),
        _ => Err(ProviderError::InvalidConfiguration(
            "extra request body must be a JSON object".to_owned(),
        )),
    }
}

/// Appends caller-supplied members after the adapter has constructed its
/// protocol body. This intentionally lets an explicit extra member override a
/// generated top-level member for gateways with non-standard requirements.
pub(crate) fn merge_extra_body(body: &mut Value, extra_body: &ExtraBody) {
    let body = body
        .as_object_mut()
        .expect("built-in provider request body must be a JSON object");
    for (key, value) in extra_body {
        body.insert(key.clone(), value.clone());
    }
}

/// The only boundary the agent core knows about.
///
/// Provider adapters own model selection, authentication, endpoints, wire
/// formats, and protocol-specific response parsing.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn stream(&self, request: ProviderRequest) -> ProviderEventStream;

    /// Adds hooks supplied by an [`crate::AgentBuilder`]. Custom providers may
    /// ignore request-wire hooks while agent lifecycle hooks still run.
    fn extend_hooks(&mut self, _hooks: HookRegistry) {}

    async fn generate(&self, request: ProviderRequest) -> Result<ProviderResponse, ProviderError> {
        let mut stream = self.stream(request);
        while let Some(event) = stream.next().await {
            if let ProviderEvent::Done(response) = event? {
                return Ok(response);
            }
        }
        Err(ProviderError::Stream(
            "stream ended without a final response".to_owned(),
        ))
    }
}

pub type ProviderEventStream =
    Pin<Box<dyn Stream<Item = Result<ProviderEvent, ProviderError>> + Send + 'static>>;

pub(crate) fn text_only(message: &Message, label: &str) -> Result<String, ProviderError> {
    let content = message
        .content
        .as_ref()
        .ok_or_else(|| ProviderError::InvalidRequest(format!("{label} message has no content")))?;
    match content {
        Content::Text(text) => Ok(text.clone()),
        Content::Parts(parts) => {
            let mut text = String::new();
            for part in parts {
                match part {
                    ContentPart::Text { text: part } => text.push_str(part),
                    ContentPart::ImageUrl { .. } => {
                        return Err(ProviderError::InvalidRequest(format!(
                            "{label} message cannot contain images"
                        )));
                    }
                }
            }
            Ok(text)
        }
    }
}

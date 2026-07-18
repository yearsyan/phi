use std::{collections::BTreeMap, fmt};

use async_stream::try_stream;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use reqwest::header::HeaderName;
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    error::ProviderError,
    hook::{BeforeRequestContext, Hook, HookRegistry, ProviderApi},
    provider::{
        ExtraBody, HttpRequestEvent, LlmProvider, ProviderEventStream, RetryConfig, header_value,
        json_headers, merge_extra_body, next_stream_item, parse_extra_body, send_with_retry,
        text_only,
    },
    types::{
        AssistantDelta, AssistantMessage, Content, ContentPart, Message, ProviderEvent,
        ProviderRequest, ProviderResponse, ProviderState, ReasoningEffort, Role, TokenUsage,
        ToolCall,
    },
};

#[derive(Clone)]
pub struct AnthropicMessagesProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
    anthropic_version: String,
    extra_body: ExtraBody,
    retry_config: RetryConfig,
    hooks: HookRegistry,
}

impl fmt::Debug for AnthropicMessagesProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AnthropicMessagesProvider")
            .field("api_key", &"[REDACTED]")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("anthropic_version", &self.anthropic_version)
            .field("retry_config", &self.retry_config)
            .field("hooks", &self.hooks)
            .finish()
    }
}

impl AnthropicMessagesProvider {
    pub fn new(
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        Self::new_with_client(reqwest::Client::new(), api_key, model)
    }

    /// Builds a provider for Anthropic's public endpoint around an existing
    /// HTTP client without constructing a throwaway client first.
    pub fn new_with_client(
        client: reqwest::Client,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        Self::with_base_url_and_client(client, api_key, "https://api.anthropic.com", model)
    }

    pub fn with_base_url(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        Self::with_base_url_and_client(reqwest::Client::new(), api_key, base_url, model)
    }

    /// Builds a provider for a custom endpoint around an existing HTTP client
    /// without constructing a throwaway client first.
    pub fn with_base_url_and_client(
        client: reqwest::Client,
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        let api_key = api_key.into();
        let base_url = base_url.into();
        let model = model.into();
        if api_key.trim().is_empty() {
            return Err(ProviderError::MissingApiKey);
        }
        if base_url.trim().is_empty() || model.trim().is_empty() {
            return Err(ProviderError::InvalidConfiguration(
                "base URL and model must not be empty".to_owned(),
            ));
        }
        Ok(Self {
            client,
            api_key,
            base_url: base_url.trim_end_matches('/').to_owned(),
            model,
            anthropic_version: "2023-06-01".to_owned(),
            extra_body: ExtraBody::default(),
            retry_config: RetryConfig::default(),
            hooks: HookRegistry::default(),
        })
    }

    /// Replaces the provider's HTTP client. Cloning and sharing one configured
    /// client across providers reuses connection pools and centralizes proxy,
    /// TLS, and client-level transport policy.
    pub fn http_client(mut self, client: reqwest::Client) -> Self {
        self.client = client;
        self
    }

    pub fn retry_config(mut self, retry_config: RetryConfig) -> Self {
        self.retry_config = retry_config;
        self
    }

    pub fn max_retries(mut self, max_retries: usize) -> Self {
        self.retry_config = self.retry_config.with_max_retries(max_retries);
        self
    }

    /// Registers hooks for direct provider use. Hooks registered on
    /// [`crate::AgentBuilder`] are injected automatically when the agent is built.
    pub fn hook(mut self, hook: impl Hook + 'static) -> Self {
        self.hooks.register(hook);
        self
    }

    pub fn hooks(mut self, hooks: HookRegistry) -> Self {
        self.hooks.extend(hooks);
        self
    }

    /// Sets fixed JSON members to append to every Messages API request.
    ///
    /// The value must be a JSON object. Its top-level keys are applied after
    /// the adapter-generated request body, so they can deliberately override
    /// standard fields for compatible gateways.
    pub fn extra_body(mut self, extra_body: Value) -> Result<Self, ProviderError> {
        self.extra_body = parse_extra_body(extra_body)?;
        Ok(self)
    }

    fn request_body(&self, request: &ProviderRequest) -> Result<Value, ProviderError> {
        let mut system = Vec::new();
        let mut messages: Vec<ClaudeMessage> = Vec::new();
        for message in &request.messages {
            match message.role {
                Role::System => system.push(text_only(message, "system")?),
                Role::User => messages.push(ClaudeMessage {
                    role: "user",
                    content: claude_content(message.content.as_ref())?,
                }),
                Role::Assistant => {
                    let content = claude_assistant_content(message)?;
                    messages.push(ClaudeMessage {
                        role: "assistant",
                        content,
                    });
                }
                Role::Tool => {
                    let (tool_content, attachments) = claude_tool_result(message.content.as_ref())?;
                    let block = json!({
                        "type": "tool_result",
                        "tool_use_id": message.tool_call_id,
                        "content": tool_content,
                        "is_error": message.tool_result_is_error
                    });
                    if let Some(last) = messages.last_mut().filter(|last| last.role == "user") {
                        // Anthropic requires every tool_result to precede other
                        // user content. Documents cannot be nested inside a
                        // tool_result (only text and image blocks can), so keep
                        // them as sibling user blocks after all tool results.
                        let insertion = last
                            .content
                            .iter()
                            .position(|item| item["type"] != "tool_result")
                            .unwrap_or(last.content.len());
                        last.content.insert(insertion, block);
                        last.content.extend(attachments);
                    } else {
                        let mut content = Vec::with_capacity(attachments.len() + 1);
                        content.push(block);
                        content.extend(attachments);
                        messages.push(ClaudeMessage {
                            role: "user",
                            content,
                        });
                    }
                }
            }
        }

        let tools = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.parameters
                })
            })
            .collect::<Vec<_>>();
        let model = request.config.model.as_deref().unwrap_or(&self.model);
        let mut body = json!({
            "model": model,
            "max_tokens": request.config.max_tokens.unwrap_or(4096),
            "messages": messages,
            "stream": true
        });
        if !system.is_empty() {
            body["system"] = json!(system.join("\n"));
        }
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools);
        }
        if let Some(temperature) = request.config.temperature {
            body["temperature"] = json!(temperature);
        }
        match request.config.reasoning_effort {
            None => {}
            Some(ReasoningEffort::None) => {
                body["thinking"] = json!({ "type": "disabled" });
            }
            Some(reasoning_effort) => {
                let effort = match reasoning_effort {
                    ReasoningEffort::Minimal => ReasoningEffort::Low,
                    supported => supported,
                };
                body["thinking"] = json!({ "type": "adaptive" });
                body["output_config"] = json!({ "effort": effort.as_str() });
            }
        }
        merge_extra_body(&mut body, &self.extra_body);
        Ok(body)
    }
}

#[derive(Debug, Serialize)]
struct ClaudeMessage {
    role: &'static str,
    content: Vec<Value>,
}

impl LlmProvider for AnthropicMessagesProvider {
    fn stream(&self, request: ProviderRequest) -> ProviderEventStream {
        let body = self.request_body(&request);
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let anthropic_version = self.anthropic_version.clone();
        let endpoint = format!("{}/v1/messages", self.base_url);
        let retry_config = self.retry_config;
        let stream_idle_timeout = retry_config.stream_idle_timeout();
        let hooks = self.hooks.clone();

        Box::pin(try_stream! {
            let body = body?;
            let mut headers = json_headers();
            headers.insert(
                HeaderName::from_static("x-api-key"),
                header_value("x-api-key", &api_key)?,
            );
            headers.insert(
                HeaderName::from_static("anthropic-version"),
                header_value("anthropic-version", &anthropic_version)?,
            );
            let mut context = BeforeRequestContext {
                api: ProviderApi::AnthropicMessages,
                endpoint,
                headers,
                body,
            };
            hooks.run_before_request(&mut context).await?;
            let BeforeRequestContext {
                endpoint,
                headers,
                body,
                ..
            } = context;
            let mut request_events = Box::pin(send_with_retry(retry_config, move || {
                client
                    .post(&endpoint)
                    .headers(headers.clone())
                    .json(&body)
            }));
            let response = loop {
                match request_events.next().await {
                    Some(Ok(HttpRequestEvent::Retry(event))) => {
                        yield ProviderEvent::Retry(event);
                    }
                    Some(Ok(HttpRequestEvent::Response(response))) => break response,
                    Some(Err(error)) => Err(error)?,
                    None => Err(ProviderError::Stream(
                        "HTTP retry stream ended without a response".to_owned(),
                    ))?,
                }
            };

            let mut source = response.bytes_stream().eventsource();
            let mut state = AnthropicStreamState::default();
            let mut stopped = false;
            while let Some(event) = next_stream_item(&mut source, stream_idle_timeout).await? {
                let event = event.map_err(|error| ProviderError::Stream(error.to_string()))?;
                let value: Value = serde_json::from_str(&event.data).map_err(|error| {
                    ProviderError::InvalidResponse(format!("invalid Claude stream event: {error}"))
                })?;
                if value["type"] == "error" {
                    Err(ProviderError::from_stream_response_error(
                        value["error"].to_string(),
                    ))?;
                }
                if value["type"] == "message_stop" {
                    yield ProviderEvent::Done(state.finish()?);
                    stopped = true;
                    break;
                }
                for delta in state.apply(&value) {
                    yield ProviderEvent::Delta(delta);
                }
            }
            if !stopped {
                Err(ProviderError::Stream(
                    "Claude stream ended without message_stop".to_owned(),
                ))?;
            }
        })
    }

    fn extend_hooks(&mut self, hooks: HookRegistry) {
        self.hooks.extend(hooks);
    }
}

#[derive(Default)]
struct AnthropicStreamState {
    reasoning: String,
    text: String,
    tools: BTreeMap<usize, PendingToolUse>,
    raw_content: BTreeMap<usize, Value>,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    has_usage: bool,
}

#[derive(Default)]
struct PendingToolUse {
    id: String,
    name: String,
    arguments: String,
}

impl AnthropicStreamState {
    fn apply(&mut self, event: &Value) -> Vec<AssistantDelta> {
        match event["type"].as_str() {
            Some("message_start") => self.update_usage(&event["message"]["usage"]),
            Some("message_delta") => self.update_usage(&event["usage"]),
            Some("content_block_start") => return self.start_block(event),
            Some("content_block_delta") => return self.apply_delta(event),
            _ => {}
        }
        Vec::new()
    }

    fn update_usage(&mut self, usage: &Value) {
        if !usage.is_object() {
            return;
        }
        self.has_usage = true;
        if let Some(value) = usage["input_tokens"].as_u64() {
            self.input_tokens = value;
        }
        if let Some(value) = usage["output_tokens"].as_u64() {
            self.output_tokens = value;
        }
        if let Some(value) = usage["cache_read_input_tokens"].as_u64() {
            self.cache_read_tokens = value;
        }
        if let Some(value) = usage["cache_creation_input_tokens"].as_u64() {
            self.cache_creation_tokens = value;
        }
    }

    fn start_block(&mut self, event: &Value) -> Vec<AssistantDelta> {
        let index = event["index"].as_u64().unwrap_or(0) as usize;
        let block = &event["content_block"];
        if block.is_object() {
            self.raw_content.insert(index, block.clone());
        }
        match block["type"].as_str() {
            Some("thinking") => {
                let thinking = block["thinking"].as_str().unwrap_or_default();
                if thinking.is_empty() {
                    Vec::new()
                } else {
                    self.reasoning.push_str(thinking);
                    vec![AssistantDelta::Reasoning {
                        delta: thinking.to_owned(),
                    }]
                }
            }
            Some("text") => {
                let text = block["text"].as_str().unwrap_or_default();
                if text.is_empty() {
                    Vec::new()
                } else {
                    self.text.push_str(text);
                    vec![AssistantDelta::Text {
                        delta: text.to_owned(),
                    }]
                }
            }
            Some("tool_use") => {
                let id = block["id"].as_str().unwrap_or_default().to_owned();
                let name = block["name"].as_str().unwrap_or_default().to_owned();
                let initial = block
                    .get("input")
                    .filter(|input| input.as_object().is_some_and(|object| !object.is_empty()))
                    .map(Value::to_string)
                    .unwrap_or_default();
                self.tools.insert(
                    index,
                    PendingToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: initial.clone(),
                    },
                );
                vec![AssistantDelta::ToolCall {
                    index,
                    id: Some(id),
                    name: Some(name),
                    arguments_delta: initial,
                }]
            }
            _ => Vec::new(),
        }
    }

    fn apply_delta(&mut self, event: &Value) -> Vec<AssistantDelta> {
        let index = event["index"].as_u64().unwrap_or(0) as usize;
        let delta = &event["delta"];
        match delta["type"].as_str() {
            Some("text_delta") => {
                let text = delta["text"].as_str().unwrap_or_default().to_owned();
                self.text.push_str(&text);
                self.append_raw_string(index, "text", &text);
                vec![AssistantDelta::Text { delta: text }]
            }
            Some("thinking_delta") => {
                let thinking = delta["thinking"].as_str().unwrap_or_default();
                self.append_raw_string(index, "thinking", thinking);
                if thinking.is_empty() {
                    Vec::new()
                } else {
                    self.reasoning.push_str(thinking);
                    vec![AssistantDelta::Reasoning {
                        delta: thinking.to_owned(),
                    }]
                }
            }
            Some("signature_delta") => {
                let signature = delta["signature"].as_str().unwrap_or_default();
                self.append_raw_string(index, "signature", signature);
                Vec::new()
            }
            Some("input_json_delta") => {
                let arguments_delta = delta["partial_json"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                self.tools
                    .entry(index)
                    .or_default()
                    .arguments
                    .push_str(&arguments_delta);
                vec![AssistantDelta::ToolCall {
                    index,
                    id: None,
                    name: None,
                    arguments_delta,
                }]
            }
            _ => Vec::new(),
        }
    }

    fn append_raw_string(&mut self, index: usize, field: &str, fragment: &str) {
        if fragment.is_empty() {
            return;
        }
        let Some(block) = self.raw_content.get_mut(&index) else {
            return;
        };
        let value = &mut block[field];
        if let Value::String(existing) = value {
            existing.push_str(fragment);
        } else {
            *value = Value::String(fragment.to_owned());
        }
    }

    fn finish(mut self) -> Result<ProviderResponse, ProviderError> {
        let content = (!self.text.is_empty()).then(|| Content::text(self.text));
        let tool_calls = self
            .tools
            .into_iter()
            .map(|(index, tool)| {
                let arguments = if tool.arguments.trim().is_empty() {
                    json!({})
                } else {
                    serde_json::from_str(&tool.arguments).map_err(|error| {
                        ProviderError::InvalidResponse(format!("invalid tool arguments: {error}"))
                    })?
                };
                if let Some(block) = self.raw_content.get_mut(&index) {
                    block["input"] = arguments.clone();
                }
                Ok(ToolCall::new(tool.id, tool.name, arguments))
            })
            .collect::<Result<Vec<_>, ProviderError>>()?;
        if content.is_none() && tool_calls.is_empty() {
            return Err(ProviderError::InvalidResponse(
                "Claude stream contains neither text nor tool use".to_owned(),
            ));
        }
        let usage = self.has_usage.then(|| {
            let input = self
                .input_tokens
                .saturating_add(self.cache_read_tokens)
                .saturating_add(self.cache_creation_tokens);
            TokenUsage::new(
                input,
                self.output_tokens,
                self.cache_read_tokens
                    .saturating_add(self.cache_creation_tokens),
            )
        });
        let has_reasoning = self.raw_content.values().any(|block| {
            matches!(
                block["type"].as_str(),
                Some("thinking" | "redacted_thinking")
            )
        });
        Ok(ProviderResponse {
            message: AssistantMessage {
                content,
                reasoning: (!self.reasoning.is_empty()).then_some(self.reasoning),
                tool_calls,
                provider_state: has_reasoning.then_some(ProviderState::AnthropicMessages {
                    content: self.raw_content.into_values().collect(),
                }),
            },
            usage,
        })
    }
}

fn claude_content(content: Option<&Content>) -> Result<Vec<Value>, ProviderError> {
    match content {
        None => Ok(Vec::new()),
        Some(Content::Text(text)) => Ok(vec![json!({ "type": "text", "text": text })]),
        Some(Content::Parts(parts)) => parts.iter().map(claude_part).collect(),
    }
}

fn claude_assistant_content(message: &Message) -> Result<Vec<Value>, ProviderError> {
    if let Some(ProviderState::AnthropicMessages { content }) = &message.provider_state
        && normalized_claude_assistant(content).is_some_and(|(normalized_content, tool_calls)| {
            normalized_content == message.content && tool_calls == message.tool_calls
        })
    {
        return Ok(content.clone());
    }

    let mut content = match &message.provider_state {
        Some(ProviderState::AnthropicMessages { content }) => content
            .iter()
            .filter(|block| {
                !matches!(
                    block.get("type").and_then(Value::as_str),
                    Some("text" | "tool_use")
                )
            })
            .cloned()
            .collect(),
        _ => Vec::new(),
    };
    content.extend(claude_content(message.content.as_ref())?);
    content.extend(message.tool_calls.iter().map(|call| {
        json!({
            "type": "tool_use",
            "id": call.id,
            "name": call.name,
            "input": call.arguments
        })
    }));
    Ok(content)
}

fn normalized_claude_assistant(content: &[Value]) -> Option<(Option<Content>, Vec<ToolCall>)> {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(value) = block.get("text").and_then(Value::as_str) {
                    text.push_str(value);
                }
            }
            Some("tool_use") => {
                tool_calls.push(ToolCall::new(
                    block.get("id")?.as_str()?,
                    block.get("name")?.as_str()?,
                    block.get("input").cloned().unwrap_or_else(|| json!({})),
                ));
            }
            _ => {}
        }
    }
    Some(((!text.is_empty()).then(|| Content::text(text)), tool_calls))
}

fn claude_part(part: &ContentPart) -> Result<Value, ProviderError> {
    match part {
        ContentPart::Text { text } => Ok(json!({ "type": "text", "text": text })),
        ContentPart::ImageUrl { image_url } => {
            if let Some(data) = image_url.url.strip_prefix("data:") {
                let (mime_type, encoded) = data.split_once(";base64,").ok_or_else(|| {
                    ProviderError::InvalidRequest("invalid base64 image data URL".to_owned())
                })?;
                Ok(json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": mime_type,
                        "data": encoded
                    }
                }))
            } else {
                Ok(json!({
                    "type": "image",
                    "source": { "type": "url", "url": image_url.url }
                }))
            }
        }
        ContentPart::Document { document } => {
            if let Some(data) = document.data.strip_prefix("data:") {
                let (mime_type, encoded) = data.split_once(";base64,").ok_or_else(|| {
                    ProviderError::InvalidRequest("invalid base64 document data URL".to_owned())
                })?;
                Ok(json!({
                    "type": "document",
                    "source": {
                        "type": "base64",
                        "media_type": mime_type,
                        "data": encoded
                    },
                    "title": document.filename
                }))
            } else {
                Ok(json!({
                    "type": "document",
                    "source": { "type": "url", "url": document.data },
                    "title": document.filename
                }))
            }
        }
    }
}

fn claude_tool_result(content: Option<&Content>) -> Result<(Value, Vec<Value>), ProviderError> {
    match content {
        None => Ok((json!(""), Vec::new())),
        Some(Content::Text(text)) => Ok((json!(text), Vec::new())),
        Some(Content::Parts(parts)) => {
            let mut nested = Vec::new();
            let mut attachments = Vec::new();
            for part in parts {
                match part {
                    ContentPart::Document { .. } => attachments.push(claude_part(part)?),
                    ContentPart::Text { .. } | ContentPart::ImageUrl { .. } => {
                        nested.push(claude_part(part)?);
                    }
                }
            }
            let content = if nested.is_empty() {
                json!("")
            } else {
                Value::Array(nested)
            };
            Ok((content, attachments))
        }
    }
}

#[cfg(test)]
fn parse_response(response: Value) -> Result<ProviderResponse, ProviderError> {
    let usage = response.get("usage").and_then(|usage| {
        let uncached = usage.get("input_tokens")?.as_u64()?;
        let cache_read = usage
            .get("cache_read_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cache_creation = usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let input = uncached
            .saturating_add(cache_read)
            .saturating_add(cache_creation);
        let output = usage.get("output_tokens")?.as_u64()?;
        Some(TokenUsage::new(
            input,
            output,
            cache_read.saturating_add(cache_creation),
        ))
    });
    let blocks = response["content"]
        .as_array()
        .ok_or_else(|| ProviderError::InvalidResponse("content is not an array".to_owned()))?;
    let mut text = Vec::new();
    let mut reasoning = Vec::new();
    let mut tool_calls = Vec::new();
    for block in blocks {
        match block["type"].as_str() {
            Some("thinking") => {
                if let Some(value) = block["thinking"].as_str() {
                    reasoning.push(value.to_owned());
                }
            }
            Some("text") => {
                if let Some(value) = block["text"].as_str() {
                    text.push(value.to_owned());
                }
            }
            Some("tool_use") => {
                let id = block["id"]
                    .as_str()
                    .ok_or_else(|| ProviderError::InvalidResponse("missing tool id".to_owned()))?;
                let name = block["name"].as_str().ok_or_else(|| {
                    ProviderError::InvalidResponse("missing tool name".to_owned())
                })?;
                tool_calls.push(ToolCall::new(id, name, block["input"].clone()));
            }
            _ => {}
        }
    }

    let content = (!text.is_empty()).then(|| Content::text(text.join("")));
    if content.is_none() && tool_calls.is_empty() {
        return Err(ProviderError::InvalidResponse(
            "response contains neither text nor tool use".to_owned(),
        ));
    }
    Ok(ProviderResponse {
        message: AssistantMessage {
            content,
            reasoning: (!reasoning.is_empty()).then(|| reasoning.join("")),
            tool_calls,
            provider_state: blocks
                .iter()
                .any(|block| {
                    matches!(
                        block["type"].as_str(),
                        Some("thinking" | "redacted_thinking")
                    )
                })
                .then_some(ProviderState::AnthropicMessages {
                    content: blocks.clone(),
                }),
        },
        usage,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use reqwest::header::{HeaderMap, HeaderValue};
    use serde_json::json;
    use tokio::net::TcpListener;

    use super::*;
    use crate::{
        error::HookError,
        types::{Document, GenerationConfig, ImageUrl, Message, ReasoningEffort, ToolDefinition},
    };

    #[test]
    fn debug_redacts_api_key_and_accepts_an_injected_http_client() {
        let provider = AnthropicMessagesProvider::new_with_client(
            reqwest::Client::new(),
            "super-secret-anthropic-key",
            "model",
        )
        .unwrap();

        let debug = format!("{provider:?}");
        assert!(!debug.contains("super-secret-anthropic-key"));
        assert!(debug.contains("[REDACTED]"));
    }

    struct RecordingRequestHook {
        observed: Arc<Mutex<Option<(ProviderApi, HeaderMap, Value)>>>,
    }

    #[async_trait::async_trait]
    impl Hook for RecordingRequestHook {
        async fn before_request(
            &self,
            context: &mut BeforeRequestContext,
        ) -> Result<(), HookError> {
            tokio::task::yield_now().await;
            context.body["messages"][0]["content"][0]["cache_control"] =
                json!({ "type": "ephemeral" });
            context
                .headers
                .insert("x-hook", HeaderValue::from_static("applied"));
            *self.observed.lock().unwrap() =
                Some((context.api, context.headers.clone(), context.body.clone()));
            Ok(())
        }
    }

    #[test]
    fn maps_normalized_request_to_claude_messages() {
        let provider =
            AnthropicMessagesProvider::with_base_url("key", "https://example.com", "claude-model")
                .unwrap();
        let mut assistant = Message::assistant(
            None,
            vec![ToolCall::new("toolu-1", "echo", json!({"text": "hi"}))],
        );
        assistant.provider_state = Some(ProviderState::AnthropicMessages {
            content: vec![
                json!({
                    "type": "thinking",
                    "thinking": "call the echo tool",
                    "signature": "signature-1"
                }),
                json!({
                    "type": "tool_use",
                    "id": "toolu-1",
                    "name": "echo",
                    "input": {"text": "hi"}
                }),
            ],
        });
        let request = ProviderRequest {
            messages: vec![
                Message::system("system"),
                Message::user("hello"),
                assistant,
                Message::tool_result("toolu-1", "hi", false),
            ],
            tools: vec![ToolDefinition::new(
                "echo",
                "echo",
                json!({"type": "object"}),
            )],
            config: GenerationConfig {
                model: Some("request-model".to_owned()),
                temperature: Some(0.3),
                max_tokens: Some(123),
                reasoning_effort: Some(ReasoningEffort::Max),
            },
        };

        let body = provider.request_body(&request).unwrap();
        assert_eq!(body["model"], "request-model");
        assert_eq!(body["system"], "system");
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["max_tokens"], 123);
        assert_eq!(body["output_config"]["effort"], "max");
        assert_eq!(body["messages"][1]["content"][0]["type"], "thinking");
        assert_eq!(
            body["messages"][1]["content"][0]["signature"],
            "signature-1"
        );
        assert_eq!(body["messages"][1]["content"][1]["type"], "tool_use");
        assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
    }

    #[test]
    fn maps_pdf_documents_to_native_anthropic_blocks() {
        let provider =
            AnthropicMessagesProvider::with_base_url("key", "https://example.com", "claude-model")
                .unwrap();
        let request = ProviderRequest {
            messages: vec![Message::user_parts([
                ContentPart::text("inspect"),
                ContentPart::document(Document::from_bytes(
                    "report.pdf",
                    "application/pdf",
                    b"%PDF-test",
                )),
            ])],
            tools: Vec::new(),
            config: GenerationConfig::default(),
        };

        let body = provider.request_body(&request).unwrap();
        let document = &body["messages"][0]["content"][1];
        assert_eq!(document["type"], "document");
        assert_eq!(document["title"], "report.pdf");
        assert_eq!(document["source"]["type"], "base64");
        assert_eq!(document["source"]["media_type"], "application/pdf");
        assert_eq!(document["source"]["data"], "JVBERi10ZXN0");
    }

    #[test]
    fn keeps_documents_outside_anthropic_tool_result_content() {
        let provider =
            AnthropicMessagesProvider::with_base_url("key", "https://example.com", "claude-model")
                .unwrap();
        let assistant = Message::assistant(
            None,
            vec![
                ToolCall::new("call-1", "read", json!({})),
                ToolCall::new("call-2", "read", json!({})),
            ],
        );
        let first = Message::tool_result_content(
            "call-1",
            Content::parts([
                ContentPart::text("first result"),
                ContentPart::image(ImageUrl::from_bytes("image/png", b"png")),
                ContentPart::document(Document::from_bytes(
                    "first.pdf",
                    "application/pdf",
                    b"first",
                )),
            ]),
            false,
            None,
        );
        let second = Message::tool_result_content(
            "call-2",
            Content::parts([
                ContentPart::text("second result"),
                ContentPart::document(Document::new(
                    "second.pdf",
                    "application/pdf",
                    "https://example.com/second.pdf",
                )),
            ]),
            false,
            None,
        );
        let request = ProviderRequest {
            messages: vec![assistant, first, second],
            tools: Vec::new(),
            config: GenerationConfig::default(),
        };

        let body = provider.request_body(&request).unwrap();
        let content = body["messages"][1]["content"].as_array().unwrap();
        assert_eq!(
            content
                .iter()
                .map(|block| block["type"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["tool_result", "tool_result", "document", "document"]
        );
        assert_eq!(content[0]["content"][0]["type"], "text");
        assert_eq!(content[0]["content"][1]["type"], "image");
        assert!(
            content[0]["content"]
                .as_array()
                .unwrap()
                .iter()
                .all(|part| part["type"] != "document")
        );
        assert_eq!(content[2]["title"], "first.pdf");
        assert_eq!(content[3]["source"]["type"], "url");
        assert_eq!(
            content[3]["source"]["url"],
            "https://example.com/second.pdf"
        );
    }

    #[test]
    fn normalized_assistant_fields_override_stale_anthropic_replay_blocks() {
        let provider =
            AnthropicMessagesProvider::with_base_url("key", "https://example.com", "claude-model")
                .unwrap();
        let mut assistant = Message::assistant(
            Some(Content::text("hooked answer")),
            vec![ToolCall::new(
                "hooked-call",
                "echo",
                json!({"text": "hooked"}),
            )],
        );
        assistant.provider_state = Some(ProviderState::AnthropicMessages {
            content: vec![
                json!({
                    "type": "thinking",
                    "thinking": "private reasoning",
                    "signature": "signature-1"
                }),
                json!({ "type": "redacted_thinking", "data": "opaque-data" }),
                json!({ "type": "text", "text": "stale answer" }),
                json!({
                    "type": "tool_use",
                    "id": "stale-call",
                    "name": "stale-tool",
                    "input": {"text": "stale"}
                }),
            ],
        });
        let request = ProviderRequest {
            messages: vec![assistant],
            tools: Vec::new(),
            config: GenerationConfig::default(),
        };

        let body = provider.request_body(&request).unwrap();
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 4);
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(
            content[1],
            json!({ "type": "redacted_thinking", "data": "opaque-data" })
        );
        assert_eq!(
            content[2],
            json!({ "type": "text", "text": "hooked answer" })
        );
        assert_eq!(content[3]["type"], "tool_use");
        assert_eq!(content[3]["id"], "hooked-call");
        assert_eq!(content[3]["name"], "echo");
        assert_eq!(content[3]["input"], json!({"text": "hooked"}));
    }

    #[tokio::test]
    async fn applies_async_hook_to_final_messages_body_and_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let observed = Arc::new(Mutex::new(None));
        let provider = AnthropicMessagesProvider::with_base_url(
            "key",
            format!("http://{address}"),
            "claude-model",
        )
        .unwrap()
        .max_retries(0)
        .hook(RecordingRequestHook {
            observed: Arc::clone(&observed),
        });
        let request = ProviderRequest {
            messages: vec![Message::user("cache me")],
            tools: Vec::new(),
            config: GenerationConfig::default(),
        };

        let error = provider.generate(request).await.unwrap_err();

        assert!(matches!(error, ProviderError::Http(_)));
        let observed = observed.lock().unwrap();
        let (api, headers, body) = observed.as_ref().unwrap();
        assert_eq!(*api, ProviderApi::AnthropicMessages);
        assert_eq!(headers["x-hook"], "applied");
        assert_eq!(headers["x-api-key"], "key");
        assert_eq!(
            body["messages"][0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
    }

    #[test]
    fn omits_claude_reasoning_fields_when_effort_is_unset() {
        let provider =
            AnthropicMessagesProvider::with_base_url("key", "https://example.com", "claude-model")
                .unwrap();
        let request = ProviderRequest {
            messages: vec![Message::user("hello")],
            tools: Vec::new(),
            config: GenerationConfig::default(),
        };

        let body = provider.request_body(&request).unwrap();
        assert!(body.get("thinking").is_none());
        assert!(body.get("output_config").is_none());
    }

    #[test]
    fn explicitly_disables_claude_thinking_for_none_effort() {
        let provider =
            AnthropicMessagesProvider::with_base_url("key", "https://example.com", "claude-model")
                .unwrap();
        let request = ProviderRequest {
            messages: vec![Message::user("hello")],
            tools: Vec::new(),
            config: GenerationConfig {
                reasoning_effort: Some(ReasoningEffort::None),
                ..GenerationConfig::default()
            },
        };

        let body = provider.request_body(&request).unwrap();
        assert_eq!(body["thinking"]["type"], "disabled");
        assert!(body.get("output_config").is_none());
    }

    #[test]
    fn maps_minimal_reasoning_effort_to_low_for_messages() {
        let provider =
            AnthropicMessagesProvider::with_base_url("key", "https://example.com", "claude-model")
                .unwrap();
        let request = ProviderRequest {
            messages: vec![Message::user("hello")],
            tools: Vec::new(),
            config: GenerationConfig {
                reasoning_effort: Some(ReasoningEffort::Minimal),
                ..GenerationConfig::default()
            },
        };

        let body = provider.request_body(&request).unwrap();
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["output_config"]["effort"], "low");
    }

    #[test]
    fn replays_thinking_blocks_for_ordinary_assistant_turns() {
        let provider =
            AnthropicMessagesProvider::with_base_url("key", "https://example.com", "claude-model")
                .unwrap();
        let parsed = parse_response(json!({
            "content": [
                {
                    "type": "thinking",
                    "thinking": "private reasoning",
                    "signature": "signature-1"
                },
                { "type": "redacted_thinking", "data": "opaque-data" },
                {
                    "type": "text",
                    "text": "answer",
                    "citations": [{ "type": "opaque_citation", "value": "kept" }]
                }
            ]
        }))
        .unwrap();
        let request = ProviderRequest {
            messages: vec![parsed.message.into_message(), Message::user("next")],
            tools: Vec::new(),
            config: GenerationConfig::default(),
        };

        let body = provider.request_body(&request).unwrap();
        assert_eq!(body["messages"][0]["content"][0]["type"], "thinking");
        assert_eq!(
            body["messages"][0]["content"][0]["signature"],
            "signature-1"
        );
        assert_eq!(
            body["messages"][0]["content"][1],
            json!({ "type": "redacted_thinking", "data": "opaque-data" })
        );
        assert_eq!(body["messages"][0]["content"][2]["text"], "answer");
        assert_eq!(
            body["messages"][0]["content"][2]["citations"],
            json!([{ "type": "opaque_citation", "value": "kept" }])
        );
    }

    #[test]
    fn includes_claude_cache_tokens_in_input_context() {
        let parsed = parse_response(json!({
            "content": [{ "type": "text", "text": "ok" }],
            "usage": {
                "input_tokens": 10,
                "cache_read_input_tokens": 70,
                "cache_creation_input_tokens": 20,
                "output_tokens": 5
            }
        }))
        .unwrap();

        assert_eq!(parsed.usage, Some(TokenUsage::new(100, 5, 90)));
    }

    #[test]
    fn assembles_claude_content_block_stream() {
        let mut state = AnthropicStreamState::default();
        state.apply(&json!({
            "type": "message_start",
            "message": { "usage": {
                "input_tokens": 10,
                "cache_read_input_tokens": 20,
                "output_tokens": 1
            }}
        }));
        state.apply(&json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "thinking",
                "thinking": "",
                "signature": ""
            }
        }));
        let reasoning_delta = state.apply(&json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "thinking_delta", "thinking": "use echo" }
        }));
        assert_eq!(
            reasoning_delta,
            vec![AssistantDelta::Reasoning {
                delta: "use echo".to_owned()
            }]
        );
        state.apply(&json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "signature_delta", "signature": "signature-1" }
        }));
        state.apply(&json!({
            "type": "content_block_start",
            "index": 1,
            "content_block": {
                "type": "tool_use",
                "id": "tool-1",
                "name": "echo",
                "input": {}
            }
        }));
        state.apply(&json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": { "type": "input_json_delta", "partial_json": "{\"text\":" }
        }));
        state.apply(&json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": { "type": "input_json_delta", "partial_json": "\"hi\"}" }
        }));
        state.apply(&json!({
            "type": "message_delta",
            "usage": { "output_tokens": 5 }
        }));

        let response = state.finish().unwrap();
        assert_eq!(response.message.reasoning.as_deref(), Some("use echo"));
        assert_eq!(
            response.message.tool_calls[0].arguments,
            json!({"text": "hi"})
        );
        let Some(ProviderState::AnthropicMessages { content }) = &response.message.provider_state
        else {
            panic!("missing Messages reasoning state");
        };
        assert_eq!(content[0]["thinking"], "use echo");
        assert_eq!(content[0]["signature"], "signature-1");
        assert_eq!(content[1]["input"], json!({"text": "hi"}));
        assert_eq!(response.usage, Some(TokenUsage::new(30, 5, 20)));
    }
}

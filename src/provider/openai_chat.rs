use std::collections::BTreeMap;

use async_stream::try_stream;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use reqwest::header::AUTHORIZATION;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    error::ProviderError,
    hook::{BeforeRequestContext, Hook, HookRegistry, ProviderApi},
    provider::{
        ExtraBody, HttpRequestEvent, LlmProvider, ProviderEventStream, RetryConfig, header_value,
        json_headers, merge_extra_body, parse_extra_body, send_with_retry,
    },
    types::{
        AssistantDelta, AssistantMessage, Content, ContentPart, Message, ProviderEvent,
        ProviderRequest, ProviderResponse, ProviderState, Role, TokenUsage, ToolCall,
    },
};

#[derive(Clone, Debug)]
pub struct OpenAiChatProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
    extra_body: ExtraBody,
    retry_config: RetryConfig,
    hooks: HookRegistry,
}

impl OpenAiChatProvider {
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        let api_key = api_key.into();
        let base_url = base_url.into();
        let model = model.into();
        validate(&api_key, &base_url, &model)?;

        Ok(Self {
            client: reqwest::Client::new(),
            api_key,
            base_url: base_url.trim_end_matches('/').to_owned(),
            model,
            extra_body: ExtraBody::default(),
            retry_config: RetryConfig::default(),
            hooks: HookRegistry::default(),
        })
    }

    /// Sets fixed JSON members to append to every Chat Completions request.
    ///
    /// The value must be a JSON object. Its top-level keys are applied after
    /// the adapter-generated request body, so they can deliberately override
    /// standard fields for compatible gateways.
    pub fn extra_body(mut self, extra_body: Value) -> Result<Self, ProviderError> {
        self.extra_body = parse_extra_body(extra_body)?;
        Ok(self)
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

    fn request_body(&self, request: &ProviderRequest) -> Value {
        let messages = request
            .messages
            .iter()
            .map(message_to_json)
            .collect::<Vec<_>>();
        let tools = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters
                    }
                })
            })
            .collect::<Vec<_>>();

        let mut body = json!({
            "model": self.model,
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true }
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools);
        }
        if let Some(temperature) = request.config.temperature {
            body["temperature"] = json!(temperature);
        }
        if let Some(max_tokens) = request.config.max_tokens {
            let field = if request.config.reasoning_effort.is_some() {
                "max_completion_tokens"
            } else {
                "max_tokens"
            };
            body[field] = json!(max_tokens);
        }
        if let Some(reasoning_effort) = request.config.reasoning_effort {
            body["reasoning_effort"] = json!(reasoning_effort.as_str());
        }
        merge_extra_body(&mut body, &self.extra_body);
        body
    }
}

impl LlmProvider for OpenAiChatProvider {
    fn stream(&self, request: ProviderRequest) -> ProviderEventStream {
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let endpoint = format!("{}/chat/completions", self.base_url);
        let body = self.request_body(&request);
        let retry_config = self.retry_config;
        let hooks = self.hooks.clone();

        Box::pin(try_stream! {
            let mut headers = json_headers();
            headers.insert(
                AUTHORIZATION,
                header_value("authorization", &format!("Bearer {api_key}"))?,
            );
            let mut context = BeforeRequestContext {
                api: ProviderApi::OpenAiChatCompletions,
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
            let mut state = ChatStreamState::default();
            while let Some(event) = source.next().await {
                let event = event.map_err(|error| ProviderError::Stream(error.to_string()))?;
                if event.data.trim() == "[DONE]" {
                    break;
                }
                let chunk: Value = serde_json::from_str(&event.data).map_err(|error| {
                    ProviderError::InvalidResponse(format!("invalid chat stream chunk: {error}"))
                })?;
                if let Some(error) = chunk.get("error") {
                    Err(ProviderError::InvalidResponse(error.to_string()))?;
                }
                for delta in state.apply(&chunk)? {
                    yield ProviderEvent::Delta(delta);
                }
            }
            yield ProviderEvent::Done(state.finish()?);
        })
    }

    fn extend_hooks(&mut self, hooks: HookRegistry) {
        self.hooks.extend(hooks);
    }
}

#[derive(Default)]
struct ChatStreamState {
    text: String,
    tools: BTreeMap<usize, PendingToolCall>,
    reasoning_fields: BTreeMap<String, Value>,
    usage: Option<TokenUsage>,
}

#[derive(Default)]
struct PendingToolCall {
    id: String,
    name: String,
    arguments: String,
}

impl ChatStreamState {
    fn apply(&mut self, chunk: &Value) -> Result<Vec<AssistantDelta>, ProviderError> {
        if let Some(usage) = chunk.get("usage").filter(|usage| !usage.is_null()) {
            self.usage = Some(normalize_chat_usage(
                serde_json::from_value(usage.clone()).map_err(|error| {
                    ProviderError::InvalidResponse(format!("invalid chat usage: {error}"))
                })?,
            ));
        }

        let mut deltas = Vec::new();
        for choice in chunk["choices"].as_array().into_iter().flatten() {
            let delta = &choice["delta"];
            self.capture_reasoning(delta);
            if let Some(text) = delta["content"].as_str().filter(|text| !text.is_empty()) {
                self.text.push_str(text);
                deltas.push(AssistantDelta::Text {
                    delta: text.to_owned(),
                });
            }
            for tool in delta["tool_calls"].as_array().into_iter().flatten() {
                let index = tool["index"].as_u64().unwrap_or(0) as usize;
                let pending = self.tools.entry(index).or_default();
                let id = tool["id"].as_str().map(str::to_owned);
                let name = tool["function"]["name"].as_str().map(str::to_owned);
                let arguments_delta = tool["function"]["arguments"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                if let Some(value) = &id {
                    pending.id.push_str(value);
                }
                if let Some(value) = &name {
                    pending.name.push_str(value);
                }
                pending.arguments.push_str(&arguments_delta);
                deltas.push(AssistantDelta::ToolCall {
                    index,
                    id,
                    name,
                    arguments_delta,
                });
            }
        }
        Ok(deltas)
    }

    fn capture_reasoning(&mut self, delta: &Value) {
        for field in ["reasoning", "reasoning_content"] {
            let Some(fragment) = delta[field].as_str().filter(|value| !value.is_empty()) else {
                continue;
            };
            let value = self
                .reasoning_fields
                .entry(field.to_owned())
                .or_insert_with(|| Value::String(String::new()));
            if let Value::String(reasoning) = value {
                reasoning.push_str(fragment);
            }
        }

        let Some(details) = delta["reasoning_details"].as_array() else {
            return;
        };
        let value = self
            .reasoning_fields
            .entry("reasoning_details".to_owned())
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Some(captured) = value.as_array_mut() {
            captured.extend(details.iter().cloned());
        }
    }

    fn finish(self) -> Result<ProviderResponse, ProviderError> {
        let content = (!self.text.is_empty()).then(|| Content::text(self.text));
        let tool_calls = self
            .tools
            .into_values()
            .map(|tool| {
                let arguments = if tool.arguments.trim().is_empty() {
                    json!({})
                } else {
                    serde_json::from_str(&tool.arguments).map_err(|error| {
                        ProviderError::InvalidResponse(format!("invalid tool arguments: {error}"))
                    })?
                };
                Ok(ToolCall::new(tool.id, tool.name, arguments))
            })
            .collect::<Result<Vec<_>, ProviderError>>()?;
        if content.is_none() && tool_calls.is_empty() {
            return Err(ProviderError::InvalidResponse(
                "chat stream contains neither content nor tool calls".to_owned(),
            ));
        }
        Ok(ProviderResponse {
            message: AssistantMessage {
                content,
                tool_calls,
                provider_state: (!self.reasoning_fields.is_empty()).then_some(
                    ProviderState::OpenAiChat {
                        fields: self.reasoning_fields,
                    },
                ),
            },
            usage: self.usage,
        })
    }
}

fn validate(api_key: &str, base_url: &str, model: &str) -> Result<(), ProviderError> {
    if api_key.trim().is_empty() {
        return Err(ProviderError::MissingApiKey);
    }
    if base_url.trim().is_empty() || model.trim().is_empty() {
        return Err(ProviderError::InvalidConfiguration(
            "base URL and model must not be empty".to_owned(),
        ));
    }
    Ok(())
}

fn message_to_json(message: &Message) -> Value {
    let role = match message.role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    };
    let mut value = json!({ "role": role });
    if let Some(content) = &message.content {
        value["content"] = content_to_json(content);
    }
    if !message.tool_calls.is_empty() {
        value["tool_calls"] = Value::Array(
            message
                .tool_calls
                .iter()
                .map(|call| {
                    json!({
                        "id": call.id,
                        "type": "function",
                        "function": {
                            "name": call.name,
                            "arguments": call.arguments.to_string()
                        }
                    })
                })
                .collect(),
        );
    }
    if let Some(tool_call_id) = &message.tool_call_id {
        value["tool_call_id"] = json!(tool_call_id);
    }
    if message.role == Role::Assistant
        && let Some(ProviderState::OpenAiChat { fields }) = &message.provider_state
    {
        for (field, field_value) in fields {
            if matches!(
                field.as_str(),
                "reasoning" | "reasoning_content" | "reasoning_details"
            ) {
                value[field] = field_value.clone();
            }
        }
    }
    value
}

fn content_to_json(content: &Content) -> Value {
    match content {
        Content::Text(text) => json!(text),
        Content::Parts(parts) => Value::Array(
            parts
                .iter()
                .map(|part| match part {
                    ContentPart::Text { text } => json!({ "type": "text", "text": text }),
                    ContentPart::ImageUrl { image_url } => {
                        let mut image = json!({ "url": image_url.url });
                        if let Some(detail) = image_url.detail {
                            image["detail"] = json!(detail.as_str());
                        }
                        json!({ "type": "image_url", "image_url": image })
                    }
                })
                .collect(),
        ),
    }
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
    usage: Option<ChatUsage>,
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: Option<Value>,
    #[serde(default)]
    tool_calls: Vec<ChatToolCall>,
    reasoning: Option<Value>,
    reasoning_content: Option<Value>,
    reasoning_details: Option<Value>,
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
struct ChatToolCall {
    id: String,
    function: ChatFunction,
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
struct ChatFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    total_tokens: Option<u64>,
    prompt_tokens_details: Option<ChatInputDetails>,
}

#[derive(Debug, Deserialize)]
struct ChatInputDetails {
    #[serde(default)]
    cached_tokens: u64,
}

#[cfg(test)]
fn parse_response(response: ChatResponse) -> Result<ProviderResponse, ProviderError> {
    let usage = response.usage.map(normalize_chat_usage);
    let message = response
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| ProviderError::InvalidResponse("choices is empty".to_owned()))?
        .message;
    let mut reasoning_fields = BTreeMap::new();
    for (field, value) in [
        ("reasoning", message.reasoning.as_ref()),
        ("reasoning_content", message.reasoning_content.as_ref()),
        ("reasoning_details", message.reasoning_details.as_ref()),
    ] {
        if let Some(value) = value.filter(|value| !value.is_null()) {
            reasoning_fields.insert(field.to_owned(), value.clone());
        }
    }
    let content = message.content.map(parse_content).transpose()?;
    let tool_calls = message
        .tool_calls
        .into_iter()
        .map(|call| {
            let arguments = serde_json::from_str(&call.function.arguments).map_err(|error| {
                ProviderError::InvalidResponse(format!("invalid tool arguments: {error}"))
            })?;
            Ok(ToolCall::new(call.id, call.function.name, arguments))
        })
        .collect::<Result<Vec<_>, ProviderError>>()?;

    if content.is_none() && tool_calls.is_empty() {
        return Err(ProviderError::InvalidResponse(
            "assistant message contains neither content nor tool calls".to_owned(),
        ));
    }
    Ok(ProviderResponse {
        message: AssistantMessage {
            content,
            tool_calls,
            provider_state: (!reasoning_fields.is_empty()).then_some(ProviderState::OpenAiChat {
                fields: reasoning_fields,
            }),
        },
        usage,
    })
}

fn normalize_chat_usage(usage: ChatUsage) -> TokenUsage {
    TokenUsage::with_total(
        usage.prompt_tokens,
        usage.completion_tokens,
        usage
            .total_tokens
            .unwrap_or_else(|| usage.prompt_tokens.saturating_add(usage.completion_tokens)),
        usage
            .prompt_tokens_details
            .map_or(0, |details| details.cached_tokens),
    )
}

#[cfg(test)]
fn parse_content(value: Value) -> Result<Content, ProviderError> {
    match value {
        Value::String(text) => Ok(Content::text(text)),
        Value::Array(items) => {
            let parts = items
                .into_iter()
                .filter_map(|item| {
                    if item.get("type")?.as_str()? == "text" {
                        item.get("text")?.as_str().map(ContentPart::text)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            Ok(Content::parts(parts))
        }
        Value::Null => Err(ProviderError::InvalidResponse("content is null".to_owned())),
        _ => Err(ProviderError::InvalidResponse(
            "unsupported assistant content".to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::types::{GenerationConfig, ImageDetail, ImageUrl, ReasoningEffort, ToolDefinition};

    #[test]
    fn maps_normalized_request_to_chat_completions() {
        let provider = OpenAiChatProvider::new("key", "https://example.com/v1", "model")
            .unwrap()
            .extra_body(json!({
                "chat_template_kwargs": { "enable_thinking": true },
                "model": "gateway-model"
            }))
            .unwrap();
        let mut assistant = Message::assistant(
            None,
            vec![ToolCall::new("call-1", "echo", json!({"text": "hi"}))],
        );
        assistant.provider_state = Some(ProviderState::OpenAiChat {
            fields: BTreeMap::from([
                ("reasoning_content".to_owned(), json!("think first")),
                (
                    "reasoning_details".to_owned(),
                    json!([{ "type": "reasoning.text", "text": "think first" }]),
                ),
            ]),
        });
        let request = ProviderRequest {
            messages: vec![
                Message::system("system"),
                Message::user_parts([
                    ContentPart::text("look"),
                    ContentPart::image(
                        ImageUrl::new("https://example.com/a.png").with_detail(ImageDetail::High),
                    ),
                ]),
                assistant,
                Message::tool_result("call-1", "hi", false),
            ],
            tools: vec![ToolDefinition::new(
                "echo",
                "echo",
                json!({"type": "object"}),
            )],
            config: GenerationConfig {
                temperature: Some(0.2),
                max_tokens: Some(100),
                reasoning_effort: Some(ReasoningEffort::High),
            },
        };

        let body = provider.request_body(&request);
        assert_eq!(body["temperature"], 0.2);
        assert_eq!(body["model"], "gateway-model");
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], true);
        assert_eq!(body["max_completion_tokens"], 100);
        assert_eq!(body["reasoning_effort"], "high");
        assert!(body.get("max_tokens").is_none());
        assert_eq!(body["messages"][1]["content"][1]["type"], "image_url");
        assert_eq!(
            body["messages"][2]["tool_calls"][0]["function"]["arguments"],
            r#"{"text":"hi"}"#
        );
        assert_eq!(body["messages"][2]["reasoning_content"], "think first");
        assert_eq!(
            body["messages"][2]["reasoning_details"][0]["type"],
            "reasoning.text"
        );
    }

    #[test]
    fn rejects_non_object_extra_body() {
        let error = OpenAiChatProvider::new("key", "https://example.com/v1", "model")
            .unwrap()
            .extra_body(json!(["not", "an", "object"]))
            .unwrap_err();

        assert!(matches!(error, ProviderError::InvalidConfiguration(_)));
    }

    #[test]
    fn normalizes_chat_token_usage() {
        let response: ChatResponse = serde_json::from_value(json!({
            "choices": [{ "message": { "content": "ok" } }],
            "usage": {
                "prompt_tokens": 80,
                "completion_tokens": 20,
                "total_tokens": 100,
                "prompt_tokens_details": { "cached_tokens": 30 }
            }
        }))
        .unwrap();

        let parsed = parse_response(response).unwrap();
        assert_eq!(parsed.usage, Some(TokenUsage::with_total(80, 20, 100, 30)));
    }

    #[test]
    fn replays_reasoning_content_for_ordinary_assistant_turns() {
        let provider = OpenAiChatProvider::new("key", "https://example.com/v1", "model").unwrap();
        let response: ChatResponse = serde_json::from_value(json!({
            "choices": [{
                "message": {
                    "content": "answer",
                    "reasoning_content": "private reasoning"
                }
            }]
        }))
        .unwrap();
        let parsed = parse_response(response).unwrap();
        let request = ProviderRequest {
            messages: vec![parsed.message.into_message(), Message::user("next")],
            tools: Vec::new(),
            config: GenerationConfig::default(),
        };

        let body = provider.request_body(&request);
        assert_eq!(
            body["messages"][0]["reasoning_content"],
            "private reasoning"
        );
        assert_eq!(body["messages"][0]["content"], "answer");
    }

    #[test]
    fn assembles_streamed_text_tool_arguments_and_usage() {
        let mut state = ChatStreamState::default();
        let deltas = state
            .apply(&json!({
                "choices": [{ "delta": {
                    "content": "hello",
                    "reasoning_content": "think ",
                    "reasoning_details": [{
                        "type": "reasoning.text",
                        "text": "think ",
                        "index": 0
                    }]
                } }]
            }))
            .unwrap();
        assert_eq!(
            deltas,
            vec![AssistantDelta::Text {
                delta: "hello".to_owned()
            }]
        );
        state
            .apply(&json!({
                "choices": [{ "delta": {
                    "reasoning_content": "before tool",
                    "reasoning_details": [{
                        "type": "reasoning.text",
                        "text": "before tool",
                        "index": 0
                    }],
                    "tool_calls": [{
                        "index": 0,
                        "id": "call-1",
                        "function": { "name": "echo", "arguments": "{\"text\":" }
                    }]
                } }]
            }))
            .unwrap();
        state
            .apply(&json!({
                "choices": [{ "delta": { "tool_calls": [{
                    "index": 0,
                    "function": { "arguments": "\"hi\"}" }
                }] } }],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 5,
                    "total_tokens": 15
                }
            }))
            .unwrap();

        let response = state.finish().unwrap();
        assert_eq!(response.message.content.unwrap().as_text(), Some("hello"));
        assert_eq!(
            response.message.tool_calls[0].arguments,
            json!({"text": "hi"})
        );
        let Some(ProviderState::OpenAiChat { fields }) = &response.message.provider_state else {
            panic!("missing chat reasoning state");
        };
        assert_eq!(fields["reasoning_content"], "think before tool");
        assert_eq!(fields["reasoning_details"].as_array().unwrap().len(), 2);
        assert_eq!(response.usage.unwrap().total_tokens, 15);
    }
}

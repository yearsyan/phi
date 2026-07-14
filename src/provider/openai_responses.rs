use async_stream::try_stream;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use reqwest::header::AUTHORIZATION;
use serde_json::{Value, json};

use crate::{
    error::ProviderError,
    hook::{BeforeRequestContext, Hook, HookRegistry, ProviderApi},
    provider::{
        ExtraBody, HttpRequestEvent, LlmProvider, ProviderEventStream, RetryConfig, header_value,
        json_headers, merge_extra_body, parse_extra_body, send_with_retry, text_only,
    },
    types::{
        AssistantDelta, AssistantMessage, Content, ContentPart, ProviderEvent, ProviderRequest,
        ProviderResponse, ProviderState, Role, TokenUsage, ToolCall,
    },
};

#[derive(Clone, Debug)]
pub struct OpenAiResponsesProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
    extra_body: ExtraBody,
    retry_config: RetryConfig,
    hooks: HookRegistry,
}

impl OpenAiResponsesProvider {
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

    pub fn openai(
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        Self::new(api_key, "https://api.openai.com/v1", model)
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

    /// Sets fixed JSON members to append to every Responses API request.
    ///
    /// The value must be a JSON object. Its top-level keys are applied after
    /// the adapter-generated request body, so they can deliberately override
    /// standard fields for compatible gateways.
    pub fn extra_body(mut self, extra_body: Value) -> Result<Self, ProviderError> {
        self.extra_body = parse_extra_body(extra_body)?;
        Ok(self)
    }

    fn request_body(&self, request: &ProviderRequest) -> Result<Value, ProviderError> {
        let mut instructions = Vec::new();
        let mut input = Vec::new();
        for message in &request.messages {
            match message.role {
                Role::System => instructions.push(text_only(message, "system")?),
                Role::User => input.push(json!({
                    "type": "message",
                    "role": "user",
                    "content": response_content(message.content.as_ref(), false)
                })),
                Role::Assistant => {
                    if let Some(ProviderState::OpenAiResponses { output }) = &message.provider_state
                    {
                        input.extend(output.iter().cloned());
                        continue;
                    }
                    if message.content.is_some() {
                        input.push(json!({
                            "type": "message",
                            "role": "assistant",
                            "content": response_content(message.content.as_ref(), true)
                        }));
                    }
                    input.extend(message.tool_calls.iter().map(|call| {
                        json!({
                            "type": "function_call",
                            "call_id": call.id,
                            "name": call.name,
                            "arguments": call.arguments.to_string()
                        })
                    }));
                }
                Role::Tool => input.push(json!({
                    "type": "function_call_output",
                    "call_id": message.tool_call_id,
                    "output": response_tool_output(message.content.as_ref())
                })),
            }
        }

        let tools = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters
                })
            })
            .collect::<Vec<_>>();
        let mut body = json!({ "model": self.model, "input": input, "stream": true });
        if !instructions.is_empty() {
            body["instructions"] = json!(instructions.join("\n"));
        }
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools);
        }
        if let Some(temperature) = request.config.temperature {
            body["temperature"] = json!(temperature);
        }
        if let Some(max_tokens) = request.config.max_tokens {
            body["max_output_tokens"] = json!(max_tokens);
        }
        if let Some(reasoning_effort) = request.config.reasoning_effort {
            body["reasoning"] = json!({ "effort": reasoning_effort.as_str() });
        }
        merge_extra_body(&mut body, &self.extra_body);
        Ok(body)
    }
}

impl LlmProvider for OpenAiResponsesProvider {
    fn stream(&self, request: ProviderRequest) -> ProviderEventStream {
        let body = self.request_body(&request);
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let endpoint = format!("{}/responses", self.base_url);
        let retry_config = self.retry_config;
        let hooks = self.hooks.clone();

        Box::pin(try_stream! {
            let body = body?;
            let mut headers = json_headers();
            headers.insert(
                AUTHORIZATION,
                header_value("authorization", &format!("Bearer {api_key}"))?,
            );
            let mut context = BeforeRequestContext {
                api: ProviderApi::OpenAiResponses,
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
            let mut completed = false;
            while let Some(event) = source.next().await {
                let event = event.map_err(|error| ProviderError::Stream(error.to_string()))?;
                let value: Value = serde_json::from_str(&event.data).map_err(|error| {
                    ProviderError::InvalidResponse(format!("invalid Responses stream event: {error}"))
                })?;
                match value["type"].as_str() {
                    Some("response.completed") => {
                        let response = value["response"].clone();
                        yield ProviderEvent::Done(parse_response(response)?);
                        completed = true;
                        break;
                    }
                    Some("response.failed" | "response.incomplete") => {
                        Err(ProviderError::InvalidResponse(value.to_string()))?;
                    }
                    _ => {
                        if let Some(delta) = response_delta(&value) {
                            yield ProviderEvent::Delta(delta);
                        }
                    }
                }
            }
            if !completed {
                Err(ProviderError::Stream(
                    "Responses stream ended without response.completed".to_owned(),
                ))?;
            }
        })
    }

    fn extend_hooks(&mut self, hooks: HookRegistry) {
        self.hooks.extend(hooks);
    }
}

fn response_delta(value: &Value) -> Option<AssistantDelta> {
    match value["type"].as_str()? {
        "response.output_text.delta" => Some(AssistantDelta::Text {
            delta: value["delta"].as_str()?.to_owned(),
        }),
        "response.output_item.added" if value["item"]["type"] == "function_call" => {
            Some(AssistantDelta::ToolCall {
                index: value["output_index"].as_u64().unwrap_or(0) as usize,
                id: value["item"]["call_id"].as_str().map(str::to_owned),
                name: value["item"]["name"].as_str().map(str::to_owned),
                arguments_delta: String::new(),
            })
        }
        "response.function_call_arguments.delta" => Some(AssistantDelta::ToolCall {
            index: value["output_index"].as_u64().unwrap_or(0) as usize,
            id: None,
            name: None,
            arguments_delta: value["delta"].as_str().unwrap_or_default().to_owned(),
        }),
        _ => None,
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

fn response_content(content: Option<&Content>, output: bool) -> Vec<Value> {
    let text_type = if output { "output_text" } else { "input_text" };
    match content {
        None => Vec::new(),
        Some(Content::Text(text)) => vec![json!({ "type": text_type, "text": text })],
        Some(Content::Parts(parts)) => parts
            .iter()
            .map(|part| match part {
                ContentPart::Text { text } => json!({ "type": text_type, "text": text }),
                ContentPart::ImageUrl { image_url } => json!({
                    "type": "input_image",
                    "image_url": image_url.url,
                    "detail": image_url.detail.map_or("auto", |detail| detail.as_str())
                }),
            })
            .collect(),
    }
}

fn response_tool_output(content: Option<&Content>) -> Value {
    match content {
        None => json!(""),
        Some(Content::Text(text)) => json!(text),
        Some(content) => Value::Array(response_content(Some(content), false)),
    }
}

fn parse_response(response: Value) -> Result<ProviderResponse, ProviderError> {
    let usage = response.get("usage").and_then(|usage| {
        let input = usage.get("input_tokens")?.as_u64()?;
        let output = usage.get("output_tokens")?.as_u64()?;
        let total = usage
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| input.saturating_add(output));
        let cached = usage
            .get("input_tokens_details")
            .and_then(|details| details.get("cached_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        Some(TokenUsage::with_total(input, output, total, cached))
    });
    let output = response["output"]
        .as_array()
        .ok_or_else(|| ProviderError::InvalidResponse("output is not an array".to_owned()))?;
    let raw_output = output.clone();
    let mut text = Vec::new();
    let mut tool_calls = Vec::new();
    for item in output {
        match item["type"].as_str() {
            Some("message") => {
                if let Some(content) = item["content"].as_array() {
                    text.extend(content.iter().filter_map(|part| {
                        (part["type"] == "output_text")
                            .then(|| part["text"].as_str().map(str::to_owned))
                            .flatten()
                    }));
                }
            }
            Some("function_call") => {
                let id = item["call_id"]
                    .as_str()
                    .ok_or_else(|| ProviderError::InvalidResponse("missing call_id".to_owned()))?;
                let name = item["name"].as_str().ok_or_else(|| {
                    ProviderError::InvalidResponse("missing tool name".to_owned())
                })?;
                let arguments =
                    serde_json::from_str(item["arguments"].as_str().ok_or_else(|| {
                        ProviderError::InvalidResponse("missing tool arguments".to_owned())
                    })?)
                    .map_err(|error| {
                        ProviderError::InvalidResponse(format!("invalid tool arguments: {error}"))
                    })?;
                tool_calls.push(ToolCall::new(id, name, arguments));
            }
            _ => {}
        }
    }

    let content = (!text.is_empty()).then(|| Content::text(text.join("")));
    if content.is_none() && tool_calls.is_empty() {
        return Err(ProviderError::InvalidResponse(
            "response contains neither output text nor function calls".to_owned(),
        ));
    }
    Ok(ProviderResponse {
        message: AssistantMessage {
            content,
            tool_calls,
            provider_state: Some(ProviderState::OpenAiResponses { output: raw_output }),
        },
        usage,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::types::{GenerationConfig, Message, ReasoningEffort, ToolDefinition};

    #[test]
    fn maps_normalized_request_to_responses_items() {
        let provider = OpenAiResponsesProvider::new("key", "https://example.com/v1", "model")
            .unwrap()
            .extra_body(json!({ "include": ["reasoning.encrypted_content"] }))
            .unwrap();
        let request = ProviderRequest {
            messages: vec![
                Message::system("system"),
                Message::user("hello"),
                Message::assistant(
                    None,
                    vec![ToolCall::new("call-1", "echo", json!({"text": "hi"}))],
                ),
                Message::tool("call-1", "hi"),
            ],
            tools: vec![ToolDefinition::new(
                "echo",
                "echo",
                json!({"type": "object"}),
            )],
            config: GenerationConfig {
                temperature: Some(0.1),
                max_tokens: Some(99),
                reasoning_effort: Some(ReasoningEffort::XHigh),
            },
        };

        let body = provider.request_body(&request).unwrap();
        assert_eq!(body["instructions"], "system");
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(body["max_output_tokens"], 99);
        assert_eq!(body["reasoning"]["effort"], "xhigh");
        assert_eq!(body["input"][1]["type"], "function_call");
        assert_eq!(body["input"][2]["type"], "function_call_output");
        assert_eq!(body["tools"][0]["name"], "echo");
    }

    #[test]
    fn replays_opaque_reasoning_items_from_the_assistant_message() {
        let provider =
            OpenAiResponsesProvider::new("key", "https://example.com/v1", "model").unwrap();
        let mut assistant = Message::assistant(
            None,
            vec![ToolCall::new("call-1", "echo", json!({"text": "hi"}))],
        );
        assistant.provider_state = Some(ProviderState::OpenAiResponses {
            output: vec![
                json!({ "type": "reasoning", "id": "rs_1", "summary": [] }),
                json!({
                    "type": "function_call",
                    "call_id": "call-1",
                    "name": "echo",
                    "arguments": "{\"text\":\"hi\"}"
                }),
            ],
        });
        let request = ProviderRequest {
            messages: vec![assistant, Message::tool("call-1", "hi")],
            tools: Vec::new(),
            config: GenerationConfig::default(),
        };

        let body = provider.request_body(&request).unwrap();
        assert_eq!(body["input"][0]["type"], "reasoning");
        assert_eq!(body["input"][1]["type"], "function_call");
        assert_eq!(body["input"][2]["type"], "function_call_output");
    }

    #[test]
    fn replays_reasoning_items_for_ordinary_assistant_turns() {
        let provider =
            OpenAiResponsesProvider::new("key", "https://example.com/v1", "model").unwrap();
        let parsed = parse_response(json!({
            "output": [
                {
                    "type": "reasoning",
                    "id": "rs_1",
                    "encrypted_content": "opaque"
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": "answer" }]
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
        assert_eq!(body["input"][0]["type"], "reasoning");
        assert_eq!(body["input"][1]["type"], "message");
        assert_eq!(body["input"][2]["role"], "user");
    }

    #[test]
    fn normalizes_responses_token_usage() {
        let parsed = parse_response(json!({
            "output": [{
                "type": "message",
                "content": [{ "type": "output_text", "text": "ok" }]
            }],
            "usage": {
                "input_tokens": 90,
                "output_tokens": 10,
                "total_tokens": 100,
                "input_tokens_details": { "cached_tokens": 40 }
            }
        }))
        .unwrap();

        assert_eq!(parsed.usage, Some(TokenUsage::with_total(90, 10, 100, 40)));
    }

    #[test]
    fn maps_responses_stream_deltas() {
        assert_eq!(
            response_delta(&json!({
                "type": "response.output_text.delta",
                "delta": "hello"
            })),
            Some(AssistantDelta::Text {
                delta: "hello".to_owned()
            })
        );
        assert_eq!(
            response_delta(&json!({
                "type": "response.function_call_arguments.delta",
                "output_index": 2,
                "delta": "{\"x\":"
            })),
            Some(AssistantDelta::ToolCall {
                index: 2,
                id: None,
                name: None,
                arguments_delta: "{\"x\":".to_owned()
            })
        );
    }
}

use std::{collections::HashMap, sync::Arc};

use futures_util::{StreamExt, stream::FuturesUnordered};

use crate::{
    error::{AgentError, HookError, McpError, ProviderError},
    hook::{Hook, HookRegistry, LlmResponseContext, TurnEndContext, TurnStartContext},
    mcp::{McpClient, McpHttpConfig, McpStdioConfig},
    provider::LlmProvider,
    storage::{SessionSnapshot, SessionStorage, StorageError, validate_session_id},
    tool::{Tool, ToolOutput, builtins::BuiltinTools},
    types::{
        AgentEvent, AgentRun, Content, ContentPart, ContextUsage, GenerationConfig, ImageUrl,
        Message, ProviderEvent, ProviderRequest, ReasoningEffort, TokenUsage, ToolCall,
        ToolDefinition, ToolExecutionMode,
    },
};

type EventListener = Arc<dyn Fn(&AgentEvent) + Send + Sync>;

pub struct AgentBuilder {
    provider: Box<dyn LlmProvider>,
    system_prompt: String,
    tools: Vec<Arc<dyn Tool>>,
    max_turns: usize,
    tool_execution: ToolExecutionMode,
    generation_config: GenerationConfig,
    max_context_tokens: Option<u64>,
    hooks: HookRegistry,
}

impl AgentBuilder {
    pub fn new(provider: impl LlmProvider + 'static) -> Self {
        Self {
            provider: Box::new(provider),
            system_prompt: "You are a helpful assistant.".to_owned(),
            tools: Vec::new(),
            max_turns: 16,
            tool_execution: ToolExecutionMode::Parallel,
            generation_config: GenerationConfig::default(),
            max_context_tokens: None,
            hooks: HookRegistry::default(),
        }
    }

    pub fn system_prompt(mut self, system_prompt: impl Into<String>) -> Self {
        self.system_prompt = system_prompt.into();
        self
    }

    pub fn tool(mut self, tool: impl Tool + 'static) -> Self {
        self.tools.push(Arc::new(tool));
        self
    }

    /// Installs an explicitly selected set of built-in local tools.
    pub fn builtin_tools(mut self, tools: BuiltinTools) -> Self {
        self.tools.extend(tools.into_tools());
        self
    }

    /// Installs the built-in read, bash, edit, and write tools for `cwd`.
    pub fn all_builtin_tools(self, cwd: impl Into<std::path::PathBuf>) -> Self {
        self.builtin_tools(BuiltinTools::all(cwd))
    }

    /// Installs the tools discovered by an already connected MCP client.
    pub fn mcp_client(mut self, client: McpClient) -> Self {
        self.tools.extend(client.into_tools());
        self
    }

    /// Connects a stdio MCP server and installs its discovered tools.
    pub async fn mcp_stdio(self, config: McpStdioConfig) -> Result<Self, McpError> {
        let client = McpClient::connect_stdio(config).await?;
        Ok(self.mcp_client(client))
    }

    /// Connects a Streamable HTTP MCP server and installs its discovered tools.
    pub async fn mcp_http(self, config: McpHttpConfig) -> Result<Self, McpError> {
        let client = McpClient::connect_http(config).await?;
        Ok(self.mcp_client(client))
    }

    pub fn max_turns(mut self, max_turns: usize) -> Self {
        self.max_turns = max_turns.max(1);
        self
    }

    pub fn tool_execution(mut self, mode: ToolExecutionMode) -> Self {
        self.tool_execution = mode;
        self
    }

    pub fn generation_config(mut self, config: GenerationConfig) -> Self {
        self.generation_config = config;
        self
    }

    pub fn temperature(mut self, temperature: f64) -> Self {
        self.generation_config.temperature = Some(temperature);
        self
    }

    pub fn max_tokens(mut self, max_tokens: u32) -> Self {
        self.generation_config.max_tokens = Some(max_tokens);
        self
    }

    pub fn reasoning_effort(mut self, reasoning_effort: ReasoningEffort) -> Self {
        self.generation_config.reasoning_effort = Some(reasoning_effort);
        self
    }

    pub fn max_context_tokens(mut self, max_context_tokens: u64) -> Self {
        self.max_context_tokens = Some(max_context_tokens);
        self
    }

    /// Registers an asynchronous lifecycle hook. Hooks run sequentially in
    /// registration order and are also injected into built-in HTTP providers.
    pub fn hook(mut self, hook: impl Hook + 'static) -> Self {
        self.hooks.register(hook);
        self
    }

    pub fn hooks(mut self, hooks: HookRegistry) -> Self {
        self.hooks.extend(hooks);
        self
    }

    pub fn build(mut self) -> Agent {
        if !self.hooks.is_empty() {
            self.provider.extend_hooks(self.hooks.clone());
        }
        let tools = self
            .tools
            .into_iter()
            .map(|tool| (tool.definition().name, tool))
            .collect();

        Agent {
            provider: Arc::from(self.provider),
            system_prompt: self.system_prompt,
            tools,
            messages: Vec::new(),
            listeners: Vec::new(),
            max_turns: self.max_turns,
            tool_execution: self.tool_execution,
            generation_config: self.generation_config,
            max_context_tokens: self.max_context_tokens,
            last_usage: None,
            context_usage: None,
            cumulative_usage: TokenUsage::default(),
            session: None,
            hooks: self.hooks,
        }
    }
}

/// A stateful agent that owns its transcript, emits events, and executes tools.
pub struct Agent {
    provider: Arc<dyn LlmProvider>,
    system_prompt: String,
    tools: HashMap<String, Arc<dyn Tool>>,
    messages: Vec<Message>,
    listeners: Vec<EventListener>,
    max_turns: usize,
    tool_execution: ToolExecutionMode,
    generation_config: GenerationConfig,
    max_context_tokens: Option<u64>,
    last_usage: Option<TokenUsage>,
    context_usage: Option<ContextUsage>,
    cumulative_usage: TokenUsage,
    session: Option<SessionBinding>,
    hooks: HookRegistry,
}

impl Agent {
    pub fn builder(provider: impl LlmProvider + 'static) -> AgentBuilder {
        AgentBuilder::new(provider)
    }

    pub fn subscribe(&mut self, listener: impl Fn(&AgentEvent) + Send + Sync + 'static) {
        self.listeners.push(Arc::new(listener));
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn clear_messages(&mut self) {
        self.messages.clear();
        self.last_usage = None;
        self.context_usage = None;
    }

    pub fn last_usage(&self) -> Option<TokenUsage> {
        self.last_usage
    }

    pub fn context_usage(&self) -> Option<ContextUsage> {
        self.context_usage
    }

    pub fn cumulative_usage(&self) -> TokenUsage {
        self.cumulative_usage
    }

    /// Loads an existing session, or attaches the current state to a new ID.
    pub async fn attach_session<S>(
        &mut self,
        session_id: impl Into<String>,
        storage: S,
    ) -> Result<(), AgentError>
    where
        S: SessionStorage + 'static,
    {
        let session_id = session_id.into();
        validate_session_id(&session_id)?;
        let storage: Arc<dyn SessionStorage> = Arc::new(storage);

        if let Some(snapshot) = storage.load(&session_id).await? {
            self.messages = snapshot.messages;
            self.last_usage = snapshot.last_usage;
            self.cumulative_usage = snapshot.cumulative_usage;
            self.context_usage = self.last_usage.and_then(|usage| {
                self.max_context_tokens
                    .map(|max_tokens| ContextUsage::from_usage(max_tokens, usage))
            });
        }

        self.session = Some(SessionBinding {
            id: session_id,
            storage,
        });
        Ok(())
    }

    /// Consuming convenience form of [`Agent::attach_session`].
    pub async fn with_session<S>(
        mut self,
        session_id: impl Into<String>,
        storage: S,
    ) -> Result<Self, AgentError>
    where
        S: SessionStorage + 'static,
    {
        self.attach_session(session_id, storage).await?;
        Ok(self)
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session.as_ref().map(|session| session.id.as_str())
    }

    pub async fn prompt(&mut self, prompt: impl Into<String>) -> Result<AgentRun, AgentError> {
        self.prompt_content(Content::text(prompt)).await
    }

    pub async fn prompt_with_images(
        &mut self,
        prompt: impl Into<String>,
        images: Vec<ImageUrl>,
    ) -> Result<AgentRun, AgentError> {
        let mut parts = Vec::with_capacity(images.len() + 1);
        parts.push(ContentPart::text(prompt));
        parts.extend(images.into_iter().map(ContentPart::image));
        self.prompt_content(Content::parts(parts)).await
    }

    pub async fn prompt_content(&mut self, content: Content) -> Result<AgentRun, AgentError> {
        let start_index = self.messages.len();
        let mut run_usage = TokenUsage::default();
        let user_message = Message::user_content(content);

        self.emit(AgentEvent::AgentStart);
        self.emit(AgentEvent::MessageStart {
            message: user_message.clone(),
        });
        self.messages.push(user_message.clone());
        self.emit(AgentEvent::MessageEnd {
            message: user_message,
        });
        self.synchronize_session_or_end().await?;

        for turn in 1..=self.max_turns {
            self.emit(AgentEvent::TurnStart { turn });

            let mut turn_start = TurnStartContext {
                turn,
                request: ProviderRequest {
                    messages: std::iter::once(Message::system(self.system_prompt.clone()))
                        .chain(self.messages.iter().cloned())
                        .collect(),
                    tools: self.tool_definitions(),
                    config: self.generation_config.clone(),
                },
            };
            if let Err(error) = self.hooks.run_turn_start(&mut turn_start).await {
                return Err(self.hook_failure(error));
            }

            let mut stream = self.provider.stream(turn_start.request);
            let mut message_started = false;
            let response = loop {
                match stream.next().await {
                    Some(Ok(ProviderEvent::Retry(event))) => {
                        self.emit(AgentEvent::ProviderRetry { event });
                    }
                    Some(Ok(ProviderEvent::Delta(delta))) => {
                        if !message_started {
                            self.emit(AgentEvent::MessageStart {
                                message: Message::assistant(None, Vec::new()),
                            });
                            message_started = true;
                        }
                        self.emit(AgentEvent::MessageUpdate { delta });
                    }
                    Some(Ok(ProviderEvent::Done(response))) => {
                        if !message_started {
                            self.emit(AgentEvent::MessageStart {
                                message: Message::assistant(None, Vec::new()),
                            });
                        }
                        break response;
                    }
                    Some(Err(error)) => {
                        self.emit(AgentEvent::Error {
                            message: error.to_string(),
                        });
                        self.emit_agent_end();
                        return Err(error.into());
                    }
                    None => {
                        let error = ProviderError::Stream(
                            "provider stream ended without a final response".to_owned(),
                        );
                        self.emit(AgentEvent::Error {
                            message: error.to_string(),
                        });
                        self.emit_agent_end();
                        return Err(error.into());
                    }
                }
            };

            let mut llm_response = LlmResponseContext { turn, response };
            if let Err(error) = self.hooks.run_llm_response(&mut llm_response).await {
                return Err(self.hook_failure(error));
            }
            let response = llm_response.response;

            self.last_usage = response.usage;
            self.context_usage = response.usage.and_then(|usage| {
                self.max_context_tokens
                    .map(|max_tokens| ContextUsage::from_usage(max_tokens, usage))
            });
            if let Some(usage) = response.usage {
                run_usage += usage;
                self.cumulative_usage += usage;
                self.emit(AgentEvent::UsageUpdate {
                    usage,
                    context_usage: self.context_usage,
                });
            }

            let tool_calls = response.message.tool_calls.clone();
            let assistant_message = response.message.into_message();
            let assistant_index = self.messages.len();
            self.messages.push(assistant_message.clone());
            self.emit(AgentEvent::MessageEnd {
                message: assistant_message.clone(),
            });
            self.synchronize_session_or_end().await?;

            let has_tool_calls = !tool_calls.is_empty();
            let tool_results = if has_tool_calls {
                self.execute_tool_calls(tool_calls)
                    .await
                    .into_iter()
                    .map(|execution| {
                        Message::tool_result(
                            execution.call.id,
                            execution.output.content,
                            execution.output.is_error,
                        )
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let original_message = assistant_message.clone();
            let original_tool_results = tool_results.clone();
            let mut turn_end = TurnEndContext {
                turn,
                message: assistant_message,
                tool_results,
            };
            if let Err(error) = self.hooks.run_turn_end(&mut turn_end).await {
                return Err(self.hook_failure(error));
            }
            let turn_output_changed = turn_end.message != original_message
                || turn_end.tool_results != original_tool_results;

            self.messages[assistant_index] = turn_end.message.clone();
            for message in &turn_end.tool_results {
                self.emit(AgentEvent::MessageStart {
                    message: message.clone(),
                });
                self.messages.push(message.clone());
                self.emit(AgentEvent::MessageEnd {
                    message: message.clone(),
                });
            }
            if turn_output_changed {
                self.synchronize_session_or_end().await?;
            }

            self.emit(AgentEvent::TurnEnd {
                turn,
                message: turn_end.message.clone(),
                tool_results: turn_end.tool_results,
            });

            if !has_tool_calls {
                let new_messages = self.messages[start_index..].to_vec();
                self.emit_agent_end();
                return Ok(AgentRun {
                    final_message: turn_end.message,
                    new_messages,
                    turns: turn,
                    run_usage,
                    context_usage: self.context_usage,
                });
            }
        }

        let error = AgentError::MaxTurnsExceeded {
            max_turns: self.max_turns,
        };
        self.emit(AgentEvent::Error {
            message: error.to_string(),
        });
        self.emit_agent_end();
        Err(error)
    }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|tool| tool.definition()).collect()
    }

    async fn synchronize_session_or_end(&self) -> Result<(), AgentError> {
        if let Err(error) = self.synchronize_session().await {
            self.emit(AgentEvent::Error {
                message: error.to_string(),
            });
            self.emit_agent_end();
            return Err(error.into());
        }
        Ok(())
    }

    async fn synchronize_session(&self) -> Result<(), StorageError> {
        let Some(session) = &self.session else {
            return Ok(());
        };
        session
            .storage
            .save(&SessionSnapshot {
                id: session.id.clone(),
                messages: self.messages.clone(),
                last_usage: self.last_usage,
                cumulative_usage: self.cumulative_usage,
            })
            .await
    }

    async fn execute_tool_calls(&self, calls: Vec<ToolCall>) -> Vec<ExecutedTool> {
        match self.tool_execution {
            ToolExecutionMode::Sequential => {
                let mut results = Vec::with_capacity(calls.len());
                for call in calls {
                    self.emit(AgentEvent::ToolExecutionStart { call: call.clone() });
                    let executed = Self::execute_one(self.tools.get(&call.name), call).await;
                    self.emit_tool_end(&executed);
                    results.push(executed);
                }
                results
            }
            ToolExecutionMode::Parallel => {
                let mut pending = FuturesUnordered::new();
                let count = calls.len();

                for (index, call) in calls.into_iter().enumerate() {
                    self.emit(AgentEvent::ToolExecutionStart { call: call.clone() });
                    let tool = self.tools.get(&call.name).cloned();
                    pending.push(async move {
                        let executed = Self::execute_one(tool.as_ref(), call).await;
                        (index, executed)
                    });
                }

                let mut ordered = vec![None; count];
                while let Some((index, executed)) = pending.next().await {
                    self.emit_tool_end(&executed);
                    ordered[index] = Some(executed);
                }

                ordered.into_iter().flatten().collect()
            }
        }
    }

    async fn execute_one(tool: Option<&Arc<dyn Tool>>, call: ToolCall) -> ExecutedTool {
        let arguments = call.arguments.clone();

        let output = match tool {
            Some(tool) => match tool.execute(arguments).await {
                Ok(output) => output,
                Err(error) => ToolOutput::error(error.to_string()),
            },
            None => ToolOutput::error(format!("unknown tool: {}", call.name)),
        };

        ExecutedTool { call, output }
    }

    fn emit_tool_end(&self, executed: &ExecutedTool) {
        self.emit(AgentEvent::ToolExecutionEnd {
            call: executed.call.clone(),
            content: executed.output.content.clone(),
            is_error: executed.output.is_error,
        });
    }

    fn hook_failure(&self, error: HookError) -> AgentError {
        let error = AgentError::from(error);
        self.emit(AgentEvent::Error {
            message: error.to_string(),
        });
        self.emit_agent_end();
        error
    }

    fn emit_agent_end(&self) {
        self.emit(AgentEvent::AgentEnd {
            messages: self.messages.clone(),
        });
    }

    fn emit(&self, event: AgentEvent) {
        for listener in &self.listeners {
            listener(&event);
        }
    }
}

#[derive(Clone, Debug)]
struct ExecutedTool {
    call: ToolCall,
    output: ToolOutput,
}

struct SessionBinding {
    id: String,
    storage: Arc<dyn SessionStorage>,
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::{
        error::{ProviderError, ToolError},
        provider::ProviderEventStream,
        storage::{InMemorySessionStorage, SessionSnapshot, SessionStorage, StorageError},
        tool::Tool,
        types::{
            AssistantDelta, AssistantMessage, ProviderResponse, ProviderRetryEvent,
            ProviderRetryReason, Role, TokenUsage, ToolCall, ToolDefinition,
        },
    };

    struct MockProvider {
        responses: Mutex<VecDeque<ProviderResponse>>,
    }

    impl LlmProvider for MockProvider {
        fn stream(&self, _request: ProviderRequest) -> ProviderEventStream {
            let response =
                self.responses.lock().unwrap().pop_front().ok_or_else(|| {
                    ProviderError::InvalidResponse("mock queue is empty".to_owned())
                });
            let events = match response {
                Ok(response) => {
                    let mut events = Vec::new();
                    if let Some(text) = response.message.content.as_ref().and_then(Content::as_text)
                    {
                        events.push(Ok(ProviderEvent::Delta(AssistantDelta::Text {
                            delta: text.to_owned(),
                        })));
                    }
                    events.push(Ok(ProviderEvent::Done(response)));
                    events
                }
                Err(error) => vec![Err(error)],
            };
            Box::pin(futures_util::stream::iter(events))
        }
    }

    struct RetryingMockProvider;

    impl LlmProvider for RetryingMockProvider {
        fn stream(&self, _request: ProviderRequest) -> ProviderEventStream {
            Box::pin(futures_util::stream::iter([
                Ok(ProviderEvent::Retry(ProviderRetryEvent {
                    retry_number: 1,
                    max_retries: 10,
                    delay: Duration::from_millis(200),
                    reason: ProviderRetryReason::HttpStatus {
                        status: 503,
                        body: "temporarily unavailable".to_owned(),
                    },
                })),
                Ok(ProviderEvent::Done(ProviderResponse {
                    message: AssistantMessage::text("recovered"),
                    usage: None,
                })),
            ]))
        }
    }

    struct RecordingProvider {
        response: Mutex<Option<ProviderResponse>>,
        requests: Arc<Mutex<Vec<ProviderRequest>>>,
    }

    impl LlmProvider for RecordingProvider {
        fn stream(&self, request: ProviderRequest) -> ProviderEventStream {
            self.requests.lock().unwrap().push(request);
            let response = self.response.lock().unwrap().take().ok_or_else(|| {
                ProviderError::InvalidResponse("recording response is missing".to_owned())
            });
            Box::pin(futures_util::stream::iter([
                response.map(ProviderEvent::Done)
            ]))
        }
    }

    struct LifecycleHook {
        stages: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl Hook for LifecycleHook {
        async fn on_turn_start(&self, context: &mut TurnStartContext) -> Result<(), HookError> {
            tokio::task::yield_now().await;
            self.stages.lock().unwrap().push("turn_start");
            context.request.messages.last_mut().unwrap().content =
                Some(Content::text("request changed by hook"));
            Ok(())
        }

        async fn on_llm_response(&self, context: &mut LlmResponseContext) -> Result<(), HookError> {
            tokio::task::yield_now().await;
            self.stages.lock().unwrap().push("llm_response");
            context.response.message.content = Some(Content::text("response changed by hook"));
            Ok(())
        }

        async fn on_turn_end(&self, context: &mut TurnEndContext) -> Result<(), HookError> {
            tokio::task::yield_now().await;
            self.stages.lock().unwrap().push("turn_end");
            context.message.content = Some(Content::text("turn changed by hook"));
            Ok(())
        }
    }

    struct EchoTool;

    #[derive(Clone, Default)]
    struct RecordingStorage {
        snapshots: Arc<Mutex<Vec<SessionSnapshot>>>,
    }

    #[async_trait]
    impl SessionStorage for RecordingStorage {
        async fn load(&self, _session_id: &str) -> Result<Option<SessionSnapshot>, StorageError> {
            Ok(None)
        }

        async fn save(&self, session: &SessionSnapshot) -> Result<(), StorageError> {
            self.snapshots.lock().unwrap().push(session.clone());
            Ok(())
        }

        async fn delete(&self, _session_id: &str) -> Result<(), StorageError> {
            Ok(())
        }
    }

    #[async_trait]
    impl Tool for EchoTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new(
                "echo",
                "Returns the supplied text",
                json!({
                    "type": "object",
                    "properties": { "text": { "type": "string" } },
                    "required": ["text"]
                }),
            )
        }

        async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
            let text = arguments["text"]
                .as_str()
                .ok_or_else(|| ToolError::new("text is required"))?;
            Ok(ToolOutput::success(text))
        }
    }

    #[test]
    fn built_in_tools_are_disabled_until_explicitly_enabled() {
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::new()),
        };
        let agent = Agent::builder(provider).build();
        assert!(agent.tools.is_empty());

        let directory = tempfile::tempdir().unwrap();
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::new()),
        };
        let agent = Agent::builder(provider)
            .builtin_tools(BuiltinTools::all(directory.path()))
            .build();
        let mut names = agent.tools.keys().map(String::as_str).collect::<Vec<_>>();
        names.sort_unstable();
        assert_eq!(names, ["bash", "edit", "read", "write"]);
    }

    #[tokio::test]
    async fn performs_tool_calls_until_the_model_returns_text() {
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::from([
                ProviderResponse {
                    message: AssistantMessage::tool_calls(vec![ToolCall::new(
                        "call-1",
                        "echo",
                        json!({"text": "hello"}),
                    )]),
                    usage: Some(TokenUsage::new(50, 5, 0)),
                },
                ProviderResponse {
                    message: AssistantMessage::text("done"),
                    usage: Some(TokenUsage::new(100, 10, 0)),
                },
            ])),
        };
        let tool_ends = Arc::new(AtomicUsize::new(0));
        let observed_tool_ends = Arc::clone(&tool_ends);
        let message_updates = Arc::new(AtomicUsize::new(0));
        let observed_message_updates = Arc::clone(&message_updates);
        let mut agent = Agent::builder(provider)
            .tool(EchoTool)
            .tool_execution(ToolExecutionMode::Sequential)
            .max_context_tokens(1_000)
            .build();
        agent.subscribe(move |event| {
            if matches!(event, AgentEvent::ToolExecutionEnd { .. }) {
                observed_tool_ends.fetch_add(1, Ordering::Relaxed);
            }
            if matches!(event, AgentEvent::MessageUpdate { .. }) {
                observed_message_updates.fetch_add(1, Ordering::Relaxed);
            }
        });

        let result = agent.prompt("echo hello").await.unwrap();

        assert_eq!(result.text(), Some("done"));
        assert_eq!(result.turns, 2);
        assert_eq!(agent.messages().len(), 4);
        assert_eq!(agent.messages()[2].text_content(), Some("hello"));
        assert_eq!(tool_ends.load(Ordering::Relaxed), 1);
        assert_eq!(message_updates.load(Ordering::Relaxed), 1);
        assert_eq!(result.run_usage.total_tokens, 165);
        assert_eq!(result.context_usage.unwrap().remaining_tokens, 890);
        assert_eq!(agent.cumulative_usage().total_tokens, 165);
    }

    #[tokio::test]
    async fn forwards_provider_retry_events_to_subscribers() {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::clone(&observed);
        let mut agent = Agent::builder(RetryingMockProvider).build();
        agent.subscribe(move |event| {
            if let AgentEvent::ProviderRetry { event } = event {
                events.lock().unwrap().push(event.clone());
            }
        });

        let result = agent.prompt("hello").await.unwrap();

        assert_eq!(result.text(), Some("recovered"));
        assert_eq!(
            observed.lock().unwrap().as_slice(),
            [ProviderRetryEvent {
                retry_number: 1,
                max_retries: 10,
                delay: Duration::from_millis(200),
                reason: ProviderRetryReason::HttpStatus {
                    status: 503,
                    body: "temporarily unavailable".to_owned(),
                },
            }]
        );
    }

    #[tokio::test]
    async fn runs_async_lifecycle_hooks_and_applies_their_mutations() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stages = Arc::new(Mutex::new(Vec::new()));
        let provider = RecordingProvider {
            response: Mutex::new(Some(ProviderResponse {
                message: AssistantMessage::text("original response"),
                usage: None,
            })),
            requests: Arc::clone(&requests),
        };
        let mut agent = Agent::builder(provider)
            .hook(LifecycleHook {
                stages: Arc::clone(&stages),
            })
            .build();

        let run = agent.prompt("original request").await.unwrap();

        assert_eq!(run.text(), Some("turn changed by hook"));
        assert_eq!(agent.messages()[1].text_content(), run.text());
        assert_eq!(
            requests.lock().unwrap()[0]
                .messages
                .last()
                .unwrap()
                .text_content(),
            Some("request changed by hook")
        );
        assert_eq!(
            stages.lock().unwrap().as_slice(),
            ["turn_start", "llm_response", "turn_end"]
        );
    }

    #[tokio::test]
    async fn synchronizes_after_user_and_each_complete_llm_response() {
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::from([
                ProviderResponse {
                    message: AssistantMessage::tool_calls(vec![ToolCall::new(
                        "call-1",
                        "echo",
                        json!({"text": "hello"}),
                    )]),
                    usage: Some(TokenUsage::new(10, 2, 0)),
                },
                ProviderResponse {
                    message: AssistantMessage::text("done"),
                    usage: Some(TokenUsage::new(20, 3, 0)),
                },
            ])),
        };
        let storage = RecordingStorage::default();
        let observed = storage.clone();
        let mut agent = Agent::builder(provider)
            .tool(EchoTool)
            .tool_execution(ToolExecutionMode::Sequential)
            .build()
            .with_session("session-1", storage)
            .await
            .unwrap();

        agent.prompt("echo hello").await.unwrap();

        let snapshots = observed.snapshots.lock().unwrap();
        assert_eq!(snapshots.len(), 3);
        assert_eq!(snapshots[0].messages.len(), 1);
        assert_eq!(snapshots[0].messages[0].role, Role::User);
        assert_eq!(snapshots[1].messages.len(), 2);
        assert_eq!(snapshots[1].messages[1].role, Role::Assistant);
        assert_eq!(snapshots[1].messages[1].tool_calls.len(), 1);
        assert_eq!(snapshots[2].messages.len(), 4);
        assert_eq!(snapshots[2].messages[2].role, Role::Tool);
        assert_eq!(snapshots[2].messages[3].role, Role::Assistant);
        assert_eq!(snapshots[2].cumulative_usage.total_tokens, 35);
    }

    #[tokio::test]
    async fn restores_messages_and_usage_when_attaching_a_session() {
        let storage = InMemorySessionStorage::new();
        storage
            .save(&SessionSnapshot {
                id: "saved".to_owned(),
                messages: vec![Message::user("before restart")],
                last_usage: Some(TokenUsage::new(100, 20, 0)),
                cumulative_usage: TokenUsage::new(250, 50, 0),
            })
            .await
            .unwrap();
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::new()),
        };

        let agent = Agent::builder(provider)
            .max_context_tokens(1_000)
            .build()
            .with_session("saved", storage)
            .await
            .unwrap();

        assert_eq!(agent.session_id(), Some("saved"));
        assert_eq!(agent.messages()[0].text_content(), Some("before restart"));
        assert_eq!(agent.last_usage().unwrap().total_tokens, 120);
        assert_eq!(agent.context_usage().unwrap().remaining_tokens, 880);
        assert_eq!(agent.cumulative_usage().total_tokens, 300);
    }
}

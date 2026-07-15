use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};

use futures_util::{StreamExt, stream::FuturesUnordered};
use tokio::sync::watch;

use crate::{
    context::{
        ContextManagementContext, ContextManagementTrigger, ContextManager, ContextManagerRegistry,
        DEFAULT_CONTEXT_MANAGEMENT_THRESHOLD_PERCENT, threshold_reached,
    },
    error::{AgentError, HookError, McpError, ProviderError},
    hook::{Hook, HookRegistry, LlmResponseContext, TurnEndContext, TurnStartContext},
    mcp::{McpClient, McpHttpConfig, McpStdioConfig},
    provider::LlmProvider,
    storage::{SessionSnapshot, SessionStorage, StorageError, validate_session_id},
    tool::{
        AgentMode, AgentModeControl, Tool, ToolCancellation, ToolConcurrency, ToolEffect,
        ToolExecutionContext, ToolOutput, ToolProgress, builtins::BuiltinTools,
    },
    types::{
        AgentEvent, AgentRun, AgentRunOutcome, AssistantDelta, Content, ContentPart, ContextUsage,
        GenerationConfig, ImageUrl, Message, ProviderEvent, ProviderRequest, ReasoningEffort, Role,
        TokenUsage, ToolCall, ToolDefinition, ToolExecutionMode,
    },
};

type EventListener = Arc<dyn Fn(&AgentEvent) + Send + Sync>;
const CANCELLED_TOOL_RESULT: &str = "tool execution cancelled before it started";
const INTERRUPTED_TOOL_RESULT: &str =
    "tool execution was interrupted before its result was persisted";
const UNKNOWN_TOOL_RESULT: &str = "tool execution outcome is unknown; it may have produced side effects and will not be retried automatically";
pub const DEFAULT_TOOL_CALL_TIMEOUT: Duration = Duration::from_secs(300);
pub const DEFAULT_MAX_PARALLEL_TOOLS: usize = 8;
const PLAN_MODE_SYSTEM_REMINDER: &str = "You are in Plan mode. Explore and design only. Do not modify workspace files, run commands with side effects, or perform external side effects. Use only read-only, internal, and plan-only tools until the plan is approved.";

/// Cooperative stop signal for a single agent run.
///
/// A control may be cloned and stopped from another task. Stopping is
/// idempotent. The active provider stream, lifecycle hook, or tool future is
/// dropped so the run can return to its last protocol-safe checkpoint without
/// waiting indefinitely for user-supplied asynchronous work.
#[derive(Clone, Debug)]
pub struct AgentRunControl {
    stopped: watch::Sender<bool>,
}

impl AgentRunControl {
    pub fn new() -> Self {
        let (stopped, _) = watch::channel(false);
        Self { stopped }
    }

    pub fn stop(&self) {
        self.stopped.send_replace(true);
    }

    pub fn is_stopped(&self) -> bool {
        *self.stopped.borrow()
    }

    pub async fn stopped(&self) {
        if self.is_stopped() {
            return;
        }

        let mut stopped = self.stopped.subscribe();
        while !*stopped.borrow_and_update() {
            if stopped.changed().await.is_err() {
                return;
            }
        }
    }

    fn tool_cancellation(&self) -> ToolCancellation {
        ToolCancellation::from_sender(&self.stopped)
    }
}

impl Default for AgentRunControl {
    fn default() -> Self {
        Self::new()
    }
}

pub struct AgentBuilder {
    provider: Box<dyn LlmProvider>,
    system_prompt: String,
    tools: Vec<Arc<dyn Tool>>,
    tool_execution: ToolExecutionMode,
    tool_call_timeout: Option<Duration>,
    max_parallel_tools: usize,
    mode: AgentMode,
    generation_config: GenerationConfig,
    max_context_tokens: Option<u64>,
    context_management_threshold_percent: u8,
    context_managers: ContextManagerRegistry,
    hooks: HookRegistry,
}

impl AgentBuilder {
    pub fn new(provider: impl LlmProvider + 'static) -> Self {
        Self {
            provider: Box::new(provider),
            system_prompt: "You are a helpful assistant.".to_owned(),
            tools: Vec::new(),
            tool_execution: ToolExecutionMode::Parallel,
            tool_call_timeout: Some(DEFAULT_TOOL_CALL_TIMEOUT),
            max_parallel_tools: DEFAULT_MAX_PARALLEL_TOOLS,
            mode: AgentMode::default(),
            generation_config: GenerationConfig::default(),
            max_context_tokens: None,
            context_management_threshold_percent: DEFAULT_CONTEXT_MANAGEMENT_THRESHOLD_PERCENT,
            context_managers: ContextManagerRegistry::default(),
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

    /// Installs the progressive-disclosure tool for an immutable skill catalog.
    pub fn skills(mut self, catalog: crate::skills::SkillCatalog) -> Self {
        if catalog.has_model_invocable() {
            self.tools
                .push(Arc::new(crate::skills::SkillTool::new(catalog)));
        }
        self
    }

    /// Loads skills from explicit directories and installs their tool.
    pub async fn load_skills(
        self,
        config: &crate::skills::SkillsConfig,
    ) -> Result<Self, crate::skills::SkillError> {
        let catalog = crate::skills::SkillCatalog::load(config).await?;
        Ok(self.skills(catalog))
    }

    pub fn tool_execution(mut self, mode: ToolExecutionMode) -> Self {
        self.tool_execution = mode;
        self
    }

    /// Bounds the number of read-only tool calls that may execute at once.
    pub fn max_parallel_tools(mut self, maximum: usize) -> Self {
        self.max_parallel_tools = maximum.max(1);
        self
    }

    /// Sets the agent's initial execution mode.
    pub fn mode(mut self, mode: AgentMode) -> Self {
        self.mode = mode;
        self
    }

    /// Sets the maximum duration of each individual tool call.
    ///
    /// Timing out drops the tool future and records an unknown outcome in the
    /// transcript, since an external side effect may already have happened.
    pub fn tool_call_timeout(mut self, timeout: Duration) -> Self {
        self.tool_call_timeout = Some(nonzero_timeout(timeout));
        self
    }

    /// Disables the agent-level tool-call timeout.
    pub fn without_tool_call_timeout(mut self) -> Self {
        self.tool_call_timeout = None;
        self
    }

    pub fn generation_config(mut self, config: GenerationConfig) -> Self {
        self.generation_config = config;
        self
    }

    /// Overrides the provider's configured model for requests made by this
    /// agent.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.generation_config.model = Some(model.into());
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

    /// Sets the successful-response usage percentage that activates the
    /// registered context-manager chain. Values are clamped to `1..=100`.
    pub fn context_management_threshold_percent(mut self, threshold_percent: u8) -> Self {
        self.context_management_threshold_percent = threshold_percent.clamp(1, 100);
        self
    }

    /// Registers one asynchronous context manager at the end of the chain.
    pub fn context_manager(mut self, manager: impl ContextManager + 'static) -> Self {
        self.context_managers.register(manager);
        self
    }

    pub fn context_managers(mut self, managers: ContextManagerRegistry) -> Self {
        self.context_managers.extend(managers);
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
            tool_execution: self.tool_execution,
            tool_call_timeout: self.tool_call_timeout,
            max_parallel_tools: self.max_parallel_tools,
            mode_control: AgentModeControl::new(self.mode),
            generation_config: self.generation_config,
            max_context_tokens: self.max_context_tokens,
            context_management_threshold_percent: self.context_management_threshold_percent,
            context_managers: self.context_managers,
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
    tool_execution: ToolExecutionMode,
    tool_call_timeout: Option<Duration>,
    max_parallel_tools: usize,
    mode_control: AgentModeControl,
    generation_config: GenerationConfig,
    max_context_tokens: Option<u64>,
    context_management_threshold_percent: u8,
    context_managers: ContextManagerRegistry,
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

    /// Installs or replaces a tool on an already built agent. The replacement
    /// is visible to the next provider request.
    pub fn add_tool(&mut self, tool: impl Tool + 'static) {
        let tool: Arc<dyn Tool> = Arc::new(tool);
        self.tools.insert(tool.definition().name, tool);
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
        if let Some(session) = &mut self.session {
            session.mark_replace_from(0);
        }
    }

    pub fn last_usage(&self) -> Option<TokenUsage> {
        self.last_usage
    }

    pub fn context_usage(&self) -> Option<ContextUsage> {
        self.context_usage
    }

    pub fn context_management_threshold_percent(&self) -> u8 {
        self.context_management_threshold_percent
    }

    /// Changes the successful-response threshold used by subsequent turns.
    /// Values are clamped to `1..=100`.
    pub fn set_context_management_threshold_percent(&mut self, threshold_percent: u8) {
        self.context_management_threshold_percent = threshold_percent.clamp(1, 100);
    }

    /// Registers a context manager on an already built agent.
    pub fn add_context_manager(&mut self, manager: impl ContextManager + 'static) {
        self.context_managers.register(manager);
    }

    pub fn mode(&self) -> AgentMode {
        self.mode_control.mode()
    }

    /// Returns a clonable in-memory mode control for transition tools.
    ///
    /// Prefer [`Agent::set_mode`] when the agent owner changes mode while the
    /// agent is idle, because that method also persists the new state.
    pub fn mode_control(&self) -> AgentModeControl {
        self.mode_control.clone()
    }

    /// Changes mode in memory. The next transcript checkpoint will persist it.
    pub fn set_mode_in_memory(&mut self, mode: AgentMode) {
        self.mode_control.set_mode(mode);
    }

    /// Changes mode and immediately persists it for an attached session.
    ///
    /// If persistence fails, the agent remains in the more restrictive of the
    /// old and requested modes. In particular, a failed attempt to leave Plan
    /// mode cannot accidentally enable side-effecting tools.
    pub async fn set_mode(&mut self, mode: AgentMode) -> Result<(), AgentError> {
        let checkpoint = self.mode();
        self.mode_control.set_mode(mode);
        if let Err(error) = self.synchronize_session().await {
            self.mode_control.restore_safely(checkpoint);
            return Err(error.into());
        }
        Ok(())
    }

    /// Explicitly persists the current in-memory state.
    pub async fn synchronize(&mut self) -> Result<(), AgentError> {
        self.synchronize_session().await.map_err(Into::into)
    }

    /// Returns this agent's per-request model override, if one is set.
    pub fn model(&self) -> Option<&str> {
        self.generation_config.model.as_deref()
    }

    pub fn set_model(&mut self, model: impl Into<String>) {
        self.generation_config.model = Some(model.into());
    }

    pub fn clear_model(&mut self) {
        self.generation_config.model = None;
    }

    pub fn reasoning_effort(&self) -> Option<ReasoningEffort> {
        self.generation_config.reasoning_effort
    }

    pub fn set_reasoning_effort(&mut self, reasoning_effort: Option<ReasoningEffort>) {
        self.generation_config.reasoning_effort = reasoning_effort;
    }

    pub fn tool_call_timeout(&self) -> Option<Duration> {
        self.tool_call_timeout
    }

    /// Changes the timeout used by subsequent tool calls. Passing `None`
    /// disables the agent-level timeout.
    pub fn set_tool_call_timeout(&mut self, timeout: Option<Duration>) {
        self.tool_call_timeout = timeout.map(nonzero_timeout);
    }

    pub fn max_parallel_tools(&self) -> usize {
        self.max_parallel_tools
    }

    pub fn set_max_parallel_tools(&mut self, maximum: usize) {
        self.max_parallel_tools = maximum.max(1);
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

        let mut persisted_message_count = 0;
        if let Some(mut snapshot) = storage.load(&session_id).await? {
            if repair_interrupted_tool_turn(&session_id, &mut snapshot.messages)? {
                storage.save(&snapshot).await?;
            }
            persisted_message_count = snapshot.messages.len();
            self.messages = snapshot.messages;
            self.last_usage = snapshot.last_usage;
            self.cumulative_usage = snapshot.cumulative_usage;
            self.mode_control.set_mode(snapshot.mode);
            self.context_usage = self.last_usage.and_then(|usage| {
                self.max_context_tokens
                    .map(|max_tokens| ContextUsage::from_usage(max_tokens, usage))
            });
        }

        self.session = Some(SessionBinding {
            id: session_id,
            storage,
            persisted_message_count,
            replace_from: None,
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

    pub async fn prompt_controlled(
        &mut self,
        prompt: impl Into<String>,
        control: AgentRunControl,
    ) -> Result<AgentRunOutcome, AgentError> {
        self.prompt_content_controlled(Content::text(prompt), control)
            .await
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
        match self
            .prompt_content_controlled(content, AgentRunControl::new())
            .await?
        {
            AgentRunOutcome::Completed(run) => Ok(run),
            AgentRunOutcome::Stopped => {
                unreachable!("a run with a private control cannot be stopped")
            }
        }
    }

    pub async fn prompt_content_controlled(
        &mut self,
        content: Content,
        control: AgentRunControl,
    ) -> Result<AgentRunOutcome, AgentError> {
        let mut run_usage = TokenUsage::default();
        let user_message = Message::user_content(content);
        let mut run_messages = vec![user_message.clone()];
        let mut checkpoint = AgentCheckpoint::capture(self);

        self.emit(AgentEvent::AgentStart);
        if control.is_stopped() {
            return self.finish_stopped_run(false).await;
        }
        self.messages.push(user_message.clone());
        self.commit_or_rollback(&checkpoint).await?;
        checkpoint = AgentCheckpoint::capture(self);
        self.emit(AgentEvent::MessageStart {
            message: user_message.clone(),
        });
        self.emit(AgentEvent::MessageEnd {
            message: user_message,
        });
        if control.is_stopped() {
            return self.finish_stopped_run(false).await;
        }

        let mut turn = 0usize;
        loop {
            turn = turn.saturating_add(1);
            if control.is_stopped() {
                return self.finish_stopped_run(false).await;
            }
            self.emit(AgentEvent::TurnStart { turn });

            let mut turn_start = TurnStartContext {
                turn,
                request: ProviderRequest {
                    messages: std::iter::once(Message::system(self.system_prompt.clone()))
                        .chain(self.messages.iter().cloned())
                        .collect(),
                    tools: self.tool_definitions_for_mode(self.mode()),
                    config: self.generation_config.clone(),
                },
            };
            let turn_start_result = tokio::select! {
                biased;
                result = self.hooks.run_turn_start(&mut turn_start) => result,
                _ = control.stopped() => {
                    return self.finish_stopped_run(false).await;
                }
            };
            if let Err(error) = turn_start_result {
                return Err(self.hook_failure(error));
            }
            if control.is_stopped() {
                return self.finish_stopped_run(false).await;
            }

            // Hooks may mutate the complete request, including reintroducing
            // hidden tools. Re-apply the capability boundary after every hook
            // and use the latest mode before the provider sees the request.
            let request_mode = self.mode();
            self.enforce_request_mode(&mut turn_start.request, request_mode);

            let mut stream = self.provider.stream(turn_start.request);
            let mut message_started = false;
            let mut received_model_output = false;
            let response = loop {
                let event = tokio::select! {
                    biased;
                    _ = control.stopped() => {
                        return self.finish_stopped_run(message_started).await;
                    }
                    event = stream.next() => event,
                };
                match event {
                    Some(Ok(ProviderEvent::Retry(event))) => {
                        self.emit(AgentEvent::ProviderRetry { event });
                    }
                    Some(Ok(ProviderEvent::Delta(delta))) => {
                        received_model_output |= delta_has_output(&delta);
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
                            message_started = true;
                        }
                        break response;
                    }
                    Some(Err(error)) => {
                        if !received_model_output && error.is_context_length_exceeded() {
                            let trigger = ContextManagementTrigger::ContextLengthExceeded {
                                error: error.to_string(),
                            };
                            if self.run_context_managers(trigger, &control).await?
                                == ContextManagerRunOutcome::Stopped
                            {
                                return self.finish_stopped_run(false).await;
                            }
                        }
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
            let llm_response_result = tokio::select! {
                biased;
                result = self.hooks.run_llm_response(&mut llm_response) => result,
                _ = control.stopped() => {
                    return self.finish_stopped_run(message_started).await;
                }
            };
            if let Err(error) = llm_response_result {
                return Err(self.hook_failure(error));
            }
            if control.is_stopped() {
                return self.finish_stopped_run(message_started).await;
            }
            let response = llm_response.response;
            let response_usage = response.usage;

            let tool_calls = response.message.tool_calls.clone();
            // Freeze both permission boundaries that were in force before the
            // tool batch. They cannot be collapsed to one AgentMode: the
            // intersection of Default and Plan allows neither PlanOnly nor
            // side-effecting tools. Every call also checks the live mode just
            // before it starts, so a transition cannot unlock or outlive a
            // sibling call in the same response.
            let tool_batch_permissions = ToolBatchPermissions::new(request_mode, self.mode());
            let assistant_message = response.message.into_message();

            let had_tool_calls = !tool_calls.is_empty();
            // Capture this turn's insertion point before hooks. If the
            // provider requested tools it is the persisted journal start;
            // otherwise it is where a hook-created complete tool turn will be
            // appended. A post-hook result count cannot recover this index.
            let journal_start = self.messages.len();
            let tool_results = if had_tool_calls {
                // Persist a complete, protocol-valid journal before any tool
                // can produce an external side effect. Unknown results are
                // deliberately pessimistic: after a crash we continue from
                // this turn instead of silently invoking the tool again.
                self.apply_response_usage(response_usage, &mut run_usage);
                self.messages.push(assistant_message.clone());
                self.messages.extend(
                    tool_calls.iter().map(|call| {
                        Message::tool_result(call.id.clone(), UNKNOWN_TOOL_RESULT, true)
                    }),
                );
                self.commit_or_rollback(&checkpoint).await?;
                checkpoint = AgentCheckpoint::capture(self);

                let outcome = self
                    .execute_tool_calls_controlled(tool_calls, &control, tool_batch_permissions)
                    .await;
                let results = outcome
                    .executions
                    .into_iter()
                    .map(execution_message)
                    .collect::<Vec<_>>();
                self.replace_journal_turn(journal_start, &assistant_message, &results);
                // Record completed/cancelled/unknown outcomes before hooks.
                // A failed hook or failed save can now roll back only to the
                // already-persisted unknown journal, never to a replayable
                // pre-tool transcript.
                self.commit_or_rollback(&checkpoint).await?;
                checkpoint = AgentCheckpoint::capture(self);

                if outcome.stopped || control.is_stopped() {
                    self.emit_committed_turn(turn, &assistant_message, &results, response_usage);
                    return self.finish_stopped_run(false).await;
                }
                results
            } else {
                Vec::new()
            };
            let mut turn_end = TurnEndContext {
                turn,
                message: assistant_message,
                tool_results,
            };
            let turn_end_result = tokio::select! {
                biased;
                result = self.hooks.run_turn_end(&mut turn_end) => result,
                _ = control.stopped() => {
                    return self.finish_stopped_run(message_started).await;
                }
            };
            if let Err(error) = turn_end_result {
                return Err(self.hook_failure(error));
            }
            if let Err(error) = validate_protocol_turn(&turn_end.message, &turn_end.tool_results) {
                return Err(self.hook_failure(error));
            }

            if had_tool_calls {
                if self.messages[journal_start] != turn_end.message
                    || self.messages[journal_start + 1..] != turn_end.tool_results
                {
                    self.replace_journal_turn(
                        journal_start,
                        &turn_end.message,
                        &turn_end.tool_results,
                    );
                    self.commit_or_rollback(&checkpoint).await?;
                }
            } else {
                self.apply_response_usage(response_usage, &mut run_usage);
                self.messages.push(turn_end.message.clone());
                self.messages.extend_from_slice(&turn_end.tool_results);
                self.commit_or_rollback(&checkpoint).await?;
            }

            self.emit_committed_turn(
                turn,
                &turn_end.message,
                &turn_end.tool_results,
                response_usage,
            );
            run_messages.push(turn_end.message.clone());
            run_messages.extend_from_slice(&turn_end.tool_results);
            let final_message = turn_end.message.clone();

            if control.is_stopped() {
                return self.finish_stopped_run(false).await;
            }

            if let Some(trigger) = self.usage_context_management_trigger()
                && self.run_context_managers(trigger, &control).await?
                    == ContextManagerRunOutcome::Stopped
            {
                return self.finish_stopped_run(false).await;
            }
            checkpoint = AgentCheckpoint::capture(self);

            if turn_end.message.tool_calls.is_empty() {
                self.emit_agent_end();
                return Ok(AgentRunOutcome::Completed(AgentRun {
                    final_message,
                    new_messages: run_messages,
                    turns: turn,
                    run_usage,
                    context_usage: self.context_usage,
                }));
            }
        }
    }

    fn tool_definitions_for_mode(&self, mode: AgentMode) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .filter(|tool| mode.allows(tool.effect()))
            .map(|tool| tool.definition())
            .collect()
    }

    fn enforce_request_mode(&self, request: &mut ProviderRequest, mode: AgentMode) {
        request.tools.retain(|definition| {
            self.tools
                .get(&definition.name)
                .is_some_and(|tool| mode.allows(tool.effect()))
        });

        request.messages.retain(|message| {
            !(message.role == Role::System
                && message.text_content() == Some(PLAN_MODE_SYSTEM_REMINDER))
        });
        if mode == AgentMode::Plan {
            let insertion = request
                .messages
                .iter()
                .take_while(|message| message.role == Role::System)
                .count();
            request.messages.insert(
                insertion,
                Message::system(PLAN_MODE_SYSTEM_REMINDER.to_owned()),
            );
        }
    }

    fn usage_context_management_trigger(&self) -> Option<ContextManagementTrigger> {
        if self.context_managers.is_empty() {
            return None;
        }
        let usage = self.context_usage?;
        threshold_reached(usage, self.context_management_threshold_percent).then_some(
            ContextManagementTrigger::UsageThreshold {
                usage,
                threshold_percent: self.context_management_threshold_percent,
            },
        )
    }

    async fn run_context_managers(
        &mut self,
        trigger: ContextManagementTrigger,
        control: &AgentRunControl,
    ) -> Result<ContextManagerRunOutcome, AgentError> {
        if self.context_managers.is_empty() {
            return Ok(ContextManagerRunOutcome::Completed);
        }

        let checkpoint = AgentCheckpoint::capture(self);
        let managers = self.context_managers.clone();
        let result = {
            let mut context = ContextManagementContext::new(
                trigger,
                &self.system_prompt,
                self.max_context_tokens,
                &mut self.messages,
            );
            tokio::select! {
                biased;
                result = managers.run(&mut context) => Some(result),
                _ = control.stopped() => None,
            }
        };

        let Some(result) = result else {
            checkpoint.restore(self);
            return Ok(ContextManagerRunOutcome::Stopped);
        };
        if let Err(error) = result {
            checkpoint.restore(self);
            return Err(self.hook_failure(error));
        }
        if let Err(error) = validate_context_transcript(&self.messages) {
            checkpoint.restore(self);
            return Err(self.hook_failure(error));
        }

        if let Some(changed_from) = first_changed_message(&checkpoint.messages, &self.messages) {
            if let Some(session) = &mut self.session {
                session.mark_replace_from(changed_from);
            }
            self.commit_or_rollback(&checkpoint).await?;
        }

        Ok(ContextManagerRunOutcome::Completed)
    }

    async fn synchronize_session_or_end(&mut self) -> Result<(), AgentError> {
        if let Err(error) = self.synchronize_session().await {
            self.emit(AgentEvent::Error {
                message: error.to_string(),
            });
            self.emit_agent_end();
            return Err(error.into());
        }
        Ok(())
    }

    async fn commit_or_rollback(&mut self, checkpoint: &AgentCheckpoint) -> Result<(), AgentError> {
        if let Err(error) = self.synchronize_session().await {
            checkpoint.restore(self);
            let error = AgentError::from(error);
            self.emit(AgentEvent::Error {
                message: error.to_string(),
            });
            self.emit_agent_end();
            return Err(error);
        }
        Ok(())
    }

    async fn finish_stopped_run(
        &mut self,
        message_started: bool,
    ) -> Result<AgentRunOutcome, AgentError> {
        if message_started {
            self.emit(AgentEvent::MessageAborted);
        }
        self.synchronize_session_or_end().await?;
        self.emit(AgentEvent::AgentStopped {
            messages: self.messages.clone(),
        });
        self.emit_agent_end();
        Ok(AgentRunOutcome::Stopped)
    }

    async fn synchronize_session(&mut self) -> Result<(), StorageError> {
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        let snapshot = SessionSnapshot {
            id: session.id.clone(),
            messages: self.messages.clone(),
            last_usage: self.last_usage,
            cumulative_usage: self.cumulative_usage,
            mode: self.mode_control.mode(),
        };
        if let Some(unchanged_message_count) = session.replace_from {
            session
                .storage
                .save_replacing_from(&snapshot, unchanged_message_count)
                .await?;
        } else {
            session
                .storage
                .save_incremental(&snapshot, session.persisted_message_count)
                .await?;
        }
        session.persisted_message_count = snapshot.messages.len();
        session.replace_from = None;
        Ok(())
    }

    async fn execute_tool_calls_controlled(
        &self,
        calls: Vec<ToolCall>,
        control: &AgentRunControl,
        permissions: ToolBatchPermissions,
    ) -> ToolExecutionOutcome {
        let visible_tool_results = Arc::new(
            self.messages
                .iter()
                .filter_map(|message| message.tool_call_id.clone())
                .collect::<HashSet<_>>(),
        );
        // Plan-only and Internal tools are coordination barriers. In
        // particular, a mode-transition tool is Internal state: later calls
        // must observe the new mode before they start. Any batch whose request
        // or pre-execution state was Plan is likewise serialized so approval
        // and plan mutations cannot race each other.
        let execution_mode = if permissions.requires_sequential(&calls, &self.tools) {
            ToolExecutionMode::Sequential
        } else {
            self.tool_execution
        };

        match execution_mode {
            ToolExecutionMode::Sequential => {
                let mut results = calls
                    .iter()
                    .cloned()
                    .map(Self::cancelled_tool)
                    .collect::<Vec<_>>();
                for (index, call) in calls.into_iter().enumerate() {
                    if control.is_stopped() {
                        continue;
                    }
                    self.emit(AgentEvent::ToolExecutionStart { call: call.clone() });
                    results[index] = Self::unknown_tool(call.clone());
                    let context =
                        self.tool_execution_context(&call, control, visible_tool_results.clone());
                    let execution = Self::execute_one(
                        self.tools.get(&call.name),
                        call,
                        self.tool_call_timeout,
                        permissions,
                        &self.mode_control,
                        context,
                    );
                    tokio::pin!(execution);
                    let executed = tokio::select! {
                        biased;
                        executed = &mut execution => executed,
                        _ = control.stopped() => {
                            self.emit_tool_end(&results[index]);
                            return ToolExecutionOutcome {
                                executions: results,
                                stopped: true,
                            };
                        },
                    };
                    self.emit_tool_end(&executed);
                    results[index] = executed;
                }
                ToolExecutionOutcome {
                    executions: results,
                    stopped: control.is_stopped(),
                }
            }
            ToolExecutionMode::Parallel => {
                let mut pending = FuturesUnordered::new();
                let count = calls.len();
                let mut ordered = calls
                    .iter()
                    .cloned()
                    .map(Self::cancelled_tool)
                    .collect::<Vec<_>>();
                let mut started = vec![false; count];
                let mut finished = vec![false; count];

                // Calls reach this branch only when every invocation is
                // classified concurrency-safe. Execute bounded waves to avoid
                // unbounded process/network fan-out from a single model turn.
                let mut next = 0;
                while next < count {
                    let wave_end = (next + self.max_parallel_tools).min(count);
                    for index in next..wave_end {
                        if control.is_stopped() {
                            continue;
                        }
                        let call = calls[index].clone();
                        self.emit(AgentEvent::ToolExecutionStart { call: call.clone() });
                        ordered[index] = Self::unknown_tool(call.clone());
                        started[index] = true;
                        let tool = self.tools.get(&call.name).cloned();
                        let timeout = self.tool_call_timeout;
                        let mode_control = self.mode_control.clone();
                        let context = self.tool_execution_context(
                            &call,
                            control,
                            visible_tool_results.clone(),
                        );
                        pending.push(async move {
                            let executed = Self::execute_one(
                                tool.as_ref(),
                                call,
                                timeout,
                                permissions,
                                &mode_control,
                                context,
                            )
                            .await;
                            (index, executed)
                        });
                    }
                    next = wave_end;

                    while !pending.is_empty() {
                        let completed = tokio::select! {
                            biased;
                            completed = pending.next() => completed,
                            _ = control.stopped() => {
                                for index in 0..count {
                                    if started[index] && !finished[index] {
                                        self.emit_tool_end(&ordered[index]);
                                    }
                                }
                                return ToolExecutionOutcome {
                                    executions: ordered,
                                    stopped: true,
                                };
                            },
                        };
                        if let Some((index, executed)) = completed {
                            self.emit_tool_end(&executed);
                            ordered[index] = executed;
                            finished[index] = true;
                        }
                    }
                }

                ToolExecutionOutcome {
                    executions: ordered,
                    stopped: control.is_stopped(),
                }
            }
        }
    }

    fn cancelled_tool(call: ToolCall) -> ExecutedTool {
        ExecutedTool {
            call,
            output: ToolOutput::error(CANCELLED_TOOL_RESULT),
        }
    }

    fn unknown_tool(call: ToolCall) -> ExecutedTool {
        ExecutedTool {
            call,
            output: ToolOutput::error(UNKNOWN_TOOL_RESULT),
        }
    }

    async fn execute_one(
        tool: Option<&Arc<dyn Tool>>,
        call: ToolCall,
        timeout: Option<Duration>,
        permissions: ToolBatchPermissions,
        mode_control: &AgentModeControl,
        context: ToolExecutionContext,
    ) -> ExecutedTool {
        let arguments = call.arguments.clone();
        let timeout_cancellation = context.cancellation().clone();
        let completion_context = context.clone();

        let execution = async {
            match tool {
                Some(tool) => {
                    let effect = tool.effect();
                    let current_mode = mode_control.mode();
                    if !permissions.allows(effect) || !current_mode.allows(effect) {
                        ToolOutput::error(format!(
                            "tool {:?} is not available under the current mode boundary (request: {:?}, batch: {:?}, current: {current_mode:?}, effect: {effect:?})",
                            call.name, permissions.request_mode, permissions.batch_mode,
                        ))
                    } else {
                        match tool.execute_with_context(arguments, context).await {
                            Ok(output) => output,
                            Err(error) => ToolOutput::error(error.to_string()),
                        }
                    }
                }
                None => ToolOutput::error(format!("unknown tool: {}", call.name)),
            }
        };
        let output = match timeout {
            Some(timeout) => match tokio::time::timeout(timeout, execution).await {
                Ok(output) => output,
                Err(_) => {
                    timeout_cancellation.cancel();
                    ToolOutput::error(format!(
                        "tool call timed out after {timeout:?}; {UNKNOWN_TOOL_RESULT}"
                    ))
                }
            },
            None => execution.await,
        };
        completion_context.finish();

        ExecutedTool { call, output }
    }

    fn apply_response_usage(
        &mut self,
        response_usage: Option<TokenUsage>,
        run_usage: &mut TokenUsage,
    ) {
        self.last_usage = response_usage;
        self.context_usage = response_usage.and_then(|usage| {
            self.max_context_tokens
                .map(|max_tokens| ContextUsage::from_usage(max_tokens, usage))
        });
        if let Some(usage) = response_usage {
            *run_usage += usage;
            self.cumulative_usage += usage;
        }
    }

    fn replace_journal_turn(
        &mut self,
        start: usize,
        assistant: &Message,
        tool_results: &[Message],
    ) {
        self.messages.truncate(start);
        self.messages.push(assistant.clone());
        self.messages.extend_from_slice(tool_results);
        if let Some(session) = &mut self.session {
            session.mark_replace_from(start);
        }
    }

    fn emit_committed_turn(
        &self,
        turn: usize,
        message: &Message,
        tool_results: &[Message],
        usage: Option<TokenUsage>,
    ) {
        if let Some(usage) = usage {
            self.emit(AgentEvent::UsageUpdate {
                usage,
                context_usage: self.context_usage,
            });
        }
        self.emit(AgentEvent::MessageEnd {
            message: message.clone(),
        });
        for message in tool_results {
            self.emit(AgentEvent::MessageStart {
                message: message.clone(),
            });
            self.emit(AgentEvent::MessageEnd {
                message: message.clone(),
            });
        }
        self.emit(AgentEvent::TurnEnd {
            turn,
            message: message.clone(),
            tool_results: tool_results.to_vec(),
        });
    }

    fn emit_tool_end(&self, executed: &ExecutedTool) {
        self.emit(AgentEvent::ToolExecutionEnd {
            call: executed.call.clone(),
            content: executed.output.content.clone(),
            is_error: executed.output.is_error,
            content_parts: executed.output.content_parts.clone(),
            metadata: executed.output.metadata.clone(),
        });
    }

    fn tool_execution_context(
        &self,
        call: &ToolCall,
        control: &AgentRunControl,
        visible_tool_results: Arc<HashSet<String>>,
    ) -> ToolExecutionContext {
        let listeners = self.listeners.clone();
        let progress_call = call.clone();
        let progress = Arc::new(move |progress: ToolProgress| {
            let event = AgentEvent::ToolExecutionProgress {
                call: progress_call.clone(),
                progress,
            };
            for listener in &listeners {
                listener(&event);
            }
        });
        ToolExecutionContext::new(
            call.id.clone(),
            control.tool_cancellation(),
            visible_tool_results,
            Some(progress),
        )
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

struct ToolExecutionOutcome {
    executions: Vec<ExecutedTool>,
    stopped: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContextManagerRunOutcome {
    Completed,
    Stopped,
}

/// Immutable permission boundaries for one assistant tool-call batch.
///
/// Keeping the request and pre-execution modes separate is important. If one
/// is Default and the other is Plan, their permission intersection permits
/// only ReadOnly/Internal tools; no single [`AgentMode`] represents that set.
#[derive(Clone, Copy, Debug)]
struct ToolBatchPermissions {
    request_mode: AgentMode,
    batch_mode: AgentMode,
}

impl ToolBatchPermissions {
    fn new(request_mode: AgentMode, batch_mode: AgentMode) -> Self {
        Self {
            request_mode,
            batch_mode,
        }
    }

    fn allows(self, effect: ToolEffect) -> bool {
        self.request_mode.allows(effect) && self.batch_mode.allows(effect)
    }

    fn requires_sequential(
        self,
        calls: &[ToolCall],
        tools: &HashMap<String, Arc<dyn Tool>>,
    ) -> bool {
        self.request_mode == AgentMode::Plan
            || self.batch_mode == AgentMode::Plan
            || calls.iter().any(|call| {
                tools.get(&call.name).is_none_or(|tool| {
                    matches!(tool.effect(), ToolEffect::Internal | ToolEffect::PlanOnly)
                        || tool.concurrency(&call.arguments) == ToolConcurrency::Exclusive
                })
            })
    }
}

fn execution_message(execution: ExecutedTool) -> Message {
    let (content, is_error, metadata) = execution.output.into_message_parts();
    Message::tool_result_content(execution.call.id, content, is_error, metadata)
}

fn nonzero_timeout(timeout: Duration) -> Duration {
    timeout.max(Duration::from_millis(1))
}

fn delta_has_output(delta: &AssistantDelta) -> bool {
    match delta {
        AssistantDelta::Text { delta } => !delta.is_empty(),
        AssistantDelta::ToolCall {
            id,
            name,
            arguments_delta,
            ..
        } => {
            id.as_ref().is_some_and(|id| !id.is_empty())
                || name.as_ref().is_some_and(|name| !name.is_empty())
                || !arguments_delta.is_empty()
        }
    }
}

#[derive(Clone)]
struct AgentCheckpoint {
    messages: Vec<Message>,
    last_usage: Option<TokenUsage>,
    context_usage: Option<ContextUsage>,
    cumulative_usage: TokenUsage,
    mode: AgentMode,
}

impl AgentCheckpoint {
    fn capture(agent: &Agent) -> Self {
        Self {
            messages: agent.messages.clone(),
            last_usage: agent.last_usage,
            context_usage: agent.context_usage,
            cumulative_usage: agent.cumulative_usage,
            mode: agent.mode(),
        }
    }

    fn restore(&self, agent: &mut Agent) {
        agent.messages.clone_from(&self.messages);
        agent.last_usage = self.last_usage;
        agent.context_usage = self.context_usage;
        agent.cumulative_usage = self.cumulative_usage;
        agent.mode_control.restore_safely(self.mode);
    }
}

fn validate_protocol_turn(assistant: &Message, tool_results: &[Message]) -> Result<(), HookError> {
    if assistant.role != Role::Assistant {
        return Err(HookError::new(
            "turn-end hook must leave the model message in the assistant role",
        ));
    }
    if assistant.tool_calls.len() != tool_results.len() {
        return Err(HookError::new(format!(
            "assistant produced {} tool calls but turn-end hook returned {} tool results",
            assistant.tool_calls.len(),
            tool_results.len()
        )));
    }
    for (call, result) in assistant.tool_calls.iter().zip(tool_results) {
        if result.role != Role::Tool || result.tool_call_id.as_deref() != Some(call.id.as_str()) {
            return Err(HookError::new(format!(
                "tool result must immediately match assistant tool call {:?}",
                call.id
            )));
        }
    }
    Ok(())
}

fn validate_context_transcript(messages: &[Message]) -> Result<(), HookError> {
    let mut pending = VecDeque::new();
    for message in messages {
        match &message.role {
            Role::Assistant => {
                if !pending.is_empty() {
                    return Err(HookError::new(
                        "context manager left an assistant message before prior tool calls were paired",
                    ));
                }
                pending.extend(message.tool_calls.iter().map(|call| call.id.as_str()));
            }
            Role::Tool => {
                let Some(expected) = pending.pop_front() else {
                    return Err(HookError::new(
                        "context manager left a tool result without a preceding assistant tool call",
                    ));
                };
                if message.tool_call_id.as_deref() != Some(expected) {
                    return Err(HookError::new(format!(
                        "context manager left tool result {:?}, expected call {:?}",
                        message.tool_call_id, expected
                    )));
                }
            }
            Role::System | Role::User => {
                if !pending.is_empty() {
                    return Err(HookError::new(
                        "context manager split an assistant tool-call batch from its results",
                    ));
                }
            }
        }
    }
    if !pending.is_empty() {
        return Err(HookError::new(
            "context manager left assistant tool calls without matching results",
        ));
    }
    Ok(())
}

fn first_changed_message(before: &[Message], after: &[Message]) -> Option<usize> {
    if before == after {
        return None;
    }
    Some(
        before
            .iter()
            .zip(after)
            .position(|(before, after)| before != after)
            .unwrap_or_else(|| before.len().min(after.len())),
    )
}

fn repair_interrupted_tool_turn(
    session_id: &str,
    messages: &mut Vec<Message>,
) -> Result<bool, StorageError> {
    let mut pending = VecDeque::new();
    for message in messages.iter() {
        match &message.role {
            Role::Assistant => {
                if !pending.is_empty() {
                    return Err(StorageError::InvalidTranscript {
                        session_id: session_id.to_owned(),
                        message: "assistant message appeared before prior tool calls were paired"
                            .to_owned(),
                    });
                }
                pending.extend(message.tool_calls.iter().map(|call| call.id.clone()));
            }
            Role::Tool => {
                let Some(expected) = pending.pop_front() else {
                    return Err(StorageError::InvalidTranscript {
                        session_id: session_id.to_owned(),
                        message: "tool result has no preceding assistant tool call".to_owned(),
                    });
                };
                if message.tool_call_id.as_deref() != Some(expected.as_str()) {
                    return Err(StorageError::InvalidTranscript {
                        session_id: session_id.to_owned(),
                        message: format!(
                            "tool result {:?} does not match expected call {:?}",
                            message.tool_call_id, expected
                        ),
                    });
                }
            }
            Role::System | Role::User => {
                if !pending.is_empty() {
                    return Err(StorageError::InvalidTranscript {
                        session_id: session_id.to_owned(),
                        message: "non-tool message appeared before all tool calls were paired"
                            .to_owned(),
                    });
                }
            }
        }
    }

    if pending.is_empty() {
        return Ok(false);
    }
    messages.extend(
        pending
            .into_iter()
            .map(|id| Message::tool_result(id, INTERRUPTED_TOOL_RESULT, true)),
    );
    Ok(true)
}

struct SessionBinding {
    id: String,
    storage: Arc<dyn SessionStorage>,
    persisted_message_count: usize,
    replace_from: Option<usize>,
}

impl SessionBinding {
    fn mark_replace_from(&mut self, unchanged_message_count: usize) {
        self.replace_from = Some(
            self.replace_from
                .map_or(unchanged_message_count, |current| {
                    current.min(unchanged_message_count)
                }),
        );
    }
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
    use tokio::sync::Notify;

    use super::*;
    use crate::{
        error::{ProviderError, ToolError},
        provider::ProviderEventStream,
        storage::{
            DiskSessionStorage, InMemorySessionStorage, SessionSnapshot, SessionStorage,
            StorageError,
        },
        tool::{Tool, ToolEffect},
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

    struct RecordingQueueProvider {
        responses: Mutex<VecDeque<ProviderResponse>>,
        requests: Arc<Mutex<Vec<ProviderRequest>>>,
    }

    impl LlmProvider for RecordingQueueProvider {
        fn stream(&self, request: ProviderRequest) -> ProviderEventStream {
            self.requests.lock().unwrap().push(request);
            let response = self.responses.lock().unwrap().pop_front().ok_or_else(|| {
                ProviderError::InvalidResponse("recording queue is empty".to_owned())
            });
            Box::pin(futures_util::stream::iter([
                response.map(ProviderEvent::Done)
            ]))
        }
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

    struct HangingDeltaProvider {
        waiting: Arc<Notify>,
    }

    struct PausedToolCallProvider {
        started: Arc<Notify>,
        release: Arc<Notify>,
        calls: AtomicUsize,
    }

    impl LlmProvider for PausedToolCallProvider {
        fn stream(&self, _request: ProviderRequest) -> ProviderEventStream {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                let started = Arc::clone(&self.started);
                let release = Arc::clone(&self.release);
                Box::pin(async_stream::stream! {
                    started.notify_one();
                    release.notified().await;
                    yield Ok(ProviderEvent::Done(ProviderResponse {
                        message: AssistantMessage::tool_calls(vec![ToolCall::new(
                            "call-after-mode-change",
                            "count",
                            json!({}),
                        )]),
                        usage: None,
                    }));
                })
            } else {
                Box::pin(futures_util::stream::iter([Ok(ProviderEvent::Done(
                    ProviderResponse {
                        message: AssistantMessage::text("done"),
                        usage: None,
                    },
                ))]))
            }
        }
    }

    impl LlmProvider for HangingDeltaProvider {
        fn stream(&self, _request: ProviderRequest) -> ProviderEventStream {
            let waiting = Arc::clone(&self.waiting);
            Box::pin(async_stream::stream! {
                yield Ok(ProviderEvent::Delta(AssistantDelta::Text {
                    delta: "partial".to_owned(),
                }));
                waiting.notify_one();
                std::future::pending::<()>().await;
            })
        }
    }

    struct LifecycleHook {
        stages: Arc<Mutex<Vec<&'static str>>>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum HangingHookStage {
        TurnStart,
        LlmResponse,
        TurnEnd,
    }

    struct HangingLifecycleHook {
        stage: HangingHookStage,
        started: Arc<Notify>,
    }

    struct ReplacingContextManager {
        triggers: Arc<Mutex<Vec<ContextManagementTrigger>>>,
        replacement: &'static str,
        continue_chain: bool,
    }

    struct CountingContextManager {
        calls: Arc<AtomicUsize>,
    }

    struct InvalidContextManager;

    struct HangingContextManager {
        started: Arc<Notify>,
    }

    struct ContextOverflowProvider {
        emit_partial_delta: bool,
    }

    #[async_trait]
    impl ContextManager for ReplacingContextManager {
        async fn manage_context(
            &self,
            context: &mut ContextManagementContext<'_>,
        ) -> Result<bool, HookError> {
            tokio::task::yield_now().await;
            self.triggers
                .lock()
                .unwrap()
                .push(context.trigger().clone());
            context.replace_messages(vec![Message::user(self.replacement)]);
            Ok(self.continue_chain)
        }
    }

    #[async_trait]
    impl ContextManager for CountingContextManager {
        async fn manage_context(
            &self,
            _context: &mut ContextManagementContext<'_>,
        ) -> Result<bool, HookError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(true)
        }
    }

    #[async_trait]
    impl ContextManager for InvalidContextManager {
        async fn manage_context(
            &self,
            context: &mut ContextManagementContext<'_>,
        ) -> Result<bool, HookError> {
            context
                .messages_mut()
                .push(Message::tool("orphan", "invalid orphan result"));
            Ok(false)
        }
    }

    #[async_trait]
    impl ContextManager for HangingContextManager {
        async fn manage_context(
            &self,
            context: &mut ContextManagementContext<'_>,
        ) -> Result<bool, HookError> {
            context.replace_messages(vec![Message::user("uncommitted manager rewrite")]);
            self.started.notify_one();
            std::future::pending().await
        }
    }

    impl LlmProvider for ContextOverflowProvider {
        fn stream(&self, _request: ProviderRequest) -> ProviderEventStream {
            let error = ProviderError::context_length_exceeded(
                "maximum context length exceeded in test provider",
            );
            if self.emit_partial_delta {
                Box::pin(futures_util::stream::iter([
                    Ok(ProviderEvent::Delta(AssistantDelta::Text {
                        delta: "partial".to_owned(),
                    })),
                    Err(error),
                ]))
            } else {
                Box::pin(futures_util::stream::iter([Err(error)]))
            }
        }
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

    #[async_trait]
    impl Hook for HangingLifecycleHook {
        async fn on_turn_start(&self, _context: &mut TurnStartContext) -> Result<(), HookError> {
            if self.stage == HangingHookStage::TurnStart {
                self.started.notify_one();
                return std::future::pending().await;
            }
            Ok(())
        }

        async fn on_llm_response(
            &self,
            _context: &mut LlmResponseContext,
        ) -> Result<(), HookError> {
            if self.stage == HangingHookStage::LlmResponse {
                self.started.notify_one();
                return std::future::pending().await;
            }
            Ok(())
        }

        async fn on_turn_end(&self, _context: &mut TurnEndContext) -> Result<(), HookError> {
            if self.stage == HangingHookStage::TurnEnd {
                self.started.notify_one();
                return std::future::pending().await;
            }
            Ok(())
        }
    }

    struct EchoTool;

    struct StopAfterTool {
        control: AgentRunControl,
        executions: Arc<AtomicUsize>,
    }

    struct HangingTool {
        started: Arc<Notify>,
    }

    struct CountingTool {
        executions: Arc<AtomicUsize>,
    }

    struct ReadOnlyTool;

    struct ProgressTool;

    struct ConcurrencyProbeTool {
        active: Arc<AtomicUsize>,
        maximum: Arc<AtomicUsize>,
        effect: ToolEffect,
    }

    struct ExitModeTool {
        mode: AgentModeControl,
    }

    struct EnterModeTool {
        mode: AgentModeControl,
    }

    struct PlanOnlyCountingTool {
        executions: Arc<AtomicUsize>,
    }

    struct FailingTurnEndHook;

    #[derive(Clone, Copy)]
    enum ToolTurnResize {
        Shrink,
        Expand,
    }

    struct ResizeToolTurnHook(ToolTurnResize);

    struct AddToolToEmptyTurnHook;

    struct RemoveAllToolsFromTurnHook;

    struct InjectToolDefinitionsHook;

    #[derive(Clone, Default)]
    struct RecordingStorage {
        snapshots: Arc<Mutex<Vec<SessionSnapshot>>>,
        operations: Arc<Mutex<Vec<SaveOperation>>>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum SaveOperation {
        Full,
        Incremental(usize),
        ReplaceFrom(usize),
    }

    #[derive(Clone)]
    struct FailOnSaveStorage {
        snapshot: Arc<Mutex<Option<SessionSnapshot>>>,
        save_calls: Arc<AtomicUsize>,
        fail_on_call: usize,
    }

    impl FailOnSaveStorage {
        fn new(fail_on_call: usize) -> Self {
            Self {
                snapshot: Arc::new(Mutex::new(None)),
                save_calls: Arc::new(AtomicUsize::new(0)),
                fail_on_call,
            }
        }

        fn snapshot(&self) -> Option<SessionSnapshot> {
            self.snapshot.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl SessionStorage for RecordingStorage {
        async fn load(&self, _session_id: &str) -> Result<Option<SessionSnapshot>, StorageError> {
            Ok(None)
        }

        async fn save(&self, session: &SessionSnapshot) -> Result<(), StorageError> {
            self.snapshots.lock().unwrap().push(session.clone());
            self.operations.lock().unwrap().push(SaveOperation::Full);
            Ok(())
        }

        async fn save_incremental(
            &self,
            session: &SessionSnapshot,
            previous_message_count: usize,
        ) -> Result<(), StorageError> {
            self.snapshots.lock().unwrap().push(session.clone());
            self.operations
                .lock()
                .unwrap()
                .push(SaveOperation::Incremental(previous_message_count));
            Ok(())
        }

        async fn save_replacing_from(
            &self,
            session: &SessionSnapshot,
            unchanged_message_count: usize,
        ) -> Result<(), StorageError> {
            self.snapshots.lock().unwrap().push(session.clone());
            self.operations
                .lock()
                .unwrap()
                .push(SaveOperation::ReplaceFrom(unchanged_message_count));
            Ok(())
        }

        async fn delete(&self, _session_id: &str) -> Result<(), StorageError> {
            Ok(())
        }
    }

    #[async_trait]
    impl SessionStorage for FailOnSaveStorage {
        async fn load(&self, _session_id: &str) -> Result<Option<SessionSnapshot>, StorageError> {
            Ok(self.snapshot())
        }

        async fn save(&self, session: &SessionSnapshot) -> Result<(), StorageError> {
            let call = self.save_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == self.fail_on_call {
                return Err(StorageError::Io {
                    path: "injected-save-failure".into(),
                    source: std::io::Error::other("injected save failure"),
                });
            }
            *self.snapshot.lock().unwrap() = Some(session.clone());
            Ok(())
        }

        async fn delete(&self, _session_id: &str) -> Result<(), StorageError> {
            *self.snapshot.lock().unwrap() = None;
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

    #[async_trait]
    impl Tool for StopAfterTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("stop_after", "Stops the run", json!({ "type": "object" }))
        }

        async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
            self.executions.fetch_add(1, Ordering::Relaxed);
            self.control.stop();
            Ok(ToolOutput::success("completed before stop"))
        }
    }

    #[async_trait]
    impl Tool for HangingTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("hang", "Waits forever", json!({ "type": "object" }))
        }

        async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
            self.started.notify_one();
            std::future::pending().await
        }
    }

    #[async_trait]
    impl Tool for CountingTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new(
                "count",
                "Records one side effect",
                json!({ "type": "object" }),
            )
        }

        async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
            self.executions.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput::success("side effect completed"))
        }
    }

    #[async_trait]
    impl Tool for ReadOnlyTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("inspect", "Reads state", json!({ "type": "object" }))
        }

        fn effect(&self) -> ToolEffect {
            ToolEffect::ReadOnly
        }

        async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::success("inspected"))
        }
    }

    #[async_trait]
    impl Tool for ProgressTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("progress", "Reports progress", json!({ "type": "object" }))
        }

        fn effect(&self) -> ToolEffect {
            ToolEffect::ReadOnly
        }

        async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::success("complete"))
        }

        async fn execute_with_context(
            &self,
            _arguments: serde_json::Value,
            context: ToolExecutionContext,
        ) -> Result<ToolOutput, ToolError> {
            context.report_progress(
                ToolProgress::new("halfway").with_metadata(json!({ "percent": 50 })),
            );
            Ok(ToolOutput::success("complete")
                .with_content_part(ContentPart::image_url("data:image/png;base64,eA=="))
                .with_metadata(json!({ "kind": "progress_test" })))
        }
    }

    #[async_trait]
    impl Tool for ConcurrencyProbeTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("probe", "Measures overlap", json!({ "type": "object" }))
        }

        fn effect(&self) -> ToolEffect {
            self.effect
        }

        async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.maximum.fetch_max(active, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(20)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(ToolOutput::success("probed"))
        }
    }

    #[async_trait]
    impl Tool for ExitModeTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new(
                "exit_plan_mode",
                "Leaves plan mode",
                json!({ "type": "object" }),
            )
        }

        fn effect(&self) -> ToolEffect {
            ToolEffect::PlanOnly
        }

        async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
            self.mode.set_mode(AgentMode::Default);
            Ok(ToolOutput::success("plan approved"))
        }
    }

    #[async_trait]
    impl Tool for EnterModeTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new(
                "enter_plan_mode",
                "Enters plan mode",
                json!({ "type": "object" }),
            )
        }

        fn effect(&self) -> ToolEffect {
            ToolEffect::Internal
        }

        async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
            self.mode.set_mode(AgentMode::Plan);
            Ok(ToolOutput::success("entered plan mode"))
        }
    }

    #[async_trait]
    impl Tool for PlanOnlyCountingTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new(
                "update_plan",
                "Mutates the current plan",
                json!({ "type": "object" }),
            )
        }

        fn effect(&self) -> ToolEffect {
            ToolEffect::PlanOnly
        }

        async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
            self.executions.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput::success("plan updated"))
        }
    }

    #[async_trait]
    impl Hook for FailingTurnEndHook {
        async fn on_turn_end(&self, _context: &mut TurnEndContext) -> Result<(), HookError> {
            Err(HookError::new("injected turn-end failure"))
        }
    }

    #[async_trait]
    impl Hook for ResizeToolTurnHook {
        async fn on_turn_end(&self, context: &mut TurnEndContext) -> Result<(), HookError> {
            if context.message.tool_calls.is_empty() {
                return Ok(());
            }

            match self.0 {
                ToolTurnResize::Shrink => {
                    context.message.tool_calls.truncate(1);
                    context.tool_results.truncate(1);
                }
                ToolTurnResize::Expand => {
                    context.message.tool_calls.push(ToolCall::new(
                        "call-added-by-hook",
                        "echo",
                        json!({"text": "added"}),
                    ));
                    context.tool_results.push(Message::tool_result(
                        "call-added-by-hook",
                        "added",
                        false,
                    ));
                }
            }
            Ok(())
        }
    }

    #[async_trait]
    impl Hook for AddToolToEmptyTurnHook {
        async fn on_turn_end(&self, context: &mut TurnEndContext) -> Result<(), HookError> {
            if context.turn == 1 && context.message.tool_calls.is_empty() {
                context.message.tool_calls.push(ToolCall::new(
                    "call-added-to-empty-turn",
                    "echo",
                    json!({"text": "added"}),
                ));
                context.tool_results.push(Message::tool_result(
                    "call-added-to-empty-turn",
                    "added",
                    false,
                ));
            }
            Ok(())
        }
    }

    #[async_trait]
    impl Hook for RemoveAllToolsFromTurnHook {
        async fn on_turn_end(&self, context: &mut TurnEndContext) -> Result<(), HookError> {
            if context.turn == 1 && !context.message.tool_calls.is_empty() {
                context.message.content = Some(Content::text("finished by hook"));
                context.message.tool_calls.clear();
                context.tool_results.clear();
            }
            Ok(())
        }
    }

    #[async_trait]
    impl Hook for InjectToolDefinitionsHook {
        async fn on_turn_start(&self, context: &mut TurnStartContext) -> Result<(), HookError> {
            context.request.tools.extend([
                ToolDefinition::new("count", "reintroduced write", json!({ "type": "object" })),
                ToolDefinition::new("ghost", "unregistered tool", json!({ "type": "object" })),
            ]);
            Ok(())
        }
    }

    async fn assert_resized_tool_turn_is_durable<S>(
        session_id: &str,
        storage: S,
        resize: ToolTurnResize,
    ) where
        S: SessionStorage + Clone + 'static,
    {
        let original_call_count = match resize {
            ToolTurnResize::Shrink => 2,
            ToolTurnResize::Expand => 1,
        };
        let expected_call_count = match resize {
            ToolTurnResize::Shrink => 1,
            ToolTurnResize::Expand => 2,
        };
        let calls = (0..original_call_count)
            .map(|index| {
                ToolCall::new(
                    format!("call-{index}"),
                    "echo",
                    json!({"text": format!("result-{index}")}),
                )
            })
            .collect();
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::from([
                ProviderResponse {
                    message: AssistantMessage::tool_calls(calls),
                    usage: None,
                },
                ProviderResponse {
                    message: AssistantMessage::text("done"),
                    usage: None,
                },
            ])),
        };
        let mut agent = Agent::builder(provider)
            .tool(EchoTool)
            .tool_execution(ToolExecutionMode::Sequential)
            .hook(ResizeToolTurnHook(resize))
            .build()
            .with_session(session_id, storage.clone())
            .await
            .unwrap();

        agent.prompt("resize the tool turn").await.unwrap();

        assert_eq!(agent.messages()[1].tool_calls.len(), expected_call_count);
        for (call, result) in agent.messages()[1]
            .tool_calls
            .iter()
            .zip(&agent.messages()[2..2 + expected_call_count])
        {
            assert_eq!(result.role, Role::Tool);
            assert_eq!(result.tool_call_id.as_deref(), Some(call.id.as_str()));
        }
        assert_eq!(
            agent.messages()[2 + expected_call_count].text_content(),
            Some("done")
        );

        let expected = agent.messages().to_vec();
        let persisted = storage.load(session_id).await.unwrap().unwrap();
        assert_eq!(persisted.messages, expected);

        let reloaded = Agent::builder(MockProvider {
            responses: Mutex::new(VecDeque::new()),
        })
        .build()
        .with_session(session_id, storage)
        .await
        .unwrap();
        assert_eq!(reloaded.messages(), expected);
    }

    async fn assert_empty_turn_expansion_is_durable<S>(session_id: &str, storage: S)
    where
        S: SessionStorage + Clone + 'static,
    {
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::from([
                ProviderResponse {
                    message: AssistantMessage::text("initial answer"),
                    usage: None,
                },
                ProviderResponse {
                    message: AssistantMessage::text("continued after hook tool turn"),
                    usage: None,
                },
            ])),
        };
        let mut agent = Agent::builder(provider)
            .hook(AddToolToEmptyTurnHook)
            .build()
            .with_session(session_id, storage.clone())
            .await
            .unwrap();

        let run = agent.prompt("add a tool turn").await.unwrap();

        assert_eq!(run.turns, 2);
        assert_eq!(agent.messages().len(), 4);
        assert_eq!(agent.messages()[1].tool_calls.len(), 1);
        assert_eq!(
            agent.messages()[2].tool_call_id.as_deref(),
            Some("call-added-to-empty-turn")
        );
        assert_eq!(
            agent.messages()[3].text_content(),
            Some("continued after hook tool turn")
        );

        let expected = agent.messages().to_vec();
        assert_eq!(
            storage.load(session_id).await.unwrap().unwrap().messages,
            expected
        );
        let reloaded = Agent::builder(MockProvider {
            responses: Mutex::new(VecDeque::new()),
        })
        .build()
        .with_session(session_id, storage)
        .await
        .unwrap();
        assert_eq!(reloaded.messages(), expected);
    }

    async fn assert_removing_all_tools_finishes_the_turn<S>(session_id: &str, storage: S)
    where
        S: SessionStorage + Clone + 'static,
    {
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::from([
                ProviderResponse {
                    message: AssistantMessage::tool_calls(vec![ToolCall::new(
                        "call-removed-by-hook",
                        "echo",
                        json!({"text": "executed"}),
                    )]),
                    usage: None,
                },
                ProviderResponse {
                    message: AssistantMessage::text("next queued response"),
                    usage: None,
                },
            ])),
        };
        let mut agent = Agent::builder(provider)
            .tool(EchoTool)
            .hook(RemoveAllToolsFromTurnHook)
            .build()
            .with_session(session_id, storage.clone())
            .await
            .unwrap();

        let first = agent.prompt("remove the tool turn").await.unwrap();

        assert_eq!(first.turns, 1);
        assert_eq!(first.text(), Some("finished by hook"));
        assert_eq!(agent.messages().len(), 2);
        assert!(agent.messages()[1].tool_calls.is_empty());
        assert_eq!(
            storage.load(session_id).await.unwrap().unwrap().messages,
            agent.messages()
        );

        let second = agent.prompt("consume the queued response").await.unwrap();
        assert_eq!(second.turns, 1);
        assert_eq!(second.text(), Some("next queued response"));

        let expected = agent.messages().to_vec();
        let reloaded = Agent::builder(MockProvider {
            responses: Mutex::new(VecDeque::new()),
        })
        .build()
        .with_session(session_id, storage)
        .await
        .unwrap();
        assert_eq!(reloaded.messages(), expected);
    }

    #[tokio::test]
    async fn shrinking_a_tool_turn_replaces_the_original_journal_tail() {
        assert_resized_tool_turn_is_durable(
            "shrink-memory-journal",
            InMemorySessionStorage::new(),
            ToolTurnResize::Shrink,
        )
        .await;

        let directory = tempfile::tempdir().unwrap();
        assert_resized_tool_turn_is_durable(
            "shrink-disk-journal",
            DiskSessionStorage::new(directory.path()),
            ToolTurnResize::Shrink,
        )
        .await;
    }

    #[tokio::test]
    async fn expanding_a_tool_turn_replaces_the_original_journal_tail() {
        assert_resized_tool_turn_is_durable(
            "expand-memory-journal",
            InMemorySessionStorage::new(),
            ToolTurnResize::Expand,
        )
        .await;

        let directory = tempfile::tempdir().unwrap();
        assert_resized_tool_turn_is_durable(
            "expand-disk-journal",
            DiskSessionStorage::new(directory.path()),
            ToolTurnResize::Expand,
        )
        .await;
    }

    #[tokio::test]
    async fn adding_a_tool_to_an_empty_turn_persists_and_continues() {
        assert_empty_turn_expansion_is_durable(
            "empty-expand-memory-journal",
            InMemorySessionStorage::new(),
        )
        .await;

        let directory = tempfile::tempdir().unwrap();
        assert_empty_turn_expansion_is_durable(
            "empty-expand-disk-journal",
            DiskSessionStorage::new(directory.path()),
        )
        .await;
    }

    #[tokio::test]
    async fn removing_all_tools_from_a_turn_finishes_without_an_extra_provider_call() {
        assert_removing_all_tools_finishes_the_turn(
            "remove-all-memory-journal",
            InMemorySessionStorage::new(),
        )
        .await;

        let directory = tempfile::tempdir().unwrap();
        assert_removing_all_tools_finishes_the_turn(
            "remove-all-disk-journal",
            DiskSessionStorage::new(directory.path()),
        )
        .await;
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
        assert_eq!(
            names,
            [
                "bash",
                "bash_task_output",
                "bash_task_stop",
                "edit",
                "read",
                "write"
            ]
        );
    }

    #[test]
    fn model_and_reasoning_can_be_changed_between_runs() {
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::new()),
        };
        let mut agent = Agent::builder(provider)
            .model("initial-model")
            .reasoning_effort(ReasoningEffort::Low)
            .build();

        assert_eq!(agent.model(), Some("initial-model"));
        assert_eq!(agent.reasoning_effort(), Some(ReasoningEffort::Low));

        agent.set_model("next-model");
        agent.set_reasoning_effort(Some(ReasoningEffort::High));
        assert_eq!(agent.model(), Some("next-model"));
        assert_eq!(agent.reasoning_effort(), Some(ReasoningEffort::High));

        agent.clear_model();
        agent.set_reasoning_effort(None);
        assert_eq!(agent.model(), None);
        assert_eq!(agent.reasoning_effort(), None);
    }

    #[tokio::test]
    async fn controlled_prompt_completes_normally() {
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::from([ProviderResponse {
                message: AssistantMessage::text("done"),
                usage: None,
            }])),
        };
        let mut agent = Agent::builder(provider).build();

        let outcome = agent
            .prompt_controlled("hello", AgentRunControl::new())
            .await
            .unwrap();

        let AgentRunOutcome::Completed(run) = outcome else {
            panic!("expected a completed controlled run");
        };
        assert_eq!(run.text(), Some("done"));
    }

    #[tokio::test]
    async fn stopping_during_a_delta_aborts_the_draft_without_an_error() {
        let waiting = Arc::new(Notify::new());
        let events = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&events);
        let mut agent = Agent::builder(HangingDeltaProvider {
            waiting: Arc::clone(&waiting),
        })
        .build();
        agent.subscribe(move |event| {
            let name = match event {
                AgentEvent::MessageAborted => Some("message_aborted"),
                AgentEvent::AgentStopped { .. } => Some("agent_stopped"),
                AgentEvent::AgentEnd { .. } => Some("agent_end"),
                AgentEvent::Error { .. } => Some("error"),
                _ => None,
            };
            if let Some(name) = name {
                observed.lock().unwrap().push(name);
            }
        });
        let control = AgentRunControl::new();
        let run_control = control.clone();
        let stop = async move {
            waiting.notified().await;
            control.stop();
        };

        let (outcome, ()) = tokio::join!(agent.prompt_controlled("hello", run_control), stop);

        assert_eq!(outcome.unwrap(), AgentRunOutcome::Stopped);
        assert_eq!(agent.messages(), [Message::user("hello")]);
        assert_eq!(
            events.lock().unwrap().as_slice(),
            ["message_aborted", "agent_stopped", "agent_end"]
        );
    }

    #[tokio::test]
    async fn stopped_run_restores_from_its_last_persisted_message() {
        let waiting = Arc::new(Notify::new());
        let storage = InMemorySessionStorage::new();
        let mut agent = Agent::builder(HangingDeltaProvider {
            waiting: Arc::clone(&waiting),
        })
        .build()
        .with_session("stopped", storage.clone())
        .await
        .unwrap();
        let control = AgentRunControl::new();
        let run_control = control.clone();
        let stop = async move {
            waiting.notified().await;
            control.stop();
        };

        let (outcome, ()) = tokio::join!(agent.prompt_controlled("keep me", run_control), stop);
        assert_eq!(outcome.unwrap(), AgentRunOutcome::Stopped);

        let restored = Agent::builder(MockProvider {
            responses: Mutex::new(VecDeque::new()),
        })
        .build()
        .with_session("stopped", storage)
        .await
        .unwrap();
        assert_eq!(restored.messages(), [Message::user("keep me")]);
    }

    #[tokio::test]
    async fn stop_interrupts_hanging_lifecycle_hooks_and_restores_the_checkpoint() {
        for stage in [
            HangingHookStage::TurnStart,
            HangingHookStage::LlmResponse,
            HangingHookStage::TurnEnd,
        ] {
            let started = Arc::new(Notify::new());
            let events = Arc::new(Mutex::new(Vec::new()));
            let observed = Arc::clone(&events);
            let provider = MockProvider {
                responses: Mutex::new(VecDeque::from([ProviderResponse {
                    message: AssistantMessage::text("discard me"),
                    usage: Some(TokenUsage::new(10, 2, 0)),
                }])),
            };
            let mut agent = Agent::builder(provider)
                .hook(HangingLifecycleHook {
                    stage,
                    started: Arc::clone(&started),
                })
                .build();
            agent.subscribe(move |event| {
                let name = match event {
                    AgentEvent::MessageAborted => Some("message_aborted"),
                    AgentEvent::AgentStopped { .. } => Some("agent_stopped"),
                    AgentEvent::AgentEnd { .. } => Some("agent_end"),
                    AgentEvent::Error { .. } => Some("error"),
                    _ => None,
                };
                if let Some(name) = name {
                    observed.lock().unwrap().push(name);
                }
            });
            let control = AgentRunControl::new();
            let stop_control = control.clone();

            let (outcome, ()) = tokio::time::timeout(Duration::from_secs(1), async {
                tokio::join!(agent.prompt_controlled("keep me", control), async move {
                    started.notified().await;
                    stop_control.stop();
                })
            })
            .await
            .unwrap_or_else(|_| panic!("stop did not interrupt the {stage:?} hook"));

            assert_eq!(outcome.unwrap(), AgentRunOutcome::Stopped);
            assert_eq!(agent.messages(), [Message::user("keep me")]);
            let expected = if stage == HangingHookStage::TurnStart {
                vec!["agent_stopped", "agent_end"]
            } else {
                vec!["message_aborted", "agent_stopped", "agent_end"]
            };
            assert_eq!(*events.lock().unwrap(), expected);
        }
    }

    #[tokio::test]
    async fn stop_interrupts_hanging_tools_in_both_execution_modes() {
        for (session_id, mode) in [
            ("stop-hanging-sequential", ToolExecutionMode::Sequential),
            ("stop-hanging-parallel", ToolExecutionMode::Parallel),
        ] {
            let started = Arc::new(Notify::new());
            let storage = InMemorySessionStorage::new();
            let provider = MockProvider {
                responses: Mutex::new(VecDeque::from([ProviderResponse {
                    message: AssistantMessage::tool_calls(vec![ToolCall::new(
                        "call-1",
                        "hang",
                        json!({}),
                    )]),
                    usage: Some(TokenUsage::new(10, 2, 0)),
                }])),
            };
            let mut agent = Agent::builder(provider)
                .tool(HangingTool {
                    started: Arc::clone(&started),
                })
                .tool_execution(mode)
                .build()
                .with_session(session_id, storage.clone())
                .await
                .unwrap();
            let control = AgentRunControl::new();
            let stop_control = control.clone();

            let (outcome, ()) = tokio::time::timeout(Duration::from_secs(1), async {
                tokio::join!(agent.prompt_controlled("keep me", control), async move {
                    started.notified().await;
                    stop_control.stop();
                })
            })
            .await
            .unwrap_or_else(|_| panic!("stop did not interrupt a {mode:?} tool"));

            assert_eq!(outcome.unwrap(), AgentRunOutcome::Stopped);
            assert_eq!(agent.messages().len(), 3);
            assert_eq!(agent.messages()[0], Message::user("keep me"));
            assert_eq!(agent.messages()[1].role, Role::Assistant);
            assert_eq!(agent.messages()[2].role, Role::Tool);
            assert_eq!(
                agent.messages()[2].text_content(),
                Some(UNKNOWN_TOOL_RESULT)
            );
            let persisted = storage.load(session_id).await.unwrap().unwrap();
            assert_eq!(persisted.messages, agent.messages());
            assert_eq!(persisted.last_usage, Some(TokenUsage::new(10, 2, 0)));
            assert_eq!(persisted.cumulative_usage, TokenUsage::new(10, 2, 0));
        }
    }

    #[tokio::test]
    async fn stop_finishes_started_tool_and_pairs_unstarted_calls_with_errors() {
        let control = AgentRunControl::new();
        let executions = Arc::new(AtomicUsize::new(0));
        let storage = InMemorySessionStorage::new();
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::from([ProviderResponse {
                message: AssistantMessage::tool_calls(vec![
                    ToolCall::new("call-1", "stop_after", json!({})),
                    ToolCall::new("call-2", "stop_after", json!({})),
                ]),
                usage: None,
            }])),
        };
        let mut agent = Agent::builder(provider)
            .tool(StopAfterTool {
                control: control.clone(),
                executions: Arc::clone(&executions),
            })
            .tool_execution(ToolExecutionMode::Sequential)
            .build()
            .with_session("stopped-tools", storage.clone())
            .await
            .unwrap();

        let outcome = agent.prompt_controlled("run tools", control).await.unwrap();

        assert_eq!(outcome, AgentRunOutcome::Stopped);
        assert_eq!(executions.load(Ordering::Relaxed), 1);
        assert_eq!(agent.messages().len(), 4);
        assert_eq!(agent.messages()[1].tool_calls.len(), 2);
        assert_eq!(agent.messages()[2].tool_call_id.as_deref(), Some("call-1"));
        assert!(!agent.messages()[2].tool_result_is_error);
        assert_eq!(agent.messages()[3].tool_call_id.as_deref(), Some("call-2"));
        assert!(agent.messages()[3].tool_result_is_error);
        assert_eq!(
            agent.messages()[3].text_content(),
            Some(CANCELLED_TOOL_RESULT)
        );
        let persisted = storage.load("stopped-tools").await.unwrap().unwrap();
        assert_eq!(persisted.messages, agent.messages());
    }

    #[tokio::test]
    async fn tool_call_is_journaled_as_unknown_before_execution() {
        let started = Arc::new(Notify::new());
        let storage = InMemorySessionStorage::new();
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::from([ProviderResponse {
                message: AssistantMessage::tool_calls(vec![ToolCall::new(
                    "call-1",
                    "hang",
                    json!({}),
                )]),
                usage: None,
            }])),
        };
        let agent = Agent::builder(provider)
            .tool(HangingTool {
                started: Arc::clone(&started),
            })
            .build()
            .with_session("atomic-tool-turn", storage.clone())
            .await
            .unwrap();

        let run = tokio::spawn(async move {
            let mut agent = agent;
            agent.prompt("run it").await
        });
        started.notified().await;

        let checkpoint = storage.load("atomic-tool-turn").await.unwrap().unwrap();
        assert_eq!(checkpoint.messages.len(), 3);
        assert_eq!(checkpoint.messages[0], Message::user("run it"));
        assert_eq!(checkpoint.messages[1].role, Role::Assistant);
        assert_eq!(checkpoint.messages[2].role, Role::Tool);
        assert_eq!(
            checkpoint.messages[2].text_content(),
            Some(UNKNOWN_TOOL_RESULT)
        );

        run.abort();
        let _ = run.await;
    }

    #[tokio::test]
    async fn completed_side_effect_is_persisted_before_a_failing_turn_end_hook() {
        let executions = Arc::new(AtomicUsize::new(0));
        let storage = InMemorySessionStorage::new();
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::from([ProviderResponse {
                message: AssistantMessage::tool_calls(vec![ToolCall::new(
                    "call-1",
                    "count",
                    json!({}),
                )]),
                usage: None,
            }])),
        };
        let mut agent = Agent::builder(provider)
            .tool(CountingTool {
                executions: Arc::clone(&executions),
            })
            .hook(FailingTurnEndHook)
            .build()
            .with_session("hook-failure-journal", storage.clone())
            .await
            .unwrap();

        let error = agent.prompt("run side effect").await.unwrap_err();

        assert!(matches!(error, AgentError::Hook(_)));
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        assert_eq!(agent.messages().len(), 3);
        assert_eq!(
            agent.messages()[2].text_content(),
            Some("side effect completed")
        );
        assert!(!agent.messages()[2].tool_result_is_error);
        let persisted = storage.load("hook-failure-journal").await.unwrap().unwrap();
        assert_eq!(persisted.messages, agent.messages());
    }

    #[tokio::test]
    async fn failed_result_save_keeps_unknown_journal_and_does_not_replay_tool() {
        let executions = Arc::new(AtomicUsize::new(0));
        // user, unknown journal, then completed-result replacement (fails)
        let storage = FailOnSaveStorage::new(3);
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::from([
                ProviderResponse {
                    message: AssistantMessage::tool_calls(vec![ToolCall::new(
                        "call-1",
                        "count",
                        json!({}),
                    )]),
                    usage: None,
                },
                ProviderResponse {
                    message: AssistantMessage::text("continued safely"),
                    usage: None,
                },
            ])),
        };
        let mut agent = Agent::builder(provider)
            .tool(CountingTool {
                executions: Arc::clone(&executions),
            })
            .build()
            .with_session("save-failure-journal", storage.clone())
            .await
            .unwrap();

        let error = agent.prompt("run side effect").await.unwrap_err();
        assert!(matches!(
            error,
            AgentError::Storage(StorageError::Io { .. })
        ));
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        assert_eq!(
            agent.messages()[2].text_content(),
            Some(UNKNOWN_TOOL_RESULT)
        );
        assert_eq!(
            storage.snapshot().unwrap().messages[2].text_content(),
            Some(UNKNOWN_TOOL_RESULT)
        );

        let run = agent.prompt("continue without replay").await.unwrap();
        assert_eq!(run.text(), Some("continued safely"));
        assert_eq!(executions.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn agent_tool_call_timeout_is_runtime_configurable() {
        let started = Arc::new(Notify::new());
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::from([
                ProviderResponse {
                    message: AssistantMessage::tool_calls(vec![ToolCall::new(
                        "call-1",
                        "hang",
                        json!({}),
                    )]),
                    usage: None,
                },
                ProviderResponse {
                    message: AssistantMessage::text("after timeout"),
                    usage: None,
                },
            ])),
        };
        let mut agent = Agent::builder(provider)
            .tool(HangingTool { started })
            .without_tool_call_timeout()
            .build();
        assert_eq!(agent.tool_call_timeout(), None);
        agent.set_tool_call_timeout(Some(Duration::from_millis(20)));

        let run = tokio::time::timeout(Duration::from_secs(1), agent.prompt("run it"))
            .await
            .expect("agent-level timeout did not finish the tool call")
            .unwrap();

        assert_eq!(run.text(), Some("after timeout"));
        assert_eq!(agent.tool_call_timeout(), Some(Duration::from_millis(20)));
        assert!(
            agent.messages()[2]
                .text_content()
                .unwrap()
                .contains("tool call timed out")
        );
        assert!(
            agent.messages()[2]
                .text_content()
                .unwrap()
                .contains("outcome is unknown")
        );
    }

    #[tokio::test]
    async fn first_checkpoint_save_failure_restores_empty_live_state() {
        let storage = FailOnSaveStorage::new(1);
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::from([ProviderResponse {
                message: AssistantMessage::text("must not be reached"),
                usage: Some(TokenUsage::new(10, 2, 1)),
            }])),
        };
        let mut agent = Agent::builder(provider)
            .max_context_tokens(1_000)
            .build()
            .with_session("first-save-fails", storage.clone())
            .await
            .unwrap();

        let error = agent.prompt("not persisted").await.unwrap_err();

        assert!(matches!(
            error,
            AgentError::Storage(StorageError::Io { .. })
        ));
        assert!(agent.messages().is_empty());
        assert_eq!(agent.last_usage(), None);
        assert_eq!(agent.context_usage(), None);
        assert_eq!(agent.cumulative_usage(), TokenUsage::default());
        assert_eq!(storage.snapshot(), None);
    }

    #[tokio::test]
    async fn turn_checkpoint_save_failure_restores_messages_and_usage_to_prior_checkpoint() {
        let storage = FailOnSaveStorage::new(2);
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::from([ProviderResponse {
                message: AssistantMessage::text("not committed"),
                usage: Some(TokenUsage::new(100, 20, 5)),
            }])),
        };
        let mut agent = Agent::builder(provider)
            .max_context_tokens(1_000)
            .build()
            .with_session("turn-save-fails", storage.clone())
            .await
            .unwrap();

        let error = agent.prompt("committed user message").await.unwrap_err();

        assert!(matches!(
            error,
            AgentError::Storage(StorageError::Io { .. })
        ));
        assert_eq!(agent.messages(), [Message::user("committed user message")]);
        assert_eq!(agent.last_usage(), None);
        assert_eq!(agent.context_usage(), None);
        assert_eq!(agent.cumulative_usage(), TokenUsage::default());
        let persisted = storage.snapshot().expect("first checkpoint must remain");
        assert_eq!(persisted.messages, agent.messages());
        assert_eq!(persisted.last_usage, None);
        assert_eq!(persisted.cumulative_usage, TokenUsage::default());
    }

    #[tokio::test]
    async fn attach_repairs_only_a_dangling_tail_tool_call_and_persists_it() {
        let storage = InMemorySessionStorage::new();
        let session_id = "repair-dangling-tail";
        let dangling = Message::assistant(
            None,
            vec![ToolCall::new("call-1", "echo", json!({ "text": "hello" }))],
        );
        storage
            .save(&SessionSnapshot {
                id: session_id.to_owned(),
                messages: vec![Message::user("run it"), dangling.clone()],
                last_usage: Some(TokenUsage::new(40, 5, 0)),
                cumulative_usage: TokenUsage::new(40, 5, 0),
                mode: AgentMode::default(),
            })
            .await
            .unwrap();

        let agent = Agent::builder(MockProvider {
            responses: Mutex::new(VecDeque::new()),
        })
        .build()
        .with_session(session_id, storage.clone())
        .await
        .unwrap();

        assert_eq!(agent.messages().len(), 3);
        assert_eq!(agent.messages()[0], Message::user("run it"));
        assert_eq!(agent.messages()[1], dangling);
        assert_eq!(agent.messages()[2].role, Role::Tool);
        assert_eq!(agent.messages()[2].tool_call_id.as_deref(), Some("call-1"));
        assert!(agent.messages()[2].tool_result_is_error);
        assert_eq!(
            agent.messages()[2].text_content(),
            Some(INTERRUPTED_TOOL_RESULT)
        );
        let repaired = storage.load(session_id).await.unwrap().unwrap();
        assert_eq!(repaired.messages, agent.messages());
        assert_eq!(repaired.last_usage, agent.last_usage());
        assert_eq!(repaired.cumulative_usage, agent.cumulative_usage());
    }

    #[tokio::test]
    async fn attach_rejects_a_dangling_tool_call_in_the_middle_of_history() {
        let storage = InMemorySessionStorage::new();
        let session_id = "reject-middle-dangling-call";
        let original = vec![
            Message::user("run it"),
            Message::assistant(
                None,
                vec![ToolCall::new("call-1", "echo", json!({ "text": "hello" }))],
            ),
            Message::user("this cannot skip the tool result"),
        ];
        storage
            .save(&SessionSnapshot {
                id: session_id.to_owned(),
                messages: original.clone(),
                last_usage: None,
                cumulative_usage: TokenUsage::default(),
                mode: AgentMode::default(),
            })
            .await
            .unwrap();

        let result = Agent::builder(MockProvider {
            responses: Mutex::new(VecDeque::new()),
        })
        .build()
        .with_session(session_id, storage.clone())
        .await;
        let error = match result {
            Ok(_) => panic!("invalid middle transcript must be rejected"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            AgentError::Storage(StorageError::InvalidTranscript {
                ref session_id,
                ..
            }) if session_id == "reject-middle-dangling-call"
        ));
        assert_eq!(
            storage.load(session_id).await.unwrap().unwrap().messages,
            original,
            "a rejected transcript must not be rewritten"
        );
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
    async fn context_manager_rewrites_and_persists_context_after_threshold_response() {
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::from([ProviderResponse {
                message: AssistantMessage::text("answer"),
                usage: Some(TokenUsage::new(75, 10, 0)),
            }])),
        };
        let triggers = Arc::new(Mutex::new(Vec::new()));
        let storage = RecordingStorage::default();
        let observed = storage.clone();
        let mut agent = Agent::builder(provider)
            .max_context_tokens(100)
            .context_management_threshold_percent(80)
            .context_manager(ReplacingContextManager {
                triggers: Arc::clone(&triggers),
                replacement: "summary",
                continue_chain: false,
            })
            .build()
            .with_session("context-threshold", storage)
            .await
            .unwrap();

        let result = agent.prompt("question").await.unwrap();

        assert_eq!(agent.messages(), [Message::user("summary")]);
        assert_eq!(
            result.new_messages,
            [
                Message::user("question"),
                Message::assistant(Some(Content::text("answer")), Vec::new()),
            ]
        );
        assert!(matches!(
            triggers.lock().unwrap().as_slice(),
            [ContextManagementTrigger::UsageThreshold {
                usage: ContextUsage {
                    used_tokens: 85,
                    ..
                },
                threshold_percent: 80,
            }]
        ));
        assert_eq!(
            *observed.operations.lock().unwrap(),
            [
                SaveOperation::Incremental(0),
                SaveOperation::Incremental(1),
                SaveOperation::ReplaceFrom(0),
            ]
        );
        assert_eq!(
            observed
                .snapshots
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .messages
                .as_slice(),
            [Message::user("summary")].as_slice()
        );
    }

    #[tokio::test]
    async fn context_manager_does_not_run_below_the_usage_threshold() {
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::from([ProviderResponse {
                message: AssistantMessage::text("answer"),
                usage: Some(TokenUsage::new(70, 9, 0)),
            }])),
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let mut agent = Agent::builder(provider)
            .max_context_tokens(100)
            .context_management_threshold_percent(80)
            .context_manager(CountingContextManager {
                calls: Arc::clone(&calls),
            })
            .build();

        agent.prompt("question").await.unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(agent.messages().len(), 2);
    }

    #[tokio::test]
    async fn invalid_context_manager_rewrite_is_rejected_and_rolled_back() {
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::from([ProviderResponse {
                message: AssistantMessage::text("answer"),
                usage: Some(TokenUsage::new(75, 10, 0)),
            }])),
        };
        let storage = RecordingStorage::default();
        let observed = storage.clone();
        let mut agent = Agent::builder(provider)
            .max_context_tokens(100)
            .context_manager(InvalidContextManager)
            .build()
            .with_session("invalid-context-rewrite", storage)
            .await
            .unwrap();

        let error = agent.prompt("question").await.unwrap_err();

        assert!(matches!(error, AgentError::Hook(_)));
        assert_eq!(
            agent.messages(),
            [
                Message::user("question"),
                Message::assistant(Some(Content::text("answer")), Vec::new()),
            ]
        );
        assert_eq!(
            *observed.operations.lock().unwrap(),
            [SaveOperation::Incremental(0), SaveOperation::Incremental(1)]
        );
    }

    #[tokio::test]
    async fn stop_interrupts_an_async_context_manager_and_rolls_back_its_rewrite() {
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::from([ProviderResponse {
                message: AssistantMessage::text("answer"),
                usage: Some(TokenUsage::new(75, 10, 0)),
            }])),
        };
        let started = Arc::new(Notify::new());
        let control = AgentRunControl::new();
        let stop = control.clone();
        let manager_started = Arc::clone(&started);
        let task = tokio::spawn(async move {
            let mut agent = Agent::builder(provider)
                .max_context_tokens(100)
                .context_manager(HangingContextManager {
                    started: manager_started,
                })
                .build();
            let outcome = agent
                .prompt_content_controlled(Content::text("question"), control)
                .await
                .unwrap();
            (agent, outcome)
        });

        started.notified().await;
        stop.stop();
        let (agent, outcome) = task.await.unwrap();

        assert_eq!(outcome, AgentRunOutcome::Stopped);
        assert_eq!(
            agent.messages(),
            [
                Message::user("question"),
                Message::assistant(Some(Content::text("answer")), Vec::new()),
            ]
        );
    }

    #[tokio::test]
    async fn context_length_error_before_output_runs_managers_and_persists_changes() {
        let triggers = Arc::new(Mutex::new(Vec::new()));
        let storage = RecordingStorage::default();
        let observed = storage.clone();
        let mut agent = Agent::builder(ContextOverflowProvider {
            emit_partial_delta: false,
        })
        .max_context_tokens(100)
        .context_manager(ReplacingContextManager {
            triggers: Arc::clone(&triggers),
            replacement: "compacted after overflow",
            continue_chain: false,
        })
        .build()
        .with_session("context-overflow", storage)
        .await
        .unwrap();

        let error = agent.prompt("oversized question").await.unwrap_err();

        assert!(matches!(
            error,
            AgentError::Provider(ProviderError::ContextLengthExceeded { .. })
        ));
        assert!(matches!(
            triggers.lock().unwrap().as_slice(),
            [ContextManagementTrigger::ContextLengthExceeded { error }]
                if error.contains("maximum context length exceeded")
        ));
        assert_eq!(
            agent.messages(),
            [Message::user("compacted after overflow")]
        );
        assert_eq!(
            *observed.operations.lock().unwrap(),
            [SaveOperation::Incremental(0), SaveOperation::ReplaceFrom(0),]
        );
    }

    #[tokio::test]
    async fn context_length_error_after_partial_output_does_not_run_managers() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut agent = Agent::builder(ContextOverflowProvider {
            emit_partial_delta: true,
        })
        .context_manager(CountingContextManager {
            calls: Arc::clone(&calls),
        })
        .build();

        let error = agent.prompt("question").await.unwrap_err();

        assert!(matches!(
            error,
            AgentError::Provider(ProviderError::ContextLengthExceeded { .. })
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(agent.messages(), [Message::user("question")]);
    }

    #[tokio::test]
    async fn forwards_tool_progress_and_retains_rich_structured_output() {
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::from([
                ProviderResponse {
                    message: AssistantMessage::tool_calls(vec![ToolCall::new(
                        "call-progress",
                        "progress",
                        json!({}),
                    )]),
                    usage: None,
                },
                ProviderResponse {
                    message: AssistantMessage::text("done"),
                    usage: None,
                },
            ])),
        };
        let observed = Arc::new(Mutex::new(Vec::new()));
        let observed_events = Arc::clone(&observed);
        let mut agent = Agent::builder(provider).tool(ProgressTool).build();
        agent.subscribe(move |event| match event {
            AgentEvent::ToolExecutionStart { .. } => {
                observed_events.lock().unwrap().push("start");
            }
            AgentEvent::ToolExecutionProgress { progress, .. } => {
                assert_eq!(progress.message, "halfway");
                assert_eq!(progress.metadata, Some(json!({ "percent": 50 })));
                observed_events.lock().unwrap().push("progress");
            }
            AgentEvent::ToolExecutionEnd {
                content_parts,
                metadata,
                ..
            } => {
                assert_eq!(content_parts.len(), 1);
                assert_eq!(metadata, &Some(json!({ "kind": "progress_test" })));
                observed_events.lock().unwrap().push("end");
            }
            _ => {}
        });

        agent.prompt("run it").await.unwrap();

        assert_eq!(*observed.lock().unwrap(), ["start", "progress", "end"]);
        let tool_result = &agent.messages()[2];
        assert_eq!(
            tool_result.tool_result_metadata,
            Some(json!({ "kind": "progress_test" }))
        );
        assert!(matches!(
            tool_result.content.as_ref(),
            Some(Content::Parts(parts)) if parts.len() == 2
        ));
    }

    #[tokio::test]
    async fn bounds_safe_parallel_tools_and_serializes_exclusive_tools() {
        async fn run_probe(effect: ToolEffect, maximum_allowed: usize) -> usize {
            let calls = (0..5)
                .map(|index| ToolCall::new(format!("probe-{index}"), "probe", json!({})))
                .collect();
            let provider = MockProvider {
                responses: Mutex::new(VecDeque::from([
                    ProviderResponse {
                        message: AssistantMessage::tool_calls(calls),
                        usage: None,
                    },
                    ProviderResponse {
                        message: AssistantMessage::text("done"),
                        usage: None,
                    },
                ])),
            };
            let active = Arc::new(AtomicUsize::new(0));
            let maximum = Arc::new(AtomicUsize::new(0));
            let mut agent = Agent::builder(provider)
                .tool(ConcurrencyProbeTool {
                    active,
                    maximum: Arc::clone(&maximum),
                    effect,
                })
                .max_parallel_tools(maximum_allowed)
                .build();
            agent.prompt("probe").await.unwrap();
            maximum.load(Ordering::SeqCst)
        }

        assert_eq!(run_probe(ToolEffect::ReadOnly, 2).await, 2);
        assert_eq!(run_probe(ToolEffect::ExternalSideEffect, 2).await, 1);
    }

    #[tokio::test]
    async fn continues_tool_calls_beyond_the_previous_turn_limit() {
        let mut responses = (1..=17)
            .map(|turn| ProviderResponse {
                message: AssistantMessage::tool_calls(vec![ToolCall::new(
                    format!("call-{turn}"),
                    "echo",
                    json!({ "text": turn.to_string() }),
                )]),
                usage: None,
            })
            .collect::<VecDeque<_>>();
        responses.push_back(ProviderResponse {
            message: AssistantMessage::text("done"),
            usage: None,
        });
        let provider = MockProvider {
            responses: Mutex::new(responses),
        };
        let mut agent = Agent::builder(provider).tool(EchoTool).build();

        let result = agent.prompt("keep going").await.unwrap();

        assert_eq!(result.text(), Some("done"));
        assert_eq!(result.turns, 18);
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
    async fn plan_mode_refilters_hook_mutations_and_injects_a_system_reminder() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let executions = Arc::new(AtomicUsize::new(0));
        let provider = RecordingProvider {
            response: Mutex::new(Some(ProviderResponse {
                message: AssistantMessage::text("planned"),
                usage: None,
            })),
            requests: Arc::clone(&requests),
        };
        let mut agent = Agent::builder(provider)
            .mode(AgentMode::Plan)
            .tool(ReadOnlyTool)
            .tool(CountingTool {
                executions: Arc::clone(&executions),
            })
            .hook(InjectToolDefinitionsHook)
            .build();

        agent.prompt("make a plan").await.unwrap();

        let requests = requests.lock().unwrap();
        let request = &requests[0];
        assert_eq!(
            request
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["inspect"]
        );
        assert!(request.messages.iter().any(|message| {
            message.role == Role::System
                && message.text_content() == Some(PLAN_MODE_SYSTEM_REMINDER)
        }));
        assert_eq!(executions.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn plan_mode_runtime_guard_rejects_an_unadvertised_side_effect() {
        let executions = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = RecordingQueueProvider {
            responses: Mutex::new(VecDeque::from([
                ProviderResponse {
                    message: AssistantMessage::tool_calls(vec![ToolCall::new(
                        "call-count",
                        "count",
                        json!({}),
                    )]),
                    usage: None,
                },
                ProviderResponse {
                    message: AssistantMessage::text("done"),
                    usage: None,
                },
            ])),
            requests,
        };
        let mut agent = Agent::builder(provider)
            .mode(AgentMode::Plan)
            .tool(CountingTool {
                executions: Arc::clone(&executions),
            })
            .build();

        agent.prompt("try it").await.unwrap();

        assert_eq!(executions.load(Ordering::SeqCst), 0);
        let result = agent
            .messages()
            .iter()
            .find(|message| message.tool_call_id.as_deref() == Some("call-count"))
            .expect("blocked call must still receive a protocol-valid tool result");
        assert!(result.tool_result_is_error);
        assert!(result.text_content().unwrap().contains("not available"));
    }

    #[tokio::test]
    async fn leaving_plan_while_a_request_is_streaming_cannot_unlock_its_tool_calls() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let executions = Arc::new(AtomicUsize::new(0));
        let provider = PausedToolCallProvider {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
            calls: AtomicUsize::new(0),
        };
        let agent = Agent::builder(provider)
            .mode(AgentMode::Plan)
            .tool(CountingTool {
                executions: Arc::clone(&executions),
            })
            .build();
        let mode = agent.mode_control();

        let run = tokio::spawn(async move {
            let mut agent = agent;
            let result = agent.prompt("plan first").await;
            (agent, result)
        });
        started.notified().await;
        mode.set_mode(AgentMode::Default);
        release.notify_one();
        let (agent, result) = run.await.unwrap();
        result.unwrap();

        assert_eq!(agent.mode(), AgentMode::Default);
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        let result = agent
            .messages()
            .iter()
            .find(|message| message.tool_call_id.as_deref() == Some("call-after-mode-change"))
            .unwrap();
        assert!(result.tool_result_is_error);
        assert!(result.text_content().unwrap().contains("not available"));
    }

    #[tokio::test]
    async fn a_plan_exit_does_not_unlock_sibling_calls_in_the_same_response() {
        let executions = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let storage = InMemorySessionStorage::new();
        let provider = RecordingQueueProvider {
            responses: Mutex::new(VecDeque::from([
                ProviderResponse {
                    message: AssistantMessage::tool_calls(vec![
                        ToolCall::new("call-exit", "exit_plan_mode", json!({})),
                        ToolCall::new("call-count", "count", json!({})),
                    ]),
                    usage: None,
                },
                ProviderResponse {
                    message: AssistantMessage::text("executing next turn"),
                    usage: None,
                },
            ])),
            requests: Arc::clone(&requests),
        };
        let mut agent = Agent::builder(provider)
            .mode(AgentMode::Plan)
            .tool(CountingTool {
                executions: Arc::clone(&executions),
            })
            .tool_execution(ToolExecutionMode::Sequential)
            .build();
        agent.add_tool(ExitModeTool {
            mode: agent.mode_control(),
        });
        agent
            .attach_session("frozen-plan-batch", storage.clone())
            .await
            .unwrap();

        agent.prompt("approve and execute").await.unwrap();

        assert_eq!(agent.mode(), AgentMode::Default);
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        {
            let requests = requests.lock().unwrap();
            assert_eq!(
                requests[0]
                    .tools
                    .iter()
                    .map(|tool| tool.name.as_str())
                    .collect::<Vec<_>>(),
                ["exit_plan_mode"]
            );
            assert_eq!(
                requests[1]
                    .tools
                    .iter()
                    .map(|tool| tool.name.as_str())
                    .collect::<Vec<_>>(),
                ["count"]
            );
        }
        assert_eq!(
            storage
                .load("frozen-plan-batch")
                .await
                .unwrap()
                .unwrap()
                .mode,
            AgentMode::Default
        );
    }

    #[tokio::test]
    async fn entering_plan_blocks_later_side_effects_in_sequential_and_parallel_agents() {
        for execution_mode in [ToolExecutionMode::Sequential, ToolExecutionMode::Parallel] {
            let executions = Arc::new(AtomicUsize::new(0));
            let provider = RecordingQueueProvider {
                responses: Mutex::new(VecDeque::from([
                    ProviderResponse {
                        message: AssistantMessage::tool_calls(vec![
                            ToolCall::new("call-enter", "enter_plan_mode", json!({})),
                            ToolCall::new("call-count", "count", json!({})),
                        ]),
                        usage: None,
                    },
                    ProviderResponse {
                        message: AssistantMessage::text("planned"),
                        usage: None,
                    },
                ])),
                requests: Arc::new(Mutex::new(Vec::new())),
            };
            let mut agent = Agent::builder(provider)
                .tool(CountingTool {
                    executions: Arc::clone(&executions),
                })
                .tool_execution(execution_mode)
                .build();
            agent.add_tool(EnterModeTool {
                mode: agent.mode_control(),
            });

            agent.prompt("enter plan first").await.unwrap();

            assert_eq!(agent.mode(), AgentMode::Plan, "mode: {execution_mode:?}");
            assert_eq!(
                executions.load(Ordering::SeqCst),
                0,
                "mode: {execution_mode:?}"
            );
            let result = agent
                .messages()
                .iter()
                .find(|message| message.tool_call_id.as_deref() == Some("call-count"))
                .unwrap();
            assert!(result.tool_result_is_error, "mode: {execution_mode:?}");
            assert!(result.text_content().unwrap().contains("not available"));
        }
    }

    #[tokio::test]
    async fn exiting_plan_blocks_later_plan_mutations_in_sequential_and_parallel_agents() {
        for execution_mode in [ToolExecutionMode::Sequential, ToolExecutionMode::Parallel] {
            let plan_updates = Arc::new(AtomicUsize::new(0));
            let provider = RecordingQueueProvider {
                responses: Mutex::new(VecDeque::from([
                    ProviderResponse {
                        message: AssistantMessage::tool_calls(vec![
                            ToolCall::new("call-exit", "exit_plan_mode", json!({})),
                            ToolCall::new("call-update", "update_plan", json!({})),
                        ]),
                        usage: None,
                    },
                    ProviderResponse {
                        message: AssistantMessage::text("approved"),
                        usage: None,
                    },
                ])),
                requests: Arc::new(Mutex::new(Vec::new())),
            };
            let mut agent = Agent::builder(provider)
                .mode(AgentMode::Plan)
                .tool(PlanOnlyCountingTool {
                    executions: Arc::clone(&plan_updates),
                })
                .tool_execution(execution_mode)
                .build();
            agent.add_tool(ExitModeTool {
                mode: agent.mode_control(),
            });

            agent.prompt("approve this revision").await.unwrap();

            assert_eq!(agent.mode(), AgentMode::Default, "mode: {execution_mode:?}");
            assert_eq!(
                plan_updates.load(Ordering::SeqCst),
                0,
                "mode: {execution_mode:?}"
            );
            let result = agent
                .messages()
                .iter()
                .find(|message| message.tool_call_id.as_deref() == Some("call-update"))
                .unwrap();
            assert!(result.tool_result_is_error, "mode: {execution_mode:?}");
            assert!(result.text_content().unwrap().contains("not available"));
        }
    }

    #[tokio::test]
    async fn set_mode_persists_without_a_transcript_change() {
        let directory = tempfile::tempdir().unwrap();
        let storage = DiskSessionStorage::new(directory.path());
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::new()),
        };
        let mut agent = Agent::builder(provider)
            .build()
            .with_session("mode-only", storage.clone())
            .await
            .unwrap();

        agent.set_mode(AgentMode::Plan).await.unwrap();

        assert_eq!(agent.mode(), AgentMode::Plan);
        let snapshot = storage.load("mode-only").await.unwrap().unwrap();
        assert!(snapshot.messages.is_empty());
        assert_eq!(snapshot.mode, AgentMode::Plan);
    }

    #[tokio::test]
    async fn failed_persistence_cannot_unlock_plan_mode() {
        let storage = FailOnSaveStorage::new(1);
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::new()),
        };
        let mut agent = Agent::builder(provider)
            .mode(AgentMode::Plan)
            .build()
            .with_session("mode-save-fails", storage)
            .await
            .unwrap();

        assert!(agent.set_mode(AgentMode::Default).await.is_err());
        assert_eq!(agent.mode(), AgentMode::Plan);
    }

    #[tokio::test]
    async fn synchronizes_after_user_llm_responses_and_tool_results() {
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
        assert_eq!(snapshots.len(), 4);
        assert_eq!(snapshots[0].messages.len(), 1);
        assert_eq!(snapshots[0].messages[0].role, Role::User);
        assert_eq!(snapshots[1].messages.len(), 3);
        assert_eq!(snapshots[1].messages[1].role, Role::Assistant);
        assert_eq!(snapshots[1].messages[1].tool_calls.len(), 1);
        assert_eq!(snapshots[1].messages[2].role, Role::Tool);
        assert_eq!(
            snapshots[1].messages[2].text_content(),
            Some(UNKNOWN_TOOL_RESULT)
        );
        assert_eq!(snapshots[2].messages.len(), 3);
        assert_eq!(snapshots[2].messages[2].text_content(), Some("hello"));
        assert_eq!(snapshots[3].messages.len(), 4);
        assert_eq!(snapshots[3].messages[3].role, Role::Assistant);
        assert_eq!(snapshots[3].cumulative_usage.total_tokens, 35);
        assert_eq!(
            *observed.operations.lock().unwrap(),
            [
                SaveOperation::Incremental(0),
                SaveOperation::Incremental(1),
                SaveOperation::ReplaceFrom(1),
                SaveOperation::Incremental(3),
            ]
        );
    }

    #[tokio::test]
    async fn clearing_messages_replaces_the_persisted_transcript_from_zero() {
        let provider = MockProvider {
            responses: Mutex::new(VecDeque::from([
                ProviderResponse {
                    message: AssistantMessage::text("first"),
                    usage: None,
                },
                ProviderResponse {
                    message: AssistantMessage::text("second"),
                    usage: None,
                },
            ])),
        };
        let storage = RecordingStorage::default();
        let observed = storage.clone();
        let mut agent = Agent::builder(provider)
            .build()
            .with_session("clear-replaces", storage)
            .await
            .unwrap();

        agent.prompt("before clear").await.unwrap();
        agent.clear_messages();
        agent.prompt("after clear").await.unwrap();

        assert_eq!(
            *observed.operations.lock().unwrap(),
            [
                SaveOperation::Incremental(0),
                SaveOperation::Incremental(1),
                SaveOperation::ReplaceFrom(0),
                SaveOperation::Incremental(1),
            ]
        );
        assert_eq!(agent.messages().len(), 2);
        assert_eq!(agent.messages()[0], Message::user("after clear"));
        assert_eq!(agent.messages()[1].text_content(), Some("second"));
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
                mode: AgentMode::Plan,
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
        assert_eq!(agent.mode(), AgentMode::Plan);
    }
}

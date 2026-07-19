use std::{
    io,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc, Barrier,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use futures_util::{StreamExt, stream};
use phi::{
    Agent, AgentEvent, AssistantDelta, AssistantMessage, CapabilityMode, Content,
    ContextCompactionError, ContextCompactionPlan, ContextCompactionRequest, ContextCompactor,
    InMemorySessionStorage, LlmProvider, Message, MessageVisibility, ProviderError, ProviderEvent,
    ProviderEventStream, ProviderRequest, ProviderResponse, Role, SessionSnapshot, SessionStorage,
    SkillCatalog, TokenUsage, Tool, ToolCall, ToolDefinition, ToolEffect, ToolError,
    ToolExecutionContext, ToolOutput, Workspace,
};
use phi_daemon::{
    api::AppState,
    runtime::{
        AgentBuildRequest, AgentFactory, AgentFactoryError, AgentHandleError, AgentRegistry,
        AgentStatus, BuiltAgent, RunId, RuntimeEvent, RuntimeEventKind, SessionId,
        compile_agent_profile, default_agent_profile,
    },
    serve,
    service::{ApplicationService, ServiceError},
    store::{
        AgentProfileStore, ControlStore, MemoryAgentProfileStore, MemoryControlStore,
        MemoryProviderStore, ProviderStore, SessionRecord,
    },
};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        TcpListener, TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::{Notify, broadcast, oneshot},
    task::JoinHandle,
};

const PROFILE: &str = "default";
const MODEL: &str = "test-model";
const AUTH_KEY: &str = "a-secure-test-key-with-at-least-32-bytes";
const WS_PROTOCOL: &str = "phi.v1";
const WS_AUTH_PROTOCOL_PREFIX: &str = "phi.auth.";

#[derive(Clone, Default)]
struct DeleteFailingSessionStorage {
    inner: InMemorySessionStorage,
}

#[async_trait]
impl SessionStorage for DeleteFailingSessionStorage {
    async fn load(&self, session_id: &str) -> Result<Option<SessionSnapshot>, phi::StorageError> {
        self.inner.load(session_id).await
    }

    async fn save(&self, session: &SessionSnapshot) -> Result<(), phi::StorageError> {
        self.inner.save(session).await
    }

    async fn delete(&self, _session_id: &str) -> Result<(), phi::StorageError> {
        Err(phi::StorageError::Io {
            path: "/injected/delete-failure".into(),
            source: std::io::Error::other("injected transcript deletion failure"),
        })
    }
}

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!("{prefix}-{}", SessionId::new()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone)]
enum ProviderScript {
    Immediate,
    PauseAtAgentEnd {
        entered: Arc<Barrier>,
        release: Arc<Barrier>,
    },
    PanicFirst,
    BlockFirst {
        started: Arc<Notify>,
        release: Arc<Notify>,
    },
    HangFirst {
        started: Arc<Notify>,
    },
    AskUser,
    ToolCall,
    PanickingToolCall,
    BackgroundNotification {
        observed: Arc<AtomicBool>,
    },
    Subagent,
    SubagentSafeBoundary {
        observed_in_parent_turn: Arc<AtomicBool>,
    },
}

#[derive(Clone)]
struct ScriptedProvider {
    script: ProviderScript,
    calls: Arc<AtomicUsize>,
}

impl LlmProvider for ScriptedProvider {
    fn stream(&self, request: ProviderRequest) -> ProviderEventStream {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        match &self.script {
            ProviderScript::PanicFirst if call == 0 => panic!("injected provider panic"),
            ProviderScript::BlockFirst { started, release } if call == 0 => {
                let started = Arc::clone(started);
                let release = Arc::clone(release);
                Box::pin(
                    stream::once(async move {
                        started.notify_one();
                        Ok(ProviderEvent::Delta(AssistantDelta::Text {
                            delta: "partial-1".to_owned(),
                        }))
                    })
                    .chain(stream::once(async move {
                        release.notified().await;
                        Ok(ProviderEvent::Done(text_response("answer-1")))
                    })),
                )
            }
            ProviderScript::HangFirst { started } if call == 0 => {
                let started = Arc::clone(started);
                Box::pin(
                    stream::once(async move {
                        started.notify_one();
                        Ok(ProviderEvent::Delta(AssistantDelta::Text {
                            delta: "partial-1".to_owned(),
                        }))
                    })
                    .chain(stream::pending::<Result<ProviderEvent, ProviderError>>()),
                )
            }
            ProviderScript::AskUser if call == 0 => {
                let askuser = request
                    .tools
                    .iter()
                    .find(|tool| tool.name == "askuser")
                    .expect("daemon-created agents must expose the askuser tool");
                assert!(askuser.description.contains("genuinely the user's to make"));
                assert_eq!(askuser.parameters["properties"]["questions"]["maxItems"], 3);
                Box::pin(stream::iter([Ok(ProviderEvent::Done(ProviderResponse {
                    message: AssistantMessage::tool_calls(vec![ToolCall::new(
                        "ask-call-1",
                        "askuser",
                        json!({
                            "questions": [
                                {
                                    "question": "Which layout should I use?",
                                    "header": "Layout",
                                    "options": [
                                        {
                                            "label": "Compact (Recommended)",
                                            "description": "Keep the interface dense",
                                            "preview": "[A] [B]"
                                        },
                                        {
                                            "label": "Spacious",
                                            "description": "Use more whitespace",
                                            "preview": "[ A ]   [ B ]"
                                        }
                                    ],
                                    "multiSelect": false
                                },
                                {
                                    "question": "Which extras should I include?",
                                    "header": "Extras",
                                    "options": [
                                        {
                                            "label": "Tests",
                                            "description": "Add automated tests"
                                        },
                                        {
                                            "label": "Docs",
                                            "description": "Add usage documentation"
                                        }
                                    ],
                                    "multiSelect": true
                                }
                            ]
                        }),
                    )]),
                    usage: None,
                }))]))
            }
            ProviderScript::AskUser => {
                let result = request
                    .messages
                    .iter()
                    .find(|message| {
                        message.role == Role::Tool
                            && message.tool_call_id.as_deref() == Some("ask-call-1")
                    })
                    .expect("the answered askuser result must be sent back to the provider");
                assert!(!result.tool_result_is_error);
                assert!(
                    result
                        .text_content()
                        .is_some_and(|content| content.contains("My custom layout"))
                );
                immediate_stream(call)
            }
            ProviderScript::ToolCall if call == 0 => {
                Box::pin(stream::iter([Ok(ProviderEvent::Done(ProviderResponse {
                    message: AssistantMessage {
                        content: Some(Content::text("I will inspect with a tool.")),
                        reasoning: None,
                        tool_calls: vec![ToolCall::new("call-1", "blocking_tool", json!({}))],
                        provider_state: None,
                    },
                    usage: None,
                }))]))
            }
            ProviderScript::PanickingToolCall if call == 0 => {
                Box::pin(stream::iter([Ok(ProviderEvent::Done(ProviderResponse {
                    message: AssistantMessage::tool_calls(vec![ToolCall::new(
                        "call-panic",
                        "panic_tool",
                        json!({}),
                    )]),
                    usage: None,
                }))]))
            }
            ProviderScript::BackgroundNotification { observed } => {
                let notification_seen = request.messages.iter().any(|message| {
                    message.role == Role::User
                        && message.visibility == MessageVisibility::Internal
                        && message
                            .text_content()
                            .is_some_and(|text| text.contains("<task_notification>"))
                });
                if notification_seen {
                    observed.store(true, Ordering::SeqCst);
                    return immediate_stream(call);
                }
                let started = request.messages.iter().any(|message| {
                    message.role == Role::Tool
                        && message.tool_call_id.as_deref() == Some("delayed-notification-call")
                });
                if started {
                    immediate_stream(call)
                } else {
                    Box::pin(stream::iter([Ok(ProviderEvent::Done(ProviderResponse {
                        message: AssistantMessage::tool_calls(vec![ToolCall::new(
                            "delayed-notification-call",
                            "delayed_notification",
                            json!({}),
                        )]),
                        usage: None,
                    }))]))
                }
            }
            ProviderScript::Subagent => {
                let is_parent = request.tools.iter().any(|tool| tool.name == "spawn_agent");
                if is_parent {
                    let tool_names = request
                        .tools
                        .iter()
                        .map(|tool| tool.name.as_str())
                        .collect::<Vec<_>>();
                    assert!(tool_names.contains(&"send_agent_message"));
                    assert!(tool_names.contains(&"close_agent"));
                    let already_spawned = request.messages.iter().any(|message| {
                        message.role == Role::Tool
                            && message.tool_call_id.as_deref() == Some("spawn-call-1")
                    });
                    if already_spawned {
                        immediate_stream(call)
                    } else {
                        Box::pin(stream::iter([Ok(ProviderEvent::Done(ProviderResponse {
                            message: AssistantMessage::tool_calls(vec![ToolCall::new(
                                "spawn-call-1",
                                "spawn_agent",
                                json!({
                                    "description": "observer test child",
                                    "prompt": "report status and finish",
                                    "run_in_background": true
                                }),
                            )]),
                            usage: None,
                        }))]))
                    }
                } else {
                    assert!(
                        request
                            .tools
                            .iter()
                            .any(|tool| tool.name == "notify_parent")
                    );
                    assert!(request.tools.iter().all(|tool| !matches!(
                        tool.name.as_str(),
                        "spawn_agent" | "send_agent_message" | "close_agent"
                    )));
                    let already_notified = request.messages.iter().any(|message| {
                        message.role == Role::Tool
                            && message.tool_call_id.as_deref() == Some("notify-call-1")
                    });
                    if already_notified {
                        immediate_stream(call)
                    } else {
                        Box::pin(stream::iter([Ok(ProviderEvent::Done(ProviderResponse {
                            message: AssistantMessage::tool_calls(vec![ToolCall::new(
                                "notify-call-1",
                                "notify_parent",
                                json!({
                                    "kind": "blocker",
                                    "message": "child needs parent attention"
                                }),
                            )]),
                            usage: None,
                        }))]))
                    }
                }
            }
            ProviderScript::SubagentSafeBoundary {
                observed_in_parent_turn,
            } => {
                let is_parent = request.tools.iter().any(|tool| tool.name == "spawn_agent");
                if !is_parent {
                    return immediate_stream(call);
                }
                let spawned = request.messages.iter().any(|message| {
                    message.role == Role::Tool
                        && message.tool_call_id.as_deref() == Some("spawn-boundary-child")
                });
                if !spawned {
                    return Box::pin(stream::iter([Ok(ProviderEvent::Done(ProviderResponse {
                        message: AssistantMessage::tool_calls(vec![ToolCall::new(
                            "spawn-boundary-child",
                            "spawn_agent",
                            json!({
                                "description": "safe-boundary child",
                                "prompt": "finish immediately",
                                "run_in_background": true
                            }),
                        )]),
                        usage: None,
                    }))]));
                }
                let notification_seen = request.messages.iter().any(|message| {
                    message.role == Role::User
                        && message.visibility == MessageVisibility::Internal
                        && message
                            .text_content()
                            .is_some_and(|text| text.contains("<subagent_notification>"))
                });
                if notification_seen {
                    observed_in_parent_turn.store(true, Ordering::SeqCst);
                    return immediate_stream(call);
                }
                let waited = request.messages.iter().any(|message| {
                    message.role == Role::Tool
                        && message.tool_call_id.as_deref() == Some("boundary-wait")
                });
                assert!(
                    !waited,
                    "notification missed the next protocol-safe boundary"
                );
                Box::pin(stream::iter([Ok(ProviderEvent::Done(ProviderResponse {
                    message: AssistantMessage::tool_calls(vec![ToolCall::new(
                        "boundary-wait",
                        "blocking_tool",
                        json!({}),
                    )]),
                    usage: None,
                }))]))
            }
            ProviderScript::Immediate
            | ProviderScript::PauseAtAgentEnd { .. }
            | ProviderScript::PanicFirst
            | ProviderScript::BlockFirst { .. }
            | ProviderScript::HangFirst { .. } => immediate_stream(call),
            ProviderScript::ToolCall | ProviderScript::PanickingToolCall => immediate_stream(call),
        }
    }
}

fn immediate_stream(call: usize) -> ProviderEventStream {
    let answer = format!("answer-{}", call + 1);
    Box::pin(stream::iter([
        Ok(ProviderEvent::Delta(AssistantDelta::Text {
            delta: answer.clone(),
        })),
        Ok(ProviderEvent::Done(text_response(answer))),
    ]))
}

fn text_response(text: impl Into<String>) -> ProviderResponse {
    ProviderResponse {
        message: AssistantMessage::text(text),
        usage: None,
    }
}

#[derive(Clone)]
struct BlockingTool {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl Tool for BlockingTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "blocking_tool",
            "Waits until the test permits the in-flight tool to finish",
            json!({ "type": "object" }),
        )
    }

    async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        self.started.notify_one();
        self.release.notified().await;
        Ok(ToolOutput::success("tool completed"))
    }
}

#[derive(Clone)]
struct DelayedNotificationTool {
    release: Arc<Notify>,
}

#[async_trait]
impl Tool for DelayedNotificationTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "delayed_notification",
            "Schedules a deterministic test notification",
            json!({ "type": "object" }),
        )
    }

    fn effect(&self) -> ToolEffect {
        ToolEffect::Internal
    }

    async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        Err(ToolError::new("test tool requires execution context"))
    }

    async fn execute_with_context(
        &self,
        _arguments: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let release = Arc::clone(&self.release);
        tokio::spawn(async move {
            release.notified().await;
            let _ = context.notify_agent(
                "<task_notification>\n<task_id>test-task</task_id>\n<status>completed</status>\n</task_notification>",
            );
        });
        Ok(ToolOutput::success("notification scheduled"))
    }
}

#[derive(Clone)]
struct PanickingTool;

#[async_trait]
impl Tool for PanickingTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "panic_tool",
            "Panics to exercise the actor supervisor",
            json!({ "type": "object" }),
        )
    }

    async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        panic!("injected tool panic")
    }
}

#[derive(Clone)]
struct BlockingCompactor {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl ContextCompactor for BlockingCompactor {
    fn name(&self) -> &'static str {
        "blocking_test"
    }

    fn should_compact(&self, request: &ContextCompactionRequest) -> bool {
        !request.messages.is_empty()
    }

    fn prompt(&self, request: &ContextCompactionRequest) -> String {
        format!(
            "test compaction prompt: {}",
            request.trigger.instructions().unwrap_or("no instructions")
        )
    }

    async fn compact(
        &self,
        _provider: &dyn LlmProvider,
        _request: ContextCompactionRequest,
        _prompt: String,
    ) -> Result<ContextCompactionPlan, ContextCompactionError> {
        self.started.notify_one();
        self.release.notified().await;
        Ok(ContextCompactionPlan {
            messages: vec![Message::user("compacted summary")],
            summary: "compacted summary".to_owned(),
            usage: Some(TokenUsage::new(6, 2, 0)),
            estimated_context_tokens: 3,
        })
    }
}

#[derive(Clone)]
struct TestFactory {
    script: ProviderScript,
    provider_calls: Arc<AtomicUsize>,
    builds: Arc<AtomicUsize>,
    tool: Option<BlockingTool>,
    notification_tool_release: Option<Arc<Notify>>,
    context_compactor: Option<Arc<dyn ContextCompactor>>,
    default_workspace: Option<Workspace>,
}

impl TestFactory {
    fn new(script: ProviderScript) -> Self {
        Self {
            script,
            provider_calls: Arc::new(AtomicUsize::new(0)),
            builds: Arc::new(AtomicUsize::new(0)),
            tool: None,
            notification_tool_release: None,
            context_compactor: None,
            default_workspace: None,
        }
    }

    fn with_tool(mut self, tool: BlockingTool) -> Self {
        self.tool = Some(tool);
        self
    }

    fn with_notification_tool(mut self, release: Arc<Notify>) -> Self {
        self.notification_tool_release = Some(release);
        self
    }

    fn with_context_compactor(mut self, compactor: impl ContextCompactor + 'static) -> Self {
        self.context_compactor = Some(Arc::new(compactor));
        self
    }

    fn with_default_workspace(mut self, workspace: Workspace) -> Self {
        self.default_workspace = Some(workspace);
        self
    }

    fn build_count(&self) -> usize {
        self.builds.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl AgentFactory for TestFactory {
    async fn build(&self, request: &AgentBuildRequest) -> Result<BuiltAgent, AgentFactoryError> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        let profile_workspace = request
            .workspace
            .as_ref()
            .or(self.default_workspace.as_ref())
            .cloned()
            .unwrap_or_else(|| Workspace::new("."));
        let agent_profile = request.pinned_agent_profile.clone().unwrap_or_else(|| {
            compile_agent_profile(&default_agent_profile(), &profile_workspace).unwrap()
        });
        let model = request
            .model
            .clone()
            .or_else(|| agent_profile.definition.model.clone())
            .unwrap_or_else(|| MODEL.to_owned());
        let capability_mode = request
            .capability_mode
            .unwrap_or(agent_profile.definition.initial_capability_mode);
        let reasoning_effort = if request.reasoning_effort_is_override {
            request.reasoning_effort
        } else {
            agent_profile.definition.reasoning_effort
        };
        let mut builder = Agent::builder(ScriptedProvider {
            script: self.script.clone(),
            calls: Arc::clone(&self.provider_calls),
        })
        .model(model.clone())
        .system_prompt(agent_profile.compiled_system_prompt.clone())
        .capability_mode(capability_mode);
        if let Some(workspace) = request
            .workspace
            .as_ref()
            .or(self.default_workspace.as_ref())
        {
            builder = builder.workspace(workspace.clone());
        }
        if let Some(tool) = self.tool.clone() {
            builder = builder.tool(tool);
        }
        if let Some(release) = &self.notification_tool_release {
            builder = builder.tool(DelayedNotificationTool {
                release: Arc::clone(release),
            });
        }
        if let Some(compactor) = &self.context_compactor {
            builder = builder.shared_context_compactor(Arc::clone(compactor));
        }
        if matches!(&self.script, ProviderScript::PanickingToolCall) {
            builder = builder.tool(PanickingTool);
        }
        let mut agent = builder.build();
        if let ProviderScript::PauseAtAgentEnd { entered, release } = &self.script {
            let entered = Arc::clone(entered);
            let release = Arc::clone(release);
            agent.subscribe(move |event| {
                if matches!(event, AgentEvent::AgentEnd { .. }) {
                    // Hold the core future after it has selected Completed but
                    // before the daemon actor publishes its terminal event.
                    // A second runtime worker can now issue a deterministic
                    // stop in precisely that linearization window.
                    entered.wait();
                    release.wait();
                }
            });
        }

        Ok(BuiltAgent {
            agent,
            skills: SkillCatalog::default(),
            profile_id: request.profile_id.clone(),
            agent_profile,
            model,
            reasoning_effort,
        })
    }
}

fn test_service(
    registry: AgentRegistry,
    store: Arc<MemoryControlStore>,
    storage: Arc<InMemorySessionStorage>,
    factory: Arc<TestFactory>,
) -> ApplicationService {
    ApplicationService::new(registry, store, storage, factory)
}

async fn spawn_server(
    service: Arc<ApplicationService>,
) -> (
    SocketAddr,
    oneshot::Sender<()>,
    JoinHandle<Result<(), io::Error>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(serve(
        listener,
        AppState::new(service, AUTH_KEY),
        async move {
            let _ = stopped.await;
        },
    ));
    (address, stop, server)
}

async fn http_json(
    address: SocketAddr,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    http_json_with_auth(address, method, path, body, Some(AUTH_KEY)).await
}

async fn http_json_with_auth(
    address: SocketAddr,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
    auth_key: Option<&str>,
) -> (u16, serde_json::Value) {
    let body = body.map_or_else(Vec::new, |body| serde_json::to_vec(&body).unwrap());
    let mut connection = TcpStream::connect(address).await.unwrap();
    let authorization = auth_key.map_or_else(String::new, |key| {
        format!("Authorization: Bearer {key}\r\n")
    });
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\n{authorization}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    connection.write_all(request.as_bytes()).await.unwrap();
    connection.write_all(&body).await.unwrap();
    let mut response = Vec::new();
    connection.read_to_end(&mut response).await.unwrap();
    let separator = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("HTTP response must contain a header separator");
    let headers = std::str::from_utf8(&response[..separator]).unwrap();
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap()
        .parse()
        .unwrap();
    let body = &response[separator + 4..];
    let payload = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(body).unwrap()
    };
    (status, payload)
}

struct RawWebSocket {
    reader: OwnedReadHalf,
    writer: OwnedWriteHalf,
}

async fn websocket_handshake(
    address: SocketAddr,
    path: &str,
    protocols: Option<&str>,
) -> (u16, String) {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let protocol_header = protocols.map_or_else(String::new, |protocols| {
        format!("Sec-WebSocket-Protocol: {protocols}\r\n")
    });
    let request = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: {address}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\
         {protocol_header}\r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    while !response.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).await.unwrap();
        response.push(byte[0]);
    }
    let response = String::from_utf8(response).unwrap();
    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap()
        .parse()
        .unwrap();
    (status, response)
}

impl RawWebSocket {
    async fn connect(address: SocketAddr, path: &str) -> Self {
        let (status, response) = http_json(address, "POST", "/v1/auth/token", None).await;
        assert_eq!(status, 200, "could not obtain WebSocket token: {response}");
        let token = response["token"].as_str().unwrap();
        let protocols = format!("{WS_PROTOCOL}, {WS_AUTH_PROTOCOL_PREFIX}{token}");
        let mut stream = TcpStream::connect(address).await.unwrap();
        let request = format!(
            "GET {path} HTTP/1.1\r\n\
             Host: {address}\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n\
             Sec-WebSocket-Protocol: {protocols}\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.unwrap();

        // Read exactly through the HTTP header boundary so a WebSocket frame
        // sent with the handshake remains available to the frame decoder.
        let mut response = Vec::new();
        while !response.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).await.unwrap();
            response.push(byte[0]);
        }
        let response = String::from_utf8(response).unwrap();
        assert!(response.starts_with("HTTP/1.1 101 Switching Protocols"));
        assert!(response.lines().any(|line| {
            line.eq_ignore_ascii_case(&format!("Sec-WebSocket-Protocol: {WS_PROTOCOL}"))
        }));
        assert!(
            !response.contains(token),
            "the credential protocol must never be selected or echoed"
        );
        let (reader, writer) = stream.into_split();
        Self { reader, writer }
    }

    async fn send_json(&mut self, value: serde_json::Value) {
        self.write_frame(0x1, value.to_string().as_bytes()).await;
    }

    async fn receive_json(&mut self) -> serde_json::Value {
        loop {
            let (opcode, payload) = self.read_frame().await;
            match opcode {
                0x1 => return serde_json::from_slice(&payload).unwrap(),
                0x8 => panic!("server closed the WebSocket before the expected message"),
                0x9 => self.write_frame(0xA, &payload).await,
                _ => {}
            }
        }
    }

    async fn receive_close(&mut self) -> (u16, String) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let (opcode, payload) = self.read_frame().await;
                match opcode {
                    0x8 => {
                        assert!(payload.len() >= 2, "close frame must include a status code");
                        let code = u16::from_be_bytes([payload[0], payload[1]]);
                        let reason = String::from_utf8(payload[2..].to_vec()).unwrap();
                        return (code, reason);
                    }
                    0x9 => self.write_frame(0xA, &payload).await,
                    _ => {}
                }
            }
        })
        .await
        .expect("server did not close the WebSocket")
    }

    async fn write_frame(&mut self, opcode: u8, payload: &[u8]) {
        let mask = [0x13, 0x37, 0x42, 0x99];
        let mut frame = vec![0x80 | opcode];
        match payload.len() {
            length @ 0..=125 => frame.push(0x80 | length as u8),
            length @ 126..=65_535 => {
                frame.push(0x80 | 126);
                frame.extend_from_slice(&(length as u16).to_be_bytes());
            }
            length => {
                frame.push(0x80 | 127);
                frame.extend_from_slice(&(length as u64).to_be_bytes());
            }
        }
        frame.extend_from_slice(&mask);
        frame.extend(
            payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ mask[index % mask.len()]),
        );
        self.writer.write_all(&frame).await.unwrap();
    }

    async fn read_frame(&mut self) -> (u8, Vec<u8>) {
        let mut header = [0; 2];
        self.reader.read_exact(&mut header).await.unwrap();
        let opcode = header[0] & 0x0F;
        let masked = header[1] & 0x80 != 0;
        let mut length = u64::from(header[1] & 0x7F);
        if length == 126 {
            let mut extended = [0; 2];
            self.reader.read_exact(&mut extended).await.unwrap();
            length = u64::from(u16::from_be_bytes(extended));
        } else if length == 127 {
            let mut extended = [0; 8];
            self.reader.read_exact(&mut extended).await.unwrap();
            length = u64::from_be_bytes(extended);
        }
        let mask = if masked {
            let mut mask = [0; 4];
            self.reader.read_exact(&mut mask).await.unwrap();
            Some(mask)
        } else {
            None
        };
        let mut payload = vec![0; usize::try_from(length).unwrap()];
        self.reader.read_exact(&mut payload).await.unwrap();
        if let Some(mask) = mask {
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[index % mask.len()];
            }
        }
        (opcode, payload)
    }
}

async fn wait_for_run_completed(
    events: &mut broadcast::Receiver<RuntimeEvent>,
    run_id: RunId,
) -> RuntimeEvent {
    wait_for_event(events, |event| {
        matches!(event.kind, RuntimeEventKind::RunCompleted { run_id: current } if current == run_id)
    })
    .await
}

async fn wait_for_run_stopped(
    events: &mut broadcast::Receiver<RuntimeEvent>,
    run_id: RunId,
) -> RuntimeEvent {
    wait_for_event(events, |event| {
        matches!(event.kind, RuntimeEventKind::RunStopped { run_id: current } if current == run_id)
    })
    .await
}

async fn wait_for_event(
    events: &mut broadcast::Receiver<RuntimeEvent>,
    mut predicate: impl FnMut(&RuntimeEvent) -> bool,
) -> RuntimeEvent {
    loop {
        let event = events.recv().await.expect("runtime event channel closed");
        if predicate(&event) {
            return event;
        }
    }
}

async fn wait_for_actor_crash_and_close(
    events: &mut broadcast::Receiver<RuntimeEvent>,
) -> (String, bool) {
    let mut crash_message = None;
    let mut active_run_failed = false;
    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
            .await
            .expect("actor did not publish a terminal event")
            .expect("runtime event channel closed");
        match event.kind {
            RuntimeEventKind::ActorCrashed { message } => crash_message = Some(message),
            RuntimeEventKind::RunFailed { .. } => active_run_failed = true,
            RuntimeEventKind::StateChanged {
                status: AgentStatus::Closed,
            } => {
                return (
                    crash_message.expect("ActorCrashed must precede Closed"),
                    active_run_failed,
                );
            }
            _ => {}
        }
    }
}

#[tokio::test]
async fn prepared_session_is_invisible_until_first_prompt_activation() {
    let registry = AgentRegistry::new();
    let store = Arc::new(MemoryControlStore::new());
    let storage = Arc::new(InMemorySessionStorage::new());
    let factory = Arc::new(TestFactory::new(ProviderScript::Immediate));
    let service = test_service(
        registry.clone(),
        Arc::clone(&store),
        Arc::clone(&storage),
        Arc::clone(&factory),
    );

    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let session_id = prepared.handle().session_id();
    assert_eq!(factory.build_count(), 1);
    assert!(!prepared.handle().snapshot().initialized);
    assert_eq!(
        prepared.handle().snapshot().status,
        AgentStatus::AwaitingFirstPrompt
    );
    assert!(service.list_sessions().await.unwrap().is_empty());
    assert!(registry.is_empty().await);
    assert!(
        storage
            .load(&session_id.to_string())
            .await
            .unwrap()
            .is_none()
    );

    // This is the operation the `/new` socket performs only when it receives
    // its first prompt; preparation alone must never make the session visible.
    let handle = service.activate_session(&prepared).await.unwrap();
    let mut events = handle.subscribe();
    let queued = handle
        .enqueue_prompt(Content::text("first prompt"))
        .await
        .unwrap();
    wait_for_run_completed(&mut events, queued.run_id).await;

    let sessions = service.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].record.id, session_id);
    assert_eq!(
        sessions[0].state.as_ref().unwrap().status,
        AgentStatus::Idle
    );
    assert_eq!(sessions[0].state.as_ref().unwrap().message_count, 2);
    assert_eq!(registry.len().await, 1);
    assert!(
        storage
            .load(&session_id.to_string())
            .await
            .unwrap()
            .is_some()
    );

    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn session_workspace_is_persisted_and_used_for_restore() {
    let store = Arc::new(MemoryControlStore::new());
    let storage = Arc::new(InMemorySessionStorage::new());
    let workspace = Workspace::new("/workspace/session-project");
    let service = test_service(
        AgentRegistry::new(),
        Arc::clone(&store),
        Arc::clone(&storage),
        Arc::new(TestFactory::new(ProviderScript::Immediate)),
    );

    let prepared = service
        .prepare_session_in_workspace(PROFILE, workspace.clone())
        .await
        .unwrap();
    let session_id = prepared.handle().session_id();
    let handle = service.activate_session(&prepared).await.unwrap();

    assert_eq!(handle.summary().workspace.as_ref(), Some(&workspace));
    assert_eq!(
        store
            .get_session(session_id)
            .await
            .unwrap()
            .unwrap()
            .workspace
            .as_ref(),
        Some(&workspace)
    );
    assert!(service.shutdown().await.is_empty());

    let restored_service = test_service(
        AgentRegistry::new(),
        Arc::clone(&store),
        storage,
        Arc::new(TestFactory::new(ProviderScript::Immediate)),
    );
    let restored = restored_service.attach_session(session_id).await.unwrap();
    assert_eq!(restored.summary().workspace.as_ref(), Some(&workspace));
    assert!(restored_service.shutdown().await.is_empty());
}

#[tokio::test]
async fn workspace_browser_and_new_websocket_select_an_explicit_directory() {
    let root = TemporaryDirectory::new("phi-daemon-workspace-api-test");
    let selected = root.0.join("selected-project");
    std::fs::create_dir(&selected).unwrap();
    std::fs::create_dir(root.0.join("another-project")).unwrap();
    std::fs::write(root.0.join("not-a-directory.txt"), "file").unwrap();
    let canonical_root = std::fs::canonicalize(&root.0).unwrap();
    let canonical_selected = std::fs::canonicalize(&selected).unwrap();

    let service = Arc::new(test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        Arc::new(TestFactory::new(ProviderScript::Immediate)),
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(serve(
        listener,
        AppState::new(Arc::clone(&service), AUTH_KEY)
            .with_default_workspace(Workspace::new(&canonical_root)),
        async move {
            let _ = stopped.await;
        },
    ));

    let (status, response) = http_json(address, "GET", "/v1/workspaces/browse", None).await;
    assert_eq!(status, 200);
    assert_eq!(response["path"], canonical_root.to_string_lossy().as_ref());
    assert_eq!(
        response["directories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["another-project", "selected-project"]
    );
    assert_eq!(response["truncated"], false);
    assert_eq!(
        http_json_with_auth(address, "GET", "/v1/workspaces/browse", None, None,)
            .await
            .0,
        401
    );

    let path = format!("/v1/ws/new?workspace={}", canonical_selected.display());
    let mut socket = RawWebSocket::connect(address, &path).await;
    assert_eq!(socket.receive_json().await["type"], "building");
    let ready = socket.receive_json().await;
    assert_eq!(ready["type"], "ready");
    assert_eq!(
        ready["workspace"],
        canonical_selected.to_string_lossy().as_ref()
    );

    drop(socket);
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn legacy_session_workspace_is_bound_on_first_restore() {
    let session_id = phi_daemon::runtime::SessionId::new();
    let store = Arc::new(MemoryControlStore::new());
    store
        .create_session(SessionRecord::new(session_id, PROFILE, MODEL, None))
        .await
        .unwrap();
    let storage = Arc::new(InMemorySessionStorage::new());
    storage
        .save(&SessionSnapshot::new(session_id.to_string(), Vec::new()).unwrap())
        .await
        .unwrap();
    let workspace = Workspace::new("/workspace/migrated-session");
    let service = test_service(
        AgentRegistry::new(),
        Arc::clone(&store),
        Arc::clone(&storage),
        Arc::new(
            TestFactory::new(ProviderScript::Immediate).with_default_workspace(workspace.clone()),
        ),
    );

    let restored = service.attach_session(session_id).await.unwrap();

    assert_eq!(restored.summary().workspace.as_ref(), Some(&workspace));
    assert_eq!(
        store
            .get_session(session_id)
            .await
            .unwrap()
            .unwrap()
            .workspace
            .as_ref(),
        Some(&workspace)
    );
    assert_eq!(
        storage
            .load(&session_id.to_string())
            .await
            .unwrap()
            .unwrap()
            .workspace
            .as_ref(),
        Some(&workspace)
    );
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn legacy_session_without_agent_profile_pins_builtin_default_zero_on_restore() {
    let session_id = phi_daemon::runtime::SessionId::new();
    let control = Arc::new(MemoryControlStore::new());
    control
        .create_session(SessionRecord::new(
            session_id,
            PROFILE,
            "legacy-session-model",
            None,
        ))
        .await
        .unwrap();
    let providers = Arc::new(MemoryProviderStore::new());
    providers
        .replace_provider(phi_daemon::store::ProviderConfig::new(
            phi_daemon::store::ProviderKind::OpenAiResponses,
            "legacy-profile-secret",
            "http://127.0.0.1:9/v1",
            "provider-model",
            128_000,
        ))
        .await
        .unwrap();
    let service = ApplicationService::managed(
        AgentRegistry::new(),
        control.clone(),
        Arc::new(InMemorySessionStorage::new()),
        providers,
    );
    let later_default = phi_daemon::runtime::AgentProfileDefinition {
        prompt: phi_daemon::runtime::PromptDefinition {
            mode: phi_daemon::runtime::PromptMode::Full,
            text: "THIS LATER DEFAULT MUST NOT AFFECT LEGACY SESSIONS".to_owned(),
        },
        initial_capability_mode: CapabilityMode::ReadOnly,
        ..phi_daemon::runtime::AgentProfileDefinition::default()
    };
    let replacement = service
        .configure_agent_profile(phi_daemon::runtime::DEFAULT_AGENT_PROFILE_ID, later_default)
        .await
        .unwrap();
    assert_eq!(replacement.revision, 1);

    let restored = service.attach_session(session_id).await.unwrap();
    assert_eq!(
        restored.summary().agent_profile_id,
        phi_daemon::runtime::DEFAULT_AGENT_PROFILE_ID
    );
    assert_eq!(
        restored.summary().agent_profile_revision,
        phi_daemon::runtime::DEFAULT_AGENT_PROFILE_REVISION
    );
    assert_eq!(
        restored.summary().capability_mode,
        CapabilityMode::FullAccess
    );
    assert!(
        !restored
            .agent_profile()
            .compiled_system_prompt
            .contains("THIS LATER DEFAULT")
    );

    let migrated = control.get_session(session_id).await.unwrap().unwrap();
    let pinned = migrated
        .agent_profile
        .expect("legacy metadata must pin default@0 after a successful restore");
    assert_eq!(
        pinned.agent_profile_id,
        phi_daemon::runtime::DEFAULT_AGENT_PROFILE_ID
    );
    assert_eq!(
        pinned.revision,
        phi_daemon::runtime::DEFAULT_AGENT_PROFILE_REVISION
    );
    assert_eq!(
        pinned.definition.initial_capability_mode,
        CapabilityMode::FullAccess
    );
    assert!(!pinned.compiled_system_prompt.contains("THIS LATER DEFAULT"));

    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn activated_session_restores_its_pinned_agent_profile_revision() {
    let control = Arc::new(MemoryControlStore::new());
    let storage = Arc::new(InMemorySessionStorage::new());
    let providers = Arc::new(MemoryProviderStore::new());
    providers
        .replace_provider(phi_daemon::store::ProviderConfig::new(
            phi_daemon::store::ProviderKind::OpenAiResponses,
            "pinned-profile-secret",
            "http://127.0.0.1:9/v1",
            "provider-model",
            128_000,
        ))
        .await
        .unwrap();
    let profiles = Arc::new(MemoryAgentProfileStore::new());
    let first_definition = phi_daemon::runtime::AgentProfileDefinition {
        prompt: phi_daemon::runtime::PromptDefinition {
            mode: phi_daemon::runtime::PromptMode::Full,
            text: "PINNED PROFILE REVISION ONE".to_owned(),
        },
        initial_capability_mode: CapabilityMode::WorkspaceEdit,
        ..phi_daemon::runtime::AgentProfileDefinition::default()
    };
    let first_profile = profiles
        .replace_agent_profile("reviewer", first_definition)
        .await
        .unwrap();
    assert_eq!(first_profile.revision, 1);

    let first_service = ApplicationService::managed_with_profiles_skills_and_builtin_tools(
        AgentRegistry::new(),
        control.clone(),
        storage.clone(),
        providers.clone(),
        profiles.clone(),
        phi::SkillsConfig::disabled(),
        phi::BuiltinTools::none("."),
    );
    let prepared = first_service
        .prepare_session_configured(PROFILE, "reviewer", None)
        .await
        .unwrap();
    let session_id = prepared.handle().session_id();
    let first_handle = first_service.activate_session(&prepared).await.unwrap();
    assert_eq!(first_handle.summary().agent_profile_revision, 1);
    assert_eq!(
        first_handle.summary().capability_mode,
        CapabilityMode::WorkspaceEdit
    );
    assert!(
        first_handle
            .agent_profile()
            .compiled_system_prompt
            .contains("PINNED PROFILE REVISION ONE")
    );

    let second_definition = phi_daemon::runtime::AgentProfileDefinition {
        prompt: phi_daemon::runtime::PromptDefinition {
            mode: phi_daemon::runtime::PromptMode::Full,
            text: "LATEST PROFILE REVISION TWO".to_owned(),
        },
        initial_capability_mode: CapabilityMode::ReadOnly,
        ..phi_daemon::runtime::AgentProfileDefinition::default()
    };
    let second_profile = profiles
        .replace_agent_profile("reviewer", second_definition)
        .await
        .unwrap();
    assert_eq!(second_profile.revision, 2);
    assert!(first_service.shutdown().await.is_empty());

    let restarted = ApplicationService::managed_with_profiles_skills_and_builtin_tools(
        AgentRegistry::new(),
        control.clone(),
        storage,
        providers,
        profiles,
        phi::SkillsConfig::disabled(),
        phi::BuiltinTools::none("."),
    );
    let restored = restarted.attach_session(session_id).await.unwrap();
    assert_eq!(restored.summary().agent_profile_id, "reviewer");
    assert_eq!(restored.summary().agent_profile_revision, 1);
    assert_eq!(
        restored.summary().capability_mode,
        CapabilityMode::WorkspaceEdit
    );
    assert!(
        restored
            .agent_profile()
            .compiled_system_prompt
            .contains("PINNED PROFILE REVISION ONE")
    );
    assert!(
        !restored
            .agent_profile()
            .compiled_system_prompt
            .contains("LATEST PROFILE REVISION TWO")
    );
    assert_eq!(
        control
            .get_session(session_id)
            .await
            .unwrap()
            .unwrap()
            .agent_profile
            .unwrap()
            .revision,
        1
    );

    assert!(restarted.shutdown().await.is_empty());
}

#[tokio::test]
async fn service_shutdown_closes_and_forgets_unactivated_prepared_sessions() {
    let registry = AgentRegistry::new();
    let store = Arc::new(MemoryControlStore::new());
    let storage = Arc::new(InMemorySessionStorage::new());
    let service = test_service(
        registry.clone(),
        Arc::clone(&store),
        Arc::clone(&storage),
        Arc::new(TestFactory::new(ProviderScript::Immediate)),
    );
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let session_id = prepared.handle().session_id();
    let mut events = prepared.handle().subscribe();

    assert!(service.shutdown().await.is_empty());

    wait_for_event(&mut events, |event| {
        matches!(
            event.kind,
            RuntimeEventKind::StateChanged {
                status: AgentStatus::Closed
            }
        )
    })
    .await;
    assert_eq!(prepared.handle().snapshot().status, AgentStatus::Closed);
    assert!(registry.is_empty().await);
    assert!(store.list_sessions().await.unwrap().is_empty());
    assert!(
        storage
            .load(&session_id.to_string())
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        service.prepare_session(PROFILE).await,
        Err(ServiceError::ShuttingDown)
    ));
    assert!(matches!(
        service.activate_session(&prepared).await,
        Err(ServiceError::ShuttingDown)
    ));
}

#[tokio::test]
async fn daemon_auth_protects_http_and_uses_single_use_websocket_tokens() {
    let service = Arc::new(test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        Arc::new(TestFactory::new(ProviderScript::Immediate)),
    ));
    let (address, stop, server) = spawn_server(Arc::clone(&service)).await;

    let (status, error) = http_json_with_auth(address, "GET", "/v1/sessions", None, None).await;
    assert_eq!(status, 401);
    assert_eq!(error["code"], "unauthorized");
    let (status, error) = http_json_with_auth(
        address,
        "GET",
        "/v1/sessions",
        None,
        Some("wrong-auth-key-that-is-still-long-enough"),
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(error["code"], "unauthorized");

    let (status, error) = http_json_with_auth(address, "POST", "/v1/auth/token", None, None).await;
    assert_eq!(status, 401);
    assert_eq!(error["code"], "unauthorized");
    let (status, token_response) = http_json(address, "POST", "/v1/auth/token", None).await;
    assert_eq!(status, 200);
    assert_eq!(token_response["token_type"], "websocket_subprotocol");
    assert_eq!(token_response["protocol"], WS_PROTOCOL);
    assert_eq!(token_response["expires_in_secs"], 60);
    let token = token_response["token"].as_str().unwrap();

    assert_eq!(
        websocket_handshake(address, "/v1/ws/new", None).await.0,
        401
    );
    assert_eq!(
        websocket_handshake(address, &format!("/v1/ws/new?token={token}"), None)
            .await
            .0,
        400,
        "query-string credentials must not be accepted"
    );
    assert_eq!(
        websocket_handshake(
            address,
            "/v1/ws/new",
            Some(&format!("{WS_AUTH_PROTOCOL_PREFIX}{token}")),
        )
        .await
        .0,
        401,
        "the fixed application protocol is required"
    );

    let protocols = format!("{WS_PROTOCOL}, {WS_AUTH_PROTOCOL_PREFIX}{token}");
    let (status, response) = websocket_handshake(address, "/v1/ws/new", Some(&protocols)).await;
    assert_eq!(status, 101);
    assert!(response.lines().any(|line| {
        line.eq_ignore_ascii_case(&format!("Sec-WebSocket-Protocol: {WS_PROTOCOL}"))
    }));
    assert!(!response.contains(token));
    assert_eq!(
        websocket_handshake(address, "/v1/ws/new", Some(&protocols))
            .await
            .0,
        401,
        "a WebSocket token must be consumed atomically"
    );

    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn expired_websocket_token_is_rejected_by_the_upgrade_route() {
    let service = Arc::new(test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        Arc::new(TestFactory::new(ProviderScript::Immediate)),
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(serve(
        listener,
        AppState::with_auth_token_ttl(
            Arc::clone(&service),
            AUTH_KEY,
            std::time::Duration::from_millis(5),
        ),
        async move {
            let _ = stopped.await;
        },
    ));
    let (status, token_response) = http_json(address, "POST", "/v1/auth/token", None).await;
    assert_eq!(status, 200);
    let token = token_response["token"].as_str().unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let protocols = format!("{WS_PROTOCOL}, {WS_AUTH_PROTOCOL_PREFIX}{token}");
    assert_eq!(
        websocket_handshake(address, "/v1/ws/new", Some(&protocols))
            .await
            .0,
        401
    );

    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn new_websocket_creates_no_session_until_its_first_prompt() {
    let compaction_started = Arc::new(Notify::new());
    let release_compaction = Arc::new(Notify::new());
    let service = Arc::new(test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        Arc::new(
            TestFactory::new(ProviderScript::Immediate).with_context_compactor(BlockingCompactor {
                started: Arc::clone(&compaction_started),
                release: Arc::clone(&release_compaction),
            }),
        ),
    ));
    let (address, stop, server) = spawn_server(Arc::clone(&service)).await;
    let mut socket = RawWebSocket::connect(address, "/v1/ws/new").await;

    assert_eq!(socket.receive_json().await["type"], "building");
    assert_eq!(socket.receive_json().await["type"], "ready");
    assert!(service.list_sessions().await.unwrap().is_empty());
    socket
        .send_json(json!({
            "type": "prompt",
            "request_id": "new-prompt",
            "content": { "type": "text", "value": "create me" }
        }))
        .await;

    let mut created_session_id = None;
    let mut accepted_run_id = None;
    let mut completed = false;
    while !completed {
        let message = socket.receive_json().await;
        match message["type"].as_str() {
            Some("session_created") => {
                created_session_id = message["session_id"].as_str().map(str::to_owned);
            }
            Some("command_accepted") if message["request_id"] == "new-prompt" => {
                accepted_run_id = message["run_id"].as_str().map(str::to_owned);
            }
            Some("event") if message["event"]["type"] == "run_completed" => {
                completed = true;
            }
            _ => {}
        }
    }

    let created_session_id = created_session_id.expect("new socket must announce its session ID");
    assert!(accepted_run_id.is_some());
    let sessions = service.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].record.id.to_string(), created_session_id);
    assert_eq!(sessions[0].state.as_ref().unwrap().message_count, 2);

    socket
        .send_json(json!({
            "type": "compact",
            "request_id": "compact-activated-new"
        }))
        .await;
    let accepted = receive_command_response(&mut socket, "compact-activated-new").await;
    assert_eq!(accepted["type"], "command_accepted");
    assert_eq!(accepted["command"], "compact");
    tokio::time::timeout(Duration::from_secs(2), compaction_started.notified())
        .await
        .expect("the activated new connection did not start compaction");
    release_compaction.notify_one();
    let completed = receive_wire_event(&mut socket, "context_compaction_completed").await;
    assert_eq!(completed["event"]["after_message_count"], 1);

    drop(socket);
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn attach_restores_history_and_concurrent_attach_reuses_one_actor() {
    let store = Arc::new(MemoryControlStore::new());
    let storage = Arc::new(InMemorySessionStorage::new());
    let factory = Arc::new(TestFactory::new(ProviderScript::Immediate));
    let first_service = test_service(
        AgentRegistry::new(),
        Arc::clone(&store),
        Arc::clone(&storage),
        Arc::clone(&factory),
    );
    let prepared = first_service.prepare_session(PROFILE).await.unwrap();
    let handle = first_service.activate_session(&prepared).await.unwrap();
    let session_id = handle.session_id();
    let mut events = handle.subscribe();
    let run = handle
        .enqueue_prompt(Content::text("persist me"))
        .await
        .unwrap();
    wait_for_run_completed(&mut events, run.run_id).await;
    assert!(first_service.shutdown().await.is_empty());

    let restarted_registry = AgentRegistry::new();
    let restarted = test_service(
        restarted_registry.clone(),
        Arc::clone(&store),
        Arc::clone(&storage),
        Arc::clone(&factory),
    );
    let (left, right) = tokio::join!(
        restarted.attach_session(session_id),
        restarted.attach_session(session_id)
    );
    let left = left.unwrap();
    let right = right.unwrap();

    assert_eq!(factory.build_count(), 2, "attach must be single-flight");
    assert_eq!(restarted_registry.len().await, 1);
    assert_eq!(left.snapshot(), right.snapshot());
    let snapshot = left.snapshot();
    assert!(snapshot.initialized);
    assert_eq!(snapshot.status, AgentStatus::Idle);
    assert_eq!(snapshot.messages.len(), 2);
    assert_eq!(snapshot.messages[0].role, Role::User);
    assert_eq!(snapshot.messages[0].text_content(), Some("persist me"));
    assert_eq!(snapshot.messages[1].role, Role::Assistant);
    assert_eq!(snapshot.messages[1].text_content(), Some("answer-1"));

    assert!(restarted.shutdown().await.is_empty());
}

#[tokio::test]
async fn two_attached_clients_receive_the_same_ordered_run_events() {
    let service = test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        Arc::new(TestFactory::new(ProviderScript::Immediate)),
    );
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    let mut client_a = handle.subscribe();
    let mut client_b = handle.subscribe();
    let run = handle
        .enqueue_prompt(Content::text("broadcast me"))
        .await
        .unwrap();

    let events_a = collect_run_projection(&mut client_a, run.run_id).await;
    let events_b = collect_run_projection(&mut client_b, run.run_id).await;
    assert_eq!(events_a, events_b);
    assert!(events_a.iter().any(|(_, kind)| *kind == "message_update"));
    assert_eq!(events_a.last().map(|(_, kind)| *kind), Some("completed"));

    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn two_attach_websockets_get_history_and_the_same_live_updates() {
    let service = Arc::new(test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        Arc::new(TestFactory::new(ProviderScript::Immediate)),
    ));
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    let mut runtime_events = handle.subscribe();
    let initial = handle
        .enqueue_prompt(Content::text("existing history"))
        .await
        .unwrap();
    wait_for_run_completed(&mut runtime_events, initial.run_id).await;

    let (address, stop, server) = spawn_server(Arc::clone(&service)).await;
    let path = format!("/v1/ws/attach/{}", handle.session_id());
    let mut client_a = RawWebSocket::connect(address, &path).await;
    let mut client_b = RawWebSocket::connect(address, &path).await;
    let snapshot_a = client_a.receive_json().await;
    let snapshot_b = client_b.receive_json().await;
    assert_eq!(snapshot_a, snapshot_b);
    assert_eq!(snapshot_a["type"], "snapshot");
    assert_eq!(snapshot_a["session"]["status"], "idle");
    assert_eq!(
        snapshot_a["session"]["history"].as_array().unwrap().len(),
        2
    );

    client_a
        .send_json(json!({
            "type": "prompt",
            "request_id": "attached-prompt",
            "content": { "type": "text", "value": "broadcast over ws" }
        }))
        .await;
    let accepted = client_a.receive_json().await;
    assert_eq!(accepted["type"], "command_accepted");
    assert_eq!(accepted["request_id"], "attached-prompt");
    let run_id = accepted["run_id"].as_str().unwrap().to_owned();

    let (events_a, events_b) = tokio::join!(
        collect_wire_run(&mut client_a, &run_id),
        collect_wire_run(&mut client_b, &run_id)
    );
    assert_eq!(events_a, events_b);
    assert_eq!(
        events_a.first().map(|(_, kind)| kind.as_str()),
        Some("run_queued")
    );
    assert_eq!(
        events_a.last().map(|(_, kind)| kind.as_str()),
        Some("run_completed")
    );
    assert!(events_a.iter().any(|(_, kind)| kind == "message_update"));

    drop(client_a);
    drop(client_b);
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn attached_websocket_compaction_broadcasts_status_without_summary_content() {
    let compaction_started = Arc::new(Notify::new());
    let release_compaction = Arc::new(Notify::new());
    let control_store = Arc::new(MemoryControlStore::new());
    let storage = Arc::new(InMemorySessionStorage::new());
    let factory = Arc::new(
        TestFactory::new(ProviderScript::Immediate).with_context_compactor(BlockingCompactor {
            started: Arc::clone(&compaction_started),
            release: Arc::clone(&release_compaction),
        }),
    );
    let service = Arc::new(test_service(
        AgentRegistry::new(),
        Arc::clone(&control_store),
        Arc::clone(&storage),
        Arc::clone(&factory),
    ));
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    let mut runtime_events = handle.subscribe();
    let initial = handle
        .enqueue_prompt(Content::text("history to compact"))
        .await
        .unwrap();
    wait_for_run_completed(&mut runtime_events, initial.run_id).await;
    assert_eq!(handle.snapshot().messages.len(), 2);

    let (address, stop, server) = spawn_server(Arc::clone(&service)).await;
    let path = format!("/v1/ws/attach/{}", handle.session_id());
    let mut client_a = RawWebSocket::connect(address, &path).await;
    let mut client_b = RawWebSocket::connect(address, &path).await;
    assert_eq!(client_a.receive_json().await["type"], "snapshot");
    assert_eq!(client_b.receive_json().await["type"], "snapshot");

    client_a
        .send_json(json!({
            "type": "compact",
            "request_id": "compact-now",
            "instructions": "Preserve deployment decisions"
        }))
        .await;
    let accepted = receive_command_response(&mut client_a, "compact-now").await;
    assert_eq!(accepted["type"], "command_accepted");
    assert_eq!(accepted["command"], "compact");

    tokio::time::timeout(Duration::from_secs(2), compaction_started.notified())
        .await
        .expect("the admitted compaction did not start");
    assert_eq!(handle.status(), AgentStatus::Compacting);
    let (started_a, started_b) = tokio::join!(
        receive_wire_event(&mut client_a, "context_compaction_started"),
        receive_wire_event(&mut client_b, "context_compaction_started")
    );
    assert_eq!(started_a["sequence"], started_b["sequence"]);
    assert_eq!(started_a["run_id"], serde_json::Value::Null);
    assert_eq!(started_a["event"]["trigger"]["type"], "manual");
    assert_eq!(
        started_a["event"]["trigger"]["instructions"],
        "Preserve deployment decisions"
    );
    assert!(started_a["event"].get("prompt").is_none());
    assert!(
        !started_a
            .to_string()
            .contains("test compaction prompt: Preserve deployment decisions")
    );

    client_b
        .send_json(json!({
            "type": "compact",
            "request_id": "compact-again"
        }))
        .await;
    let rejected = receive_command_response(&mut client_b, "compact-again").await;
    assert_eq!(rejected["type"], "command_rejected");
    assert_eq!(rejected["code"], "session_busy");

    let fork_path = format!("/v1/sessions/{}/fork", handle.session_id());
    let (fork_status, fork_error) = http_json(
        address,
        "POST",
        &fork_path,
        Some(json!({ "message_index": 1 })),
    )
    .await;
    assert_eq!(fork_status, 409);
    assert_eq!(fork_error["code"], "session_busy");

    release_compaction.notify_one();
    let (completed_a, completed_b) = tokio::join!(
        receive_wire_event(&mut client_a, "context_compaction_completed"),
        receive_wire_event(&mut client_b, "context_compaction_completed")
    );
    assert_eq!(completed_a["sequence"], completed_b["sequence"]);
    let event = &completed_a["event"];
    assert_eq!(event["before_message_count"], 2);
    assert_eq!(event["after_message_count"], 1);
    assert!(event.get("changed_from").is_none());
    assert!(event.get("replacement").is_none());
    assert!(event.get("summary").is_none());
    assert!(!completed_a.to_string().contains("compacted summary"));
    assert_eq!(event["usage"]["total_tokens"], 8);

    tokio::time::timeout(Duration::from_secs(2), async {
        while handle.status() != AgentStatus::Idle {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the actor did not return to idle after compaction");
    let snapshot = handle.snapshot();
    assert_eq!(snapshot.messages, [Message::user("compacted summary")]);
    assert_eq!(snapshot.display_messages.len(), 2);
    assert_eq!(
        snapshot.display_messages[0].text_content(),
        Some("history to compact")
    );
    assert_eq!(
        snapshot.display_messages[1].text_content(),
        Some("answer-1")
    );
    assert_eq!(snapshot.context_compactions.len(), 1);
    assert_eq!(snapshot.context_compactions[0].history_index, 2);
    assert_eq!(snapshot.last_usage, None);
    assert_eq!(snapshot.context_usage, None);
    assert_eq!(snapshot.cumulative_usage, TokenUsage::new(6, 2, 0));
    let persisted = storage
        .load(&handle.session_id().to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.messages, snapshot.messages);
    assert_eq!(persisted.history.messages, snapshot.display_messages);
    assert_eq!(persisted.history.compactions.len(), 1);
    assert_eq!(persisted.history.compactions[0].history_index, 2);
    assert_eq!(persisted.cumulative_usage, snapshot.cumulative_usage);

    let mut client_c = RawWebSocket::connect(address, &path).await;
    let compacted_snapshot = client_c.receive_json().await;
    assert_eq!(compacted_snapshot["type"], "snapshot");
    assert_eq!(
        compacted_snapshot["session"]["context_compaction"]["phase"],
        "completed"
    );
    assert_eq!(
        compacted_snapshot["session"]["context_compaction"]["history_index"],
        2
    );
    assert_eq!(
        compacted_snapshot["session"]["context_compactions"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        compacted_snapshot["session"]["context_compactions"][0]["history_index"],
        2
    );
    assert_eq!(
        compacted_snapshot["session"]["history"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert!(
        compacted_snapshot
            .to_string()
            .contains("history to compact")
    );
    assert!(compacted_snapshot.to_string().contains("answer-1"));
    assert!(!compacted_snapshot.to_string().contains("compacted summary"));

    let mut new_socket = RawWebSocket::connect(address, "/v1/ws/new").await;
    assert_eq!(new_socket.receive_json().await["type"], "building");
    assert_eq!(new_socket.receive_json().await["type"], "ready");
    new_socket
        .send_json(json!({
            "type": "compact",
            "request_id": "compact-new"
        }))
        .await;
    let rejected = receive_command_response(&mut new_socket, "compact-new").await;
    assert_eq!(rejected["type"], "command_rejected");
    assert_eq!(rejected["code"], "invalid_command");

    drop(new_socket);
    drop(client_a);
    drop(client_b);
    drop(client_c);
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(service.shutdown().await.is_empty());

    let restarted_service = Arc::new(test_service(
        AgentRegistry::new(),
        control_store,
        storage,
        factory,
    ));
    let (restarted_address, restarted_stop, restarted_server) =
        spawn_server(Arc::clone(&restarted_service)).await;
    let mut reattached = RawWebSocket::connect(restarted_address, &path).await;
    let restored_snapshot = reattached.receive_json().await;
    assert_eq!(restored_snapshot["type"], "snapshot");
    assert_eq!(
        restored_snapshot["session"]["history"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        restored_snapshot["session"]["context_compactions"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert!(restored_snapshot.to_string().contains("history to compact"));
    assert!(restored_snapshot.to_string().contains("answer-1"));
    assert!(!restored_snapshot.to_string().contains("compacted summary"));

    drop(reattached);
    restarted_stop.send(()).unwrap();
    restarted_server.await.unwrap().unwrap();
    assert!(restarted_service.shutdown().await.is_empty());
}

#[tokio::test]
async fn context_compaction_is_rejected_while_a_run_is_active() {
    let provider_started = Arc::new(Notify::new());
    let compaction_started = Arc::new(Notify::new());
    let factory = Arc::new(
        TestFactory::new(ProviderScript::HangFirst {
            started: Arc::clone(&provider_started),
        })
        .with_context_compactor(BlockingCompactor {
            started: compaction_started,
            release: Arc::new(Notify::new()),
        }),
    );
    let service = test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        factory,
    );
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    let mut events = handle.subscribe();
    let run = handle
        .enqueue_prompt(Content::text("keep running"))
        .await
        .unwrap();
    provider_started.notified().await;
    assert_eq!(handle.status(), AgentStatus::Running);

    assert!(matches!(
        handle.compact_context(None).await,
        Err(AgentHandleError::Busy {
            status: AgentStatus::Running,
            ..
        })
    ));

    handle.stop(run.run_id).unwrap();
    wait_for_run_stopped(&mut events, run.run_id).await;
    assert!(service.shutdown().await.is_empty());
}

async fn collect_wire_run(socket: &mut RawWebSocket, run_id: &str) -> Vec<(u64, String)> {
    let mut events = Vec::new();
    loop {
        let message = socket.receive_json().await;
        if message["type"] != "event" {
            continue;
        }
        let event = &message["event"];
        let Some(kind) = event["type"].as_str() else {
            continue;
        };
        let belongs_to_run = message["run_id"] == run_id || event["run_id"] == run_id;
        if !belongs_to_run
            || !matches!(
                kind,
                "run_queued" | "run_started" | "message_update" | "run_completed"
            )
        {
            continue;
        }
        events.push((message["sequence"].as_u64().unwrap(), kind.to_owned()));
        if kind == "run_completed" {
            return events;
        }
    }
}

async fn receive_wire_event(socket: &mut RawWebSocket, kind: &str) -> serde_json::Value {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let message = socket.receive_json().await;
            if message["type"] == "event" && message["event"]["type"] == kind {
                return message;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for wire event {kind}"))
}

async fn receive_command_response(
    socket: &mut RawWebSocket,
    request_id: &str,
) -> serde_json::Value {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let message = socket.receive_json().await;
            if matches!(
                message["type"].as_str(),
                Some("command_accepted" | "command_rejected")
            ) && message["request_id"] == request_id
            {
                return message;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for command response {request_id}"))
}

#[tokio::test]
async fn background_tool_notification_wakes_an_idle_agent_without_subagents() {
    let notification_release = Arc::new(Notify::new());
    let observed = Arc::new(AtomicBool::new(false));
    let factory = Arc::new(
        TestFactory::new(ProviderScript::BackgroundNotification {
            observed: Arc::clone(&observed),
        })
        .with_notification_tool(Arc::clone(&notification_release)),
    );
    let service = test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        Arc::clone(&factory),
    );
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    assert!(handle.subagents().is_none());
    let mut events = handle.subscribe();
    let queued = handle
        .enqueue_prompt(Content::text("schedule a background notification"))
        .await
        .unwrap();

    tokio::time::timeout(
        Duration::from_secs(2),
        wait_for_run_completed(&mut events, queued.run_id),
    )
    .await
    .expect("the originating run did not complete");
    assert!(!observed.load(Ordering::SeqCst));
    assert_eq!(factory.provider_calls.load(Ordering::SeqCst), 2);

    // Release the background work only after the actor is idle. The generic
    // Agent mailbox must wake the daemon even though subagents are disabled.
    notification_release.notify_one();
    let mailbox_run_id = tokio::time::timeout(Duration::from_secs(2), async {
        let mut started = None;
        loop {
            let event = events.recv().await.expect("runtime event channel closed");
            match event.kind {
                RuntimeEventKind::RunStarted { run_id } if run_id != queued.run_id => {
                    started = Some(run_id);
                }
                RuntimeEventKind::RunCompleted { run_id } if run_id != queued.run_id => {
                    assert_eq!(started, Some(run_id));
                    break run_id;
                }
                RuntimeEventKind::RunFailed { run_id, message } if run_id != queued.run_id => {
                    panic!("mailbox-driven run failed: {message}");
                }
                _ => {}
            }
        }
    })
    .await
    .expect("the background notification did not wake the idle actor");

    assert_ne!(mailbox_run_id, queued.run_id);
    assert!(observed.load(Ordering::SeqCst));
    assert_eq!(factory.provider_calls.load(Ordering::SeqCst), 3);
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn background_subagent_result_joins_the_active_parent_turn_at_a_safe_boundary() {
    let blocking_started = Arc::new(Notify::new());
    let blocking_release = Arc::new(Notify::new());
    let observed_in_parent_turn = Arc::new(AtomicBool::new(false));
    let factory = Arc::new(
        TestFactory::new(ProviderScript::SubagentSafeBoundary {
            observed_in_parent_turn: Arc::clone(&observed_in_parent_turn),
        })
        .with_tool(BlockingTool {
            started: Arc::clone(&blocking_started),
            release: Arc::clone(&blocking_release),
        }),
    );
    let service = test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        factory,
    )
    .with_subagents_enabled(true);
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    let mut events = handle.subscribe();
    let queued = handle
        .enqueue_prompt(Content::text("delegate in background"))
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(2), blocking_started.notified())
        .await
        .expect("parent did not enter the boundary tool");
    let mut run_starts = 0;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.unwrap();
            match event.kind {
                RuntimeEventKind::RunStarted { .. } => run_starts += 1,
                RuntimeEventKind::Subagent(phi::SubagentEvent {
                    kind:
                        phi::SubagentEventKind::Notification(phi::SubagentNotification {
                            kind: phi::SubagentNotificationKind::Result,
                            wake_parent: true,
                            ..
                        }),
                    ..
                }) => break,
                _ => {}
            }
        }
    })
    .await
    .expect("background child did not publish its terminal notification");
    blocking_release.notify_one();

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.unwrap();
            match event.kind {
                RuntimeEventKind::RunStarted { .. } => run_starts += 1,
                RuntimeEventKind::RunCompleted { run_id } if run_id == queued.run_id => {
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("parent run did not complete after receiving child result");
    assert!(observed_in_parent_turn.load(Ordering::SeqCst));
    assert_eq!(
        run_starts, 1,
        "notification must not start a second parent run"
    );
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn stopped_parent_run_requeues_an_unconsumed_background_result() {
    let blocking_started = Arc::new(Notify::new());
    let blocking_release = Arc::new(Notify::new());
    let observed_after_stop = Arc::new(AtomicBool::new(false));
    let factory = Arc::new(
        TestFactory::new(ProviderScript::SubagentSafeBoundary {
            observed_in_parent_turn: Arc::clone(&observed_after_stop),
        })
        .with_tool(BlockingTool {
            started: Arc::clone(&blocking_started),
            release: Arc::clone(&blocking_release),
        }),
    );
    let service = test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        factory,
    )
    .with_subagents_enabled(true);
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    let mut events = handle.subscribe();
    let queued = handle
        .enqueue_prompt(Content::text("stop after the child finishes"))
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(2), blocking_started.notified())
        .await
        .expect("parent did not enter the boundary tool");
    let mut run_starts = 0;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.unwrap();
            match event.kind {
                RuntimeEventKind::RunStarted { .. } => run_starts += 1,
                RuntimeEventKind::Subagent(phi::SubagentEvent {
                    kind:
                        phi::SubagentEventKind::Notification(phi::SubagentNotification {
                            kind: phi::SubagentNotificationKind::Result,
                            wake_parent: true,
                            ..
                        }),
                    ..
                }) => break,
                _ => {}
            }
        }
    })
    .await
    .expect("background child did not finish before the stop");

    handle.stop(queued.run_id).unwrap();
    blocking_release.notify_one();
    let mut original_stopped = false;
    let mut mailbox_run_completed = false;
    tokio::time::timeout(Duration::from_secs(2), async {
        while !original_stopped || !mailbox_run_completed {
            let event = events.recv().await.unwrap();
            match event.kind {
                RuntimeEventKind::RunStarted { .. } => run_starts += 1,
                RuntimeEventKind::RunStopped { run_id } if run_id == queued.run_id => {
                    original_stopped = true;
                }
                RuntimeEventKind::RunCompleted { run_id } if run_id != queued.run_id => {
                    mailbox_run_completed = true;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("pending child result did not wake a replacement parent run");

    assert!(observed_after_stop.load(Ordering::SeqCst));
    assert_eq!(
        run_starts, 2,
        "exactly one mailbox-driven run must be added"
    );
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn subagent_spawn_is_broadcast_and_child_websocket_is_strictly_read_only() {
    let factory = Arc::new(TestFactory::new(ProviderScript::Subagent));
    let service = Arc::new(
        test_service(
            AgentRegistry::new(),
            Arc::new(MemoryControlStore::new()),
            Arc::new(InMemorySessionStorage::new()),
            Arc::clone(&factory),
        )
        .with_subagents_enabled(true),
    );
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    assert!(handle.subagents().is_some());

    let (address, stop, server) = spawn_server(Arc::clone(&service)).await;
    let parent_path = format!("/v1/ws/attach/{}", handle.session_id());
    let mut parent = RawWebSocket::connect(address, &parent_path).await;
    let initial = parent.receive_json().await;
    assert_eq!(initial["type"], "snapshot");
    assert_eq!(initial["session"]["subagents"], json!([]));

    parent
        .send_json(json!({
            "type": "prompt",
            "request_id": "spawn-child",
            "content": { "type": "text", "value": "delegate this work" }
        }))
        .await;

    let (agent_id, observer_path) = tokio::time::timeout(Duration::from_secs(2), async {
        let mut spawned = None;
        let mut child_notification_seen = false;
        loop {
            let message = parent.receive_json().await;
            if message["type"] != "event" {
                continue;
            }
            match message["event"]["type"].as_str() {
                Some("subagent_spawned") => {
                    assert_eq!(message["event"]["description"], "observer test child");
                    spawned = Some((
                        message["event"]["agent_id"].as_str().unwrap().to_owned(),
                        message["event"]["observer_path"]
                            .as_str()
                            .unwrap()
                            .to_owned(),
                    ));
                }
                Some("subagent_notification")
                    if message["event"]["notification"]["source"] == "child" =>
                {
                    assert_eq!(message["event"]["notification"]["kind"], "blocker");
                    assert_eq!(message["event"]["notification"]["wake_parent"], true);
                    child_notification_seen = true;
                }
                _ => {}
            }
            if child_notification_seen && let Some(spawned) = spawned {
                break spawned;
            }
        }
    })
    .await
    .expect("parent caller did not receive subagent creation and notification events");
    assert_eq!(
        observer_path,
        format!("/v1/ws/attach/{}/subagents/{agent_id}", handle.session_id())
    );

    let mut observer = RawWebSocket::connect(address, &observer_path).await;
    let snapshot = observer.receive_json().await;
    assert_eq!(snapshot["type"], "subagent_snapshot");
    assert_eq!(snapshot["input_allowed"], false);
    assert_eq!(
        snapshot["subagent"]["parent_session_id"],
        handle.session_id().to_string()
    );
    assert_eq!(snapshot["subagent"]["agent_id"], agent_id);

    let runtime = handle.subagents().unwrap().clone();
    let queued = runtime
        .send_message(&agent_id, "follow-up from the parent runtime")
        .unwrap();
    let queued_event = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let message = observer.receive_json().await;
            if message["type"] == "subagent_event" && message["event"]["type"] == "message_queued" {
                break message;
            }
        }
    })
    .await
    .expect("child observer did not receive a live event");
    assert_eq!(queued_event["agent_id"], agent_id);
    assert_eq!(queued_event["event"]["delivery_id"], queued.delivery_id);

    let forbidden = "observer input must never reach the child";
    observer
        .send_json(json!({
            "type": "prompt",
            "request_id": "forbidden",
            "content": { "type": "text", "value": forbidden }
        }))
        .await;
    let (code, reason) = observer.receive_close().await;
    assert_eq!(code, 1008);
    assert_eq!(reason, "read_only_subagent_stream");

    let mut binary_observer = RawWebSocket::connect(address, &observer_path).await;
    assert_eq!(
        binary_observer.receive_json().await["type"],
        "subagent_snapshot"
    );
    binary_observer
        .write_frame(0x2, b"binary input is forbidden")
        .await;
    let (code, reason) = binary_observer.receive_close().await;
    assert_eq!(code, 1008);
    assert_eq!(reason, "read_only_subagent_stream");

    tokio::task::yield_now().await;
    let child = runtime.snapshot(&agent_id).unwrap();
    assert!(child.messages.iter().all(|message| {
        message
            .text_content()
            .is_none_or(|content| !content.contains(forbidden))
    }));

    let first_close = runtime.close(&agent_id, "test complete").await.unwrap();
    assert!(!first_close.already_closed);
    let second_close = runtime.close(&agent_id, "duplicate").await.unwrap();
    assert!(second_close.already_closed);
    assert!(runtime.send_message(&agent_id, "too late").is_err());

    drop(parent);
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn askuser_round_trips_custom_answers_and_survives_websocket_reconnect() {
    let service = Arc::new(test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        Arc::new(TestFactory::new(ProviderScript::AskUser)),
    ));
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    let path = format!("/v1/ws/attach/{}", handle.session_id());
    let (address, stop, server) = spawn_server(Arc::clone(&service)).await;
    let mut client_a = RawWebSocket::connect(address, &path).await;
    let initial = client_a.receive_json().await;
    assert_eq!(initial["type"], "snapshot");
    assert_eq!(initial["session"]["pending_asks"], json!([]));

    client_a
        .send_json(json!({
            "type": "prompt",
            "request_id": "ask-prompt",
            "content": { "type": "text", "value": "Ask me before deciding" }
        }))
        .await;
    let accepted = client_a.receive_json().await;
    assert_eq!(accepted["type"], "command_accepted");
    assert_eq!(accepted["command"], "prompt");

    let requested = receive_wire_event(&mut client_a, "askuser_requested").await;
    let request = &requested["event"]["request"];
    let ask_id = request["ask_id"].as_str().unwrap().to_owned();
    assert_eq!(request["questions"].as_array().unwrap().len(), 2);
    assert_eq!(request["questions"][0]["multiSelect"], false);
    assert_eq!(request["questions"][0]["options"][0]["preview"], "[A] [B]");
    assert_eq!(request["questions"][1]["multiSelect"], true);
    assert_eq!(handle.snapshot().pending_asks.len(), 1);

    // A newly attached client receives enough state to render the unanswered
    // question without replaying old broadcast events.
    let mut client_b = RawWebSocket::connect(address, &path).await;
    let reconnected = client_b.receive_json().await;
    assert_eq!(reconnected["type"], "snapshot");
    assert_eq!(reconnected["session"]["pending_asks"][0], *request);

    client_b
        .send_json(json!({
            "type": "answer_askuser",
            "request_id": "invalid-answer",
            "ask_id": ask_id,
            "answers": [
                {
                    "question_index": 0,
                    "selected_options": ["Compact (Recommended)", "Spacious"]
                },
                {
                    "question_index": 1,
                    "selected_options": ["Tests"]
                }
            ]
        }))
        .await;
    let rejected = client_b.receive_json().await;
    assert_eq!(rejected["type"], "command_rejected");
    assert_eq!(rejected["request_id"], "invalid-answer");
    assert_eq!(rejected["code"], "invalid_askuser_answer");
    assert_eq!(handle.snapshot().pending_asks.len(), 1);

    client_b
        .send_json(json!({
            "type": "answer_askuser",
            "request_id": "custom-answer",
            "ask_id": ask_id,
            "answers": [
                {
                    "question_index": 0,
                    "custom_text": "My custom layout"
                },
                {
                    "question_index": 1,
                    "selected_options": ["Tests", "Docs"],
                    "custom_text": "Accessibility polish"
                }
            ]
        }))
        .await;
    let accepted = client_b.receive_json().await;
    assert_eq!(accepted["type"], "command_accepted");
    assert_eq!(accepted["request_id"], "custom-answer");
    assert_eq!(accepted["command"], "answer_askuser");

    let answered = receive_wire_event(&mut client_b, "askuser_answered").await;
    assert_eq!(answered["event"]["ask_id"], ask_id);
    receive_wire_event(&mut client_b, "run_completed").await;

    let snapshot = handle.snapshot();
    assert!(snapshot.pending_asks.is_empty());
    let tool_result = snapshot
        .messages
        .iter()
        .find(|message| {
            message.role == Role::Tool && message.tool_call_id.as_deref() == Some("ask-call-1")
        })
        .expect("the transcript must contain the askuser result");
    let result: serde_json::Value =
        serde_json::from_str(tool_result.text_content().unwrap()).unwrap();
    assert_eq!(result["answers"][0]["custom_text"], "My custom layout");
    assert_eq!(
        result["answers"][1]["selected_options"],
        json!(["Tests", "Docs"])
    );
    assert_eq!(result["answers"][1]["custom_text"], "Accessibility polish");

    drop(client_a);
    drop(client_b);
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn stopping_a_run_cancels_its_pending_askuser_request() {
    let service = test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        Arc::new(TestFactory::new(ProviderScript::AskUser)),
    );
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    let mut events = handle.subscribe();
    let run = handle
        .enqueue_prompt(Content::text("ask, then stop"))
        .await
        .unwrap();
    let requested = wait_for_event(&mut events, |event| {
        matches!(event.kind, RuntimeEventKind::AskUserRequested { .. })
    })
    .await;
    let ask_id = match requested.kind {
        RuntimeEventKind::AskUserRequested { request } => request.ask_id,
        _ => unreachable!(),
    };
    assert_eq!(handle.snapshot().pending_asks[0].ask_id, ask_id);

    handle.stop(run.run_id).unwrap();
    let error = handle
        .answer_ask_user(ask_id, Vec::new())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AgentHandleError::AskUserNotPending {
            ask_id: current,
            ..
        } if current == ask_id
    ));

    let mut cancellation_count = 0;
    let mut cancellation_sequence = None;
    let stopped = loop {
        let event = events.recv().await.expect("runtime event channel closed");
        match event.kind {
            RuntimeEventKind::AskUserCancelled { ask_id: current } if current == ask_id => {
                cancellation_count += 1;
                cancellation_sequence.get_or_insert(event.sequence);
            }
            RuntimeEventKind::RunStopped { run_id } if run_id == run.run_id => break event,
            _ => {}
        }
    };
    tokio::task::yield_now().await;
    while let Ok(event) = events.try_recv() {
        if matches!(
            event.kind,
            RuntimeEventKind::AskUserCancelled { ask_id: current } if current == ask_id
        ) {
            cancellation_count += 1;
        }
    }
    assert_eq!(cancellation_count, 1);
    assert!(cancellation_sequence.unwrap() < stopped.sequence);
    assert!(handle.snapshot().pending_asks.is_empty());

    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn http_session_list_exposes_the_activated_live_session() {
    let service = Arc::new(test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        Arc::new(TestFactory::new(ProviderScript::Immediate)),
    ));
    let workspace = Workspace::new("/workspace/http-session");
    let prepared = service
        .prepare_session_in_workspace(PROFILE, workspace.clone())
        .await
        .unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    let mut events = handle.subscribe();
    let run = handle
        .enqueue_prompt(Content::text("visible over HTTP"))
        .await
        .unwrap();
    wait_for_run_completed(&mut events, run.run_id).await;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(serve(
        listener,
        AppState::new(Arc::clone(&service), AUTH_KEY),
        async move {
            let _ = stopped.await;
        },
    ));

    let mut connection = TcpStream::connect(address).await.unwrap();
    connection
        .write_all(
            format!(
                "GET /v1/sessions HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {AUTH_KEY}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = String::new();
    connection.read_to_string(&mut response).await.unwrap();
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .expect("HTTP response must contain a header separator");
    assert!(headers.starts_with("HTTP/1.1 200 OK"), "{headers}");
    let payload: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(payload["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(
        payload["sessions"][0]["session_id"],
        handle.session_id().to_string()
    );
    assert_eq!(payload["sessions"][0]["status"], "idle");
    assert_eq!(
        payload["sessions"][0]["workspace"],
        workspace.root().to_string_lossy().as_ref()
    );
    assert_eq!(payload["sessions"][0]["message_count"], 2);
    assert_eq!(payload["workspaces"].as_array().unwrap().len(), 1);
    assert_eq!(
        payload["workspaces"][0]["workspace"],
        workspace.root().to_string_lossy().as_ref()
    );
    assert_eq!(
        payload["workspaces"][0]["sessions"][0]["session_id"],
        handle.session_id().to_string()
    );

    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn http_session_management_orders_pins_and_deletes_live_sessions() {
    let registry = AgentRegistry::new();
    let control = Arc::new(MemoryControlStore::new());
    let storage = Arc::new(InMemorySessionStorage::new());
    let service = Arc::new(test_service(
        registry.clone(),
        Arc::clone(&control),
        storage.clone(),
        Arc::new(TestFactory::new(ProviderScript::Immediate)),
    ));
    let mut handles = Vec::new();
    for index in 0..3 {
        let workspace = if index == 1 {
            Workspace::new("/workspace/group-b")
        } else {
            Workspace::new("/workspace/group-a")
        };
        let prepared = service
            .prepare_session_in_workspace(PROFILE, workspace)
            .await
            .unwrap();
        handles.push(service.activate_session(&prepared).await.unwrap());
    }
    let oldest = handles[0].clone();
    let deleted = handles[1].clone();
    let newest = handles[2].clone();
    let mut events = deleted.subscribe();
    let run = deleted
        .enqueue_prompt(Content::text("persist before deletion"))
        .await
        .unwrap();
    wait_for_run_completed(&mut events, run.run_id).await;
    assert!(
        storage
            .load(&deleted.session_id().to_string())
            .await
            .unwrap()
            .is_some()
    );

    let (address, stop, server) = spawn_server(Arc::clone(&service)).await;
    let oldest_path = format!("/v1/sessions/{}", oldest.session_id());
    let (status, pinned) = http_json(
        address,
        "PATCH",
        &oldest_path,
        Some(json!({ "pinned": true })),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(pinned["pinned"], true);
    assert!(
        control
            .get_session(oldest.session_id())
            .await
            .unwrap()
            .unwrap()
            .pinned
    );

    let (status, sessions) = http_json(address, "GET", "/v1/sessions", None).await;
    assert_eq!(status, 200);
    let flat_sessions = sessions["sessions"].as_array().unwrap();
    assert_eq!(
        flat_sessions[0]["session_id"],
        oldest.session_id().to_string()
    );
    assert_eq!(flat_sessions[0]["pinned"], true);
    assert_eq!(
        flat_sessions[1]["session_id"],
        newest.session_id().to_string()
    );
    assert_eq!(
        flat_sessions[2]["session_id"],
        deleted.session_id().to_string()
    );
    let workspace_groups = sessions["workspaces"].as_array().unwrap();
    assert_eq!(workspace_groups.len(), 2);
    assert_eq!(workspace_groups[0]["workspace"], "/workspace/group-a");
    assert_eq!(
        workspace_groups[0]["sessions"][0]["session_id"],
        oldest.session_id().to_string()
    );
    assert_eq!(
        workspace_groups[0]["sessions"][1]["session_id"],
        newest.session_id().to_string()
    );
    assert_eq!(workspace_groups[1]["workspace"], "/workspace/group-b");
    assert_eq!(
        workspace_groups[1]["sessions"][0]["session_id"],
        deleted.session_id().to_string()
    );

    let (status, error) = http_json(
        address,
        "PATCH",
        &oldest_path,
        Some(json!({ "unexpected": true })),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(error["code"], "invalid_session_update");

    let deleted_path = format!("/v1/sessions/{}", deleted.session_id());
    let (status, body) = http_json(address, "DELETE", &deleted_path, None).await;
    assert_eq!(status, 204);
    assert_eq!(body, serde_json::Value::Null);
    assert_eq!(deleted.status(), AgentStatus::Closed);
    assert!(registry.get(deleted.session_id()).await.is_none());
    assert!(
        control
            .get_session(deleted.session_id())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        storage
            .load(&deleted.session_id().to_string())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(http_json(address, "GET", &deleted_path, None).await.0, 404);

    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn http_forks_a_session_from_a_public_assistant_boundary() {
    let registry = AgentRegistry::new();
    let control = Arc::new(MemoryControlStore::new());
    let storage = Arc::new(InMemorySessionStorage::new());
    let service = Arc::new(test_service(
        registry,
        Arc::clone(&control),
        storage.clone(),
        Arc::new(TestFactory::new(ProviderScript::Immediate)),
    ));
    let workspace = Workspace::new("/workspace/fork-source");
    let prepared = service
        .prepare_session_in_workspace(PROFILE, workspace.clone())
        .await
        .unwrap();
    let source = service.activate_session(&prepared).await.unwrap();
    source.set_title("Fork source".to_owned()).await.unwrap();
    source
        .set_capability_mode(CapabilityMode::WorkspaceEdit)
        .await
        .unwrap();
    let mut events = source.subscribe();
    for prompt in ["first", "second"] {
        let run = source.enqueue_prompt(Content::text(prompt)).await.unwrap();
        wait_for_run_completed(&mut events, run.run_id).await;
    }
    assert_eq!(source.snapshot().messages.len(), 4);

    let (address, stop, server) = spawn_server(Arc::clone(&service)).await;
    let path = format!("/v1/sessions/{}/fork", source.session_id());
    let (status, forked) =
        http_json(address, "POST", &path, Some(json!({ "message_index": 1 }))).await;
    assert_eq!(status, 201);
    assert_eq!(forked["title"], "Fork source");
    assert_eq!(forked["pinned"], false);
    assert_eq!(forked["status"], "offline");
    assert_eq!(forked["workspace"], "/workspace/fork-source");
    let fork_id: SessionId = forked["session_id"].as_str().unwrap().parse().unwrap();
    assert_ne!(fork_id, source.session_id());

    let snapshot = storage.load(&fork_id.to_string()).await.unwrap().unwrap();
    assert_eq!(snapshot.messages, source.snapshot().messages[..2]);
    assert_eq!(snapshot.workspace, Some(workspace));
    assert_eq!(snapshot.capability_mode, CapabilityMode::WorkspaceEdit);
    assert_eq!(snapshot.last_usage, None);
    assert_eq!(snapshot.cumulative_usage, TokenUsage::default());
    let record = control.get_session(fork_id).await.unwrap().unwrap();
    assert_eq!(record.title.as_deref(), Some("Fork source"));
    assert!(!record.pinned);
    assert_eq!(record.profile_id, PROFILE);

    let attached = service.attach_session(fork_id).await.unwrap();
    assert_eq!(attached.snapshot().messages.len(), 2);

    let (status, error) =
        http_json(address, "POST", &path, Some(json!({ "message_index": 0 }))).await;
    assert_eq!(status, 400);
    assert_eq!(error["code"], "invalid_fork_point");

    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn http_forks_before_tool_calls_while_the_source_actor_is_running() {
    let tool_started = Arc::new(Notify::new());
    let tool_release = Arc::new(Notify::new());
    let storage = Arc::new(InMemorySessionStorage::new());
    let factory = TestFactory::new(ProviderScript::ToolCall).with_tool(BlockingTool {
        started: Arc::clone(&tool_started),
        release: Arc::clone(&tool_release),
    });
    let service = Arc::new(test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::clone(&storage),
        Arc::new(factory),
    ));
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let source = service.activate_session(&prepared).await.unwrap();
    let mut events = source.subscribe();
    let (address, stop, server) = spawn_server(Arc::clone(&service)).await;
    let attach_path = format!("/v1/ws/attach/{}", source.session_id());
    let mut live_client = RawWebSocket::connect(address, &attach_path).await;
    assert_eq!(live_client.receive_json().await["type"], "snapshot");

    let run = source
        .enqueue_prompt(Content::text("keep running"))
        .await
        .unwrap();
    tool_started.notified().await;
    receive_wire_event(&mut live_client, "tool_execution_start").await;
    assert_eq!(
        source
            .snapshot()
            .draft
            .as_ref()
            .and_then(|draft| draft.fork_message_index),
        Some(1)
    );

    let mut reconnected_client = RawWebSocket::connect(address, &attach_path).await;
    let live_snapshot = reconnected_client.receive_json().await;
    assert_eq!(live_snapshot["type"], "snapshot");
    assert_eq!(live_snapshot["session"]["draft"]["fork_message_index"], 1);

    let path = format!("/v1/sessions/{}/fork", source.session_id());
    let (status, forked) = http_json(
        address,
        "POST",
        &path,
        Some(json!({
            "message_index": 1,
            "position": "before_tool_calls"
        })),
    )
    .await;
    assert_eq!(status, 201);
    let fork_id: SessionId = forked["session_id"].as_str().unwrap().parse().unwrap();
    let snapshot = storage.load(&fork_id.to_string()).await.unwrap().unwrap();
    assert_eq!(snapshot.messages.len(), 2);
    assert_eq!(snapshot.messages[0].text_content(), Some("keep running"));
    assert_eq!(
        snapshot.messages[1].text_content(),
        Some("I will inspect with a tool.")
    );
    assert!(snapshot.messages[1].tool_calls.is_empty());
    assert_eq!(snapshot.messages[1].provider_state, None);
    assert_eq!(source.status(), AgentStatus::Running);

    tool_release.notify_one();
    wait_for_run_completed(&mut events, run.run_id).await;
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn failed_transcript_deletion_restores_session_metadata_for_retry() {
    let registry = AgentRegistry::new();
    let control = Arc::new(MemoryControlStore::new());
    let storage = Arc::new(DeleteFailingSessionStorage::default());
    let service = ApplicationService::new(
        registry.clone(),
        control.clone(),
        storage.clone(),
        Arc::new(TestFactory::new(ProviderScript::Immediate)),
    );
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    let session_id = handle.session_id();
    let mut events = handle.subscribe();
    let run = handle
        .enqueue_prompt(Content::text("keep this transcript"))
        .await
        .unwrap();
    wait_for_run_completed(&mut events, run.run_id).await;

    assert!(matches!(
        service.delete_session(session_id).await,
        Err(ServiceError::Storage(phi::StorageError::Io { .. }))
    ));
    assert_eq!(handle.status(), AgentStatus::Closed);
    assert!(registry.get(session_id).await.is_none());
    assert!(control.get_session(session_id).await.unwrap().is_some());
    assert!(
        storage
            .load(&session_id.to_string())
            .await
            .unwrap()
            .is_some()
    );

    let restored = service.attach_session(session_id).await.unwrap();
    assert_eq!(restored.snapshot().messages.len(), 2);
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn provider_http_manages_named_profiles_without_echoing_api_keys() {
    let providers = Arc::new(MemoryProviderStore::new());
    let service = Arc::new(ApplicationService::managed(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        providers.clone(),
    ));
    let (address, stop, server) = spawn_server(Arc::clone(&service)).await;

    let (status, empty) = http_json(address, "GET", "/v1/provider", None).await;
    assert_eq!(status, 200);
    assert_eq!(empty["configured"], false);
    assert_eq!(empty["provider"], serde_json::Value::Null);
    let (status, empty_list) = http_json(address, "GET", "/v1/providers", None).await;
    assert_eq!(status, 200);
    assert_eq!(empty_list["providers"], json!([]));

    let secret = "must-never-be-returned";
    let request = json!({
        "provider": "openai_responses",
        "api_key": secret,
        "base_url": "https://example.test/v1",
        "model": "configured-model",
        "max_context_tokens": 128000,
        "reasoning_effort": "medium",
        "max_retries": 3,
        "request_timeout_secs": 12
    });
    let (status, configured) = http_json(address, "PUT", "/v1/provider", Some(request)).await;
    assert_eq!(status, 200);
    assert_eq!(configured["configured"], true);
    assert_eq!(configured["provider"]["profile_id"], PROFILE);
    assert_eq!(configured["provider"]["provider"], "openai_responses");
    assert_eq!(configured["provider"]["model"], "configured-model");
    assert_eq!(configured["provider"]["max_context_tokens"], 128000);
    assert_eq!(configured["provider"]["api_key_configured"], true);
    assert_eq!(configured["provider"]["revision"], 1);
    assert!(!configured.to_string().contains(secret));

    let (status, fetched) = http_json(address, "GET", "/v1/provider", None).await;
    assert_eq!(status, 200);
    assert_eq!(fetched, configured);
    assert!(!fetched.to_string().contains(secret));
    assert_eq!(
        providers.get_provider().await.unwrap().unwrap().api_key,
        secret
    );

    let secondary_secret = "secondary-must-never-be-returned";
    let secondary_request = json!({
        "provider": "anthropic",
        "api_key": secondary_secret,
        "base_url": "https://example.test/v1",
        "model": "secondary-model",
        "max_context_tokens": 200000
    });
    let (status, secondary) = http_json(
        address,
        "PUT",
        "/v1/providers/secondary",
        Some(secondary_request),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(secondary["provider"]["profile_id"], "secondary");
    assert_eq!(secondary["provider"]["model"], "secondary-model");
    assert_eq!(secondary["provider"]["revision"], 1);
    assert!(!secondary.to_string().contains(secondary_secret));

    let (status, listed) = http_json(address, "GET", "/v1/providers", None).await;
    assert_eq!(status, 200);
    assert_eq!(listed["providers"].as_array().unwrap().len(), 2);
    assert_eq!(listed["providers"][0]["profile_id"], PROFILE);
    assert_eq!(listed["providers"][1]["profile_id"], "secondary");
    assert!(!listed.to_string().contains(secret));
    assert!(!listed.to_string().contains(secondary_secret));

    let (status, fetched_secondary) =
        http_json(address, "GET", "/v1/providers/secondary", None).await;
    assert_eq!(status, 200);
    assert_eq!(fetched_secondary, secondary);
    assert_eq!(
        providers
            .get_provider_by_id("secondary")
            .await
            .unwrap()
            .unwrap()
            .api_key,
        secondary_secret
    );

    let mut secondary_socket =
        RawWebSocket::connect(address, "/v1/ws/new?profile_id=secondary").await;
    assert_eq!(secondary_socket.receive_json().await["type"], "building");
    let secondary_ready = secondary_socket.receive_json().await;
    assert_eq!(secondary_ready["type"], "ready");
    assert_eq!(secondary_ready["config"]["model"], "secondary-model");
    drop(secondary_socket);

    let prepared = service.prepare_session(PROFILE).await.unwrap();
    assert_eq!(prepared.handle().snapshot().model, "configured-model");
    service.discard_prepared(&prepared).await;
    let secondary_prepared = service.prepare_session("secondary").await.unwrap();
    assert_eq!(
        secondary_prepared.handle().snapshot().profile_id,
        "secondary"
    );
    assert_eq!(
        secondary_prepared.handle().snapshot().model,
        "secondary-model"
    );
    service.discard_prepared(&secondary_prepared).await;

    let invalid = json!({
        "provider": "openai_chat",
        "api_key": "secret",
        "base_url": "https://example.test/v1",
        "model": "model",
        "max_context_tokens": 128000,
        "max_output_tokens": 0
    });
    let (status, error) = http_json(address, "PUT", "/v1/provider", Some(invalid)).await;
    assert_eq!(status, 400);
    assert_eq!(error["code"], "invalid_provider_config");

    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn agent_profile_http_manages_revisions_and_configures_new_websockets() {
    let providers = Arc::new(MemoryProviderStore::new());
    providers
        .replace_provider(phi_daemon::store::ProviderConfig::new(
            phi_daemon::store::ProviderKind::OpenAiResponses,
            "profile-test-secret",
            "http://127.0.0.1:9/v1",
            "provider-model",
            128_000,
        ))
        .await
        .unwrap();
    let service = Arc::new(ApplicationService::managed(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        providers,
    ));
    let (address, stop, server) = spawn_server(Arc::clone(&service)).await;

    let (status, initial) = http_json(address, "GET", "/v1/agent-profiles", None).await;
    assert_eq!(status, 200);
    assert_eq!(initial["agent_profiles"].as_array().unwrap().len(), 1);
    assert_eq!(
        initial["agent_profiles"][0]["agent_profile_id"],
        phi_daemon::runtime::DEFAULT_AGENT_PROFILE_ID
    );
    assert_eq!(
        initial["agent_profiles"][0]["revision"],
        phi_daemon::runtime::DEFAULT_AGENT_PROFILE_REVISION
    );
    assert_eq!(
        initial["agent_profiles"][0]["initial_capability_mode"],
        "full_access"
    );

    let (status, missing) = http_json(address, "GET", "/v1/agent-profiles/missing", None).await;
    assert_eq!(status, 200);
    assert_eq!(missing["configured"], false);
    assert_eq!(missing["agent_profile"], serde_json::Value::Null);

    let definition = json!({
        "prompt": {
            "mode": "extend",
            "text": "Act as a focused reviewer."
        },
        "tools": {
            "allow": ["read", "edit"],
            "deny": ["bash"]
        },
        "skills": {
            "allow": null,
            "deny": ["dangerous-skill"]
        },
        "initial_capability_mode": "workspace_edit",
        "model": "profile-model",
        "reasoning_effort": "high"
    });
    let (status, created) = http_json(
        address,
        "PUT",
        "/v1/agent-profiles/reviewer",
        Some(definition.clone()),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(created["configured"], true);
    assert_eq!(created["agent_profile"]["agent_profile_id"], "reviewer");
    assert_eq!(created["agent_profile"]["revision"], 1);
    assert_eq!(
        created["agent_profile"]["initial_capability_mode"],
        "workspace_edit"
    );
    assert_eq!(created["agent_profile"]["model"], "profile-model");
    assert_eq!(
        created["agent_profile"]["tools"]["allow"],
        json!(["edit", "read"])
    );

    let (status, updated) = http_json(
        address,
        "PUT",
        "/v1/agent-profiles/reviewer",
        Some(definition),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(updated["agent_profile"]["revision"], 2);

    let (status, fetched) = http_json(address, "GET", "/v1/agent-profiles/reviewer", None).await;
    assert_eq!(status, 200);
    assert_eq!(fetched, updated);
    let (status, listed) = http_json(address, "GET", "/v1/agent-profiles", None).await;
    assert_eq!(status, 200);
    assert_eq!(listed["agent_profiles"].as_array().unwrap().len(), 2);
    assert_eq!(listed["agent_profiles"][0]["agent_profile_id"], "default");
    assert_eq!(listed["agent_profiles"][1]["agent_profile_id"], "reviewer");

    let invalid = json!({
        "prompt": {
            "mode": "full",
            "text": "   "
        }
    });
    let (status, error) =
        http_json(address, "PUT", "/v1/agent-profiles/invalid", Some(invalid)).await;
    assert_eq!(status, 400);
    assert_eq!(error["code"], "invalid_agent_profile");

    let (status, error) = http_json(
        address,
        "PUT",
        "/v1/agent-profiles/unknown-field",
        Some(json!({ "unexpected": true })),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(error["code"], "invalid_agent_profile");

    let mut socket = RawWebSocket::connect(
        address,
        "/v1/ws/new?agent_profile_id=reviewer&capability_mode=read_only",
    )
    .await;
    assert_eq!(socket.receive_json().await["type"], "building");
    let ready = socket.receive_json().await;
    assert_eq!(ready["type"], "ready");
    assert_eq!(ready["config"]["model"], "profile-model");
    assert_eq!(ready["config"]["reasoning_effort"], "high");
    assert_eq!(ready["capability_mode"], "read_only");
    assert_eq!(ready["agent_profile"]["agent_profile_id"], "reviewer");
    assert_eq!(ready["agent_profile"]["revision"], 2);
    drop(socket);

    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn agent_profile_http_reports_management_unavailable_for_custom_services() {
    let service = Arc::new(test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        Arc::new(TestFactory::new(ProviderScript::Immediate)),
    ));
    let (address, stop, server) = spawn_server(Arc::clone(&service)).await;

    let (status, error) = http_json(address, "GET", "/v1/agent-profiles", None).await;
    assert_eq!(status, 501);
    assert_eq!(error["code"], "agent_profile_management_unavailable");

    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn websocket_model_change_is_visible_from_session_http_detail() {
    let control = Arc::new(MemoryControlStore::new());
    let service = Arc::new(test_service(
        AgentRegistry::new(),
        Arc::clone(&control),
        Arc::new(InMemorySessionStorage::new()),
        Arc::new(TestFactory::new(ProviderScript::Immediate)),
    ));
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    let session_id = handle.session_id();
    let (address, stop, server) = spawn_server(Arc::clone(&service)).await;
    let mut socket = RawWebSocket::connect(address, &format!("/v1/ws/attach/{session_id}")).await;
    let snapshot = socket.receive_json().await;
    assert_eq!(snapshot["type"], "snapshot");
    assert_eq!(snapshot["session"]["config"]["model"], MODEL);

    socket
        .send_json(json!({
            "type": "set_model",
            "request_id": "model-over-ws",
            "model": "ws-selected-model"
        }))
        .await;
    let accepted = socket.receive_json().await;
    assert_eq!(accepted["type"], "command_accepted");
    assert_eq!(accepted["request_id"], "model-over-ws");
    let changed = socket.receive_json().await;
    assert_eq!(changed["type"], "event");
    assert_eq!(changed["event"]["type"], "config_changed");
    assert_eq!(changed["event"]["config"]["model"], "ws-selected-model");

    let (status, detail) =
        http_json(address, "GET", &format!("/v1/sessions/{session_id}"), None).await;
    assert_eq!(status, 200);
    assert_eq!(detail["session_id"], session_id.to_string());
    assert_eq!(detail["config"]["model"], "ws-selected-model");
    assert_eq!(detail["config"]["revision"], 1);
    assert_eq!(
        control
            .get_session(session_id)
            .await
            .unwrap()
            .unwrap()
            .model,
        "ws-selected-model"
    );

    drop(socket);
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn websocket_capability_change_is_broadcast_persisted_and_rejected_while_busy() {
    let provider_started = Arc::new(Notify::new());
    let storage = Arc::new(InMemorySessionStorage::new());
    let service = Arc::new(test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::clone(&storage),
        Arc::new(TestFactory::new(ProviderScript::HangFirst {
            started: Arc::clone(&provider_started),
        })),
    ));
    let prepared = service
        .prepare_session_configured(
            PROFILE,
            phi_daemon::runtime::DEFAULT_AGENT_PROFILE_ID,
            Some(CapabilityMode::WorkspaceEdit),
        )
        .await
        .unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    let session_id = handle.session_id();
    let (address, stop, server) = spawn_server(Arc::clone(&service)).await;
    let mut socket = RawWebSocket::connect(address, &format!("/v1/ws/attach/{session_id}")).await;
    let snapshot = socket.receive_json().await;
    assert_eq!(snapshot["type"], "snapshot");
    assert_eq!(snapshot["session"]["capability_mode"], "workspace_edit");

    socket
        .send_json(json!({
            "type": "set_capability_mode",
            "request_id": "capability-idle",
            "capability_mode": "full_access"
        }))
        .await;
    let accepted = receive_command_response(&mut socket, "capability-idle").await;
    assert_eq!(accepted["type"], "command_accepted");
    assert_eq!(accepted["command"], "set_capability_mode");
    let changed = receive_wire_event(&mut socket, "capability_mode_changed").await;
    assert_eq!(changed["event"]["capability_mode"], "full_access");
    assert_eq!(
        handle.snapshot().capability_mode,
        CapabilityMode::FullAccess
    );
    assert_eq!(
        storage
            .load(&session_id.to_string())
            .await
            .unwrap()
            .unwrap()
            .capability_mode,
        CapabilityMode::FullAccess
    );

    socket
        .send_json(json!({
            "type": "prompt",
            "request_id": "capability-running-prompt",
            "content": { "type": "text", "value": "keep running" }
        }))
        .await;
    let prompt = receive_command_response(&mut socket, "capability-running-prompt").await;
    assert_eq!(prompt["type"], "command_accepted");
    provider_started.notified().await;
    assert_eq!(handle.status(), AgentStatus::Running);
    let run_id = handle.snapshot().active_run_id.unwrap();
    assert_eq!(prompt["run_id"], run_id.to_string());

    socket
        .send_json(json!({
            "type": "set_capability_mode",
            "request_id": "capability-busy",
            "capability_mode": "read_only"
        }))
        .await;
    let rejected = receive_command_response(&mut socket, "capability-busy").await;
    assert_eq!(rejected["type"], "command_rejected");
    assert_eq!(rejected["code"], "session_busy");
    assert_eq!(
        handle.snapshot().capability_mode,
        CapabilityMode::FullAccess
    );

    let mut events = handle.subscribe();
    handle.stop(run_id).unwrap();
    wait_for_run_stopped(&mut events, run_id).await;

    drop(socket);
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(service.shutdown().await.is_empty());
}

async fn collect_run_projection(
    events: &mut broadcast::Receiver<RuntimeEvent>,
    run_id: RunId,
) -> Vec<(u64, &'static str)> {
    let mut projected = Vec::new();
    loop {
        let event = events.recv().await.expect("runtime event channel closed");
        let kind = match &event.kind {
            RuntimeEventKind::RunQueued { run_id: current } if *current == run_id => "queued",
            RuntimeEventKind::RunStarted { run_id: current } if *current == run_id => "started",
            RuntimeEventKind::Agent(AgentEvent::MessageUpdate { .. })
                if event.run_id == Some(run_id) =>
            {
                "message_update"
            }
            RuntimeEventKind::RunCompleted { run_id: current } if *current == run_id => {
                projected.push((event.sequence, "completed"));
                break;
            }
            _ => continue,
        };
        projected.push((event.sequence, kind));
    }
    projected
}

#[tokio::test]
async fn prompt_received_while_running_waits_for_the_current_turn() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let factory = Arc::new(TestFactory::new(ProviderScript::BlockFirst {
        started: Arc::clone(&started),
        release: Arc::clone(&release),
    }));
    let service = test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        factory,
    );
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    let mut events = handle.subscribe();

    let first = handle.enqueue_prompt(Content::text("first")).await.unwrap();
    started.notified().await;
    assert_eq!(handle.snapshot().status, AgentStatus::Running);
    let second = handle
        .enqueue_prompt(Content::text("second"))
        .await
        .unwrap();
    assert_eq!(handle.snapshot().queued_runs, 1);
    release.notify_one();

    let mut milestones = Vec::new();
    loop {
        let event = events.recv().await.expect("runtime event channel closed");
        match event.kind {
            RuntimeEventKind::RunStarted { run_id } if run_id == first.run_id => {
                milestones.push("first_started")
            }
            RuntimeEventKind::RunCompleted { run_id } if run_id == first.run_id => {
                milestones.push("first_completed")
            }
            RuntimeEventKind::RunStarted { run_id } if run_id == second.run_id => {
                milestones.push("second_started")
            }
            RuntimeEventKind::RunCompleted { run_id } if run_id == second.run_id => {
                milestones.push("second_completed");
                break;
            }
            _ => {}
        }
    }
    assert_eq!(
        milestones,
        [
            "first_started",
            "first_completed",
            "second_started",
            "second_completed"
        ]
    );
    let snapshot = handle.snapshot();
    assert_eq!(snapshot.status, AgentStatus::Idle);
    assert_eq!(snapshot.queued_runs, 0);
    assert_eq!(snapshot.messages.len(), 4);
    assert_eq!(snapshot.messages[0].text_content(), Some("first"));
    assert_eq!(snapshot.messages[1].text_content(), Some("answer-1"));
    assert_eq!(snapshot.messages[2].text_content(), Some("second"));
    assert_eq!(snapshot.messages[3].text_content(), Some("answer-2"));

    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn shutdown_stops_the_active_run_fails_queued_runs_and_closes_the_actor() {
    let started = Arc::new(Notify::new());
    let service = test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        Arc::new(TestFactory::new(ProviderScript::HangFirst {
            started: Arc::clone(&started),
        })),
    );
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    let mut events = handle.subscribe();
    let active = handle
        .enqueue_prompt(Content::text("active"))
        .await
        .unwrap();
    started.notified().await;
    let queued = handle
        .enqueue_prompt(Content::text("must not run"))
        .await
        .unwrap();
    assert_eq!(handle.snapshot().active_run_id, Some(active.run_id));
    assert_eq!(handle.snapshot().queued_runs, 1);

    assert!(service.shutdown().await.is_empty());

    let mut active_stopped = false;
    let mut queued_failed = false;
    let mut closed = false;
    while !(active_stopped && queued_failed && closed) {
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
            .await
            .expect("shutdown did not publish all terminal events")
            .expect("runtime event channel closed");
        match event.kind {
            RuntimeEventKind::RunStopped { run_id } if run_id == active.run_id => {
                active_stopped = true;
            }
            RuntimeEventKind::RunFailed { run_id, message } if run_id == queued.run_id => {
                assert!(message.contains("closing"), "unexpected failure: {message}");
                queued_failed = true;
            }
            RuntimeEventKind::StateChanged {
                status: AgentStatus::Closed,
            } => closed = true,
            _ => {}
        }
    }
    let snapshot = handle.snapshot();
    assert_eq!(snapshot.status, AgentStatus::Closed);
    assert_eq!(snapshot.active_run_id, None);
    assert_eq!(snapshot.queued_runs, 0);
    assert_eq!(snapshot.messages, [Message::user("active")]);
}

#[tokio::test]
async fn stop_discards_partial_assistant_output_and_broadcasts_the_safe_snapshot() {
    let started = Arc::new(Notify::new());
    let storage = Arc::new(InMemorySessionStorage::new());
    let service = test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::clone(&storage),
        Arc::new(TestFactory::new(ProviderScript::HangFirst {
            started: Arc::clone(&started),
        })),
    );
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    let session_id = handle.session_id();
    let mut client_a = handle.subscribe();
    let mut client_b = handle.subscribe();
    let run = handle
        .enqueue_prompt(Content::text("keep the user message"))
        .await
        .unwrap();

    started.notified().await;
    wait_for_event(&mut client_a, |event| {
        matches!(
            event.kind,
            RuntimeEventKind::Agent(AgentEvent::MessageUpdate { .. })
        )
    })
    .await;
    assert_eq!(
        handle
            .snapshot()
            .draft
            .as_ref()
            .map(|draft| draft.text.as_str()),
        Some("partial-1")
    );

    handle.stop(run.run_id).unwrap();
    wait_for_run_stopped(&mut client_a, run.run_id).await;
    wait_for_run_stopped(&mut client_b, run.run_id).await;

    let snapshot = handle.snapshot();
    assert_eq!(snapshot.status, AgentStatus::Idle);
    assert!(snapshot.draft.is_none());
    assert_eq!(snapshot.messages, [Message::user("keep the user message")]);
    let persisted = storage
        .load(&session_id.to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.messages, snapshot.messages);

    assert!(service.shutdown().await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_stop_wins_a_ready_completion_and_a_late_stop_is_rejected() {
    let completion_entered = Arc::new(Barrier::new(2));
    let completion_release = Arc::new(Barrier::new(2));
    let service = test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        Arc::new(TestFactory::new(ProviderScript::PauseAtAgentEnd {
            entered: Arc::clone(&completion_entered),
            release: Arc::clone(&completion_release),
        })),
    );
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    let mut events = handle.subscribe();
    let run = handle
        .enqueue_prompt(Content::text("race completion with stop"))
        .await
        .unwrap();

    // The core run has committed its final response and selected Completed,
    // but its AgentEnd listener keeps the actor from terminalizing the run.
    completion_entered.wait();
    handle
        .stop(run.run_id)
        .expect("the still-active run must accept stop");
    completion_release.wait();

    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
            .await
            .expect("accepted stop did not publish a terminal event")
            .expect("runtime event channel closed");
        match event.kind {
            RuntimeEventKind::RunCompleted { run_id } if run_id == run.run_id => {
                panic!("an accepted stop must never be followed by RunCompleted")
            }
            RuntimeEventKind::RunStopped { run_id } if run_id == run.run_id => break,
            _ => {}
        }
    }

    assert!(matches!(
        handle.stop(run.run_id),
        Err(AgentHandleError::NoActiveRun { .. })
    ));
    assert_eq!(handle.snapshot().status, AgentStatus::Idle);
    assert_eq!(handle.snapshot().active_run_id, None);
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn stop_after_a_tool_call_persists_a_protocol_complete_checkpoint() {
    let tool_started = Arc::new(Notify::new());
    let tool_release = Arc::new(Notify::new());
    let storage = Arc::new(InMemorySessionStorage::new());
    let factory = TestFactory::new(ProviderScript::ToolCall).with_tool(BlockingTool {
        started: Arc::clone(&tool_started),
        release: Arc::clone(&tool_release),
    });
    let service = test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::clone(&storage),
        Arc::new(factory),
    );
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    let session_id = handle.session_id();
    let mut events = handle.subscribe();
    let run = handle
        .enqueue_prompt(Content::text("call the tool"))
        .await
        .unwrap();

    tool_started.notified().await;
    handle.stop(run.run_id).unwrap();
    tool_release.notify_one();
    wait_for_run_stopped(&mut events, run.run_id).await;

    let persisted = storage
        .load(&session_id.to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.messages, handle.snapshot().messages);
    assert_eq!(persisted.messages.len(), 3);
    assert_eq!(persisted.messages[0].role, Role::User);
    assert_eq!(persisted.messages[1].role, Role::Assistant);
    assert_eq!(persisted.messages[1].tool_calls.len(), 1);
    assert_eq!(persisted.messages[1].tool_calls[0].id, "call-1");
    assert_eq!(persisted.messages[2].role, Role::Tool);
    assert_eq!(
        persisted.messages[2].tool_call_id.as_deref(),
        Some("call-1")
    );
    assert_eq!(persisted.messages[2].text_content(), Some("tool completed"));
    assert!(!persisted.messages[2].tool_result_is_error);

    assert!(service.shutdown().await.is_empty());
}

async fn assert_actor_panic_is_recoverable(
    script: ProviderScript,
    panic_text: &str,
    tool_outcome_is_unknown: bool,
) {
    let registry = AgentRegistry::new();
    let store = Arc::new(MemoryControlStore::new());
    let storage = Arc::new(InMemorySessionStorage::new());
    let factory = Arc::new(TestFactory::new(script));
    let service = test_service(
        registry,
        Arc::clone(&store),
        Arc::clone(&storage),
        Arc::clone(&factory),
    );
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    let session_id = handle.session_id();
    let mut events = handle.subscribe();
    let failed = handle
        .enqueue_prompt(Content::text("persist before panic"))
        .await
        .unwrap();

    let (crash_message, active_failed) = wait_for_actor_crash_and_close(&mut events).await;
    assert!(
        crash_message.contains(panic_text),
        "unexpected crash message: {crash_message}"
    );
    assert!(active_failed, "the active run must receive RunFailed");
    assert_eq!(handle.snapshot().status, AgentStatus::Closed);
    assert_eq!(handle.snapshot().active_run_id, None);
    assert!(matches!(
        events.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
    let persisted = storage
        .load(&session_id.to_string())
        .await
        .unwrap()
        .unwrap();
    if tool_outcome_is_unknown {
        assert_eq!(persisted.messages.len(), 3);
        assert_eq!(persisted.messages[0], Message::user("persist before panic"));
        assert_eq!(persisted.messages[1].role, Role::Assistant);
        assert_eq!(persisted.messages[1].tool_calls[0].id, "call-panic");
        assert_eq!(persisted.messages[2].role, Role::Tool);
        assert!(persisted.messages[2].tool_result_is_error);
        assert!(
            persisted.messages[2]
                .text_content()
                .is_some_and(|text| text.contains("outcome is unknown"))
        );
    } else {
        assert_eq!(persisted.messages, [Message::user("persist before panic")]);
    }

    let rebuilt = service.attach_session(session_id).await.unwrap();
    assert_eq!(factory.build_count(), 2);
    assert_eq!(rebuilt.snapshot().status, AgentStatus::Idle);
    assert_eq!(rebuilt.snapshot().messages, persisted.messages);
    let mut rebuilt_events = rebuilt.subscribe();
    let recovered = rebuilt
        .enqueue_prompt(Content::text("continue from checkpoint"))
        .await
        .unwrap();
    wait_for_run_completed(&mut rebuilt_events, recovered.run_id).await;
    assert_ne!(recovered.run_id, failed.run_id);
    let previous_len = persisted.messages.len();
    assert_eq!(rebuilt.snapshot().messages.len(), previous_len + 2);
    assert_eq!(
        rebuilt.snapshot().messages[..previous_len],
        persisted.messages
    );
    assert_eq!(
        rebuilt.snapshot().messages[previous_len],
        Message::user("continue from checkpoint")
    );
    assert_eq!(
        rebuilt.snapshot().messages[previous_len + 1].text_content(),
        Some("answer-2")
    );

    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn provider_panic_closes_actor_and_attach_rebuilds_from_checkpoint() {
    assert_actor_panic_is_recoverable(ProviderScript::PanicFirst, "injected provider panic", false)
        .await;
}

#[tokio::test]
async fn tool_panic_closes_actor_and_attach_rebuilds_from_checkpoint() {
    assert_actor_panic_is_recoverable(
        ProviderScript::PanickingToolCall,
        "injected tool panic",
        true,
    )
    .await;
}

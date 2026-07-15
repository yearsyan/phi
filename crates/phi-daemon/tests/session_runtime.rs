use std::{
    io,
    net::SocketAddr,
    sync::{
        Arc, Barrier,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use futures_util::{StreamExt, stream};
use phi::{
    Agent, AgentEvent, AgentMode, AssistantDelta, AssistantMessage, Content, InMemoryPlanStore,
    InMemorySessionStorage, LlmProvider, Message, PlanStore, ProviderError, ProviderEvent,
    ProviderEventStream, ProviderRequest, ProviderResponse, Role, SessionSnapshot, SessionStorage,
    SkillCatalog, StorageError, Tool, ToolCall, ToolDefinition, ToolError, ToolOutput,
};
use phi_daemon::{
    api::AppState,
    runtime::{
        AgentBuildRequest, AgentFactory, AgentFactoryError, AgentHandleError, AgentRegistry,
        AgentStatus, BuiltAgent, PlanApprovalDecision, RunId, RuntimeEvent, RuntimeEventKind,
    },
    serve,
    service::{ApplicationService, ServiceError},
    store::{ControlStore, MemoryControlStore, MemoryProviderStore, ProviderStore},
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
    ExitPlanMode,
    ExitPlanModeStale,
    ExitPlanModeTimeoutThenRetry {
        retry_ready: Arc<Notify>,
        release_retry: Arc<Notify>,
    },
    PlanBatchWriteThenExit,
    PlanBatchExitThenWrite,
    ToolCall,
    PanickingToolCall,
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
            ProviderScript::ExitPlanMode
            | ProviderScript::ExitPlanModeStale
            | ProviderScript::ExitPlanModeTimeoutThenRetry { .. }
                if call == 0 =>
            {
                let tool_names = request
                    .tools
                    .iter()
                    .map(|tool| tool.name.as_str())
                    .collect::<Vec<_>>();
                assert!(tool_names.contains(&"read_plan"));
                assert!(tool_names.contains(&"write_plan"));
                assert!(tool_names.contains(&"exit_plan_mode"));
                Box::pin(stream::iter([Ok(ProviderEvent::Done(ProviderResponse {
                    message: AssistantMessage::tool_calls(vec![ToolCall::new(
                        "exit-plan-call-1",
                        "exit_plan_mode",
                        json!({}),
                    )]),
                    usage: None,
                }))]))
            }
            ProviderScript::ExitPlanMode => {
                assert!(
                    request.tools.iter().all(|tool| !matches!(
                        tool.name.as_str(),
                        "read_plan" | "write_plan" | "exit_plan_mode"
                    )),
                    "plan-only tools must disappear immediately after approval"
                );
                let result = request
                    .messages
                    .iter()
                    .find(|message| {
                        message.role == Role::Tool
                            && message.tool_call_id.as_deref() == Some("exit-plan-call-1")
                    })
                    .expect("the exit approval result must be sent back to the provider");
                assert!(!result.tool_result_is_error);
                let decision: serde_json::Value =
                    serde_json::from_str(result.text_content().unwrap()).unwrap();
                assert_eq!(decision["type"], "approve");
                assert_eq!(decision["revision"], 1);
                immediate_stream(call)
            }
            ProviderScript::ExitPlanModeStale => {
                assert!(
                    request
                        .tools
                        .iter()
                        .any(|tool| tool.name == "exit_plan_mode"),
                    "a rejected stale approval must leave the agent in plan mode"
                );
                let result = request
                    .messages
                    .iter()
                    .find(|message| {
                        message.role == Role::Tool
                            && message.tool_call_id.as_deref() == Some("exit-plan-call-1")
                    })
                    .expect("the stale exit result must be sent back to the provider");
                assert!(result.tool_result_is_error);
                assert!(result.text_content().is_some_and(|content| {
                    content.contains("plan changed while approval was pending")
                }));
                immediate_stream(call)
            }
            ProviderScript::ExitPlanModeTimeoutThenRetry {
                retry_ready,
                release_retry,
            } if call == 1 => {
                assert!(
                    request
                        .tools
                        .iter()
                        .any(|tool| tool.name == "exit_plan_mode"),
                    "timing out an exit must leave the agent in plan mode"
                );
                let first = request
                    .messages
                    .iter()
                    .find(|message| {
                        message.role == Role::Tool
                            && message.tool_call_id.as_deref() == Some("exit-plan-call-1")
                    })
                    .expect("the timed-out exit result must be sent back to the provider");
                assert!(first.tool_result_is_error);
                assert!(
                    first
                        .text_content()
                        .is_some_and(|content| content.contains("tool call timed out"))
                );
                let retry_ready = Arc::clone(retry_ready);
                let release_retry = Arc::clone(release_retry);
                Box::pin(stream::once(async move {
                    retry_ready.notify_one();
                    release_retry.notified().await;
                    Ok(ProviderEvent::Done(ProviderResponse {
                        message: AssistantMessage::tool_calls(vec![ToolCall::new(
                            "exit-plan-call-2",
                            "exit_plan_mode",
                            json!({}),
                        )]),
                        usage: None,
                    }))
                }))
            }
            ProviderScript::ExitPlanModeTimeoutThenRetry { .. } => {
                assert!(request.tools.iter().all(|tool| !matches!(
                    tool.name.as_str(),
                    "read_plan" | "write_plan" | "exit_plan_mode"
                )));
                let second = request
                    .messages
                    .iter()
                    .find(|message| {
                        message.role == Role::Tool
                            && message.tool_call_id.as_deref() == Some("exit-plan-call-2")
                    })
                    .expect("the retried exit approval result must reach the provider");
                assert!(!second.tool_result_is_error);
                immediate_stream(call)
            }
            ProviderScript::PlanBatchWriteThenExit if call == 0 => {
                Box::pin(stream::iter([Ok(ProviderEvent::Done(ProviderResponse {
                    message: AssistantMessage::tool_calls(vec![
                        ToolCall::new(
                            "batch-write",
                            "write_plan",
                            json!({
                                "expected_revision": 1,
                                "content": "Updated batch plan"
                            }),
                        ),
                        ToolCall::new("batch-exit", "exit_plan_mode", json!({})),
                    ]),
                    usage: None,
                }))]))
            }
            ProviderScript::PlanBatchExitThenWrite if call == 0 => {
                Box::pin(stream::iter([Ok(ProviderEvent::Done(ProviderResponse {
                    message: AssistantMessage::tool_calls(vec![
                        ToolCall::new("batch-exit", "exit_plan_mode", json!({})),
                        ToolCall::new(
                            "batch-write",
                            "write_plan",
                            json!({
                                "expected_revision": 1,
                                "content": "This write must be rejected"
                            }),
                        ),
                    ]),
                    usage: None,
                }))]))
            }
            ProviderScript::PlanBatchWriteThenExit => {
                assert!(request.tools.iter().all(|tool| !matches!(
                    tool.name.as_str(),
                    "read_plan" | "write_plan" | "exit_plan_mode"
                )));
                let write = request
                    .messages
                    .iter()
                    .find(|message| message.tool_call_id.as_deref() == Some("batch-write"))
                    .expect("write_plan must return before the approved exit");
                assert!(!write.tool_result_is_error);
                let artifact: serde_json::Value =
                    serde_json::from_str(write.text_content().unwrap()).unwrap();
                assert_eq!(artifact["revision"], 2);
                assert_eq!(artifact["content"], "Updated batch plan");
                let exit = request
                    .messages
                    .iter()
                    .find(|message| message.tool_call_id.as_deref() == Some("batch-exit"))
                    .expect("exit_plan_mode must return its approval result");
                assert!(!exit.tool_result_is_error);
                immediate_stream(call)
            }
            ProviderScript::PlanBatchExitThenWrite => {
                assert!(request.tools.iter().all(|tool| !matches!(
                    tool.name.as_str(),
                    "read_plan" | "write_plan" | "exit_plan_mode"
                )));
                let exit = request
                    .messages
                    .iter()
                    .find(|message| message.tool_call_id.as_deref() == Some("batch-exit"))
                    .expect("exit_plan_mode must return its approval result");
                assert!(!exit.tool_result_is_error);
                let write = request
                    .messages
                    .iter()
                    .find(|message| message.tool_call_id.as_deref() == Some("batch-write"))
                    .expect("the post-exit write must receive a result");
                assert!(
                    write.tool_result_is_error,
                    "write_plan after an approved exit must be rejected"
                );
                immediate_stream(call)
            }
            ProviderScript::ToolCall if call == 0 => {
                Box::pin(stream::iter([Ok(ProviderEvent::Done(ProviderResponse {
                    message: AssistantMessage::tool_calls(vec![ToolCall::new(
                        "call-1",
                        "blocking_tool",
                        json!({}),
                    )]),
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

#[derive(Clone, Default)]
struct FailNextSaveStorage {
    inner: InMemorySessionStorage,
    fail_next_save: Arc<AtomicBool>,
}

impl FailNextSaveStorage {
    fn fail_next_save(&self) {
        self.fail_next_save.store(true, Ordering::SeqCst);
    }

    fn maybe_fail(&self) -> Result<(), StorageError> {
        if self.fail_next_save.swap(false, Ordering::SeqCst) {
            return Err(StorageError::Io {
                path: "injected-session-save".into(),
                source: io::Error::other("injected session save failure"),
            });
        }
        Ok(())
    }
}

#[async_trait]
impl SessionStorage for FailNextSaveStorage {
    async fn load(&self, session_id: &str) -> Result<Option<SessionSnapshot>, StorageError> {
        self.inner.load(session_id).await
    }

    async fn save(&self, session: &SessionSnapshot) -> Result<(), StorageError> {
        self.maybe_fail()?;
        self.inner.save(session).await
    }

    async fn save_incremental(
        &self,
        session: &SessionSnapshot,
        previous_message_count: usize,
    ) -> Result<(), StorageError> {
        self.maybe_fail()?;
        self.inner
            .save_incremental(session, previous_message_count)
            .await
    }

    async fn save_replacing_from(
        &self,
        session: &SessionSnapshot,
        unchanged_message_count: usize,
    ) -> Result<(), StorageError> {
        self.maybe_fail()?;
        self.inner
            .save_replacing_from(session, unchanged_message_count)
            .await
    }

    async fn delete(&self, session_id: &str) -> Result<(), StorageError> {
        self.inner.delete(session_id).await
    }
}

#[derive(Clone)]
struct TestFactory {
    script: ProviderScript,
    provider_calls: Arc<AtomicUsize>,
    builds: Arc<AtomicUsize>,
    tool: Option<BlockingTool>,
}

impl TestFactory {
    fn new(script: ProviderScript) -> Self {
        Self {
            script,
            provider_calls: Arc::new(AtomicUsize::new(0)),
            builds: Arc::new(AtomicUsize::new(0)),
            tool: None,
        }
    }

    fn with_tool(mut self, tool: BlockingTool) -> Self {
        self.tool = Some(tool);
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
        let model = request.model.clone().unwrap_or_else(|| MODEL.to_owned());
        let mut builder = Agent::builder(ScriptedProvider {
            script: self.script.clone(),
            calls: Arc::clone(&self.provider_calls),
        })
        .model(model.clone());
        if let Some(tool) = self.tool.clone() {
            builder = builder.tool(tool);
        }
        if matches!(&self.script, ProviderScript::PanickingToolCall) {
            builder = builder.tool(PanickingTool);
        }
        if matches!(
            &self.script,
            ProviderScript::ExitPlanModeTimeoutThenRetry { .. }
        ) {
            builder = builder.tool_call_timeout(Duration::from_millis(500));
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
            model,
            reasoning_effort: request.reasoning_effort,
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
    let payload = serde_json::from_slice(&response[separator + 4..]).unwrap();
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
    let service = Arc::new(test_service(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        Arc::new(TestFactory::new(ProviderScript::Immediate)),
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
async fn plan_exit_requires_exact_revision_approval_and_survives_websocket_reconnect() {
    let plan_store = Arc::new(InMemoryPlanStore::new());
    let service = Arc::new(ApplicationService::new_with_plan_store(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        plan_store.clone(),
        Arc::new(TestFactory::new(ProviderScript::ExitPlanMode)),
    ));
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    let path = format!("/v1/ws/attach/{}", handle.session_id());
    let (address, stop, server) = spawn_server(Arc::clone(&service)).await;
    let mut client_a = RawWebSocket::connect(address, &path).await;
    let initial = client_a.receive_json().await;
    assert_eq!(initial["type"], "snapshot");
    assert_eq!(initial["session"]["mode"], "default");
    assert_eq!(initial["session"]["pending_plan_approvals"], json!([]));

    client_a
        .send_json(json!({
            "type": "set_mode",
            "request_id": "enter-plan",
            "mode": "plan"
        }))
        .await;
    let entered = client_a.receive_json().await;
    assert_eq!(entered["type"], "command_accepted");
    assert_eq!(entered["command"], "set_mode");
    let mode_changed = receive_wire_event(&mut client_a, "mode_changed").await;
    assert_eq!(mode_changed["event"]["mode"], "plan");
    assert_eq!(handle.snapshot().mode, AgentMode::Plan);

    let plan = plan_store
        .update(
            &handle.session_id().to_string(),
            0,
            "# Plan\n\n1. Make the change.\n2. Run tests.".to_owned(),
        )
        .await
        .unwrap();
    assert_eq!(plan.revision, 1);

    client_a
        .send_json(json!({
            "type": "prompt",
            "request_id": "plan-prompt",
            "content": { "type": "text", "value": "Finish the plan" }
        }))
        .await;
    let accepted = client_a.receive_json().await;
    assert_eq!(accepted["type"], "command_accepted");
    assert_eq!(accepted["command"], "prompt");

    let requested = receive_wire_event(&mut client_a, "plan_approval_requested").await;
    let request = &requested["event"]["request"];
    let approval_id = request["approval_id"].as_str().unwrap().to_owned();
    assert_eq!(request["plan"]["revision"], 1);
    assert_eq!(request["plan"]["content"], plan.content);
    assert_eq!(handle.snapshot().pending_plan_approvals.len(), 1);

    // Reconnect receives the exact immutable revision without relying on old
    // broadcast events.
    let mut client_b = RawWebSocket::connect(address, &path).await;
    let reconnected = client_b.receive_json().await;
    assert_eq!(reconnected["type"], "snapshot");
    assert_eq!(reconnected["session"]["mode"], "plan");
    assert_eq!(
        reconnected["session"]["pending_plan_approvals"][0],
        *request
    );

    client_b
        .send_json(json!({
            "type": "decide_plan_approval",
            "request_id": "wrong-revision",
            "approval_id": approval_id,
            "decision": { "type": "approve", "revision": 2 }
        }))
        .await;
    let rejected = client_b.receive_json().await;
    assert_eq!(rejected["type"], "command_rejected");
    assert_eq!(rejected["code"], "invalid_plan_approval_decision");
    assert_eq!(handle.snapshot().mode, AgentMode::Plan);
    assert_eq!(handle.snapshot().pending_plan_approvals.len(), 1);

    client_b
        .send_json(json!({
            "type": "decide_plan_approval",
            "request_id": "approve-plan",
            "approval_id": approval_id,
            "decision": { "type": "approve", "revision": 1 }
        }))
        .await;
    let accepted = client_b.receive_json().await;
    assert_eq!(accepted["type"], "command_accepted");
    assert_eq!(accepted["command"], "decide_plan_approval");
    let decided = receive_wire_event(&mut client_b, "plan_approval_decided").await;
    assert_eq!(decided["event"]["decision"]["type"], "approve");
    let mode_changed = receive_wire_event(&mut client_b, "mode_changed").await;
    assert_eq!(mode_changed["event"]["mode"], "default");
    receive_wire_event(&mut client_b, "run_completed").await;

    let snapshot = handle.snapshot();
    assert_eq!(snapshot.mode, AgentMode::Default);
    assert!(snapshot.pending_plan_approvals.is_empty());
    let tool_result = snapshot
        .messages
        .iter()
        .find(|message| {
            message.role == Role::Tool
                && message.tool_call_id.as_deref() == Some("exit-plan-call-1")
        })
        .expect("the transcript must contain the plan approval result");
    assert!(!tool_result.tool_result_is_error);

    drop(client_a);
    drop(client_b);
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn timed_out_plan_exit_is_cancelled_and_a_retry_can_be_approved() {
    let retry_ready = Arc::new(Notify::new());
    let release_retry = Arc::new(Notify::new());
    let plan_store = Arc::new(InMemoryPlanStore::new());
    let service = ApplicationService::new_with_plan_store(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        plan_store.clone(),
        Arc::new(TestFactory::new(
            ProviderScript::ExitPlanModeTimeoutThenRetry {
                retry_ready: Arc::clone(&retry_ready),
                release_retry: Arc::clone(&release_retry),
            },
        )),
    );
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    handle.set_mode(AgentMode::Plan).await.unwrap();
    let plan = plan_store
        .update(
            &handle.session_id().to_string(),
            0,
            "Plan that will be retried".to_owned(),
        )
        .await
        .unwrap();
    let mut events = handle.subscribe();
    let run = handle
        .enqueue_prompt(Content::text("submit, time out, then retry"))
        .await
        .unwrap();

    let first_requested = wait_for_event(&mut events, |event| {
        matches!(event.kind, RuntimeEventKind::PlanApprovalRequested { .. })
    })
    .await;
    let first_approval_id = match first_requested.kind {
        RuntimeEventKind::PlanApprovalRequested { request } => request.approval_id,
        _ => unreachable!(),
    };
    wait_for_event(&mut events, |event| {
        matches!(
            event.kind,
            RuntimeEventKind::PlanApprovalCancelled { approval_id }
                if approval_id == first_approval_id
        )
    })
    .await;
    let mut first_cancellation_count = 1;
    tokio::time::timeout(Duration::from_secs(2), retry_ready.notified())
        .await
        .expect("provider did not receive the timed-out tool result");

    let snapshot = handle.snapshot();
    assert_eq!(snapshot.mode, AgentMode::Plan);
    assert!(snapshot.pending_plan_approvals.is_empty());
    let error = handle
        .decide_plan_approval(
            first_approval_id,
            PlanApprovalDecision::Approve {
                revision: plan.revision,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AgentHandleError::PlanApprovalNotPending { approval_id, .. }
            if approval_id == first_approval_id
    ));

    release_retry.notify_one();
    let second_request = loop {
        let event = events.recv().await.expect("runtime event channel closed");
        match event.kind {
            RuntimeEventKind::PlanApprovalCancelled { approval_id }
                if approval_id == first_approval_id =>
            {
                first_cancellation_count += 1;
            }
            RuntimeEventKind::PlanApprovalRequested { request } => break request,
            RuntimeEventKind::RunFailed { message, .. } => {
                panic!("run failed before the retry approval: {message}");
            }
            _ => {}
        }
    };
    assert_ne!(second_request.approval_id, first_approval_id);
    assert_eq!(second_request.plan, plan);
    let snapshot = handle.snapshot();
    assert_eq!(snapshot.pending_plan_approvals.len(), 1);
    assert_eq!(
        snapshot.pending_plan_approvals[0].approval_id,
        second_request.approval_id
    );

    handle
        .decide_plan_approval(
            second_request.approval_id,
            PlanApprovalDecision::Approve {
                revision: plan.revision,
            },
        )
        .await
        .unwrap();
    loop {
        let event = events.recv().await.expect("runtime event channel closed");
        match event.kind {
            RuntimeEventKind::PlanApprovalCancelled { approval_id }
                if approval_id == first_approval_id =>
            {
                first_cancellation_count += 1;
            }
            RuntimeEventKind::RunCompleted { run_id } if run_id == run.run_id => break,
            RuntimeEventKind::RunFailed { message, .. } => {
                panic!("run failed after the retry approval: {message}");
            }
            _ => {}
        }
    }
    assert_eq!(first_cancellation_count, 1);
    let snapshot = handle.snapshot();
    assert_eq!(snapshot.mode, AgentMode::Default);
    assert!(snapshot.pending_plan_approvals.is_empty());
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn plan_exit_refuses_an_approval_after_the_persisted_plan_changes() {
    let plan_store = Arc::new(InMemoryPlanStore::new());
    let service = ApplicationService::new_with_plan_store(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        plan_store.clone(),
        Arc::new(TestFactory::new(ProviderScript::ExitPlanModeStale)),
    );
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    handle.set_mode(AgentMode::Plan).await.unwrap();
    let first = plan_store
        .update(&handle.session_id().to_string(), 0, "First plan".to_owned())
        .await
        .unwrap();
    let mut events = handle.subscribe();
    let run = handle
        .enqueue_prompt(Content::text("submit the plan"))
        .await
        .unwrap();
    let requested = wait_for_event(&mut events, |event| {
        matches!(event.kind, RuntimeEventKind::PlanApprovalRequested { .. })
    })
    .await;
    let approval_id = match requested.kind {
        RuntimeEventKind::PlanApprovalRequested { request } => request.approval_id,
        _ => unreachable!(),
    };

    let second = plan_store
        .update(
            &handle.session_id().to_string(),
            first.revision,
            "A newer plan".to_owned(),
        )
        .await
        .unwrap();
    let error = handle
        .decide_plan_approval(
            approval_id,
            PlanApprovalDecision::Approve {
                revision: first.revision,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AgentHandleError::StalePlanApproval {
            expected_revision,
            current_revision,
            ..
        } if expected_revision == first.revision && current_revision == second.revision
    ));
    wait_for_event(&mut events, |event| {
        matches!(
            event.kind,
            RuntimeEventKind::PlanApprovalCancelled { approval_id: current }
                if current == approval_id
        )
    })
    .await;
    wait_for_run_completed(&mut events, run.run_id).await;

    let snapshot = handle.snapshot();
    assert_eq!(snapshot.mode, AgentMode::Plan);
    assert!(snapshot.pending_plan_approvals.is_empty());
    assert!(service.shutdown().await.is_empty());
}

async fn assert_plan_batch_order(
    script: ProviderScript,
    expected_approval_revision: u64,
    expected_approval_content: &str,
) {
    let plan_store = Arc::new(InMemoryPlanStore::new());
    let service = ApplicationService::new_with_plan_store(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        plan_store.clone(),
        Arc::new(TestFactory::new(script)),
    );
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    handle.set_mode(AgentMode::Plan).await.unwrap();
    plan_store
        .update(
            &handle.session_id().to_string(),
            0,
            "Initial batch plan".to_owned(),
        )
        .await
        .unwrap();
    let mut events = handle.subscribe();
    let run = handle
        .enqueue_prompt(Content::text("submit a two-tool plan batch"))
        .await
        .unwrap();
    let requested = wait_for_event(&mut events, |event| {
        matches!(event.kind, RuntimeEventKind::PlanApprovalRequested { .. })
    })
    .await;
    let request = match requested.kind {
        RuntimeEventKind::PlanApprovalRequested { request } => request,
        _ => unreachable!(),
    };
    assert_eq!(request.plan.revision, expected_approval_revision);
    assert_eq!(request.plan.content, expected_approval_content);

    handle
        .decide_plan_approval(
            request.approval_id,
            PlanApprovalDecision::Approve {
                revision: expected_approval_revision,
            },
        )
        .await
        .unwrap();
    wait_for_run_completed(&mut events, run.run_id).await;

    let current = plan_store
        .current(&handle.session_id().to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.revision, expected_approval_revision);
    assert_eq!(current.content, expected_approval_content);
    assert_eq!(handle.snapshot().mode, AgentMode::Default);
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn plan_batch_write_then_exit_approves_the_written_revision() {
    assert_plan_batch_order(
        ProviderScript::PlanBatchWriteThenExit,
        2,
        "Updated batch plan",
    )
    .await;
}

#[tokio::test]
async fn plan_batch_exit_then_write_rejects_the_post_approval_write() {
    assert_plan_batch_order(
        ProviderScript::PlanBatchExitThenWrite,
        1,
        "Initial batch plan",
    )
    .await;
}

#[tokio::test]
async fn failed_tool_result_save_restores_plan_mode_in_the_runtime_projection() {
    let plan_store = Arc::new(InMemoryPlanStore::new());
    let storage = Arc::new(FailNextSaveStorage::default());
    let service = ApplicationService::new_with_plan_store(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        storage.clone(),
        plan_store.clone(),
        Arc::new(TestFactory::new(ProviderScript::ExitPlanMode)),
    );
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    handle.set_mode(AgentMode::Plan).await.unwrap();
    plan_store
        .update(&handle.session_id().to_string(), 0, "Plan".to_owned())
        .await
        .unwrap();
    let mut events = handle.subscribe();
    let run = handle
        .enqueue_prompt(Content::text("submit the plan"))
        .await
        .unwrap();
    let requested = wait_for_event(&mut events, |event| {
        matches!(event.kind, RuntimeEventKind::PlanApprovalRequested { .. })
    })
    .await;
    let (approval_id, revision) = match requested.kind {
        RuntimeEventKind::PlanApprovalRequested { request } => {
            (request.approval_id, request.plan.revision)
        }
        _ => unreachable!(),
    };

    // The unknown-result journal is durable by the time approval is exposed;
    // fail the next save, which is the completed exit tool result carrying the
    // optimistic Default transition.
    storage.fail_next_save();
    handle
        .decide_plan_approval(approval_id, PlanApprovalDecision::Approve { revision })
        .await
        .unwrap();

    let mut mode_changes = Vec::new();
    loop {
        let event = events.recv().await.expect("runtime event channel closed");
        match event.kind {
            RuntimeEventKind::ModeChanged { mode } => mode_changes.push(mode),
            RuntimeEventKind::RunFailed { run_id, message } if run_id == run.run_id => {
                assert!(message.contains("injected session save failure"));
                break;
            }
            _ => {}
        }
    }
    assert_eq!(mode_changes, [AgentMode::Default, AgentMode::Plan]);
    assert_eq!(handle.snapshot().mode, AgentMode::Plan);
    let persisted = storage
        .load(&handle.session_id().to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.mode, AgentMode::Plan);
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn failed_idle_plan_mode_save_still_projects_the_fail_safe_mode() {
    let storage = Arc::new(FailNextSaveStorage::default());
    let service = ApplicationService::new(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        storage.clone(),
        Arc::new(TestFactory::new(ProviderScript::Immediate)),
    );
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    let mut events = handle.subscribe();
    storage.fail_next_save();

    let error = handle.set_mode(AgentMode::Plan).await.unwrap_err();
    assert!(matches!(error, AgentHandleError::Operation { .. }));
    let mode_changed = wait_for_event(&mut events, |event| {
        matches!(
            event.kind,
            RuntimeEventKind::ModeChanged {
                mode: AgentMode::Plan
            }
        )
    })
    .await;
    let operation_failed = wait_for_event(&mut events, |event| {
        matches!(
            &event.kind,
            RuntimeEventKind::OperationFailed { operation, .. } if operation == "set_mode"
        )
    })
    .await;
    assert!(mode_changed.sequence < operation_failed.sequence);
    assert_eq!(handle.snapshot().mode, AgentMode::Plan);
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn stopping_a_run_cancels_its_pending_plan_approval() {
    let plan_store = Arc::new(InMemoryPlanStore::new());
    let service = ApplicationService::new_with_plan_store(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        plan_store.clone(),
        Arc::new(TestFactory::new(ProviderScript::ExitPlanMode)),
    );
    let prepared = service.prepare_session(PROFILE).await.unwrap();
    let handle = service.activate_session(&prepared).await.unwrap();
    handle.set_mode(AgentMode::Plan).await.unwrap();
    plan_store
        .update(&handle.session_id().to_string(), 0, "Plan".to_owned())
        .await
        .unwrap();
    let mut events = handle.subscribe();
    let run = handle
        .enqueue_prompt(Content::text("submit then stop"))
        .await
        .unwrap();
    let requested = wait_for_event(&mut events, |event| {
        matches!(event.kind, RuntimeEventKind::PlanApprovalRequested { .. })
    })
    .await;
    let (approval_id, revision) = match requested.kind {
        RuntimeEventKind::PlanApprovalRequested { request } => {
            (request.approval_id, request.plan.revision)
        }
        _ => unreachable!(),
    };

    handle.stop(run.run_id).unwrap();
    let error = handle
        .decide_plan_approval(approval_id, PlanApprovalDecision::Approve { revision })
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AgentHandleError::PlanApprovalNotPending {
            approval_id: current,
            ..
        } if current == approval_id
    ));

    let mut cancellation_count = 0;
    let mut cancellation_sequence = None;
    let stopped = loop {
        let event = events.recv().await.expect("runtime event channel closed");
        match event.kind {
            RuntimeEventKind::PlanApprovalCancelled {
                approval_id: current,
            } if current == approval_id => {
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
            RuntimeEventKind::PlanApprovalCancelled {
                approval_id: current
            } if current == approval_id
        ) {
            cancellation_count += 1;
        }
    }
    assert_eq!(cancellation_count, 1);
    assert!(cancellation_sequence.unwrap() < stopped.sequence);
    let snapshot = handle.snapshot();
    assert_eq!(snapshot.mode, AgentMode::Plan);
    assert!(snapshot.pending_plan_approvals.is_empty());
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
    let prepared = service.prepare_session(PROFILE).await.unwrap();
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
    assert_eq!(payload["sessions"][0]["message_count"], 2);

    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
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

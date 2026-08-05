use std::{
    io,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use chrono::Utc;
use futures_util::stream;
use phi::{
    Agent, AssistantMessage, CapabilityMode, InMemorySessionStorage, LlmProvider, ProviderEvent,
    ProviderEventStream, ProviderRequest, ProviderResponse, SkillCatalog, TokenUsage, Tool,
    ToolCall, ToolDefinition, ToolEffect, ToolError, ToolOutput, Workspace,
};
use phi_daemon::{
    api::AppState,
    output_channel::{
        BotAccountDefinition, OutputChannelDefinition, OutputChannelDeliveryError,
        OutputChannelManager, OutputChannelSender,
    },
    runtime::{
        AgentBuildRequest, AgentFactory, AgentFactoryError, AgentProfileDefinition, AgentRegistry,
        BuiltAgent, compile_agent_profile, default_agent_profile,
    },
    scheduled_task::{
        ScheduledIntervalUnit, ScheduledRunOutcome, ScheduledTask, ScheduledTaskError,
        ScheduledTaskId, ScheduledTaskManager, ScheduledTaskRun, ScheduledTaskSchedule,
    },
    serve,
    service::ApplicationService,
    store::{
        MemoryControlStore, MemoryOutputChannelStore, MemoryProviderStore,
        MemoryScheduledTaskStore, OutputChannelStore, ProviderConfig, ProviderKind, ProviderStore,
        ScheduledTaskStore,
    },
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::{
    net::TcpListener,
    sync::{Mutex, Notify, oneshot},
    task::JoinHandle,
};

const AUTH_KEY: &str = "a-secure-test-key-with-at-least-32-bytes";

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!("{prefix}-{}", ScheduledTaskId::new()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn authenticated_http_crud_preserves_schedule_shape_and_revision() {
    let workspace = TemporaryDirectory::new("phi-scheduled-http");
    let providers = Arc::new(MemoryProviderStore::new());
    providers
        .replace_provider(ProviderConfig::new(
            ProviderKind::OpenAiResponses,
            "test-secret",
            "http://127.0.0.1:9/v1",
            "test-model",
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
    service
        .configure_agent_profile("reviewer", AgentProfileDefinition::default())
        .await
        .unwrap();
    let channel_store = Arc::new(MemoryOutputChannelStore::new());
    channel_store
        .replace_bot_account(
            "primary",
            BotAccountDefinition::Telegram {
                bot_token: "123456789:abcdefghijklmnopqrstuvwxyz_ABCDEFG".to_owned(),
            },
        )
        .await
        .unwrap();
    channel_store
        .replace_output_channel(
            "alerts",
            OutputChannelDefinition::Telegram {
                bot_account_id: "primary".to_owned(),
                chat_id: "-1001234567890".to_owned(),
            },
        )
        .await
        .unwrap();
    let output_channels = Arc::new(OutputChannelManager::new(
        channel_store,
        Arc::new(RecordingOutputChannelSender::default()),
    ));
    let manager = Arc::new(
        ScheduledTaskManager::new(
            Arc::clone(&service),
            Arc::new(MemoryScheduledTaskStore::new()),
        )
        .with_output_channels(Arc::clone(&output_channels)),
    );
    let state = AppState::new(Arc::clone(&service), AUTH_KEY)
        .with_default_workspace(Workspace::new(&workspace.0))
        .with_output_channels(output_channels)
        .with_scheduled_tasks(Arc::clone(&manager));
    let (address, stop, server) = spawn_server(state).await;
    let client = reqwest::Client::new();
    let base = format!("http://{address}");

    let unauthorized = client
        .get(format!("{base}/v1/scheduled-tasks"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let missing_channel = authorized(
        client.post(format!("{base}/v1/scheduled-tasks")),
        json!({
            "name": "Invalid channel",
            "prompt": "This task must not be created",
            "workspace": workspace.0,
            "agent_profile_id": "reviewer",
            "output_channel_id": "missing",
            "schedule": {
                "type": "interval",
                "every": 1,
                "unit": "hours"
            }
        }),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(missing_channel.status(), StatusCode::BAD_REQUEST);
    let missing_channel: Value = missing_channel.json().await.unwrap();
    assert_eq!(missing_channel["code"], "invalid_scheduled_task");

    let create = authorized(
        client.post(format!("{base}/v1/scheduled-tasks")),
        json!({
            "name": "Weekday review",
            "prompt": "Review the latest workspace changes",
            "workspace": workspace.0,
            "agent_profile_id": "reviewer",
            "output_channel_id": "alerts",
            "schedule": {
                "type": "daily",
                "time": "09:00",
                "weekdays": ["monday", "wednesday", "friday"],
                "timezone": "Asia/Singapore"
            }
        }),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);
    let created: Value = serde_json::from_slice(&create.bytes().await.unwrap()).unwrap();
    let task_id = created["task_id"].as_str().unwrap();
    assert_eq!(created["prompt"], "Review the latest workspace changes");
    assert_eq!(created["agent_profile_id"], "reviewer");
    assert_eq!(created["output_channel_id"], "alerts");
    assert_eq!(created["schedule"]["type"], "daily");
    assert_eq!(created["schedule"]["timezone"], "Asia/Singapore");
    assert_eq!(created["revision"], 1);
    assert!(created["next_run_at"].is_string());

    let listed = client
        .get(format!("{base}/v1/scheduled-tasks"))
        .bearer_auth(AUTH_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed: Value = serde_json::from_slice(&listed.bytes().await.unwrap()).unwrap();
    assert_eq!(listed["tasks"].as_array().unwrap().len(), 1);

    let replaced = authorized(
        client.put(format!("{base}/v1/scheduled-tasks/{task_id}")),
        json!({
            "name": "Edited review",
            "prompt": "Review failures and suggest fixes",
            "workspace": workspace.0,
            "profile_id": "default",
            "agent_profile_id": "reviewer",
            "capability_mode": "read_only",
            "output_channel_id": null,
            "schedule": {
                "type": "interval",
                "every": 30,
                "unit": "minutes"
            },
            "expected_revision": 1
        }),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(replaced.status(), StatusCode::OK);
    let replaced: Value = serde_json::from_slice(&replaced.bytes().await.unwrap()).unwrap();
    assert_eq!(replaced["name"], "Edited review");
    assert_eq!(replaced["prompt"], "Review failures and suggest fixes");
    assert_eq!(replaced["capability_mode"], "read_only");
    assert_eq!(replaced["output_channel_id"], Value::Null);
    assert_eq!(replaced["schedule"]["type"], "interval");
    assert_eq!(replaced["schedule"]["every"], 30);
    assert_eq!(replaced["revision"], 2);
    assert!(replaced["next_run_at"].is_string());

    let replace_conflict = authorized(
        client.put(format!("{base}/v1/scheduled-tasks/{task_id}")),
        json!({
            "name": "Stale edit",
            "prompt": "This edit must not be stored",
            "workspace": workspace.0,
            "profile_id": "default",
            "agent_profile_id": "reviewer",
            "capability_mode": null,
            "output_channel_id": "alerts",
            "schedule": {
                "type": "interval",
                "every": 1,
                "unit": "hours"
            },
            "expected_revision": 1
        }),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(replace_conflict.status(), StatusCode::CONFLICT);

    let paused = authorized(
        client.patch(format!("{base}/v1/scheduled-tasks/{task_id}")),
        json!({ "enabled": false, "expected_revision": 2 }),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(paused.status(), StatusCode::OK);
    let paused: Value = serde_json::from_slice(&paused.bytes().await.unwrap()).unwrap();
    assert_eq!(paused["enabled"], false);
    assert_eq!(paused["next_run_at"], Value::Null);
    assert_eq!(paused["revision"], 3);

    let conflict = authorized(
        client.patch(format!("{base}/v1/scheduled-tasks/{task_id}")),
        json!({ "enabled": true, "expected_revision": 1 }),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let deleted = client
        .delete(format!("{base}/v1/scheduled-tasks/{task_id}"))
        .bearer_auth(AUTH_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

    let _ = stop.send(());
    server.await.unwrap().unwrap();
    manager.shutdown().await;
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn due_task_creates_a_named_independent_session_and_records_success() {
    let workspace = TemporaryDirectory::new("phi-scheduled-run");
    let provider_calls = Arc::new(AtomicUsize::new(0));
    let askuser_exposed = Arc::new(AtomicBool::new(false));
    let service = Arc::new(ApplicationService::new(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        Arc::new(ImmediateFactory {
            provider_calls: Arc::clone(&provider_calls),
            askuser_exposed: Arc::clone(&askuser_exposed),
            install_permission_tool: true,
            emit_tool_call: true,
        }),
    ));
    let store = Arc::new(MemoryScheduledTaskStore::new());
    let channel_store = Arc::new(MemoryOutputChannelStore::new());
    channel_store
        .replace_bot_account(
            "primary",
            BotAccountDefinition::Telegram {
                bot_token: "123456789:abcdefghijklmnopqrstuvwxyz_ABCDEFG".to_owned(),
            },
        )
        .await
        .unwrap();
    channel_store
        .replace_output_channel(
            "alerts",
            OutputChannelDefinition::Telegram {
                bot_account_id: "primary".to_owned(),
                chat_id: "-1001234567890".to_owned(),
            },
        )
        .await
        .unwrap();
    let notifications = Arc::new(RecordingOutputChannelSender::with_failures(1));
    let output_channels = Arc::new(OutputChannelManager::new(
        channel_store,
        notifications.clone(),
    ));
    let now = Utc::now();
    let task = ScheduledTask {
        id: ScheduledTaskId::new(),
        name: "Automated review".to_owned(),
        prompt: "Review the workspace and summarize risks".to_owned(),
        workspace: Workspace::new(&workspace.0),
        profile_id: "default".to_owned(),
        agent_profile_id: "default".to_owned(),
        capability_mode: Some(CapabilityMode::WorkspaceEdit),
        output_channel_id: Some("alerts".to_owned()),
        schedule: ScheduledTaskSchedule::Interval {
            every: 1,
            unit: ScheduledIntervalUnit::Minutes,
        },
        enabled: true,
        created_at: now - chrono::Duration::minutes(2),
        updated_at: now - chrono::Duration::minutes(2),
        next_run_at: Some(now - chrono::Duration::seconds(1)),
        last_run: None,
        skipped_runs: 0,
        revision: 1,
    };
    store.create_task(task.clone()).await.unwrap();
    let manager = Arc::new(
        ScheduledTaskManager::new(Arc::clone(&service), store.clone())
            .with_output_channels(output_channels),
    );
    manager.start().await.unwrap();

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let current = store.get_task(task.id).await.unwrap().unwrap();
            if current.last_run.as_ref().is_some_and(|run| {
                run.outcome == ScheduledRunOutcome::Succeeded && run.session_id.is_some()
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    assert_eq!(provider_calls.load(Ordering::SeqCst), 2);
    assert!(
        !askuser_exposed.load(Ordering::SeqCst),
        "noninteractive scheduled tasks must not expose askuser"
    );
    let sessions = service.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0].record.title.as_deref(),
        Some("Automated review")
    );
    assert_eq!(
        sessions[0].record.scheduled_task_id.as_deref(),
        Some(task.id.to_string().as_str()),
        "scheduled runs must mark their owning task on the session record"
    );
    assert_eq!(
        sessions[0].record.workspace,
        Some(Workspace::new(&workspace.0))
    );
    assert_eq!(sessions[0].state.as_ref().unwrap().message_count, 4);
    assert_eq!(notifications.failures_remaining.load(Ordering::Acquire), 0);
    let messages = notifications.messages.lock().await;
    assert_eq!(messages.len(), 2);
    assert!(messages[0].starts_with("## Scheduled task started\n"));
    assert!(messages[0].contains("- **Status:** ⏳"));
    assert!(messages[1].starts_with("## Scheduled task finished\n"));
    assert!(messages[1].contains("- **Status:** ✅"));
    assert!(!messages[0].contains("Phi scheduled task"));
    assert!(!messages[1].contains("Phi scheduled task"));
    assert!(!messages[1].contains("Session:"));
    assert!(messages[1].contains(
        "| Total | Input | Output | Cached |\n| ---: | ---: | ---: | ---: |\n| 220 | 200 | 20 | 120 |"
    ));
    assert!(messages[1].contains("- **Token cache rate:** 60.0%"));
    assert!(messages[1].contains("- **Tool calls:** 1"));
    assert!(messages[1].contains(r"- **Tools:** scheduled\_inspection ×1"));
    assert!(messages[1].contains("### Final response\n\nscheduled result"));

    manager.shutdown().await;
    assert!(service.shutdown().await.is_empty());
}

#[derive(Default)]
struct RecordingOutputChannelSender {
    messages: Mutex<Vec<String>>,
    failures_remaining: AtomicUsize,
}

impl RecordingOutputChannelSender {
    fn with_failures(failures: usize) -> Self {
        Self {
            messages: Mutex::new(Vec::new()),
            failures_remaining: AtomicUsize::new(failures),
        }
    }
}

#[async_trait]
impl OutputChannelSender for RecordingOutputChannelSender {
    async fn send(
        &self,
        _bot_account: &BotAccountDefinition,
        _definition: &OutputChannelDefinition,
        message: &str,
    ) -> Result<(), OutputChannelDeliveryError> {
        self.messages.lock().await.push(message.to_owned());
        if self
            .failures_remaining
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(OutputChannelDeliveryError::Transport);
        }
        Ok(())
    }
}

#[tokio::test]
async fn paused_task_runs_manually_and_rejects_overlap() {
    let workspace = TemporaryDirectory::new("phi-scheduled-manual");
    let release = Arc::new(Notify::new());
    let service = Arc::new(ApplicationService::new(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        Arc::new(BlockingFactory {
            release: Arc::clone(&release),
        }),
    ));
    let store = Arc::new(MemoryScheduledTaskStore::new());
    let now = Utc::now();
    let task = ScheduledTask {
        id: ScheduledTaskId::new(),
        name: "Manual review".to_owned(),
        prompt: "Review on demand".to_owned(),
        workspace: Workspace::new(&workspace.0),
        profile_id: "default".to_owned(),
        agent_profile_id: "default".to_owned(),
        capability_mode: None,
        output_channel_id: None,
        schedule: ScheduledTaskSchedule::Interval {
            every: 1,
            unit: ScheduledIntervalUnit::Hours,
        },
        enabled: false,
        created_at: now,
        updated_at: now,
        next_run_at: None,
        last_run: None,
        skipped_runs: 0,
        revision: 1,
    };
    store.create_task(task.clone()).await.unwrap();
    let manager = Arc::new(ScheduledTaskManager::new(
        Arc::clone(&service),
        store.clone(),
    ));
    manager.start().await.unwrap();

    manager.run_now(task.id).await.unwrap();
    assert!(matches!(
        manager.run_now(task.id).await,
        Err(ScheduledTaskError::AlreadyRunning { task_id }) if task_id == task.id
    ));
    assert!(!store.get_task(task.id).await.unwrap().unwrap().enabled);

    release.notify_one();
    wait_for_outcome(&store, task.id, ScheduledRunOutcome::Succeeded).await;
    manager.shutdown().await;
    assert!(service.shutdown().await.is_empty());
}

#[tokio::test]
async fn startup_marks_an_uncertain_persisted_run_interrupted() {
    let workspace = TemporaryDirectory::new("phi-scheduled-recovery");
    let service = Arc::new(ApplicationService::new(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        Arc::new(ImmediateFactory {
            provider_calls: Arc::new(AtomicUsize::new(0)),
            askuser_exposed: Arc::new(AtomicBool::new(false)),
            install_permission_tool: false,
            emit_tool_call: false,
        }),
    ));
    let store = Arc::new(MemoryScheduledTaskStore::new());
    let now = Utc::now();
    let task = ScheduledTask {
        id: ScheduledTaskId::new(),
        name: "Recovered review".to_owned(),
        prompt: "Review after restart".to_owned(),
        workspace: Workspace::new(&workspace.0),
        profile_id: "default".to_owned(),
        agent_profile_id: "default".to_owned(),
        capability_mode: None,
        output_channel_id: None,
        schedule: ScheduledTaskSchedule::Interval {
            every: 1,
            unit: ScheduledIntervalUnit::Hours,
        },
        enabled: true,
        created_at: now - chrono::Duration::hours(2),
        updated_at: now - chrono::Duration::hours(2),
        next_run_at: Some(now + chrono::Duration::hours(1)),
        last_run: Some(ScheduledTaskRun {
            scheduled_for: now - chrono::Duration::hours(1),
            started_at: now - chrono::Duration::hours(1),
            finished_at: None,
            outcome: ScheduledRunOutcome::Running,
            session_id: None,
            error: None,
        }),
        skipped_runs: 0,
        revision: 1,
    };
    store.create_task(task.clone()).await.unwrap();
    let manager = Arc::new(ScheduledTaskManager::new(
        Arc::clone(&service),
        store.clone(),
    ));

    manager.start().await.unwrap();
    let recovered = store.get_task(task.id).await.unwrap().unwrap();
    let run = recovered.last_run.unwrap();
    assert_eq!(run.outcome, ScheduledRunOutcome::Interrupted);
    assert!(run.finished_at.is_some());
    assert_eq!(
        run.error.as_deref(),
        Some("daemon restarted before the run completed")
    );

    manager.shutdown().await;
    assert!(service.shutdown().await.is_empty());
}

async fn wait_for_outcome(
    store: &MemoryScheduledTaskStore,
    task_id: ScheduledTaskId,
    expected: ScheduledRunOutcome,
) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let task = store.get_task(task_id).await.unwrap().unwrap();
            if task
                .last_run
                .as_ref()
                .is_some_and(|run| run.outcome == expected)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

fn authorized(builder: reqwest::RequestBuilder, body: Value) -> reqwest::RequestBuilder {
    builder
        .bearer_auth(AUTH_KEY)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(serde_json::to_vec(&body).unwrap())
}

async fn spawn_server(
    state: AppState,
) -> (
    SocketAddr,
    oneshot::Sender<()>,
    JoinHandle<Result<(), io::Error>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(serve(listener, state, async move {
        let _ = stopped.await;
    }));
    (address, stop, server)
}

#[derive(Clone)]
struct ImmediateProvider {
    calls: Arc<AtomicUsize>,
    askuser_exposed: Arc<AtomicBool>,
    expect_permission_tool_hidden: bool,
    emit_tool_call: bool,
}

impl LlmProvider for ImmediateProvider {
    fn stream(&self, request: ProviderRequest) -> ProviderEventStream {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        self.askuser_exposed.store(
            request.tools.iter().any(|tool| tool.name == "askuser"),
            Ordering::SeqCst,
        );
        if self.expect_permission_tool_hidden {
            assert!(
                request
                    .tools
                    .iter()
                    .all(|tool| tool.name != "scheduled_external"),
                "noninteractive scheduled tasks must not expose tools that require approval"
            );
        }
        let message = if self.emit_tool_call && call == 0 {
            AssistantMessage::tool_calls(vec![ToolCall::new(
                "scheduled-inspection-1",
                "scheduled_inspection",
                json!({}),
            )])
        } else {
            AssistantMessage::text("scheduled result")
        };
        let usage = if call == 0 {
            TokenUsage::new(120, 12, 80)
        } else {
            TokenUsage::new(80, 8, 40)
        };
        Box::pin(stream::iter([Ok(ProviderEvent::Done(ProviderResponse {
            message,
            usage: Some(usage),
        }))]))
    }
}

#[derive(Clone)]
struct ImmediateFactory {
    provider_calls: Arc<AtomicUsize>,
    askuser_exposed: Arc<AtomicBool>,
    install_permission_tool: bool,
    emit_tool_call: bool,
}

#[derive(Clone)]
struct ScheduledExternalTool;

#[derive(Clone)]
struct ScheduledInspectionTool;

#[async_trait]
impl Tool for ScheduledInspectionTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "scheduled_inspection",
            "returns a deterministic inspection result",
            json!({ "type": "object" }),
        )
    }

    fn effect(&self) -> ToolEffect {
        ToolEffect::ReadOnly
    }

    async fn execute(&self, _arguments: Value) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::success("inspection complete"))
    }
}

#[async_trait]
impl Tool for ScheduledExternalTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "scheduled_external",
            "must remain hidden from a noninteractive scheduled task",
            json!({ "type": "object" }),
        )
    }

    fn effect(&self) -> ToolEffect {
        ToolEffect::ExternalSideEffect
    }

    async fn execute(&self, _arguments: Value) -> Result<ToolOutput, ToolError> {
        panic!("the hidden scheduled external tool must never execute")
    }
}

#[derive(Clone)]
struct BlockingProvider {
    release: Arc<Notify>,
}

impl LlmProvider for BlockingProvider {
    fn stream(&self, _request: ProviderRequest) -> ProviderEventStream {
        let release = Arc::clone(&self.release);
        Box::pin(stream::once(async move {
            release.notified().await;
            Ok(ProviderEvent::Done(ProviderResponse {
                message: AssistantMessage::text("scheduled result"),
                usage: None,
            }))
        }))
    }
}

#[derive(Clone)]
struct BlockingFactory {
    release: Arc<Notify>,
}

#[async_trait]
impl AgentFactory for BlockingFactory {
    async fn build(&self, request: &AgentBuildRequest) -> Result<BuiltAgent, AgentFactoryError> {
        let workspace = request
            .workspace
            .clone()
            .unwrap_or_else(|| Workspace::new("."));
        let agent_profile = request.pinned_agent_profile.clone().unwrap_or_else(|| {
            compile_agent_profile(&default_agent_profile(), &workspace).unwrap()
        });
        let model = request
            .model
            .clone()
            .unwrap_or_else(|| "test-model".to_owned());
        let agent = Agent::builder(BlockingProvider {
            release: Arc::clone(&self.release),
        })
        .workspace(workspace)
        .model(model.clone())
        .system_prompt(agent_profile.compiled_system_prompt.clone())
        .build();
        Ok(BuiltAgent {
            agent,
            skills: SkillCatalog::default(),
            profile_id: request.profile_id.clone(),
            agent_profile,
            model,
            reasoning_effort: None,
        })
    }
}

#[async_trait]
impl AgentFactory for ImmediateFactory {
    async fn build(&self, request: &AgentBuildRequest) -> Result<BuiltAgent, AgentFactoryError> {
        let workspace = request
            .workspace
            .clone()
            .unwrap_or_else(|| Workspace::new("."));
        let agent_profile = request.pinned_agent_profile.clone().unwrap_or_else(|| {
            compile_agent_profile(&default_agent_profile(), &workspace).unwrap()
        });
        let model = request
            .model
            .clone()
            .unwrap_or_else(|| "test-model".to_owned());
        let capability_mode = request
            .capability_mode
            .unwrap_or(agent_profile.definition.initial_capability_mode);
        let mut builder = Agent::builder(ImmediateProvider {
            calls: Arc::clone(&self.provider_calls),
            askuser_exposed: Arc::clone(&self.askuser_exposed),
            expect_permission_tool_hidden: self.install_permission_tool,
            emit_tool_call: self.emit_tool_call,
        })
        .workspace(workspace)
        .model(model.clone())
        .capability_mode(capability_mode)
        .system_prompt(agent_profile.compiled_system_prompt.clone());
        if self.install_permission_tool {
            builder = builder.tool(ScheduledExternalTool);
        }
        if self.emit_tool_call {
            builder = builder.tool(ScheduledInspectionTool);
        }
        let agent = builder.build();
        Ok(BuiltAgent {
            agent,
            skills: SkillCatalog::default(),
            profile_id: request.profile_id.clone(),
            agent_profile,
            model,
            reasoning_effort: None,
        })
    }
}

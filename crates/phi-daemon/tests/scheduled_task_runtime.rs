use std::{
    io,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use chrono::Utc;
use futures_util::stream;
use phi::{
    Agent, AssistantMessage, InMemorySessionStorage, LlmProvider, ProviderEvent,
    ProviderEventStream, ProviderRequest, ProviderResponse, SkillCatalog, Workspace,
};
use phi_daemon::{
    api::AppState,
    runtime::{
        AgentBuildRequest, AgentFactory, AgentFactoryError, AgentRegistry, BuiltAgent,
        compile_agent_profile, default_agent_profile,
    },
    scheduled_task::{
        ScheduledIntervalUnit, ScheduledRunOutcome, ScheduledTask, ScheduledTaskError,
        ScheduledTaskId, ScheduledTaskManager, ScheduledTaskRun, ScheduledTaskSchedule,
    },
    serve,
    service::ApplicationService,
    store::{
        MemoryControlStore, MemoryProviderStore, MemoryScheduledTaskStore, ProviderConfig,
        ProviderKind, ProviderStore, ScheduledTaskStore,
    },
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::{
    net::TcpListener,
    sync::{Notify, oneshot},
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
    let manager = Arc::new(ScheduledTaskManager::new(
        Arc::clone(&service),
        Arc::new(MemoryScheduledTaskStore::new()),
    ));
    let state = AppState::new(Arc::clone(&service), AUTH_KEY)
        .with_default_workspace(Workspace::new(&workspace.0))
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

    let create = authorized(
        client.post(format!("{base}/v1/scheduled-tasks")),
        json!({
            "name": "Weekday review",
            "prompt": "Review the latest workspace changes",
            "workspace": workspace.0,
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

    let paused = authorized(
        client.patch(format!("{base}/v1/scheduled-tasks/{task_id}")),
        json!({ "enabled": false, "expected_revision": 1 }),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(paused.status(), StatusCode::OK);
    let paused: Value = serde_json::from_slice(&paused.bytes().await.unwrap()).unwrap();
    assert_eq!(paused["enabled"], false);
    assert_eq!(paused["next_run_at"], Value::Null);
    assert_eq!(paused["revision"], 2);

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
    let service = Arc::new(ApplicationService::new(
        AgentRegistry::new(),
        Arc::new(MemoryControlStore::new()),
        Arc::new(InMemorySessionStorage::new()),
        Arc::new(ImmediateFactory {
            provider_calls: Arc::clone(&provider_calls),
        }),
    ));
    let store = Arc::new(MemoryScheduledTaskStore::new());
    let now = Utc::now();
    let task = ScheduledTask {
        id: ScheduledTaskId::new(),
        name: "Automated review".to_owned(),
        prompt: "Review the workspace and summarize risks".to_owned(),
        workspace: Workspace::new(&workspace.0),
        profile_id: "default".to_owned(),
        agent_profile_id: "default".to_owned(),
        capability_mode: None,
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
    let manager = Arc::new(ScheduledTaskManager::new(
        Arc::clone(&service),
        store.clone(),
    ));
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

    assert_eq!(provider_calls.load(Ordering::SeqCst), 1);
    let sessions = service.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0].record.title.as_deref(),
        Some("Automated review")
    );
    assert_eq!(
        sessions[0].record.workspace,
        Some(Workspace::new(&workspace.0))
    );
    assert_eq!(sessions[0].state.as_ref().unwrap().message_count, 2);

    manager.shutdown().await;
    assert!(service.shutdown().await.is_empty());
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
}

impl LlmProvider for ImmediateProvider {
    fn stream(&self, _request: ProviderRequest) -> ProviderEventStream {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(stream::iter([Ok(ProviderEvent::Done(ProviderResponse {
            message: AssistantMessage::text("scheduled result"),
            usage: None,
        }))]))
    }
}

#[derive(Clone)]
struct ImmediateFactory {
    provider_calls: Arc<AtomicUsize>,
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
        let agent = Agent::builder(ImmediateProvider {
            calls: Arc::clone(&self.provider_calls),
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

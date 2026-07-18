use std::sync::Arc;

use async_trait::async_trait;
use futures_util::stream;
use phi::{
    Agent, AssistantMessage, Content, InMemorySessionStorage, LlmProvider, ProviderEvent,
    ProviderEventStream, ProviderRequest, ProviderResponse, ReasoningEffort, SkillCatalog,
    Workspace,
};
use phi_daemon::{
    runtime::{
        AgentBuildRequest, AgentFactory, AgentFactoryError, AgentRegistry, BuiltAgent,
        RuntimeEventKind, compile_agent_profile, default_agent_profile,
    },
    service::ApplicationService,
    session_title::{SessionTitleError, SessionTitleGenerator, SessionTitleRequest},
    store::{ControlStore, MemoryControlStore},
};
use tokio::sync::{Mutex, Notify};

#[derive(Clone)]
struct BlockingProvider {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

impl LlmProvider for BlockingProvider {
    fn stream(&self, _request: ProviderRequest) -> ProviderEventStream {
        let started = Arc::clone(&self.started);
        let release = Arc::clone(&self.release);
        Box::pin(stream::once(async move {
            started.notify_one();
            release.notified().await;
            Ok(ProviderEvent::Done(ProviderResponse {
                message: AssistantMessage::text("main answer"),
                usage: None,
            }))
        }))
    }
}

#[derive(Clone)]
struct TestFactory {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl AgentFactory for TestFactory {
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
            .unwrap_or_else(|| "session-model".to_owned());
        let reasoning_effort = ReasoningEffort::Low;
        let agent = Agent::builder(BlockingProvider {
            started: Arc::clone(&self.started),
            release: Arc::clone(&self.release),
        })
        .workspace(workspace)
        .model(model.clone())
        .reasoning_effort(reasoning_effort)
        .build();
        Ok(BuiltAgent {
            agent,
            skills: SkillCatalog::default(),
            profile_id: request.profile_id.clone(),
            agent_profile,
            model,
            reasoning_effort: Some(reasoning_effort),
        })
    }
}

#[derive(Clone)]
struct RecordingTitleGenerator {
    requests: Arc<Mutex<Vec<SessionTitleRequest>>>,
}

#[async_trait]
impl SessionTitleGenerator for RecordingTitleGenerator {
    async fn generate_title(
        &self,
        request: SessionTitleRequest,
    ) -> Result<String, SessionTitleError> {
        self.requests.lock().await.push(request);
        Ok("Fix flaky storage tests".to_owned())
    }
}

#[tokio::test]
async fn first_prompt_generates_persists_and_broadcasts_a_session_title() {
    let control = Arc::new(MemoryControlStore::new());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let service = ApplicationService::new(
        AgentRegistry::new(),
        control.clone(),
        Arc::new(InMemorySessionStorage::new()),
        Arc::new(TestFactory {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        }),
    )
    .with_session_title_generator(RecordingTitleGenerator {
        requests: Arc::clone(&requests),
    });
    let prepared = service.prepare_session("conversation").await.unwrap();
    let handle = prepared.handle().clone();
    let session_id = handle.session_id();
    let mut events = handle.subscribe();

    let (_, queued) = service
        .activate_and_enqueue_with_title_content(
            &prepared,
            Content::text("expanded skill instructions"),
            Content::text("Fix the flaky storage tests"),
        )
        .await
        .unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
        .await
        .expect("the main run did not reach the Provider");

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.unwrap();
            match event.kind {
                RuntimeEventKind::RunCompleted { run_id } if run_id == queued.run_id => {
                    panic!("the title was delayed until after the main run completed");
                }
                RuntimeEventKind::TitleChanged { title } => {
                    assert_eq!(title, "Fix flaky storage tests");
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("the title task did not finish");

    assert_eq!(
        handle.summary().title.as_deref(),
        Some("Fix flaky storage tests")
    );
    assert_eq!(
        control
            .get_session(session_id)
            .await
            .unwrap()
            .unwrap()
            .title
            .as_deref(),
        Some("Fix flaky storage tests")
    );

    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].profile_id, "conversation");
    assert_eq!(requests[0].model, "session-model");
    assert_eq!(requests[0].reasoning_effort, Some(ReasoningEffort::Low));
    assert_eq!(
        requests[0].initial_content,
        Content::text("Fix the flaky storage tests")
    );
    drop(requests);

    release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.unwrap();
            if matches!(
                event.kind,
                RuntimeEventKind::RunCompleted { run_id } if run_id == queued.run_id
            ) {
                break;
            }
        }
    })
    .await
    .expect("the main run did not complete after release");

    assert!(service.shutdown().await.is_empty());
}

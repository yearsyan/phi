use std::{fs, path::PathBuf, sync::Arc};

use async_trait::async_trait;
use futures_util::stream;
use phi::{
    Agent, Content, InMemorySessionStorage, LlmProvider, ProviderEventStream, ProviderRequest,
    SkillCatalog, SkillInvocation, SkillsConfig, Workspace,
};
use phi_daemon::{
    api::AppState,
    runtime::{
        AgentBuildRequest, AgentFactory, AgentFactoryError, AgentHandle, AgentRegistry, BuiltAgent,
        SessionId, compile_agent_profile, default_agent_profile,
    },
    serve,
    service::ApplicationService,
    store::{ControlStore, MemoryControlStore, SessionRecord},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};

const AUTH_KEY: &str = "a-secure-test-key-with-at-least-32-bytes";

#[derive(Clone)]
struct TestFactory {
    skills: SkillCatalog,
}

#[async_trait]
impl AgentFactory for TestFactory {
    async fn build(&self, request: &AgentBuildRequest) -> Result<BuiltAgent, AgentFactoryError> {
        let mut builder = Agent::builder(NeverProvider).skills(self.skills.clone());
        if let Some(workspace) = &request.workspace {
            builder = builder.workspace(workspace.clone());
        }
        Ok(BuiltAgent {
            agent: builder.build(),
            skills: self.skills.clone(),
            profile_id: request.profile_id.clone(),
            agent_profile: request.pinned_agent_profile.clone().unwrap_or_else(|| {
                compile_agent_profile(
                    &default_agent_profile(),
                    request.workspace.as_ref().unwrap_or(&Workspace::new(".")),
                )
                .unwrap()
            }),
            model: request
                .model
                .clone()
                .unwrap_or_else(|| "test-model".to_owned()),
            reasoning_effort: request.reasoning_effort,
        })
    }
}

#[derive(Clone)]
struct NeverProvider;

impl LlmProvider for NeverProvider {
    fn stream(&self, _request: ProviderRequest) -> ProviderEventStream {
        Box::pin(stream::empty())
    }
}

struct TestSkillsDir(PathBuf);

impl Drop for TestSkillsDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

async fn test_catalog() -> (TestSkillsDir, SkillCatalog) {
    let root = std::env::temp_dir().join(format!("phi-daemon-skills-{}", uuid::Uuid::now_v7()));
    let skill = root.join("review");
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\ndescription: Review code safely\nargument-hint: topic\n---\nReview $ARGUMENTS carefully.",
    )
    .unwrap();
    let catalog = SkillCatalog::load(&SkillsConfig::new().directory(&root))
        .await
        .unwrap();
    (TestSkillsDir(root), catalog)
}

#[tokio::test]
async fn handle_exposes_snapshot_and_expands_selected_skill() {
    let (_root, catalog) = test_catalog().await;
    let session_id = SessionId::new();
    let handle = AgentHandle::spawn_with_skills(
        session_id,
        Agent::builder(NeverProvider)
            .skills(catalog.clone())
            .build(),
        "default",
        "test-model",
        None,
        catalog,
    );

    assert_eq!(handle.skills().len(), 1);
    assert_eq!(handle.skills()[0].name, "review");
    let prompt = handle
        .prepare_prompt(
            Content::text("authentication"),
            Some(&SkillInvocation::new("review")),
        )
        .unwrap();
    assert!(
        prompt
            .as_text()
            .unwrap()
            .contains("Review authentication carefully.")
    );
    assert!(
        handle
            .prepare_prompt(
                Content::text("anything"),
                Some(&SkillInvocation::new("missing")),
            )
            .is_err()
    );

    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn http_lists_offline_session_skills_without_registering_an_actor() {
    let (_root, catalog) = test_catalog().await;
    let session_id = SessionId::new();
    let memory_store = Arc::new(MemoryControlStore::new());
    memory_store
        .create_session(SessionRecord::new(
            session_id,
            "default",
            "test-model",
            None,
        ))
        .await
        .unwrap();
    let control_store: Arc<dyn ControlStore> = memory_store;
    let service = Arc::new(ApplicationService::new(
        AgentRegistry::new(),
        control_store,
        Arc::new(InMemorySessionStorage::new()),
        Arc::new(TestFactory { skills: catalog }),
    ));
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

    let mut socket = TcpStream::connect(address).await.unwrap();
    socket
        .write_all(
            format!(
                "GET /v1/sessions/{session_id}/skills HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {AUTH_KEY}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    socket.read_to_end(&mut response).await.unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let body = response.split("\r\n\r\n").nth(1).unwrap();
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(body["session_id"], session_id.to_string());
    assert_eq!(body["skills"][0]["name"], "review");
    assert_eq!(body["skills"][0]["model_invocable"], true);
    assert_eq!(body["skills"][0]["user_invocable"], true);
    assert!(body["skills"][0].get("path").is_none());
    assert!(service.registry().get(session_id).await.is_none());

    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

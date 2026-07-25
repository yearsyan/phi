use std::{io, net::SocketAddr, sync::Arc};

use async_trait::async_trait;
use phi_daemon::{
    api::AppState,
    output_channel::{
        OutputChannelDefinition, OutputChannelDeliveryError, OutputChannelManager,
        OutputChannelSender,
    },
    serve,
    service::ApplicationService,
    store::MemoryOutputChannelStore,
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::{
    net::TcpListener,
    sync::{Mutex, oneshot},
    task::JoinHandle,
};

const AUTH_KEY: &str = "a-secure-test-key-with-at-least-32-bytes";
const BOT_TOKEN: &str = "123456789:abcdefghijklmnopqrstuvwxyz_ABCDEFG";

#[tokio::test]
async fn authenticated_api_persists_redacted_telegram_channel_and_tests_delivery() {
    let sender = Arc::new(RecordingSender::default());
    let manager = Arc::new(OutputChannelManager::new(
        Arc::new(MemoryOutputChannelStore::new()),
        sender.clone(),
    ));
    let service = Arc::new(ApplicationService::unconfigured());
    let state = AppState::new(service, AUTH_KEY).with_output_channels(manager);
    let (address, stop, server) = spawn_server(state).await;
    let client = reqwest::Client::new();
    let base = format!("http://{address}");

    let unauthorized = client
        .get(format!("{base}/v1/output-channels"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let invalid = client
        .put(format!("{base}/v1/output-channels/invalid"))
        .bearer_auth(AUTH_KEY)
        .json(&json!({
            "type": BOT_TOKEN,
            "bot_token": BOT_TOKEN,
            "chat_id": "-1001234567890"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert!(!invalid.text().await.unwrap().contains(BOT_TOKEN));

    let saved = client
        .put(format!("{base}/v1/output-channels/telegram-alerts"))
        .bearer_auth(AUTH_KEY)
        .json(&json!({
            "type": "telegram",
            "bot_token": BOT_TOKEN,
            "chat_id": "-1001234567890"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::OK);
    let saved: Value = saved.json().await.unwrap();
    assert_eq!(saved["configured"], true);
    assert_eq!(saved["output_channel"]["type"], "telegram");
    assert_eq!(
        saved["output_channel"]["bot_token_configured"],
        Value::Bool(true)
    );
    assert!(saved.get("bot_token").is_none());
    assert!(!saved.to_string().contains(BOT_TOKEN));

    let listed = client
        .get(format!("{base}/v1/output-channels"))
        .bearer_auth(AUTH_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed: Value = listed.json().await.unwrap();
    assert_eq!(listed["output_channels"].as_array().unwrap().len(), 1);
    assert!(!listed.to_string().contains(BOT_TOKEN));

    let tested = client
        .post(format!("{base}/v1/output-channels/telegram-alerts/test"))
        .bearer_auth(AUTH_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(tested.status(), StatusCode::NO_CONTENT);
    let deliveries = sender.deliveries.lock().await;
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].0, "-1001234567890");
    assert!(deliveries[0].1.contains("test succeeded"));

    let _ = stop.send(());
    server.await.unwrap().unwrap();
}

#[derive(Default)]
struct RecordingSender {
    deliveries: Mutex<Vec<(String, String)>>,
}

#[async_trait]
impl OutputChannelSender for RecordingSender {
    async fn send(
        &self,
        definition: &OutputChannelDefinition,
        message: &str,
    ) -> Result<(), OutputChannelDeliveryError> {
        let OutputChannelDefinition::Telegram { chat_id, .. } = definition;
        self.deliveries
            .lock()
            .await
            .push((chat_id.clone(), message.to_owned()));
        Ok(())
    }
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

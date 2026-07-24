use std::{io, sync::Arc};

use phi_daemon::{api::AppState, serve, service::ApplicationService};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

const AUTH_KEY: &str = "a-secure-test-key-with-at-least-32-bytes";

struct TestServer {
    base_url: String,
    stop: Option<oneshot::Sender<()>>,
    server: JoinHandle<io::Result<()>>,
}

impl TestServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let state = AppState::new(Arc::new(ApplicationService::unconfigured()), AUTH_KEY);
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(serve(listener, state, async move {
            let _ = stopped.await;
        }));
        Self {
            base_url: format!("http://{address}"),
            stop: Some(stop),
            server,
        }
    }

    fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("reqwest client")
    }

    async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.server.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn embedded_web_client_serves_index_spa_fallback_and_cache_headers() {
    let server = TestServer::start().await;
    let client = server.client();

    let index = client
        .get(format!("{}/", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(index.status(), reqwest::StatusCode::OK);
    let content_type = index
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        content_type.starts_with("text/html"),
        "unexpected content type: {content_type}"
    );
    let cache_control = index
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(cache_control, "no-cache");
    let body = index.text().await.unwrap().to_lowercase();
    assert!(
        body.contains("<html"),
        "index response should be an HTML document"
    );

    // Unknown client-side routes fall back to the SPA shell.
    let spa = client
        .get(format!("{}/settings/profiles", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(spa.status(), reqwest::StatusCode::OK);
    let spa_content_type = spa
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        spa_content_type.starts_with("text/html"),
        "unexpected content type: {spa_content_type}"
    );

    // HEAD is served like GET (hyper omits the body on the wire).
    let head = client
        .head(format!("{}/", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), reqwest::StatusCode::OK);

    server.shutdown().await;
}

#[tokio::test]
async fn embedded_web_client_preserves_api_routing_and_404_semantics() {
    let server = TestServer::start().await;
    let client = server.client();

    // Real API routes still take priority over the fallback and keep auth.
    let protected = client
        .get(format!("{}/v1/providers", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(protected.status(), reqwest::StatusCode::UNAUTHORIZED);

    // Unknown /v1 paths keep plain API 404 semantics (no SPA shell).
    let unknown_api = client
        .get(format!("{}/v1/definitely-not-a-route", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown_api.status(), reqwest::StatusCode::NOT_FOUND);

    // Missing asset-like files must not fall back to the SPA shell.
    let missing_asset = client
        .get(format!(
            "{}/assets/definitely-missing-deadbeef.js",
            server.base_url
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(missing_asset.status(), reqwest::StatusCode::NOT_FOUND);

    // The embedded client is read-only.
    let post = client
        .post(format!("{}/", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(post.status(), reqwest::StatusCode::METHOD_NOT_ALLOWED);

    server.shutdown().await;
}

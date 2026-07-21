use std::{
    future::Future, io, net::SocketAddr, path::PathBuf, pin::Pin, sync::Arc, time::Duration,
};

use axum::serve::Listener;
use futures_util::{StreamExt, stream::FuturesUnordered};
use phi::{BuiltinTools, DiskSessionStorage, Workspace};
use thiserror::Error;
use tokio::{
    net::{TcpListener, TcpStream},
    signal,
    sync::oneshot,
    time::timeout,
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        ServerConfig,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    },
    server::TlsStream,
};
use tracing::{error, info, warn};

use crate::{
    api::{self, AppState},
    config::{ConfigError, DaemonConfig, TlsConfig},
    connection_qr,
    runtime::AgentRegistry,
    scheduled_task::{ScheduledTaskError, ScheduledTaskManager},
    service::ApplicationService,
    session_title::ProviderSessionTitleGenerator,
    store::{
        DiskAgentProfileStore, DiskControlStore, DiskProviderStore, DiskScheduledTaskStore,
        ProviderStore, ScheduledTaskStore,
    },
};

const CONTROL_DIRECTORY: &str = "control";
const SESSION_DIRECTORY: &str = "sessions";
const PROVIDER_CONFIG_FILE: &str = "provider.json";
const AGENT_PROFILE_CONFIG_FILE: &str = "agent-profiles.json";
const SCHEDULED_TASK_CONFIG_FILE: &str = "scheduled-tasks.json";
const MAX_PENDING_TLS_HANDSHAKES: usize = 128;
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

type PendingTlsHandshake = Pin<
    Box<dyn Future<Output = (SocketAddr, Result<TlsStream<TcpStream>, TlsHandshakeError>)> + Send>,
>;

pub async fn run(config: DaemonConfig) -> Result<(), DaemonError> {
    let provider_http_client = config.provider_http_client()?;
    let tls_acceptor = match config.tls_config() {
        Some(tls) => Some(load_tls_acceptor(tls).await?),
        None => None,
    };
    let service = Arc::new(application_service(&config, provider_http_client));
    let scheduled_task_store: Arc<dyn ScheduledTaskStore> = Arc::new(DiskScheduledTaskStore::new(
        config.data_dir().join(SCHEDULED_TASK_CONFIG_FILE),
    ));
    let scheduled_tasks = Arc::new(ScheduledTaskManager::new(
        Arc::clone(&service),
        scheduled_task_store,
    ));
    let state = AppState::new(Arc::clone(&service), config.auth_key())
        .with_default_workspace(Workspace::new(config.workspace_dir()))
        .with_scheduled_tasks(Arc::clone(&scheduled_tasks));
    let address = config.bind_address();
    let listener = TcpListener::bind(address)
        .await
        .map_err(|source| DaemonError::Bind { address, source })?;
    scheduled_tasks.start().await?;
    let local_address = listener.local_addr().map_err(DaemonError::LocalAddress)?;

    info!(
        %local_address,
        transport = if tls_acceptor.is_some() { "https" } else { "http" },
        data_dir = %config.data_dir().display(),
        workspace_dir = %config.workspace_dir().display(),
        "phi daemon listening"
    );
    connection_qr::print_for_terminal(&config, local_address, tls_acceptor.is_some());
    let (begin_graceful_shutdown, graceful_shutdown) = oneshot::channel();
    let shutdown_service = Arc::clone(&service);
    let shutdown_scheduled_tasks = Arc::clone(&scheduled_tasks);
    let shutdown_task = tokio::spawn(async move {
        shutdown_signal().await;
        // Stop accepting new HTTP/WS connections before draining the actors.
        let _ = begin_graceful_shutdown.send(());
        shutdown_runtime(&shutdown_scheduled_tasks, &shutdown_service).await;
    });
    let shutdown = async move {
        let _ = graceful_shutdown.await;
    };
    let result = match tls_acceptor {
        Some(tls_acceptor) => serve_tls(listener, tls_acceptor, state, shutdown).await,
        None => serve(listener, state, shutdown).await,
    };

    if result.is_ok() {
        let _ = shutdown_task.await;
    } else {
        shutdown_task.abort();
    }

    // Also clean up if the HTTP server exits because of an I/O failure before
    // the signal task runs. After a normal signal this is an empty no-op.
    shutdown_runtime(&scheduled_tasks, &service).await;

    result.map_err(DaemonError::Serve)
}

async fn shutdown_runtime(scheduled_tasks: &ScheduledTaskManager, service: &ApplicationService) {
    scheduled_tasks.shutdown().await;
    shutdown_agents(service).await;
}

async fn shutdown_agents(service: &ApplicationService) {
    for failure in service.shutdown().await {
        warn!(
            session_id = %failure.session_id,
            error = %failure.error,
            "agent actor did not shut down cleanly"
        );
    }
}

fn application_service(
    config: &DaemonConfig,
    provider_http_client: reqwest::Client,
) -> ApplicationService {
    let data_dir = config.data_dir();
    let control_store = Arc::new(DiskControlStore::new(data_dir.join(CONTROL_DIRECTORY)));
    let session_storage = Arc::new(DiskSessionStorage::new(data_dir.join(SESSION_DIRECTORY)));
    let provider_store: Arc<dyn ProviderStore> =
        Arc::new(DiskProviderStore::new(data_dir.join(PROVIDER_CONFIG_FILE)));
    let agent_profile_store = Arc::new(DiskAgentProfileStore::new(
        data_dir.join(AGENT_PROFILE_CONFIG_FILE),
    ));

    let title_generator = ProviderSessionTitleGenerator::new(Arc::clone(&provider_store))
        .http_client(provider_http_client.clone());
    let title_generator = match config.session_title_profile_id() {
        Some(profile_id) => title_generator.with_profile_id(profile_id),
        None => title_generator,
    };

    ApplicationService::managed_with_profiles_skills_and_builtin_tools_http_client(
        AgentRegistry::new(),
        control_store,
        session_storage,
        provider_store,
        agent_profile_store,
        config.skills_config_template(),
        BuiltinTools::all(config.workspace_dir()),
        provider_http_client,
    )
    .with_session_title_generator(title_generator)
    .with_subagent_worktree_root(data_dir.join("subagent-worktrees"))
    .with_subagents_enabled(config.subagents_enabled())
}

pub async fn serve<F>(listener: TcpListener, state: AppState, shutdown: F) -> Result<(), io::Error>
where
    F: Future<Output = ()> + Send + 'static,
{
    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(shutdown)
        .await
}

async fn serve_tls<F>(
    listener: TcpListener,
    tls_acceptor: TlsAcceptor,
    state: AppState,
    shutdown: F,
) -> Result<(), io::Error>
where
    F: Future<Output = ()> + Send + 'static,
{
    axum::serve(TlsListener::new(listener, tls_acceptor), api::router(state))
        .with_graceful_shutdown(shutdown)
        .await
}

async fn load_tls_acceptor(config: &TlsConfig) -> Result<TlsAcceptor, DaemonError> {
    let certificate_file = config.certificate_file().to_owned();
    let certificate_pem = tokio::fs::read(&certificate_file).await.map_err(|source| {
        DaemonError::TlsCertificateFile {
            path: certificate_file.clone(),
            source,
        }
    })?;
    let certificates = CertificateDer::pem_slice_iter(&certificate_pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| DaemonError::TlsCertificatePem {
            path: certificate_file.clone(),
            source,
        })?;
    if certificates.is_empty() {
        return Err(DaemonError::EmptyTlsCertificateChain {
            path: certificate_file,
        });
    }

    let private_key_file = config.private_key_file().to_owned();
    let private_key_pem = tokio::fs::read(&private_key_file).await.map_err(|source| {
        DaemonError::TlsPrivateKeyFile {
            path: private_key_file.clone(),
            source,
        }
    })?;
    let private_key = PrivateKeyDer::from_pem_slice(&private_key_pem).map_err(|source| {
        DaemonError::TlsPrivateKeyPem {
            path: private_key_file,
            source,
        }
    })?;

    let mut server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certificates, private_key)
        .map_err(DaemonError::TlsConfiguration)?;
    server_config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsAcceptor::from(Arc::new(server_config)))
}

struct TlsListener {
    listener: TcpListener,
    acceptor: TlsAcceptor,
    pending: FuturesUnordered<PendingTlsHandshake>,
}

impl TlsListener {
    fn new(listener: TcpListener, acceptor: TlsAcceptor) -> Self {
        Self {
            listener,
            acceptor,
            pending: FuturesUnordered::new(),
        }
    }

    fn queue_handshake(&mut self, stream: TcpStream, peer_address: SocketAddr) {
        let acceptor = self.acceptor.clone();
        self.pending.push(Box::pin(async move {
            let result = match timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await {
                Ok(result) => result.map_err(TlsHandshakeError::Io),
                Err(_) => Err(TlsHandshakeError::Timeout),
            };
            (peer_address, result)
        }));
    }

    async fn next_handshake(
        &mut self,
    ) -> Option<(SocketAddr, Result<TlsStream<TcpStream>, TlsHandshakeError>)> {
        self.pending.next().await
    }
}

impl Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        enum NextEvent {
            Tcp(TcpStream, SocketAddr),
            Handshake(
                SocketAddr,
                Box<Result<TlsStream<TcpStream>, TlsHandshakeError>>,
            ),
        }

        loop {
            let next = if self.pending.is_empty() {
                let (stream, peer_address) = Listener::accept(&mut self.listener).await;
                NextEvent::Tcp(stream, peer_address)
            } else if self.pending.len() >= MAX_PENDING_TLS_HANDSHAKES {
                match self.next_handshake().await {
                    Some((peer_address, result)) => {
                        NextEvent::Handshake(peer_address, Box::new(result))
                    }
                    None => continue,
                }
            } else {
                let listener = &mut self.listener;
                let pending = &mut self.pending;
                tokio::select! {
                    (stream, peer_address) = Listener::accept(listener) => {
                        NextEvent::Tcp(stream, peer_address)
                    }
                    Some((peer_address, result)) = pending.next() => {
                        NextEvent::Handshake(peer_address, Box::new(result))
                    }
                }
            };

            match next {
                NextEvent::Tcp(stream, peer_address) => {
                    self.queue_handshake(stream, peer_address);
                }
                NextEvent::Handshake(peer_address, result) => match *result {
                    Ok(stream) => return (stream, peer_address),
                    Err(source) => {
                        warn!(
                            %peer_address,
                            error = %source,
                            "daemon TLS handshake failed"
                        );
                    }
                },
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

#[derive(Debug, Error)]
enum TlsHandshakeError {
    #[error("handshake timed out after {TLS_HANDSHAKE_TIMEOUT:?}")]
    Timeout,

    #[error(transparent)]
    Io(#[from] io::Error),
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = signal::ctrl_c().await {
            error!(%error, "could not install Ctrl-C handler");
        }
    };

    #[cfg(unix)]
    {
        let terminate = async {
            match signal::unix::signal(signal::unix::SignalKind::terminate()) {
                Ok(mut signal) => {
                    signal.recv().await;
                }
                Err(error) => {
                    error!(%error, "could not install SIGTERM handler");
                }
            }
        };
        tokio::select! {
            _ = ctrl_c => {},
            _ = terminate => {},
        }
    }

    #[cfg(not(unix))]
    ctrl_c.await;

    info!("shutdown signal received");
}

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error(transparent)]
    ScheduledTask(#[from] ScheduledTaskError),

    #[error("could not bind daemon to {address}: {source}")]
    Bind {
        address: SocketAddr,
        #[source]
        source: io::Error,
    },

    #[error("could not read daemon listener address: {0}")]
    LocalAddress(#[source] io::Error),

    #[error("could not read daemon TLS certificate file {path}: {source}")]
    TlsCertificateFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("could not parse daemon TLS certificate file {path}: {source}")]
    TlsCertificatePem {
        path: PathBuf,
        #[source]
        source: tokio_rustls::rustls::pki_types::pem::Error,
    },

    #[error("daemon TLS certificate file {path} contains no certificates")]
    EmptyTlsCertificateChain { path: PathBuf },

    #[error("could not read daemon TLS private key file {path}: {source}")]
    TlsPrivateKeyFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("could not parse daemon TLS private key file {path}: {source}")]
    TlsPrivateKeyPem {
        path: PathBuf,
        #[source]
        source: tokio_rustls::rustls::pki_types::pem::Error,
    },

    #[error("daemon TLS certificate and private key are invalid: {0}")]
    TlsConfiguration(#[source] tokio_rustls::rustls::Error),

    #[error("daemon server failed: {0}")]
    Serve(#[source] io::Error),
}

#[cfg(test)]
mod tests {
    use rcgen::{CertifiedKey, generate_simple_self_signed};
    use tokio::{net::TcpStream, sync::oneshot};

    use super::*;

    #[tokio::test]
    async fn server_starts_and_stops_gracefully() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let state = AppState::new(
            Arc::new(ApplicationService::unconfigured()),
            "a-secure-test-key-with-at-least-32-bytes",
        );
        let (stop, stopped) = oneshot::channel();

        let server = tokio::spawn(serve(listener, state, async move {
            let _ = stopped.await;
        }));
        let connection = TcpStream::connect(address).await.unwrap();
        drop(connection);
        stop.send(()).unwrap();

        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn tls_server_accepts_a_trusted_localhost_certificate() {
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec!["localhost".to_owned(), "127.0.0.1".to_owned()])
                .unwrap();
        let certificate_pem = cert.pem();
        let private_key_pem = signing_key.serialize_pem();
        let directory = tempfile::tempdir().unwrap();
        let certificate_file = directory.path().join("localhost.crt");
        let private_key_file = directory.path().join("localhost.key");
        tokio::fs::write(&certificate_file, &certificate_pem)
            .await
            .unwrap();
        tokio::fs::write(&private_key_file, private_key_pem)
            .await
            .unwrap();
        let tls_config = TlsConfig::new(certificate_file, private_key_file).unwrap();
        let tls_acceptor = load_tls_acceptor(&tls_config).await.unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let state = AppState::new(
            Arc::new(ApplicationService::unconfigured()),
            "a-secure-test-key-with-at-least-32-bytes",
        );
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(serve_tls(listener, tls_acceptor, state, async move {
            let _ = stopped.await;
        }));

        // A client that opens TCP but never starts TLS must not serialize the
        // listener's handshakes and block a healthy client behind it.
        let stalled_connection = TcpStream::connect(address).await.unwrap();
        tokio::task::yield_now().await;

        let certificate = reqwest::Certificate::from_pem(certificate_pem.as_bytes()).unwrap();
        let client = reqwest::Client::builder()
            .no_proxy()
            .tls_certs_only([certificate])
            .build()
            .unwrap();
        let response = timeout(
            Duration::from_secs(2),
            client
                .get(format!("https://localhost:{}/v1/providers", address.port()))
                .send(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

        drop(stalled_connection);
        stop.send(()).unwrap();
        server.await.unwrap().unwrap();
    }
}

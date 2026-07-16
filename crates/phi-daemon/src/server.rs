use std::{future::Future, io, net::SocketAddr, sync::Arc};

use phi::{BuiltinTools, DiskPlanStore, DiskSessionStorage};
use thiserror::Error;
use tokio::{net::TcpListener, signal, sync::oneshot};
use tracing::{error, info, warn};

use crate::{
    api::{self, AppState},
    config::{ConfigError, DaemonConfig},
    runtime::AgentRegistry,
    service::ApplicationService,
    store::{DiskAgentProfileStore, DiskControlStore, DiskProviderStore},
};

const CONTROL_DIRECTORY: &str = "control";
const SESSION_DIRECTORY: &str = "sessions";
const PLAN_DIRECTORY: &str = "plans";
const PROVIDER_CONFIG_FILE: &str = "provider.json";
const AGENT_PROFILE_CONFIG_FILE: &str = "agent-profiles.json";

pub async fn run(config: DaemonConfig) -> Result<(), DaemonError> {
    let service = Arc::new(application_service(&config));
    let state = AppState::new(Arc::clone(&service), config.auth_key());
    let address = config.bind_address();
    let listener = TcpListener::bind(address)
        .await
        .map_err(|source| DaemonError::Bind { address, source })?;
    let local_address = listener.local_addr().map_err(DaemonError::LocalAddress)?;

    info!(
        %local_address,
        data_dir = %config.data_dir().display(),
        workspace_dir = %config.workspace_dir().display(),
        "phi daemon listening"
    );
    let (begin_graceful_shutdown, graceful_shutdown) = oneshot::channel();
    let shutdown_service = Arc::clone(&service);
    let shutdown_task = tokio::spawn(async move {
        shutdown_signal().await;
        // Stop accepting new HTTP/WS connections before draining the actors.
        let _ = begin_graceful_shutdown.send(());
        shutdown_agents(&shutdown_service).await;
    });
    let result = serve(listener, state, async move {
        let _ = graceful_shutdown.await;
    })
    .await;

    if result.is_ok() {
        let _ = shutdown_task.await;
    } else {
        shutdown_task.abort();
    }

    // Also clean up if the HTTP server exits because of an I/O failure before
    // the signal task runs. After a normal signal this is an empty no-op.
    shutdown_agents(&service).await;

    result.map_err(DaemonError::Serve)
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

fn application_service(config: &DaemonConfig) -> ApplicationService {
    let data_dir = config.data_dir();
    let control_store = Arc::new(DiskControlStore::new(data_dir.join(CONTROL_DIRECTORY)));
    let session_storage = Arc::new(DiskSessionStorage::new(data_dir.join(SESSION_DIRECTORY)));
    let plan_store = Arc::new(DiskPlanStore::new(data_dir.join(PLAN_DIRECTORY)));
    let provider_store = Arc::new(DiskProviderStore::new(data_dir.join(PROVIDER_CONFIG_FILE)));
    let agent_profile_store = Arc::new(DiskAgentProfileStore::new(
        data_dir.join(AGENT_PROFILE_CONFIG_FILE),
    ));

    ApplicationService::managed_with_plan_store_profiles_skills_and_builtin_tools(
        AgentRegistry::new(),
        control_store,
        session_storage,
        plan_store,
        provider_store,
        agent_profile_store,
        config.skills_config_template(),
        BuiltinTools::all(config.workspace_dir()),
    )
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

    #[error("could not bind daemon to {address}: {source}")]
    Bind {
        address: SocketAddr,
        #[source]
        source: io::Error,
    },

    #[error("could not read daemon listener address: {0}")]
    LocalAddress(#[source] io::Error),

    #[error("daemon server failed: {0}")]
    Serve(#[source] io::Error),
}

#[cfg(test)]
mod tests {
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
}

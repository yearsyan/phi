use std::{future::Future, io, net::SocketAddr, sync::Arc};

use thiserror::Error;
use tokio::{net::TcpListener, signal};
use tracing::{error, info, warn};

use crate::{
    api::{self, AppState},
    config::{ConfigError, DaemonConfig},
    service::ApplicationService,
};

pub async fn run(config: DaemonConfig) -> Result<(), DaemonError> {
    let address = config.bind_address();
    let listener = TcpListener::bind(address)
        .await
        .map_err(|source| DaemonError::Bind { address, source })?;
    let local_address = listener.local_addr().map_err(DaemonError::LocalAddress)?;
    let service = Arc::new(ApplicationService::unconfigured());
    let state = AppState::new(Arc::clone(&service));

    info!(%local_address, "phi daemon listening; public API routes are not enabled yet");
    let result = serve(listener, state, shutdown_signal()).await;

    for failure in service.shutdown().await {
        warn!(
            session_id = %failure.session_id,
            error = %failure.error,
            "agent actor did not shut down cleanly"
        );
    }

    result.map_err(DaemonError::Serve)
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
        let state = AppState::new(Arc::new(ApplicationService::unconfigured()));
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

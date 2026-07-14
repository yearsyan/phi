use phi_daemon::{config::DaemonConfig, run, telemetry};

#[tokio::main]
async fn main() -> Result<(), phi_daemon::DaemonError> {
    telemetry::init();
    run(DaemonConfig::from_env()?).await
}

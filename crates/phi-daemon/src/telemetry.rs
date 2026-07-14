use tracing_subscriber::EnvFilter;

pub const LOG_FILTER_ENV: &str = "RUST_LOG";
const DEFAULT_LOG_FILTER: &str = "phi_daemon=info";

pub fn init() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

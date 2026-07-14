use std::sync::Arc;

use axum::{Router, http::StatusCode};

use crate::service::ApplicationService;

mod http;
mod ws;

/// Shared dependencies visible to transport handlers.
#[derive(Clone)]
pub struct AppState {
    service: Arc<ApplicationService>,
}

impl AppState {
    pub fn new(service: Arc<ApplicationService>) -> Self {
        Self { service }
    }

    pub fn service(&self) -> &Arc<ApplicationService> {
        &self.service
    }
}

/// Builds the transport shell. HTTP and WebSocket routes are intentionally empty
/// until their public contract is defined.
pub fn router(state: AppState) -> Router {
    Router::<AppState>::new()
        .merge(http::routes())
        .merge(ws::routes())
        .fallback(not_found)
        .with_state(state)
}

async fn not_found() -> StatusCode {
    StatusCode::NOT_FOUND
}

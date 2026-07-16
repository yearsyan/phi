use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router,
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
};

use crate::service::ApplicationService;
use crate::{runtime::AgentFactoryError, service::ServiceError, store::AgentProfileStoreError};

mod auth;
mod dto;
mod http;
mod ws;

use auth::{AuthManager, DEFAULT_WS_TOKEN_TTL};
use dto::ErrorResponse;

/// Shared dependencies visible to transport handlers.
#[derive(Clone)]
pub struct AppState {
    service: Arc<ApplicationService>,
    auth: AuthManager,
}

impl AppState {
    pub fn new(service: Arc<ApplicationService>, auth_key: impl Into<String>) -> Self {
        Self::with_auth_token_ttl(service, auth_key, DEFAULT_WS_TOKEN_TTL)
    }

    pub fn with_auth_token_ttl(
        service: Arc<ApplicationService>,
        auth_key: impl Into<String>,
        auth_token_ttl: Duration,
    ) -> Self {
        Self {
            service,
            auth: AuthManager::new(auth_key, auth_token_ttl),
        }
    }

    pub fn service(&self) -> &Arc<ApplicationService> {
        &self.service
    }

    fn auth(&self) -> &AuthManager {
        &self.auth
    }
}

pub fn router(state: AppState) -> Router {
    let protected_http = Router::<AppState>::new()
        .merge(auth::routes())
        .merge(http::routes())
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_http_auth,
        ));

    Router::<AppState>::new()
        .merge(protected_http)
        .merge(ws::routes())
        .fallback(not_found)
        .with_state(state)
}

async fn not_found() -> StatusCode {
    StatusCode::NOT_FOUND
}

struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            message: message.into(),
        }
    }

    fn service(error: ServiceError) -> Self {
        let (status, code) = match &error {
            ServiceError::SessionNotFound { .. } => (StatusCode::NOT_FOUND, "session_not_found"),
            ServiceError::ProviderManagementUnavailable => (
                StatusCode::NOT_IMPLEMENTED,
                "provider_management_unavailable",
            ),
            ServiceError::AgentProfileManagementUnavailable => (
                StatusCode::NOT_IMPLEMENTED,
                "agent_profile_management_unavailable",
            ),
            ServiceError::AgentProfileStore(AgentProfileStoreError::Validation(_))
            | ServiceError::Factory(
                AgentFactoryError::AgentProfile(_)
                | AgentFactoryError::AgentProfileUnavailable { .. },
            ) => (StatusCode::BAD_REQUEST, "invalid_agent_profile"),
            ServiceError::Factory(
                AgentFactoryError::InvalidProviderConfig { .. } | AgentFactoryError::Provider(_),
            ) => (StatusCode::BAD_REQUEST, "invalid_provider_config"),
            ServiceError::ShuttingDown => (StatusCode::SERVICE_UNAVAILABLE, "daemon_shutting_down"),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
        };
        Self {
            status,
            code,
            message: error.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                code: self.code,
                message: self.message,
            }),
        )
            .into_response()
    }
}

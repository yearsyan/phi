use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router,
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
};
use phi::Workspace;

use crate::service::ApplicationService;
use crate::{
    runtime::{AgentFactoryError, AgentHandleError},
    scheduled_task::{ScheduledTaskError, ScheduledTaskManager},
    service::ServiceError,
    store::AgentProfileStoreError,
};

mod auth;
mod dto;
mod http;
mod scheduled_task;
mod workspace;
mod ws;

use auth::{AuthManager, DEFAULT_WS_TOKEN_TTL};
use dto::ErrorResponse;

/// Shared dependencies visible to transport handlers.
#[derive(Clone)]
pub struct AppState {
    service: Arc<ApplicationService>,
    auth: AuthManager,
    default_workspace: Workspace,
    scheduled_tasks: Option<Arc<ScheduledTaskManager>>,
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
            default_workspace: Workspace::new("."),
            scheduled_tasks: None,
        }
    }

    pub fn with_default_workspace(mut self, workspace: Workspace) -> Self {
        self.default_workspace = workspace;
        self
    }

    pub fn with_scheduled_tasks(mut self, scheduled_tasks: Arc<ScheduledTaskManager>) -> Self {
        self.scheduled_tasks = Some(scheduled_tasks);
        self
    }

    pub fn service(&self) -> &Arc<ApplicationService> {
        &self.service
    }

    fn default_workspace(&self) -> &Workspace {
        &self.default_workspace
    }

    fn auth(&self) -> &AuthManager {
        &self.auth
    }

    fn scheduled_tasks(&self) -> Option<&Arc<ScheduledTaskManager>> {
        self.scheduled_tasks.as_ref()
    }
}

pub fn router(state: AppState) -> Router {
    let protected_http = Router::<AppState>::new()
        .merge(auth::routes())
        .merge(http::routes())
        .merge(scheduled_task::routes())
        .merge(workspace::routes())
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

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    fn service(error: ServiceError) -> Self {
        let (status, code) = match &error {
            ServiceError::SessionNotFound { .. } => (StatusCode::NOT_FOUND, "session_not_found"),
            ServiceError::InvalidForkPoint { .. } => {
                (StatusCode::BAD_REQUEST, "invalid_fork_point")
            }
            ServiceError::Agent(AgentHandleError::Busy { .. }) => {
                (StatusCode::CONFLICT, "session_busy")
            }
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

    fn scheduled_task(error: ScheduledTaskError) -> Self {
        match error {
            ScheduledTaskError::Service(error) => Self::service(error),
            ScheduledTaskError::NotFound { .. } => Self::new(
                StatusCode::NOT_FOUND,
                "scheduled_task_not_found",
                error.to_string(),
            ),
            ScheduledTaskError::InvalidField { .. }
            | ScheduledTaskError::ProviderNotFound { .. }
            | ScheduledTaskError::AgentProfileNotFound { .. } => {
                Self::bad_request("invalid_scheduled_task", error.to_string())
            }
            ScheduledTaskError::RevisionConflict { .. } => Self::new(
                StatusCode::CONFLICT,
                "scheduled_task_revision_conflict",
                error.to_string(),
            ),
            ScheduledTaskError::AlreadyRunning { .. } => Self::new(
                StatusCode::CONFLICT,
                "scheduled_task_already_running",
                error.to_string(),
            ),
            ScheduledTaskError::RunCapacity { .. } | ScheduledTaskError::TaskLimit { .. } => {
                Self::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    "scheduled_task_capacity",
                    error.to_string(),
                )
            }
            ScheduledTaskError::ShuttingDown => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "daemon_shutting_down",
                error.to_string(),
            ),
            ScheduledTaskError::RevisionExhausted { .. } => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "scheduled-task revision could not be advanced",
            ),
            ScheduledTaskError::Store(_) => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "scheduled-task storage failed",
            ),
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

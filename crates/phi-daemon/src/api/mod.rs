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
    output_channel::{OutputChannelError, OutputChannelManager},
    runtime::{AgentFactoryError, AgentHandleError},
    scheduled_task::{ScheduledTaskError, ScheduledTaskManager},
    service::ServiceError,
    store::{AgentProfileStoreError, McpProfileStoreError, OutputChannelStoreError},
};

mod auth;
mod dto;
mod http;
mod output_channel;
mod scheduled_task;
mod web;
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
    output_channels: Option<Arc<OutputChannelManager>>,
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
            output_channels: None,
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

    pub fn with_output_channels(mut self, output_channels: Arc<OutputChannelManager>) -> Self {
        self.output_channels = Some(output_channels);
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

    fn output_channels(&self) -> Option<&Arc<OutputChannelManager>> {
        self.output_channels.as_ref()
    }
}

pub fn router(state: AppState) -> Router {
    let protected_http = Router::<AppState>::new()
        .merge(auth::routes())
        .merge(http::routes())
        .merge(output_channel::routes())
        .merge(scheduled_task::routes())
        .merge(workspace::routes())
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_http_auth,
        ));

    Router::<AppState>::new()
        .merge(protected_http)
        .merge(ws::routes())
        .fallback(web::serve_embedded_web_client)
        .with_state(state)
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
        // Each arm yields `(status, code, redact)`. When `redact` is true the
        // outgoing `message` is replaced with a static generic string and the
        // original error is logged server-side via `tracing::error!`. This is
        // mandatory for any error whose `Display` may carry disk paths,
        // temporary file names, provider response bodies, URLs, or other
        // internal data — see AGENTS.md "Provider 中性边界" and "持久化与安全".
        // Errors whose `Display` is a user-facing validation message keep
        // `redact = false` so callers can render the message.
        let (status, code, redact): (StatusCode, &'static str, bool) = match &error {
            ServiceError::SessionNotFound { .. } => {
                (StatusCode::NOT_FOUND, "session_not_found", false)
            }
            ServiceError::InvalidForkPoint { .. } => {
                (StatusCode::BAD_REQUEST, "invalid_fork_point", false)
            }
            ServiceError::Agent(AgentHandleError::Busy { .. }) => {
                (StatusCode::CONFLICT, "session_busy", false)
            }
            ServiceError::ProviderManagementUnavailable => (
                StatusCode::NOT_IMPLEMENTED,
                "provider_management_unavailable",
                false,
            ),
            ServiceError::AgentProfileManagementUnavailable => (
                StatusCode::NOT_IMPLEMENTED,
                "agent_profile_management_unavailable",
                false,
            ),
            ServiceError::McpProfileManagementUnavailable => (
                StatusCode::NOT_IMPLEMENTED,
                "mcp_profile_management_unavailable",
                false,
            ),
            ServiceError::AgentProfileStore(AgentProfileStoreError::Validation(_))
            | ServiceError::AgentProfileValidation(_)
            | ServiceError::InvalidMcpProfileReference { .. }
            | ServiceError::Factory(
                AgentFactoryError::AgentProfile(_)
                | AgentFactoryError::AgentProfileUnavailable { .. }
                | AgentFactoryError::McpProfileUnavailable { .. }
                | AgentFactoryError::DuplicateMcpToolPrefix { .. }
                | AgentFactoryError::DuplicateMcpToolName { .. },
            ) => (StatusCode::BAD_REQUEST, "invalid_agent_profile", false),
            ServiceError::McpProfileStore(McpProfileStoreError::Validation(_))
            | ServiceError::Factory(AgentFactoryError::McpProfile(_)) => {
                (StatusCode::BAD_REQUEST, "invalid_mcp_profile", false)
            }
            // `McpConnection` wraps `McpError`, whose `Spawn` variant carries
            // an `io::Error` that may expose the stdio MCP server command line.
            ServiceError::Factory(AgentFactoryError::McpConnection { .. }) => {
                (StatusCode::BAD_GATEWAY, "mcp_connection_failed", true)
            }
            // `Provider(_)` wraps `ProviderError::Api { body }` (upstream
            // response body) and `ProviderError::Http(reqwest::Error)` (URL).
            // `InvalidProviderConfig` is a field-validation message and safe.
            ServiceError::Factory(AgentFactoryError::InvalidProviderConfig { .. }) => {
                (StatusCode::BAD_REQUEST, "invalid_provider_config", false)
            }
            ServiceError::Factory(AgentFactoryError::Provider(_)) => {
                (StatusCode::BAD_REQUEST, "invalid_provider_config", true)
            }
            ServiceError::ShuttingDown => (
                StatusCode::SERVICE_UNAVAILABLE,
                "daemon_shutting_down",
                false,
            ),
            // Fallback: store / storage / factory-build / registry errors all
            // carry disk paths or other internal data in their `Display`.
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error", true),
        };
        let message = if redact {
            tracing::error!(error = %error, code, status = %status.as_u16(), "service error redacted for client");
            redacted_message(code)
        } else {
            error.to_string()
        };
        Self {
            status,
            code,
            message,
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
            | ScheduledTaskError::AgentProfileNotFound { .. }
            | ScheduledTaskError::OutputChannelNotFound { .. } => {
                Self::bad_request("invalid_scheduled_task", error.to_string())
            }
            ScheduledTaskError::OutputChannelManagementUnavailable => Self::new(
                StatusCode::NOT_IMPLEMENTED,
                "output_channels_unavailable",
                error.to_string(),
            ),
            ScheduledTaskError::OutputChannel(error) => Self::output_channel(error),
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

    fn output_channel(error: OutputChannelError) -> Self {
        match error {
            OutputChannelError::NotFound { .. } => Self::new(
                StatusCode::NOT_FOUND,
                "output_channel_not_found",
                error.to_string(),
            ),
            OutputChannelError::BotAccountNotFound { .. }
            | OutputChannelError::Store(OutputChannelStoreError::BotAccountNotFound { .. }) => {
                Self::bad_request("bot_account_not_found", error.to_string())
            }
            OutputChannelError::Store(OutputChannelStoreError::Validation(_)) => {
                Self::bad_request("invalid_output_channel", error.to_string())
            }
            OutputChannelError::Delivery(_) => Self::new(
                StatusCode::BAD_GATEWAY,
                "output_channel_delivery_failed",
                error.to_string(),
            ),
            OutputChannelError::Store(_) => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "output channel storage failed",
            ),
        }
    }

    fn bot_account(error: OutputChannelError) -> Self {
        match error {
            OutputChannelError::BotAccountNotFound { .. } => Self::new(
                StatusCode::NOT_FOUND,
                "bot_account_not_found",
                error.to_string(),
            ),
            OutputChannelError::Store(OutputChannelStoreError::Validation(_)) => {
                Self::bad_request("invalid_bot_account", error.to_string())
            }
            OutputChannelError::Store(_) => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "bot account storage failed",
            ),
            OutputChannelError::NotFound { .. } | OutputChannelError::Delivery(_) => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "unexpected bot account operation failed",
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

/// Generic, client-facing message for an error whose `Display` may leak
/// internal data (disk paths, temp file names, provider response bodies,
/// URLs, command lines). The original error must be logged server-side via
/// `tracing::error!` by the caller; this string never discloses internals.
///
/// The message is keyed by the stable error `code` so clients that branch on
/// `code` keep their semantics, while the human-readable text stays generic.
pub(crate) fn redacted_message(code: &str) -> String {
    match code {
        "mcp_connection_failed" => "could not connect to MCP server".to_owned(),
        "invalid_provider_config" => "provider configuration is invalid or unreachable".to_owned(),
        "internal_error" => "daemon operation failed".to_owned(),
        _ => "daemon operation failed".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::{io, path::PathBuf};

    use axum::http::StatusCode;
    use phi::{ProviderError, StorageError};

    use super::ApiError;
    use crate::{
        runtime::{AgentFactoryError, SessionId},
        service::ServiceError,
        store::ControlStoreError,
    };

    /// Asserts the `message` field of an [`ApiError`] does not leak any of the
    /// given substrings. Use this to lock down the redaction contract: errors
    /// whose `Display` carries disk paths, temp file names, provider response
    /// bodies, or other internal data must be replaced with a generic message
    /// before reaching the wire.
    fn assert_no_leaks(api_error: &ApiError, forbidden: &[&str]) {
        for needle in forbidden {
            assert!(
                !api_error.message.contains(needle),
                "error code `{}` leaked internal data in message: {:?} contains {:?}",
                api_error.code,
                api_error.message,
                needle,
            );
        }
    }

    #[test]
    fn redacts_control_store_io_path_on_internal_error() {
        let secret_path = "/secret/.phi/daemon/sessions/abc-12345";
        let error = ServiceError::Store(ControlStoreError::Io {
            path: PathBuf::from(secret_path),
            source: io::Error::other("os-level-xyz-disk-failure"),
        });
        let api_error = ApiError::service(error);
        assert_eq!(api_error.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(api_error.code, "internal_error");
        assert_eq!(api_error.message, "daemon operation failed");
        // Forbidden substrings are chosen so they cannot collide with the
        // generic message ("daemon operation failed") but still cover each
        // piece of internal data the original `Display` carried.
        assert_no_leaks(
            &api_error,
            &[
                "/secret",
                ".phi",
                "sessions",
                "abc-12345",
                "os-level-xyz-disk-failure",
                "control store I/O failed",
            ],
        );
    }

    #[test]
    fn redacts_provider_error_body_on_invalid_provider_config() {
        // A provider API error whose body contains sensitive upstream data.
        let error = ServiceError::Factory(AgentFactoryError::Provider(ProviderError::Api {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: "secret-upstream-body-with-endpoint-info".to_owned(),
        }));
        let api_error = ApiError::service(error);
        assert_eq!(api_error.status, StatusCode::BAD_REQUEST);
        assert_eq!(api_error.code, "invalid_provider_config");
        assert_eq!(
            api_error.message,
            "provider configuration is invalid or unreachable"
        );
        assert_no_leaks(
            &api_error,
            &[
                "secret-upstream-body",
                "endpoint-info",
                "provider API returned",
            ],
        );
    }

    #[test]
    fn redacts_storage_workspace_mismatch_paths_on_internal_error() {
        let error = StorageError::WorkspaceMismatch {
            session_id: "sess-secret-987".to_owned(),
            stored: PathBuf::from("/very/secret/stored/path"),
            requested: Some(PathBuf::from("/very/secret/requested")),
        };
        let api_error = ApiError::service(error.into());
        assert_eq!(api_error.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(api_error.code, "internal_error");
        assert_eq!(api_error.message, "daemon operation failed");
        assert_no_leaks(
            &api_error,
            &[
                "/very/secret",
                "stored/path",
                "requested",
                "sess-secret-987",
                "is bound to workspace",
            ],
        );
    }

    #[test]
    fn keeps_user_facing_session_not_found_message_unredacted() {
        // Errors whose Display is a user-facing validation message must keep
        // the original text — clients may render it and it carries no internal
        // data (the session id is a client-provided UUID).
        let session_id = SessionId::from_uuid(uuid::Uuid::now_v7());
        let error = ServiceError::SessionNotFound { session_id };
        let api_error = ApiError::service(error);
        assert_eq!(api_error.status, StatusCode::NOT_FOUND);
        assert_eq!(api_error.code, "session_not_found");
        assert!(api_error.message.contains(&session_id.to_string()));
    }
}

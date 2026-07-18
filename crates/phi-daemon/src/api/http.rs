use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
    routing::{get, post},
};

use super::{
    ApiError, AppState,
    dto::{
        AgentProfileResponse, AgentProfilesResponse, ForkSessionRequest, ProviderResponse,
        ProvidersResponse, PublicAgentProfile, PublicProviderConfig, PutAgentProfileRequest,
        PutProviderRequest, SessionSummaryDto, SessionsResponse, SkillDiagnosticDto,
        SkillSummaryDto, SkillsResponse, UpdateSessionRequest,
    },
};
use crate::{runtime::SessionId, store::DEFAULT_PROFILE_ID};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/provider", get(get_provider).put(put_provider))
        .route("/v1/providers", get(list_providers))
        .route("/v1/agent-profiles", get(list_agent_profiles))
        .route(
            "/v1/agent-profiles/{agent_profile_id}",
            get(get_agent_profile).put(put_agent_profile),
        )
        .route(
            "/v1/providers/{profile_id}",
            get(get_provider_by_id).put(put_provider_by_id),
        )
        .route("/v1/sessions", get(list_sessions))
        .route("/v1/sessions/{session_id}/skills", get(get_session_skills))
        .route("/v1/sessions/{session_id}/fork", post(fork_session))
        .route(
            "/v1/sessions/{session_id}",
            get(get_session)
                .patch(update_session)
                .delete(delete_session),
        )
}

async fn get_provider(State(state): State<AppState>) -> Result<Json<ProviderResponse>, ApiError> {
    let config = state
        .service()
        .provider_config()
        .await
        .map_err(ApiError::service)?;
    Ok(Json(ProviderResponse::from_config(
        DEFAULT_PROFILE_ID,
        config.as_ref(),
    )))
}

async fn put_provider(
    State(state): State<AppState>,
    Json(request): Json<PutProviderRequest>,
) -> Result<Json<ProviderResponse>, ApiError> {
    let config = state
        .service()
        .configure_provider(request.into())
        .await
        .map_err(ApiError::service)?;
    Ok(Json(ProviderResponse::from_config(
        DEFAULT_PROFILE_ID,
        Some(&config),
    )))
}

async fn list_providers(
    State(state): State<AppState>,
) -> Result<Json<ProvidersResponse>, ApiError> {
    let providers = state
        .service()
        .provider_configs()
        .await
        .map_err(ApiError::service)?
        .iter()
        .map(PublicProviderConfig::from)
        .collect();
    Ok(Json(ProvidersResponse { providers }))
}

async fn get_provider_by_id(
    State(state): State<AppState>,
    Path(profile_id): Path<String>,
) -> Result<Json<ProviderResponse>, ApiError> {
    let config = state
        .service()
        .provider_config_for(&profile_id)
        .await
        .map_err(ApiError::service)?;
    Ok(Json(ProviderResponse::from_config(
        profile_id.trim(),
        config.as_ref(),
    )))
}

async fn put_provider_by_id(
    State(state): State<AppState>,
    Path(profile_id): Path<String>,
    Json(request): Json<PutProviderRequest>,
) -> Result<Json<ProviderResponse>, ApiError> {
    let config = state
        .service()
        .configure_provider_for(&profile_id, request.into())
        .await
        .map_err(ApiError::service)?;
    Ok(Json(ProviderResponse::from_config(
        profile_id.trim(),
        Some(&config),
    )))
}

async fn list_agent_profiles(
    State(state): State<AppState>,
) -> Result<Json<AgentProfilesResponse>, ApiError> {
    let agent_profiles = state
        .service()
        .agent_profiles()
        .await
        .map_err(ApiError::service)?
        .into_iter()
        .map(PublicAgentProfile::from)
        .collect();
    Ok(Json(AgentProfilesResponse { agent_profiles }))
}

async fn get_agent_profile(
    State(state): State<AppState>,
    Path(agent_profile_id): Path<String>,
) -> Result<Json<AgentProfileResponse>, ApiError> {
    let profile = state
        .service()
        .agent_profile(&agent_profile_id)
        .await
        .map_err(ApiError::service)?;
    Ok(Json(AgentProfileResponse::from_profile(profile)))
}

async fn put_agent_profile(
    State(state): State<AppState>,
    Path(agent_profile_id): Path<String>,
    request: Result<Json<PutAgentProfileRequest>, JsonRejection>,
) -> Result<Json<AgentProfileResponse>, ApiError> {
    let Json(request) = request
        .map_err(|error| ApiError::bad_request("invalid_agent_profile", error.body_text()))?;
    let profile = state
        .service()
        .configure_agent_profile(&agent_profile_id, request.into())
        .await
        .map_err(ApiError::service)?;
    Ok(Json(AgentProfileResponse::from_profile(Some(profile))))
}

async fn list_sessions(State(state): State<AppState>) -> Result<Json<SessionsResponse>, ApiError> {
    let sessions = state
        .service()
        .list_sessions()
        .await
        .map_err(ApiError::service)?
        .into_iter()
        .map(SessionSummaryDto::from)
        .collect();
    Ok(Json(SessionsResponse::from_sessions(sessions)))
}

async fn get_session(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
) -> Result<Json<SessionSummaryDto>, ApiError> {
    let session = state
        .service()
        .get_session(session_id)
        .await
        .map_err(ApiError::service)?;
    Ok(Json(SessionSummaryDto::from(session)))
}

async fn update_session(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    request: Result<Json<UpdateSessionRequest>, JsonRejection>,
) -> Result<Json<SessionSummaryDto>, ApiError> {
    let Json(request) = request
        .map_err(|error| ApiError::bad_request("invalid_session_update", error.body_text()))?;
    let session = state
        .service()
        .set_session_pinned(session_id, request.pinned)
        .await
        .map_err(ApiError::service)?;
    Ok(Json(SessionSummaryDto::from(session)))
}

async fn delete_session(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
) -> Result<StatusCode, ApiError> {
    state
        .service()
        .delete_session(session_id)
        .await
        .map_err(ApiError::service)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn fork_session(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    request: Result<Json<ForkSessionRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<SessionSummaryDto>), ApiError> {
    let Json(request) = request
        .map_err(|error| ApiError::bad_request("invalid_session_fork", error.body_text()))?;
    let session = state
        .service()
        .fork_session(session_id, request.message_index, request.position.into())
        .await
        .map_err(ApiError::service)?;
    Ok((StatusCode::CREATED, Json(SessionSummaryDto::from(session))))
}

async fn get_session_skills(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
) -> Result<Json<SkillsResponse>, ApiError> {
    let catalog = state
        .service()
        .session_skills(session_id)
        .await
        .map_err(ApiError::service)?;
    Ok(Json(SkillsResponse {
        session_id,
        skills: catalog.skills().iter().map(SkillSummaryDto::from).collect(),
        diagnostics: catalog
            .diagnostics()
            .iter()
            .map(SkillDiagnosticDto::from)
            .collect(),
    }))
}

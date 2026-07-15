use axum::{
    Json, Router,
    extract::{Path, State},
    routing::get,
};

use super::{
    ApiError, AppState,
    dto::{
        ProviderResponse, ProvidersResponse, PublicProviderConfig, PutProviderRequest,
        SessionSummaryDto, SessionsResponse, SkillDiagnosticDto, SkillSummaryDto, SkillsResponse,
    },
};
use crate::{runtime::SessionId, store::DEFAULT_PROFILE_ID};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/provider", get(get_provider).put(put_provider))
        .route("/v1/providers", get(list_providers))
        .route(
            "/v1/providers/{profile_id}",
            get(get_provider_by_id).put(put_provider_by_id),
        )
        .route("/v1/sessions", get(list_sessions))
        .route("/v1/sessions/{session_id}/skills", get(get_session_skills))
        .route("/v1/sessions/{session_id}", get(get_session))
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

async fn list_sessions(State(state): State<AppState>) -> Result<Json<SessionsResponse>, ApiError> {
    let sessions = state
        .service()
        .list_sessions()
        .await
        .map_err(ApiError::service)?
        .into_iter()
        .map(SessionSummaryDto::from)
        .collect();
    Ok(Json(SessionsResponse { sessions }))
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

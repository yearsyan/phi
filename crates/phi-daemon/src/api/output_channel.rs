use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
    routing::{get, post},
};

use super::{
    ApiError, AppState,
    dto::{
        OutputChannelResponse, OutputChannelsResponse, PublicOutputChannel, PutOutputChannelRequest,
    },
};
use crate::output_channel::OutputChannelManager;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/output-channels", get(list_output_channels))
        .route(
            "/v1/output-channels/{output_channel_id}",
            get(get_output_channel).put(put_output_channel),
        )
        .route(
            "/v1/output-channels/{output_channel_id}/test",
            post(test_output_channel),
        )
}

async fn list_output_channels(
    State(state): State<AppState>,
) -> Result<Json<OutputChannelsResponse>, ApiError> {
    let output_channels = manager(&state)?
        .list_channels()
        .await
        .map_err(ApiError::output_channel)?
        .into_iter()
        .map(PublicOutputChannel::from)
        .collect();
    Ok(Json(OutputChannelsResponse { output_channels }))
}

async fn get_output_channel(
    State(state): State<AppState>,
    Path(output_channel_id): Path<String>,
) -> Result<Json<OutputChannelResponse>, ApiError> {
    let channel = manager(&state)?
        .get_channel(&output_channel_id)
        .await
        .map_err(ApiError::output_channel)?;
    Ok(Json(OutputChannelResponse::from_channel(channel)))
}

async fn put_output_channel(
    State(state): State<AppState>,
    Path(output_channel_id): Path<String>,
    request: Result<Json<PutOutputChannelRequest>, JsonRejection>,
) -> Result<Json<OutputChannelResponse>, ApiError> {
    let Json(request) = request.map_err(|_| {
        ApiError::bad_request(
            "invalid_output_channel",
            "invalid output channel request body",
        )
    })?;
    let channel = manager(&state)?
        .configure_channel(&output_channel_id, request.into())
        .await
        .map_err(ApiError::output_channel)?;
    Ok(Json(OutputChannelResponse::from_channel(Some(channel))))
}

async fn test_output_channel(
    State(state): State<AppState>,
    Path(output_channel_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    manager(&state)?
        .test_channel(&output_channel_id)
        .await
        .map_err(ApiError::output_channel)?;
    Ok(StatusCode::NO_CONTENT)
}

fn manager(state: &AppState) -> Result<Arc<OutputChannelManager>, ApiError> {
    state.output_channels().cloned().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "output_channels_unavailable",
            "output channel management is unavailable for this embedded daemon",
        )
    })
}

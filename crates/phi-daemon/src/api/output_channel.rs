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
        BotAccountResponse, BotAccountsResponse, OutputChannelResponse, OutputChannelsResponse,
        PublicBotAccount, PublicOutputChannel, PutBotAccountRequest, PutOutputChannelRequest,
    },
};
use crate::output_channel::{OutputChannelDefinition, OutputChannelManager};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/bot-accounts", get(list_bot_accounts))
        .route(
            "/v1/bot-accounts/{bot_account_id}",
            get(get_bot_account).put(put_bot_account),
        )
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

async fn list_bot_accounts(
    State(state): State<AppState>,
) -> Result<Json<BotAccountsResponse>, ApiError> {
    let bot_accounts = manager(&state)?
        .list_bot_accounts()
        .await
        .map_err(ApiError::bot_account)?
        .into_iter()
        .map(PublicBotAccount::from)
        .collect();
    Ok(Json(BotAccountsResponse { bot_accounts }))
}

async fn get_bot_account(
    State(state): State<AppState>,
    Path(bot_account_id): Path<String>,
) -> Result<Json<BotAccountResponse>, ApiError> {
    let account = manager(&state)?
        .get_bot_account(&bot_account_id)
        .await
        .map_err(ApiError::bot_account)?;
    Ok(Json(BotAccountResponse::from_account(account)))
}

async fn put_bot_account(
    State(state): State<AppState>,
    Path(bot_account_id): Path<String>,
    request: Result<Json<PutBotAccountRequest>, JsonRejection>,
) -> Result<Json<BotAccountResponse>, ApiError> {
    let Json(request) = request.map_err(|_| {
        ApiError::bad_request("invalid_bot_account", "invalid bot account request body")
    })?;
    let account = manager(&state)?
        .configure_bot_account(&bot_account_id, request.into())
        .await
        .map_err(ApiError::bot_account)?;
    Ok(Json(BotAccountResponse::from_account(Some(account))))
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
    let manager = manager(&state)?;
    let channel = match request {
        PutOutputChannelRequest::Telegram {
            bot_account_id: Some(bot_account_id),
            bot_token: None,
            chat_id,
        } => {
            manager
                .configure_channel(
                    &output_channel_id,
                    OutputChannelDefinition::Telegram {
                        bot_account_id,
                        chat_id,
                    },
                )
                .await
        }
        PutOutputChannelRequest::Telegram {
            bot_account_id: None,
            bot_token: Some(bot_token),
            chat_id,
        } => {
            manager
                .configure_legacy_telegram_channel(&output_channel_id, bot_token, chat_id)
                .await
        }
        PutOutputChannelRequest::Telegram { .. } => {
            return Err(ApiError::bad_request(
                "invalid_output_channel",
                "Telegram output target must contain exactly one of bot_account_id or bot_token",
            ));
        }
    }
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

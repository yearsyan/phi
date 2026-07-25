use std::{fmt, sync::Arc, time::Duration};

use async_trait::async_trait;
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::store::{OutputChannelStore, OutputChannelStoreError};

pub const MAX_OUTPUT_CHANNEL_ID_BYTES: usize = 64;
const MAX_TELEGRAM_BOT_TOKEN_BYTES: usize = 512;
const MAX_TELEGRAM_CHAT_ID_BYTES: usize = 256;
const MAX_TELEGRAM_MESSAGE_CHARS: usize = 4_096;
const TELEGRAM_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const TELEGRAM_API_BASE_URL: &str = "https://api.telegram.org";

/// Secret-bearing configuration for one daemon output destination.
///
/// Public HTTP responses use a separate redacted DTO. In particular, the
/// Telegram bot token must never appear in Debug output or a GET response.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OutputChannelDefinition {
    Telegram { bot_token: String, chat_id: String },
}

impl fmt::Debug for OutputChannelDefinition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Telegram { chat_id, .. } => formatter
                .debug_struct("Telegram")
                .field("bot_token", &"[REDACTED]")
                .field("chat_id", chat_id)
                .finish(),
        }
    }
}

impl OutputChannelDefinition {
    pub fn normalized(&self) -> Result<Self, OutputChannelValidationError> {
        match self {
            Self::Telegram { bot_token, chat_id } => Ok(Self::Telegram {
                bot_token: normalize_telegram_bot_token(bot_token)?,
                chat_id: normalize_telegram_chat_id(chat_id)?,
            }),
        }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputChannel {
    pub output_channel_id: String,
    pub revision: u64,
    pub definition: OutputChannelDefinition,
}

impl fmt::Debug for OutputChannel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OutputChannel")
            .field("output_channel_id", &self.output_channel_id)
            .field("revision", &self.revision)
            .field("definition", &self.definition)
            .finish()
    }
}

impl OutputChannel {
    pub fn normalized(&self) -> Result<Self, OutputChannelValidationError> {
        validate_output_channel_id(&self.output_channel_id)?;
        if self.revision == 0 {
            return Err(invalid("revision", "must be greater than zero"));
        }
        Ok(Self {
            output_channel_id: self.output_channel_id.clone(),
            revision: self.revision,
            definition: self.definition.normalized()?,
        })
    }
}

pub fn validate_output_channel_id(
    output_channel_id: &str,
) -> Result<(), OutputChannelValidationError> {
    if output_channel_id.is_empty() {
        return Err(invalid("output_channel_id", "must not be empty"));
    }
    if output_channel_id != output_channel_id.trim() {
        return Err(invalid(
            "output_channel_id",
            "must not have surrounding whitespace",
        ));
    }
    if output_channel_id.len() > MAX_OUTPUT_CHANNEL_ID_BYTES {
        return Err(invalid(
            "output_channel_id",
            format!("must not exceed {MAX_OUTPUT_CHANNEL_ID_BYTES} bytes"),
        ));
    }
    if !output_channel_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(invalid(
            "output_channel_id",
            "must contain only ASCII letters, digits, '_' or '-'",
        ));
    }
    Ok(())
}

fn normalize_telegram_bot_token(value: &str) -> Result<String, OutputChannelValidationError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid("bot_token", "must not be empty"));
    }
    if value.len() > MAX_TELEGRAM_BOT_TOKEN_BYTES {
        return Err(invalid(
            "bot_token",
            format!("must not exceed {MAX_TELEGRAM_BOT_TOKEN_BYTES} bytes"),
        ));
    }
    let Some((bot_id, secret)) = value.split_once(':') else {
        return Err(invalid("bot_token", "must use Telegram bot-token format"));
    };
    if bot_id.is_empty()
        || !bot_id.bytes().all(|byte| byte.is_ascii_digit())
        || secret.len() < 20
        || !secret
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(invalid("bot_token", "must use Telegram bot-token format"));
    }
    Ok(value.to_owned())
}

fn normalize_telegram_chat_id(value: &str) -> Result<String, OutputChannelValidationError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid("chat_id", "must not be empty"));
    }
    if value.len() > MAX_TELEGRAM_CHAT_ID_BYTES {
        return Err(invalid(
            "chat_id",
            format!("must not exceed {MAX_TELEGRAM_CHAT_ID_BYTES} bytes"),
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(invalid("chat_id", "must not contain control characters"));
    }
    Ok(value.to_owned())
}

fn invalid(field: &'static str, message: impl Into<String>) -> OutputChannelValidationError {
    OutputChannelValidationError::InvalidField {
        field,
        message: message.into(),
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum OutputChannelValidationError {
    #[error("invalid output channel field {field}: {message}")]
    InvalidField {
        field: &'static str,
        message: String,
    },
}

#[async_trait]
pub trait OutputChannelSender: Send + Sync {
    async fn send(
        &self,
        definition: &OutputChannelDefinition,
        message: &str,
    ) -> Result<(), OutputChannelDeliveryError>;
}

/// Sends output-channel messages through Telegram's Bot API.
///
/// Transport errors are intentionally collapsed into credential-safe variants:
/// a `reqwest::Error` can contain the request URL, and Telegram places the bot
/// token in that URL.
#[derive(Clone)]
pub struct TelegramOutputChannelSender {
    client: reqwest::Client,
    api_base_url: String,
}

impl TelegramOutputChannelSender {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            api_base_url: TELEGRAM_API_BASE_URL.to_owned(),
        }
    }

    #[cfg(test)]
    fn with_api_base_url(client: reqwest::Client, api_base_url: impl Into<String>) -> Self {
        Self {
            client,
            api_base_url: api_base_url.into(),
        }
    }
}

impl fmt::Debug for TelegramOutputChannelSender {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TelegramOutputChannelSender")
            .field("api_base_url", &self.api_base_url)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize)]
struct TelegramSendMessage<'a> {
    chat_id: &'a str,
    text: &'a str,
}

#[derive(Deserialize)]
struct TelegramResponse {
    ok: bool,
    #[serde(default)]
    description: Option<String>,
}

#[async_trait]
impl OutputChannelSender for TelegramOutputChannelSender {
    async fn send(
        &self,
        definition: &OutputChannelDefinition,
        message: &str,
    ) -> Result<(), OutputChannelDeliveryError> {
        let definition = definition
            .normalized()
            .map_err(OutputChannelDeliveryError::InvalidConfiguration)?;
        let OutputChannelDefinition::Telegram { bot_token, chat_id } = definition;
        if message.is_empty() {
            return Err(OutputChannelDeliveryError::InvalidMessage(
                "message must not be empty".to_owned(),
            ));
        }
        if message.chars().count() > MAX_TELEGRAM_MESSAGE_CHARS {
            return Err(OutputChannelDeliveryError::InvalidMessage(format!(
                "message must not exceed {MAX_TELEGRAM_MESSAGE_CHARS} characters"
            )));
        }

        let body = serde_json::to_vec(&TelegramSendMessage {
            chat_id: &chat_id,
            text: message,
        })
        .map_err(|_| OutputChannelDeliveryError::RequestEncoding)?;
        let url = format!(
            "{}/bot{bot_token}/sendMessage",
            self.api_base_url.trim_end_matches('/')
        );
        let response = self
            .client
            .post(url)
            .header(CONTENT_TYPE, "application/json")
            .timeout(TELEGRAM_REQUEST_TIMEOUT)
            .body(body)
            .send()
            .await
            .map_err(|_| OutputChannelDeliveryError::Transport)?;
        let status = response.status().as_u16();
        let bytes = response
            .bytes()
            .await
            .map_err(|_| OutputChannelDeliveryError::ResponseRead)?;
        let telegram = serde_json::from_slice::<TelegramResponse>(&bytes).ok();
        if !(200..300).contains(&status) || !telegram.as_ref().is_some_and(|body| body.ok) {
            let description = telegram
                .and_then(|body| body.description)
                .map(|description| sanitize_remote_description(&description, &bot_token));
            return Err(OutputChannelDeliveryError::Rejected {
                status,
                description,
            });
        }
        Ok(())
    }
}

fn sanitize_remote_description(description: &str, bot_token: &str) -> String {
    const MAX_DESCRIPTION_CHARS: usize = 500;
    let redacted = description.replace(bot_token, "[REDACTED]");
    let mut sanitized = redacted
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(MAX_DESCRIPTION_CHARS)
        .collect::<String>();
    if redacted.chars().count() > MAX_DESCRIPTION_CHARS {
        sanitized.push('…');
    }
    sanitized
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum OutputChannelDeliveryError {
    #[error(transparent)]
    InvalidConfiguration(OutputChannelValidationError),

    #[error("invalid output channel message: {0}")]
    InvalidMessage(String),

    #[error("could not encode the Telegram request")]
    RequestEncoding,

    #[error("could not connect to the Telegram API")]
    Transport,

    #[error("could not read the Telegram API response")]
    ResponseRead,

    #[error("Telegram rejected the message with HTTP {status}{detail}", detail = .description.as_ref().map(|value| format!(": {value}")).unwrap_or_default())]
    Rejected {
        status: u16,
        description: Option<String>,
    },
}

#[derive(Clone)]
pub struct OutputChannelManager {
    store: Arc<dyn OutputChannelStore>,
    sender: Arc<dyn OutputChannelSender>,
}

impl OutputChannelManager {
    pub fn new(store: Arc<dyn OutputChannelStore>, sender: Arc<dyn OutputChannelSender>) -> Self {
        Self { store, sender }
    }

    pub async fn list_channels(&self) -> Result<Vec<OutputChannel>, OutputChannelError> {
        Ok(self.store.list_output_channels().await?)
    }

    pub async fn get_channel(
        &self,
        output_channel_id: &str,
    ) -> Result<Option<OutputChannel>, OutputChannelError> {
        Ok(self.store.get_output_channel(output_channel_id).await?)
    }

    pub async fn configure_channel(
        &self,
        output_channel_id: &str,
        definition: OutputChannelDefinition,
    ) -> Result<OutputChannel, OutputChannelError> {
        Ok(self
            .store
            .replace_output_channel(output_channel_id, definition)
            .await?)
    }

    pub async fn send(
        &self,
        output_channel_id: &str,
        message: &str,
    ) -> Result<(), OutputChannelError> {
        let channel = self
            .store
            .get_output_channel(output_channel_id)
            .await?
            .ok_or_else(|| OutputChannelError::NotFound {
                output_channel_id: output_channel_id.to_owned(),
            })?;
        self.sender.send(&channel.definition, message).await?;
        Ok(())
    }

    pub async fn test_channel(&self, output_channel_id: &str) -> Result<(), OutputChannelError> {
        self.send(
            output_channel_id,
            "Phi output channel test succeeded. Scheduled-task notifications are enabled.",
        )
        .await
    }
}

#[derive(Debug, Error)]
pub enum OutputChannelError {
    #[error("output channel {output_channel_id:?} was not found")]
    NotFound { output_channel_id: String },

    #[error(transparent)]
    Store(#[from] OutputChannelStoreError),

    #[error(transparent)]
    Delivery(#[from] OutputChannelDeliveryError),
}

#[cfg(test)]
mod tests {
    use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
    use serde_json::{Value, json};
    use tokio::sync::{Mutex, oneshot};

    use super::*;

    fn telegram_definition(token: &str) -> OutputChannelDefinition {
        OutputChannelDefinition::Telegram {
            bot_token: token.to_owned(),
            chat_id: "-1001234567890".to_owned(),
        }
    }

    #[test]
    fn telegram_definition_normalizes_and_redacts_bot_token() {
        let token = "123456789:abcdefghijklmnopqrstuvwxyz_ABCDEFG";
        let definition = telegram_definition(&format!(" {token} "))
            .normalized()
            .unwrap();
        let debug = format!("{definition:?}");
        assert!(debug.contains("-1001234567890"));
        assert!(!debug.contains(token));
    }

    #[test]
    fn output_channel_ids_and_telegram_credentials_are_validated() {
        assert!(validate_output_channel_id("alerts").is_ok());
        assert!(validate_output_channel_id("bad channel").is_err());
        assert!(telegram_definition("not-a-token").normalized().is_err());
        assert!(
            OutputChannelDefinition::Telegram {
                bot_token: "123456789:abcdefghijklmnopqrstuvwxyz_ABCDEFG".to_owned(),
                chat_id: " \n ".to_owned(),
            }
            .normalized()
            .is_err()
        );
    }

    #[test]
    fn remote_descriptions_are_bounded_and_single_line() {
        let token = "123456789:abcdefghijklmnopqrstuvwxyz_ABCDEFG";
        let description = format!("bad\n{token}{}", "x".repeat(600));
        let sanitized = sanitize_remote_description(&description, token);
        assert!(!sanitized.contains('\n'));
        assert!(!sanitized.contains(token));
        assert!(sanitized.contains("[REDACTED]"));
        assert!(sanitized.ends_with('…'));
        assert_eq!(sanitized.chars().count(), 501);
    }

    #[test]
    fn sender_debug_does_not_contain_credentials() {
        let sender = TelegramOutputChannelSender::with_api_base_url(
            reqwest::Client::new(),
            "http://127.0.0.1:1234",
        );
        assert_eq!(
            format!("{sender:?}"),
            "TelegramOutputChannelSender { api_base_url: \"http://127.0.0.1:1234\", .. }"
        );
    }

    #[tokio::test]
    async fn telegram_sender_posts_json_and_redacts_remote_token_echoes() {
        let token = "123456789:abcdefghijklmnopqrstuvwxyz_ABCDEFG";
        let received = Arc::new(Mutex::new(None));
        let state = (Arc::clone(&received), token.to_owned());
        let app = Router::new()
            .route(
                &format!("/bot{token}/sendMessage"),
                post(
                    |State((received, token)): State<(Arc<Mutex<Option<Value>>>, String)>,
                     Json(body): Json<Value>| async move {
                        *received.lock().await = Some(body);
                        (
                            StatusCode::UNAUTHORIZED,
                            Json(json!({
                                "ok": false,
                                "description": format!("rejected {token}\n")
                            })),
                        )
                    },
                ),
            )
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });

        let sender = TelegramOutputChannelSender::with_api_base_url(
            reqwest::Client::new(),
            format!("http://{address}"),
        );
        let error = sender
            .send(&telegram_definition(token), "hello")
            .await
            .unwrap_err();
        let rendered = error.to_string();
        assert!(rendered.contains("[REDACTED]"));
        assert!(!rendered.contains(token));
        assert_eq!(
            *received.lock().await,
            Some(json!({
                "chat_id": "-1001234567890",
                "text": "hello"
            }))
        );

        let _ = stop.send(());
        server.await.unwrap();
    }
}

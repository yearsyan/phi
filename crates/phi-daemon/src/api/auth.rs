use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::post,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Serialize;
use subtle::ConstantTimeEq;

use super::{AppState, dto::ErrorResponse};

pub(super) const DEFAULT_WS_TOKEN_TTL: Duration = Duration::from_secs(60);
pub(super) const WS_PROTOCOL: &str = "phi.v1";
pub(super) const WS_AUTH_PROTOCOL_PREFIX: &str = "phi.auth.";
const WS_TOKEN_BYTES: usize = 32;
const MAX_PENDING_WS_TOKENS: usize = 4096;

#[derive(Clone)]
pub(super) struct AuthManager {
    inner: Arc<AuthInner>,
}

struct AuthInner {
    key: Box<str>,
    ws_token_ttl: Duration,
    max_pending_tokens: usize,
    pending_tokens: Mutex<HashMap<String, Instant>>,
}

impl AuthManager {
    pub(super) fn new(key: impl Into<String>, ws_token_ttl: Duration) -> Self {
        Self::with_capacity(key, ws_token_ttl, MAX_PENDING_WS_TOKENS)
    }

    fn with_capacity(
        key: impl Into<String>,
        ws_token_ttl: Duration,
        max_pending_tokens: usize,
    ) -> Self {
        let key = key.into();
        assert!(!key.is_empty(), "daemon auth key must not be empty");
        assert!(
            !ws_token_ttl.is_zero(),
            "WebSocket token TTL must not be zero"
        );
        assert!(
            max_pending_tokens > 0,
            "pending WebSocket token capacity must not be zero"
        );
        Self {
            inner: Arc::new(AuthInner {
                key: key.into_boxed_str(),
                ws_token_ttl,
                max_pending_tokens,
                pending_tokens: Mutex::new(HashMap::new()),
            }),
        }
    }

    fn authorize_http(&self, headers: &HeaderMap) -> bool {
        let mut values = headers.get_all(header::AUTHORIZATION).iter();
        let Some(value) = values.next() else {
            return false;
        };
        if values.next().is_some() {
            return false;
        }
        let Ok(value) = value.to_str() else {
            return false;
        };
        let Some(presented) = value.strip_prefix("Bearer ") else {
            return false;
        };
        if presented.is_empty() || presented.bytes().any(|byte| byte.is_ascii_whitespace()) {
            return false;
        }
        constant_time_eq(presented.as_bytes(), self.inner.key.as_bytes())
    }

    fn issue_ws_token(&self) -> Result<IssuedToken, TokenIssueError> {
        let now = Instant::now();
        let expires_at = now
            .checked_add(self.inner.ws_token_ttl)
            .ok_or(TokenIssueError::RandomnessUnavailable)?;

        for _ in 0..4 {
            let mut random = [0_u8; WS_TOKEN_BYTES];
            getrandom::fill(&mut random).map_err(|_| TokenIssueError::RandomnessUnavailable)?;
            let token = URL_SAFE_NO_PAD.encode(random);

            let mut pending = self
                .inner
                .pending_tokens
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            pending.retain(|_, expiry| *expiry > now);
            if pending.len() >= self.inner.max_pending_tokens {
                return Err(TokenIssueError::CapacityExceeded);
            }
            if pending.insert(token.clone(), expires_at).is_none() {
                return Ok(IssuedToken {
                    token,
                    expires_in_secs: self.inner.ws_token_ttl.as_secs(),
                });
            }
        }

        Err(TokenIssueError::RandomnessUnavailable)
    }

    pub(super) fn consume_ws_token(&self, token: &str) -> bool {
        if token.len() != base64::encoded_len(WS_TOKEN_BYTES, false).unwrap_or(usize::MAX) {
            return false;
        }
        let now = Instant::now();
        let mut pending = self
            .inner
            .pending_tokens
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pending
            .remove(token)
            .is_some_and(|expires_at| expires_at > now)
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && bool::from(left.ct_eq(right))
}

#[derive(Debug, Eq, PartialEq)]
enum TokenIssueError {
    CapacityExceeded,
    RandomnessUnavailable,
}

struct IssuedToken {
    token: String,
    expires_in_secs: u64,
}

#[derive(Serialize)]
struct TokenResponse {
    token: String,
    token_type: &'static str,
    protocol: &'static str,
    expires_in_secs: u64,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new().route("/v1/auth/token", post(issue_token))
}

pub(super) async fn require_http_auth(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if state.auth().authorize_http(request.headers()) {
        next.run(request).await
    } else {
        unauthorized_response()
    }
}

async fn issue_token(State(state): State<AppState>) -> Response {
    match state.auth().issue_ws_token() {
        Ok(issued) => (
            [
                (header::CACHE_CONTROL, "no-store"),
                (header::PRAGMA, "no-cache"),
            ],
            Json(TokenResponse {
                token: issued.token,
                token_type: "websocket_subprotocol",
                protocol: WS_PROTOCOL,
                expires_in_secs: issued.expires_in_secs,
            }),
        )
            .into_response(),
        Err(TokenIssueError::CapacityExceeded) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "token_capacity_exceeded",
            "too many pending WebSocket tokens",
        ),
        Err(TokenIssueError::RandomnessUnavailable) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "token_generation_failed",
            "could not generate a WebSocket token",
        ),
    }
}

pub(super) fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
        Json(ErrorResponse {
            code: "unauthorized",
            message: "authentication failed".to_owned(),
        }),
    )
        .into_response()
}

fn error_response(status: StatusCode, code: &'static str, message: &'static str) -> Response {
    (
        status,
        Json(ErrorResponse {
            code,
            message: message.to_owned(),
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    const KEY: &str = "a-secure-test-key-with-at-least-32-bytes";

    fn authorization(value: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static(value));
        headers
    }

    #[test]
    fn authorization_is_strict_and_does_not_accept_wrong_keys() {
        let auth = AuthManager::new(KEY, DEFAULT_WS_TOKEN_TTL);
        assert!(auth.authorize_http(&authorization(
            "Bearer a-secure-test-key-with-at-least-32-bytes"
        )));
        assert!(!auth.authorize_http(&HeaderMap::new()));
        assert!(!auth.authorize_http(&authorization(
            "Bearer a-secure-test-key-with-at-least-32-byteX"
        )));
        assert!(!auth.authorize_http(&authorization(
            "bearer a-secure-test-key-with-at-least-32-bytes"
        )));
        assert!(!auth.authorize_http(&authorization(
            "Bearer  a-secure-test-key-with-at-least-32-bytes"
        )));
        let mut duplicate = authorization("Bearer a-secure-test-key-with-at-least-32-bytes");
        duplicate.append(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer a-secure-test-key-with-at-least-32-bytes"),
        );
        assert!(!auth.authorize_http(&duplicate));
    }

    #[test]
    fn websocket_token_is_url_safe_and_single_use() {
        let auth = AuthManager::new(KEY, DEFAULT_WS_TOKEN_TTL);
        let issued = auth.issue_ws_token().unwrap();
        assert_eq!(issued.expires_in_secs, 60);
        assert!(
            issued
                .token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        );
        assert!(auth.consume_ws_token(&issued.token));
        assert!(!auth.consume_ws_token(&issued.token));
    }

    #[test]
    fn expired_websocket_token_is_rejected() {
        let auth = AuthManager::new(KEY, Duration::from_millis(1));
        let issued = auth.issue_ws_token().unwrap();
        std::thread::sleep(Duration::from_millis(5));
        assert!(!auth.consume_ws_token(&issued.token));
    }

    #[test]
    fn pending_token_pool_is_bounded_and_expired_entries_are_reclaimed() {
        let bounded = AuthManager::with_capacity(KEY, Duration::from_secs(1), 1);
        let _first = bounded.issue_ws_token().unwrap();
        assert!(matches!(
            bounded.issue_ws_token(),
            Err(TokenIssueError::CapacityExceeded)
        ));

        let reclaiming = AuthManager::with_capacity(KEY, Duration::from_millis(1), 1);
        let _expired = reclaiming.issue_ws_token().unwrap();
        std::thread::sleep(Duration::from_millis(5));
        assert!(reclaiming.issue_ws_token().is_ok());
    }
}

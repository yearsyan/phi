use axum::Router;

use super::AppState;

/// WebSocket subscription and command routes will be assembled here.
pub(super) fn routes() -> Router<AppState> {
    Router::new()
}

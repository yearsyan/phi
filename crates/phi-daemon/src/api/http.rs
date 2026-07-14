use axum::Router;

use super::AppState;

/// HTTP management routes will be assembled here.
pub(super) fn routes() -> Router<AppState> {
    Router::new()
}

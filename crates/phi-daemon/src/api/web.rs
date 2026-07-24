use std::borrow::Cow;

use axum::{
    body::Body,
    extract::OriginalUri,
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use rust_embed::{EmbeddedFile, RustEmbed};

/// The `web/` Vite build output, embedded into the daemon binary at compile
/// time (Go `embed.FS` style). Rebuild `phi-daemon` after `pnpm build` to pick
/// up new assets; `build.rs` writes a placeholder page when `web/dist` does
/// not exist yet.
#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct EmbeddedWebClient;

/// Fallback for every request that did not match an API route. `/v1` keeps
/// plain API 404 semantics; everything else is served from the embedded web
/// client with a SPA fallback to `index.html`.
pub async fn serve_embedded_web_client(method: Method, OriginalUri(uri): OriginalUri) -> Response {
    let path = uri.path();
    if path == "/v1" || path.starts_with("/v1/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }

    let trimmed = path.trim_start_matches('/');
    let key = if trimmed.is_empty() {
        "index.html"
    } else {
        trimmed
    };
    if let Some(file) = EmbeddedWebClient::get(key) {
        return embedded_response(key, &file);
    }

    // Asset-like paths (hashed bundles, images, fonts, ...) must 404 instead
    // of silently returning the SPA shell.
    let looks_like_file = key
        .rsplit('/')
        .next()
        .is_some_and(|name| name.contains('.'));
    if looks_like_file {
        return StatusCode::NOT_FOUND.into_response();
    }

    match EmbeddedWebClient::get("index.html") {
        Some(file) => embedded_response("index.html", &file),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn embedded_response(path: &str, file: &EmbeddedFile) -> Response {
    // Vite emits content-hashed files under `assets/`; everything else is
    // revalidated so a redeployed shell is picked up immediately.
    let cache_control = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let content_type = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();
    let body = match &file.data {
        Cow::Borrowed(bytes) => Body::from(*bytes),
        Cow::Owned(bytes) => Body::from(bytes.clone()),
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, cache_control.to_owned()),
        ],
        body,
    )
        .into_response()
}

//! Static-asset router for the Data Browser UI.
//!
//! Files live under `static/` at the repo root. `rust-embed` bakes them
//! into the release binary; in debug builds (no `debug-embed` feature) it
//! reads from disk so the frontend can be live-edited without recompile.
//!
//! Routes:
//!   - `GET /`              → `static/index.html`
//!   - `GET /assets/*path`  → `static/<path>`
//!
//! `path` deliberately excludes `/api/*` — the axum router matches the
//! `/api/...` routes first, so UI routes never shadow the API.

use axum::body::Body;
use axum::extract::Path;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use rust_embed::RustEmbed;

use super::state::SharedState;

#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/", get(serve_index))
        .route("/assets/{*path}", get(serve_asset))
}

async fn serve_index() -> Response {
    respond_with("index.html")
}

async fn serve_asset(Path(path): Path<String>) -> Response {
    respond_with(&path)
}

fn respond_with(path: &str) -> Response {
    match Assets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            let ctype = HeaderValue::from_str(mime.as_ref())
                .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, ctype)
                .body(Body::from(file.data.into_owned()))
                .expect("valid response")
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

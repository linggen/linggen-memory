//! Standard response envelope — every handler returns `{ok, data}` on
//! success or `{ok:false, error, code}` on failure.
//!
//! The shape is the spec's HTTP dispatch contract (see
//! `../../linggen/doc/memory-spec.md`, § HTTP dispatch contract). Keeping
//! it uniform lets Linggen's dispatcher parse every provider the same way.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};

/// Emit a success envelope `{ok:true, data}` with a given status code.
pub fn ok<T: Serialize>(data: T) -> Response {
    (StatusCode::OK, Json(json!({"ok": true, "data": data}))).into_response()
}

/// Machine-readable error codes. Kept short and uppercase so the
/// dispatcher can surface them in `provider error [CODE]: MSG`.
#[derive(Debug, Clone, Copy)]
pub enum ErrorCode {
    BadRequest,
    NotFound,
    Internal,
}

impl ErrorCode {
    fn status(self) -> StatusCode {
        match self {
            ErrorCode::BadRequest => StatusCode::BAD_REQUEST,
            ErrorCode::NotFound => StatusCode::NOT_FOUND,
            ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn code(self) -> &'static str {
        match self {
            ErrorCode::BadRequest => "BAD_REQUEST",
            ErrorCode::NotFound => "NOT_FOUND",
            ErrorCode::Internal => "INTERNAL",
        }
    }
}

/// Error returned from any handler. Converts to the standard error
/// envelope with the matching HTTP status code.
#[derive(Debug)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
}

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::BadRequest,
            message: msg.into(),
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::NotFound,
            message: msg.into(),
        }
    }

    pub fn internal(err: anyhow::Error) -> Self {
        Self {
            code: ErrorCode::Internal,
            message: format!("{err:#}"),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body: Value = json!({
            "ok": false,
            "error": self.message,
            "code": self.code.code(),
        });
        (self.code.status(), Json(body)).into_response()
    }
}

/// Map an `anyhow::Error` into a 500 response. Used for store errors
/// that don't have a narrower classification.
impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        ApiError::internal(err)
    }
}

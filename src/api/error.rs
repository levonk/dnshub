//! API error types and `IntoResponse` implementation.
//!
//! All route handlers return `Result<axum::response::Response, ApiError>`.
//! The [`IntoResponse`] impl converts an [`ApiError`] into a JSON error
//! body with the appropriate HTTP status code, so handlers can use `?`
//! on fallible operations and let axum render the error.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

/// Error returned by API route handlers.
#[derive(Debug)]
pub enum ApiError {
    /// The requested resource was not found.
    NotFound(String),
    /// The request body or parameters were invalid.
    BadRequest(String),
    /// A backend store operation failed (SQLite, LMDB, etc.).
    Internal(String),
    /// The requested subsystem (e.g. DHCP lease store) is not configured.
    Unavailable(String),
    /// Config validation failed on a PUT /config request.
    ConfigValidation(Vec<String>),
}

impl ApiError {
    /// Map this error to an HTTP [`StatusCode`].
    fn status_code(&self) -> StatusCode {
        match self {
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::ConfigValidation(_) => StatusCode::BAD_REQUEST,
        }
    }

    /// Human-readable error message.
    fn message(&self) -> String {
        match self {
            ApiError::NotFound(msg) => format!("not found: {msg}"),
            ApiError::BadRequest(msg) => format!("bad request: {msg}"),
            ApiError::Internal(msg) => format!("internal error: {msg}"),
            ApiError::Unavailable(msg) => format!("subsystem unavailable: {msg}"),
            ApiError::ConfigValidation(errs) => {
                format!("config validation failed: {}", errs.join("; "))
            }
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for ApiError {}

/// JSON error body returned to the client.
#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Vec<String>>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            error: self.message(),
            details: match &self {
                ApiError::ConfigValidation(errs) => Some(errs.clone()),
                _ => None,
            },
        };
        let status = self.status_code();
        tracing::warn!(status = %status, error = %body.error, "API error");
        (status, Json(body)).into_response()
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        ApiError::Internal(format!("sqlite: {e}"))
    }
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        ApiError::Internal(format!("io: {e}"))
    }
}

impl From<crate::config::ConfigError> for ApiError {
    fn from(e: crate::config::ConfigError) -> Self {
        match e {
            crate::config::ConfigError::Validation(errs) => ApiError::ConfigValidation(errs),
            other => ApiError::Internal(format!("config: {other}")),
        }
    }
}

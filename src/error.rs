use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("upstream error ({status}): {body}")]
    Upstream { status: u16, body: String },
    #[error("no available upstream: {0}")]
    NoUpstream(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl AppError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Upstream { status, .. } => {
                StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY)
            }
            AppError::NoUpstream(_) => StatusCode::SERVICE_UNAVAILABLE,
            AppError::Internal(_) | AppError::Db(_) | AppError::Json(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let message = self.to_string();
        let body = Json(json!({
            "error": {
                "message": message,
                "type": "thinrouter_error",
            }
        }));
        (status, body).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;

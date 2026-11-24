use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

pub struct ApiError(pub StatusCode, pub String);
pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn invalid(error: impl std::fmt::Display) -> Self {
        Self(StatusCode::BAD_REQUEST, error.to_string())
    }
    pub fn conflict(error: impl std::fmt::Display) -> Self {
        Self(StatusCode::CONFLICT, error.to_string())
    }
    pub fn missing() -> Self {
        Self(StatusCode::NOT_FOUND, "Job not found".into())
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        eprintln!("Internal API error: {error:#}");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal operation failed; inspect server diagnostics".into(),
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(json!({"error":{"code":self.0.as_u16(),"message":self.1}})),
        )
            .into_response()
    }
}

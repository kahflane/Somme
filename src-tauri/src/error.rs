use serde::Serialize;
use somme_application::AppError;

#[derive(Debug, Serialize)]
pub struct ApiError {
    kind: &'static str,
    message: String,
}

impl ApiError {
    pub fn io(message: impl Into<String>) -> Self {
        Self {
            kind: "io",
            message: message.into(),
        }
    }
}

impl From<std::io::Error> for ApiError {
    fn from(err: std::io::Error) -> Self {
        Self::io(err.to_string())
    }
}

impl From<AppError> for ApiError {
    fn from(err: AppError) -> Self {
        Self {
            kind: err.kind(),
            message: err.to_string(),
        }
    }
}

impl From<uuid::Error> for ApiError {
    fn from(err: uuid::Error) -> Self {
        Self {
            kind: "validation",
            message: err.to_string(),
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

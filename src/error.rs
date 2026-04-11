use thiserror::Error;

#[derive(Debug, Error)]
pub enum HttpClientError {
    #[error("request failed: {0}")]
    RequestError(#[from] reqwest::Error),

    #[error("deserialization failed: {0}")]
    DeserializationError(#[from] serde_json::Error),

    #[error("API error (status {status}): {body}")]
    ApiError { status: u16, body: String },

    #[error("builder error: {0}")]
    BuilderError(String),
}

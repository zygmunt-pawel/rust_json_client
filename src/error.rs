use reqwest::StatusCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HttpClientError {
    #[error("request failed: {0}")]
    RequestError(#[from] reqwest::Error),

    #[error("serialization failed: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("deserialization failed: {0}")]
    DeserializationError(serde_json::Error),

    #[error("invalid URL: {0}")]
    UrlError(#[from] url::ParseError),

    #[error("invalid request path: {0}")]
    InvalidRequestPath(String),

    #[error("successful response body was empty")]
    EmptyResponseBody,

    // `body` contains a bounded preview of the upstream error payload, not the
    // full response body. This keeps `Display` useful for debugging while
    // avoiding unbounded buffering and reducing the chance of dumping large
    // upstream payloads straight into logs.
    #[error("API error (status {status}): {body}")]
    ApiError {
        status: StatusCode,
        body: String,
        retry_after: Option<std::time::Duration>,
    },

    #[error("response body exceeded limit ({received} > {limit} bytes)")]
    ResponseTooLarge { limit: usize, received: usize },
}

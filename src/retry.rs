use bon::bon;
use getset::{CopyGetters, Getters};
use reqwest::StatusCode;
use std::num::NonZeroU32;
use std::time::Duration;

use crate::HttpClientError;

const DEFAULT_RETRYABLE_STATUS_CODES: [StatusCode; 5] = [
    StatusCode::TOO_MANY_REQUESTS,
    StatusCode::INTERNAL_SERVER_ERROR,
    StatusCode::BAD_GATEWAY,
    StatusCode::SERVICE_UNAVAILABLE,
    StatusCode::GATEWAY_TIMEOUT,
];

#[derive(Debug, Clone, CopyGetters, Getters)]
pub struct RetryPolicy {
    #[getset(get_copy = "pub")]
    max_attempts: NonZeroU32,
    #[getset(get_copy = "pub")]
    base_delay: Duration,
    #[getset(get_copy = "pub")]
    max_delay: Duration,
    #[getset(get = "pub")]
    retryable_status_codes: Vec<StatusCode>,
}

#[bon]
impl RetryPolicy {
    #[builder]
    pub fn new(
        #[builder(default = NonZeroU32::new(3).expect("default retry attempts must be non-zero"))]
        max_attempts: NonZeroU32,
        #[builder(default = Duration::from_secs(1))] base_delay: Duration,
        #[builder(default = Duration::from_secs(30))] max_delay: Duration,
        #[builder(default = DEFAULT_RETRYABLE_STATUS_CODES.to_vec())]
        retryable_status_codes: Vec<StatusCode>,
    ) -> Self {
        Self {
            max_attempts,
            base_delay,
            max_delay,
            retryable_status_codes,
        }
    }
    pub fn is_retryable(&self, err: &HttpClientError) -> bool {
        match err {
            // Retry only transient transport failures by default. This keeps retry
            // useful for flaky networks without pointlessly repeating permanent
            // failures such as TLS/redirect/configuration problems.
            HttpClientError::RequestError(err) => err.is_timeout() || err.is_connect(),
            HttpClientError::ApiError { status, .. } => {
                self.retryable_status_codes.contains(status)
            }
            HttpClientError::SerializationError(_)
            | HttpClientError::DeserializationError(_)
            | HttpClientError::UrlError(_)
            | HttpClientError::InvalidRequestPath(_)
            | HttpClientError::EmptyResponseBody
            | HttpClientError::ResponseTooLarge { .. } => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_values() {
        let policy = RetryPolicy::builder().build();
        assert_eq!(policy.max_attempts().get(), 3);
        assert_eq!(policy.base_delay(), Duration::from_secs(1));
        assert_eq!(policy.max_delay(), Duration::from_secs(30));
        assert_eq!(policy.retryable_status_codes(), &DEFAULT_RETRYABLE_STATUS_CODES);
    }

    #[test]
    fn builder_rejects_zero_attempts() {
        assert!(NonZeroU32::new(0).is_none());
    }

    #[test]
    fn retryable_api_errors() {
        let policy = RetryPolicy::builder().build();

        let retryable = HttpClientError::ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            body: String::new(),
            retry_after: None,
        };
        let not_retryable = HttpClientError::ApiError {
            status: StatusCode::BAD_REQUEST,
            body: String::new(),
            retry_after: None,
        };

        assert!(policy.is_retryable(&retryable));
        assert!(!policy.is_retryable(&not_retryable));
    }

    #[test]
    fn deserialization_error_not_retryable() {
        let policy = RetryPolicy::builder().build();
        let err = HttpClientError::DeserializationError(
            serde_json::from_str::<String>("not json").unwrap_err(),
        );
        assert!(!policy.is_retryable(&err));
    }
}

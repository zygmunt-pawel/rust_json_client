use backon::{BackoffBuilder, ExponentialBuilder};
use bon::bon;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, RETRY_AFTER};
use reqwest::{Client, Method, Response, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::time::{Duration, Instant};
use tracing::{debug, instrument, warn};
use url::Url;

use crate::HttpClientError;
use crate::retry::RetryPolicy;

const DEFAULT_MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;
const DEFAULT_POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const DEFAULT_POOL_MAX_IDLE_PER_HOST: usize = 64;
const ERROR_BODY_PREVIEW_BYTES: usize = 8 * 1024;
const TRUNCATED_BODY_SUFFIX: &str = "... [truncated]";

// This client is intentionally scoped to JSON APIs.
// Successful responses are expected to contain JSON bodies, with empty bodies
// supported only for callers that explicitly deserialize into `()` or
// `Option<T>`.
pub struct HttpClient {
    client: Client,
    base_url: Url,
    retry_policy: Option<RetryPolicy>,
    max_response_bytes: usize,
    #[cfg_attr(not(test), allow(dead_code))]
    pool_idle_timeout: Duration,
    #[cfg_attr(not(test), allow(dead_code))]
    pool_max_idle_per_host: usize,
}

const _: () = {
    #[allow(dead_code)]
    fn assert_send_sync<T: Send + Sync>() {}
    #[allow(dead_code)]
    fn check() {
        assert_send_sync::<HttpClient>();
    }
};

#[must_use = "request is not sent until .send() is called"]
pub struct RequestBuilder<'a> {
    client: &'a HttpClient,
    method: Method,
    path: &'a str,
    json_body: Option<Vec<u8>>,
    retry_policy: Option<RetryPolicy>,
}

#[bon]
impl HttpClient {
    #[builder]
    pub fn new(
        base_url: Url,
        #[builder(default)] default_headers: HeaderMap,
        retry_policy: Option<RetryPolicy>,
        #[builder(default = DEFAULT_MAX_RESPONSE_BYTES)] max_response_bytes: usize,
        #[builder(default = DEFAULT_POOL_IDLE_TIMEOUT)] pool_idle_timeout: Duration,
        #[builder(default = DEFAULT_POOL_MAX_IDLE_PER_HOST)] pool_max_idle_per_host: usize,
        #[builder(default = Duration::from_secs(5))] connect_timeout: Duration,
        #[builder(default = Duration::from_secs(30))] request_timeout: Duration,
    ) -> Self {
        let scheme = base_url.scheme();
        assert!(
            scheme == "https" || scheme == "http",
            "base_url must use http or https scheme, got: {scheme}"
        );

        let base_url = Self::normalize_base_url(base_url);
        let mut headers = default_headers;
        headers
            .entry(ACCEPT)
            .or_insert("application/json".parse().unwrap());

        let builder = Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .pool_idle_timeout(pool_idle_timeout)
            .pool_max_idle_per_host(pool_max_idle_per_host)
            .connect_timeout(connect_timeout)
            .timeout(request_timeout)
            .tcp_keepalive(Duration::from_secs(30))
            .tcp_nodelay(true);

        // Fail fast during startup: in this monolith a broken reqwest/TLS setup is
        // considered a fatal environment problem rather than a recoverable user error.
        let client = builder.build().expect("failed to build reqwest client");

        Self {
            client,
            base_url,
            retry_policy,
            max_response_bytes,
            pool_idle_timeout,
            pool_max_idle_per_host,
        }
    }

    pub fn get<'a>(&'a self, path: &'a str) -> RequestBuilder<'a> {
        RequestBuilder {
            client: self,
            method: Method::GET,
            path,
            json_body: None,
            retry_policy: self.retry_policy.clone(),
        }
    }

    pub fn post<'a, T: Serialize>(
        &'a self,
        path: &'a str,
        body: &T,
    ) -> Result<RequestBuilder<'a>, HttpClientError> {
        Ok(RequestBuilder {
            client: self,
            method: Method::POST,
            path,
            json_body: Some(serde_json::to_vec(body)?),
            retry_policy: None,
        })
    }

    async fn execute_with_retry<F, Fut, R>(
        &self,
        retry_policy: Option<&RetryPolicy>,
        operation: F,
    ) -> Result<R, HttpClientError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<R, HttpClientError>>,
    {
        let Some(policy) = retry_policy else {
            return operation().await;
        };

        let mut backoff = ExponentialBuilder::default()
            .with_jitter()
            .with_min_delay(policy.base_delay())
            .with_max_delay(policy.max_delay())
            .build();

        let max_attempts = policy.max_attempts().get() as usize;

        let mut last_err = operation().await.err();
        for attempt in 1..max_attempts {
            let err = match last_err.take() {
                Some(err) if policy.is_retryable(&err) => err,
                Some(err) => return Err(err),
                None => unreachable!(),
            };

            let backoff_delay = backoff.next().unwrap_or(policy.max_delay());
            let delay = match &err {
                HttpClientError::ApiError {
                    retry_after: Some(retry_after),
                    ..
                } => backoff_delay.max(*retry_after),
                _ => backoff_delay,
            };

            warn!(
                retry = attempt,
                delay_ms = delay.as_millis() as u64,
                error = %err,
                "request failed, scheduling retry"
            );

            tokio::time::sleep(delay).await;

            match operation().await {
                Ok(result) => return Ok(result),
                Err(err) => last_err = Some(err),
            }
        }

        Err(last_err.unwrap())
    }

    async fn check_error_status(
        response: Response,
        max_response_bytes: usize,
    ) -> Result<Response, HttpClientError> {
        let status = response.status();

        if !status.is_success() {
            let retry_after = if status == StatusCode::TOO_MANY_REQUESTS {
                Self::parse_retry_after(&response)
            } else {
                None
            };
            let body = Self::read_error_body_preview(response, max_response_bytes).await?;
            warn!(status = %status, "received non-success HTTP response");
            return Err(HttpClientError::ApiError {
                status,
                body,
                retry_after,
            });
        }

        Ok(response)
    }

    async fn handle_json_response<R: DeserializeOwned>(
        response: Response,
        max_response_bytes: usize,
    ) -> Result<R, HttpClientError> {
        let response = Self::check_error_status(response, max_response_bytes).await?;
        let status = response.status();

        if let Some(content_length) = response.content_length()
            && content_length > max_response_bytes as u64
        {
            warn!(
                status = %status,
                limit = max_response_bytes,
                received = content_length,
                "response exceeded configured size limit"
            );
            return Err(HttpClientError::ResponseTooLarge {
                limit: max_response_bytes,
                received: content_length.try_into().unwrap_or(usize::MAX),
            });
        }

        let bytes = Self::read_response_body_limited(response, max_response_bytes).await?;
        let parsed = Self::deserialize_success_body(&bytes)?;
        debug!(status = %status, bytes = bytes.len(), "successfully decoded JSON response");
        Ok(parsed)
    }

    async fn handle_sse_response<R: DeserializeOwned>(
        mut response: Response,
        max_response_bytes: usize,
    ) -> Result<Vec<R>, HttpClientError> {
        response = Self::check_error_status(response, max_response_bytes).await?;

        let mut chunks = Vec::new();
        let mut byte_buf: Vec<u8> = Vec::new();
        let mut received = 0usize;

        while let Some(chunk) = response.chunk().await? {
            received += chunk.len();

            if received > max_response_bytes {
                warn!(
                    limit = max_response_bytes,
                    received, "SSE stream exceeded configured size limit"
                );
                return Err(HttpClientError::ResponseTooLarge {
                    limit: max_response_bytes,
                    received,
                });
            }

            byte_buf.extend_from_slice(&chunk);

            if Self::process_sse_byte_lines(&mut byte_buf, &mut chunks)? {
                debug!(
                    chunks = chunks.len(),
                    bytes = received,
                    "SSE stream received [DONE]"
                );
                return Ok(chunks);
            }
        }

        // Process any remaining data in the buffer (last line without trailing \n).
        if !byte_buf.is_empty() {
            byte_buf.push(b'\n');
            Self::process_sse_byte_lines(&mut byte_buf, &mut chunks)?;
        }

        debug!(
            chunks = chunks.len(),
            bytes = received,
            "SSE stream ended without [DONE]"
        );
        Ok(chunks)
    }

    /// Parses complete lines from `byte_buf`, deserializes `data:` payloads into
    /// `chunks`, and returns `Ok(true)` when the `[DONE]` sentinel is encountered.
    ///
    /// Operates on raw bytes to avoid corrupting multi-byte UTF-8 characters that
    /// may be split across network chunk boundaries. Only complete lines (delimited
    /// by `\n`) are decoded to UTF-8.
    fn process_sse_byte_lines<R: DeserializeOwned>(
        byte_buf: &mut Vec<u8>,
        chunks: &mut Vec<R>,
    ) -> Result<bool, HttpClientError> {
        while let Some(newline_pos) = byte_buf.iter().position(|&b| b == b'\n') {
            let line_bytes = &byte_buf[..newline_pos];

            // Trim \r for servers that send \r\n line endings.
            let line_bytes = line_bytes.strip_suffix(b"\r").unwrap_or(line_bytes);

            // Trim leading/trailing ASCII whitespace.
            let line_bytes = line_bytes.trim_ascii();

            if line_bytes.is_empty() || line_bytes[0] == b':' {
                byte_buf.drain(..=newline_pos);
                continue;
            }

            let data_bytes = if let Some(rest) = line_bytes.strip_prefix(b"data: ") {
                rest
            } else if let Some(rest) = line_bytes.strip_prefix(b"data:") {
                rest
            } else {
                byte_buf.drain(..=newline_pos);
                continue;
            };

            let data_bytes = data_bytes.trim_ascii();

            if data_bytes == b"[DONE]" {
                byte_buf.drain(..=newline_pos);
                return Ok(true);
            }

            let parsed: R = serde_json::from_slice(data_bytes)
                .map_err(HttpClientError::DeserializationError)?;
            chunks.push(parsed);
            byte_buf.drain(..=newline_pos);
        }

        Ok(false)
    }

    fn deserialize_success_body<R: DeserializeOwned>(bytes: &[u8]) -> Result<R, HttpClientError> {
        if bytes.iter().all(|byte| byte.is_ascii_whitespace()) {
            return serde_json::from_slice(b"null").map_err(|_| HttpClientError::EmptyResponseBody);
        }

        serde_json::from_slice(bytes).map_err(HttpClientError::DeserializationError)
    }

    fn normalize_base_url(mut base_url: Url) -> Url {
        if !base_url.path().ends_with('/') {
            let normalized_path = format!("{}/", base_url.path());
            base_url.set_path(&normalized_path);
        }

        base_url
    }

    fn build_request_url(base_url: &Url, path: &str) -> Result<Url, HttpClientError> {
        let trimmed = path.trim();

        if Self::looks_like_absolute_url(trimmed) {
            warn!(path = %Self::loggable_path(trimmed), "rejected absolute URL in request path");
            return Err(HttpClientError::InvalidRequestPath(
                "path must be relative to base_url".to_string(),
            ));
        }

        if trimmed.contains('#') {
            warn!(path = %Self::loggable_path(trimmed), "rejected fragment in request path");
            return Err(HttpClientError::InvalidRequestPath(
                "path must not contain a fragment".to_string(),
            ));
        }

        let (raw_path, raw_query) = match trimmed.split_once('?') {
            Some((raw_path, raw_query)) => (raw_path, Some(raw_query)),
            None => (trimmed, None),
        };

        if raw_path
            .trim_start_matches('/')
            .split('/')
            .filter(|segment| !segment.is_empty())
            .any(Self::is_forbidden_path_segment)
        {
            warn!(path = %Self::loggable_path(trimmed), "rejected dot segment in request path");
            return Err(HttpClientError::InvalidRequestPath(
                "path must not contain '.' or '..' segments".to_string(),
            ));
        }

        let mut url = base_url.clone();
        let relative_path = raw_path.trim_start_matches('/');

        if relative_path.is_empty() {
            url.set_path(base_url.path());
        } else {
            let combined_path = format!("{}{}", base_url.path(), relative_path);
            url.set_path(&combined_path);
        }

        url.set_query(raw_query);
        url.set_fragment(None);

        Ok(url)
    }

    fn looks_like_absolute_url(path: &str) -> bool {
        if path.starts_with("//") {
            return true;
        }

        let Some((scheme, _)) = path.split_once("://") else {
            return false;
        };

        let mut chars = scheme.chars();
        matches!(chars.next(), Some(ch) if ch.is_ascii_alphabetic())
            && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
    }

    fn is_forbidden_path_segment(segment: &str) -> bool {
        let normalized = segment.to_ascii_lowercase();
        matches!(
            normalized.as_str(),
            "." | ".." | "%2e" | ".%2e" | "%2e." | "%2e%2e"
        )
    }

    fn loggable_path(path: &str) -> &str {
        path.split(['?', '#']).next().unwrap_or(path)
    }

    async fn read_response_body_limited(
        mut response: Response,
        max_response_bytes: usize,
    ) -> Result<Vec<u8>, HttpClientError> {
        let capacity = response
            .content_length()
            .and_then(|len| usize::try_from(len).ok())
            .map(|len| len.min(max_response_bytes))
            .unwrap_or(0);
        let mut body = Vec::with_capacity(capacity);
        let mut received = 0usize;

        while let Some(chunk) = response.chunk().await? {
            received += chunk.len();

            if received > max_response_bytes {
                warn!(
                    limit = max_response_bytes,
                    received, "streamed response exceeded configured size limit"
                );
                return Err(HttpClientError::ResponseTooLarge {
                    limit: max_response_bytes,
                    received,
                });
            }

            body.extend_from_slice(&chunk);
        }

        Ok(body)
    }

    async fn read_error_body_preview(
        mut response: Response,
        max_response_bytes: usize,
    ) -> Result<String, HttpClientError> {
        let preview_limit = max_response_bytes.min(ERROR_BODY_PREVIEW_BYTES);
        let mut body = Vec::new();
        let mut truncated = false;

        while let Some(chunk) = response.chunk().await? {
            let remaining = preview_limit.saturating_sub(body.len());

            if remaining == 0 {
                truncated = true;
                break;
            }

            if chunk.len() > remaining {
                body.extend_from_slice(&chunk[..remaining]);
                truncated = true;
                break;
            }

            body.extend_from_slice(&chunk);
        }

        if truncated {
            Self::truncate_incomplete_utf8_suffix(&mut body);
        }

        let mut preview = String::from_utf8_lossy(&body).into_owned();

        if truncated {
            preview.push_str(TRUNCATED_BODY_SUFFIX);
        }

        Ok(preview)
    }

    fn parse_retry_after(response: &Response) -> Option<Duration> {
        let value = response.headers().get(RETRY_AFTER)?;
        let secs: u64 = value.to_str().ok()?.trim().parse().ok()?;
        Some(Duration::from_secs(secs))
    }

    fn truncate_incomplete_utf8_suffix(bytes: &mut Vec<u8>) {
        if let Err(err) = std::str::from_utf8(bytes)
            && err.error_len().is_none()
        {
            bytes.truncate(err.valid_up_to());
        }
    }
}

impl<'a> RequestBuilder<'a> {
    pub fn with_retry(mut self, retry_policy: RetryPolicy) -> Self {
        self.retry_policy = Some(retry_policy);
        self
    }

    #[instrument(
        name = "http_client.send",
        skip_all,
        fields(
            method = %self.method,
            path = %HttpClient::loggable_path(self.path),
            retry_enabled = self.retry_policy.is_some()
        )
    )]
    pub async fn send<R: DeserializeOwned>(self) -> Result<R, HttpClientError> {
        let retry_policy = self.retry_policy.as_ref();
        let url = HttpClient::build_request_url(&self.client.base_url, self.path)?;
        let started_at = Instant::now();
        debug!("sending HTTP request");

        let result = self
            .client
            .execute_with_retry(retry_policy, || async {
                let mut request = self.client.client.request(self.method.clone(), url.clone());

                if let Some(body) = &self.json_body {
                    request = request
                        .header(CONTENT_TYPE, "application/json")
                        .body(body.clone());
                }

                let response = request.send().await?;
                HttpClient::handle_json_response(response, self.client.max_response_bytes).await
            })
            .await;

        let elapsed_ms = started_at.elapsed().as_millis() as u64;

        match &result {
            Ok(_) => debug!(elapsed_ms, "request completed successfully"),
            Err(err) => warn!(elapsed_ms, error = %err, "request failed"),
        }

        result
    }

    #[instrument(
        name = "http_client.send_sse",
        skip_all,
        fields(
            method = %self.method,
            path = %HttpClient::loggable_path(self.path),
            retry_enabled = self.retry_policy.is_some()
        )
    )]
    pub async fn send_sse<R: DeserializeOwned>(self) -> Result<Vec<R>, HttpClientError> {
        let retry_policy = self.retry_policy.as_ref();
        let url = HttpClient::build_request_url(&self.client.base_url, self.path)?;
        let started_at = Instant::now();
        debug!("sending SSE request");

        let result = self
            .client
            .execute_with_retry(retry_policy, || async {
                let mut request = self
                    .client
                    .client
                    .request(self.method.clone(), url.clone())
                    .header(ACCEPT, "text/event-stream");

                if let Some(body) = &self.json_body {
                    request = request
                        .header(CONTENT_TYPE, "application/json")
                        .body(body.clone());
                }

                let response = request.send().await?;
                HttpClient::handle_sse_response(response, self.client.max_response_bytes).await
            })
            .await;

        let elapsed_ms = started_at.elapsed().as_millis() as u64;

        match &result {
            Ok(chunks) => debug!(elapsed_ms, chunks = chunks.len(), "SSE stream completed"),
            Err(err) => warn!(elapsed_ms, error = %err, "SSE request failed"),
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU32;
    #[test]
    fn builder_with_base_url_succeeds() {
        let base = Url::parse("https://example.com").unwrap();
        let client = HttpClient::builder().base_url(base).build();
        assert_eq!(client.base_url.as_str(), "https://example.com/");
    }

    #[test]
    fn builder_normalizes_base_url_path_prefix() {
        let base = Url::parse("https://example.com/api").unwrap();
        let client = HttpClient::builder().base_url(base).build();
        assert_eq!(client.base_url.as_str(), "https://example.com/api/");
    }

    #[test]
    fn builder_with_retry_policy() {
        let base = Url::parse("https://example.com").unwrap();
        let client = HttpClient::builder()
            .base_url(base)
            .retry_policy(RetryPolicy::builder().build())
            .build();
        assert!(client.retry_policy.is_some());
    }

    #[test]
    fn builder_without_retry_policy() {
        let base = Url::parse("https://example.com").unwrap();
        let client = HttpClient::builder().base_url(base).build();
        assert!(client.retry_policy.is_none());
    }

    #[test]
    fn builder_uses_default_max_response_bytes() {
        let base = Url::parse("https://example.com").unwrap();
        let client = HttpClient::builder().base_url(base).build();
        assert_eq!(client.max_response_bytes, DEFAULT_MAX_RESPONSE_BYTES);
    }

    #[test]
    fn builder_can_override_max_response_bytes() {
        let base = Url::parse("https://example.com").unwrap();
        let client = HttpClient::builder()
            .base_url(base)
            .max_response_bytes(2048)
            .build();
        assert_eq!(client.max_response_bytes, 2048);
    }

    #[test]
    fn builder_uses_default_connection_pool_settings() {
        let base = Url::parse("https://example.com").unwrap();
        let client = HttpClient::builder().base_url(base).build();

        assert_eq!(client.pool_idle_timeout, DEFAULT_POOL_IDLE_TIMEOUT);
        assert_eq!(
            client.pool_max_idle_per_host,
            DEFAULT_POOL_MAX_IDLE_PER_HOST
        );
    }

    #[test]
    fn builder_can_override_connection_pool_settings() {
        let base = Url::parse("https://example.com").unwrap();
        let client = HttpClient::builder()
            .base_url(base)
            .pool_idle_timeout(Duration::from_secs(15))
            .pool_max_idle_per_host(32)
            .build();

        assert_eq!(client.pool_idle_timeout, Duration::from_secs(15));
        assert_eq!(client.pool_max_idle_per_host, 32);
    }

    #[test]
    fn get_inherits_client_retry_policy() {
        let base = Url::parse("https://example.com").unwrap();
        let retry_policy = RetryPolicy::builder().build();
        let client = HttpClient::builder()
            .base_url(base)
            .retry_policy(retry_policy.clone())
            .build();

        let request = client.get("/health");
        assert!(request.retry_policy.is_some());
        assert_eq!(
            request.retry_policy.unwrap().max_attempts().get(),
            retry_policy.max_attempts().get()
        );
    }

    #[test]
    fn post_does_not_retry_by_default() {
        let base = Url::parse("https://example.com").unwrap();
        let client = HttpClient::builder()
            .base_url(base)
            .retry_policy(RetryPolicy::builder().build())
            .build();

        let payload = serde_json::json!({ "ping": true });
        let request = client.post("/jobs", &payload).unwrap();
        assert!(request.retry_policy.is_none());
    }

    #[test]
    fn post_can_override_retry_policy() {
        let base = Url::parse("https://example.com").unwrap();
        let client = HttpClient::builder().base_url(base).build();
        let payload = serde_json::json!({ "ping": true });
        let retry_policy = RetryPolicy::builder().build();

        let request = client
            .post("/jobs", &payload)
            .unwrap()
            .with_retry(retry_policy.clone());
        assert!(request.retry_policy.is_some());
        assert_eq!(
            request.retry_policy.unwrap().max_attempts().get(),
            retry_policy.max_attempts().get()
        );
    }

    #[test]
    fn send_preserves_base_url_path_prefix_for_leading_slash_paths() {
        let base = Url::parse("https://example.com/api/").unwrap();
        let client = HttpClient::builder().base_url(base).build();

        let request = client.get("/users");
        let joined = HttpClient::build_request_url(&client.base_url, request.path).unwrap();

        assert_eq!(joined.as_str(), "https://example.com/api/users");
    }

    #[test]
    fn build_request_url_preserves_base_prefix_without_trailing_slash() {
        let base = HttpClient::normalize_base_url(Url::parse("https://example.com/api").unwrap());
        let joined = HttpClient::build_request_url(&base, "users").unwrap();

        assert_eq!(joined.as_str(), "https://example.com/api/users");
    }

    #[test]
    fn build_request_url_rejects_absolute_urls() {
        let base = Url::parse("https://example.com/api/").unwrap();
        let err = HttpClient::build_request_url(&base, "https://evil.example/steal").unwrap_err();

        assert!(matches!(err, HttpClientError::InvalidRequestPath(_)));
    }

    #[test]
    fn build_request_url_rejects_dot_segments() {
        let base = Url::parse("https://example.com/api/").unwrap();
        let err = HttpClient::build_request_url(&base, "../users").unwrap_err();

        assert!(matches!(err, HttpClientError::InvalidRequestPath(_)));
    }

    #[test]
    fn retry_policy_can_be_configured_for_single_attempt() {
        let policy = RetryPolicy::builder()
            .max_attempts(NonZeroU32::new(1).unwrap())
            .build();

        assert_eq!(policy.max_attempts().get(), 1);
    }

    #[test]
    fn empty_success_body_can_deserialize_to_unit() {
        let response: () = HttpClient::deserialize_success_body(b"").unwrap();
        assert_eq!(response, ());
    }

    #[test]
    fn whitespace_only_success_body_can_deserialize_to_none() {
        let response: Option<serde_json::Value> =
            HttpClient::deserialize_success_body(b" \n\t").unwrap();
        assert_eq!(response, None);
    }

    #[test]
    fn empty_success_body_returns_clear_error_for_required_json() {
        let result: Result<Vec<String>, HttpClientError> =
            HttpClient::deserialize_success_body(b"");
        assert!(matches!(result, Err(HttpClientError::EmptyResponseBody)));
    }

    #[test]
    fn truncating_error_preview_keeps_utf8_boundary() {
        let mut bytes = "zaż".as_bytes()[..3].to_vec();

        HttpClient::truncate_incomplete_utf8_suffix(&mut bytes);

        assert_eq!(bytes, b"za");
    }

    #[test]
    #[should_panic(expected = "base_url must use http or https scheme")]
    fn rejects_non_http_scheme() {
        let base = Url::parse("ftp://example.com").unwrap();
        HttpClient::builder().base_url(base).build();
    }
}

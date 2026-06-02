use reqwest::StatusCode;
use rust_json_client::{HttpClient, HttpClientError};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[derive(Serialize)]
struct PostPayload {
    name: String,
    value: i32,
}

#[derive(Deserialize, Debug)]
struct HttpBinResponse {
    json: serde_json::Value,
}

#[tokio::test]
async fn post_returns_deserialized_response() {
    let mock_server = MockServer::start().await;
    let payload = PostPayload {
        name: "test".to_string(),
        value: 42,
    };

    Mock::given(method("POST"))
        .and(path("/post"))
        .and(body_json(&payload))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "json": {
                "name": "test",
                "value": 42
            }
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let response: HttpBinResponse = client
        .post("/post", &payload)
        .unwrap()
        .send()
        .await
        .unwrap();

    assert_eq!(response.json["name"], "test");
    assert_eq!(response.json["value"], 42);
}

#[tokio::test]
async fn post_to_bad_endpoint_returns_api_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/status/404"))
        .respond_with(ResponseTemplate::new(404).set_body_string("missing endpoint"))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let payload = serde_json::json!({"foo": "bar"});

    let result: Result<serde_json::Value, HttpClientError> =
        client.post("/status/404", &payload).unwrap().send().await;

    assert!(matches!(
        result,
        Err(HttpClientError::ApiError {
            status: StatusCode::NOT_FOUND,
            ..
        })
    ));
}

#[derive(Deserialize, Debug)]
struct HttpBinGetResponse {
    url: String,
}

#[tokio::test]
async fn get_returns_deserialized_response() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/get"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "url": format!("{}/get", mock_server.uri())
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let response: HttpBinGetResponse = client.get("/get").send().await.unwrap();

    assert_eq!(response.url, format!("{}/get", mock_server.uri()));
}

#[tokio::test]
async fn get_rejects_success_response_larger_than_client_limit() {
    let mock_server = MockServer::start().await;
    let large_payload = "x".repeat(16 * 1024);

    Mock::given(method("GET"))
        .and(path("/large-success"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": large_payload
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .max_response_bytes(1024)
        .build();

    let result: Result<serde_json::Value, HttpClientError> =
        client.get("/large-success").send().await;

    assert!(matches!(
        result,
        Err(HttpClientError::ResponseTooLarge { limit: 1024, .. })
    ));
}

#[tokio::test]
async fn client_configured_response_limit_allows_large_body() {
    let mock_server = MockServer::start().await;
    let large_payload = "x".repeat(16 * 1024);

    Mock::given(method("GET"))
        .and(path("/large-success-allowed"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": large_payload
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .max_response_bytes(32 * 1024)
        .build();

    let response: serde_json::Value = client.get("/large-success-allowed").send().await.unwrap();

    assert_eq!(response["data"].as_str().unwrap().len(), 16 * 1024);
}

#[tokio::test]
async fn api_error_body_is_truncated_to_preview() {
    let mock_server = MockServer::start().await;
    let large_error_body = "e".repeat(16 * 1024);

    Mock::given(method("GET"))
        .and(path("/large-error"))
        .respond_with(ResponseTemplate::new(500).set_body_string(large_error_body.clone()))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let result: Result<serde_json::Value, HttpClientError> =
        client.get("/large-error").send().await;

    match result {
        Err(HttpClientError::ApiError { status, body, .. }) => {
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            assert!(body.len() < large_error_body.len());
            assert!(body.ends_with("... [truncated]"));
        }
        other => panic!("expected ApiError, got {other:?}"),
    }
}

#[tokio::test]
async fn no_content_response_can_deserialize_to_unit() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/no-content"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let response: () = client.get("/no-content").send().await.unwrap();
    assert_eq!(response, ());
}

#[tokio::test]
async fn reset_content_response_can_deserialize_to_unit() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/reset-content"))
        .respond_with(ResponseTemplate::new(205))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let response: () = client.get("/reset-content").send().await.unwrap();
    assert_eq!(response, ());
}

#[tokio::test]
async fn empty_success_body_returns_clear_error_for_required_json() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/empty-success"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let result: Result<HttpBinGetResponse, HttpClientError> =
        client.get("/empty-success").send().await;

    assert!(matches!(result, Err(HttpClientError::EmptyResponseBody)));
}

#[tokio::test]
async fn absolute_url_path_is_rejected_before_request_is_sent() {
    let mock_server = MockServer::start().await;
    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let result: Result<serde_json::Value, HttpClientError> =
        client.get("https://evil.example/steal").send().await;

    assert!(matches!(
        result,
        Err(HttpClientError::InvalidRequestPath(_))
    ));
}

#[tokio::test]
async fn dot_segments_in_path_are_rejected_before_request_is_sent() {
    let mock_server = MockServer::start().await;
    let client = HttpClient::builder()
        .base_url(url::Url::parse(&format!("{}/api", mock_server.uri())).unwrap())
        .build();

    let result: Result<serde_json::Value, HttpClientError> = client.get("../users").send().await;

    assert!(matches!(
        result,
        Err(HttpClientError::InvalidRequestPath(_))
    ));
}

#[tokio::test]
async fn leading_slash_path_preserves_base_url_prefix() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/users"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "url": format!("{}/api/users", mock_server.uri())
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&format!("{}/api/", mock_server.uri())).unwrap())
        .build();

    let response: HttpBinGetResponse = client.get("/users").send().await.unwrap();

    assert_eq!(response.url, format!("{}/api/users", mock_server.uri()));
}

#[tokio::test]
async fn base_url_without_trailing_slash_preserves_path_prefix() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/users"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "url": format!("{}/api/users", mock_server.uri())
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&format!("{}/api", mock_server.uri())).unwrap())
        .build();

    let response: HttpBinGetResponse = client.get("users").send().await.unwrap();

    assert_eq!(response.url, format!("{}/api/users", mock_server.uri()));
}

#[derive(Clone)]
struct RetryResponder {
    attempts: Arc<AtomicUsize>,
}

impl wiremock::Respond for RetryResponder {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);

        if attempt == 0 {
            ResponseTemplate::new(503).set_body_string("temporary failure")
        } else {
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "url": "retried"
            }))
        }
    }
}

#[tokio::test]
async fn get_retries_on_transient_error_using_client_policy() {
    let mock_server = MockServer::start().await;
    let attempts = Arc::new(AtomicUsize::new(0));

    Mock::given(method("GET"))
        .and(path("/retry"))
        .respond_with(RetryResponder {
            attempts: attempts.clone(),
        })
        .expect(2)
        .mount(&mock_server)
        .await;

    let retry_policy = rust_json_client::RetryPolicy::builder()
        .max_attempts(NonZeroU32::new(2).unwrap())
        .build();

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .retry_policy(retry_policy)
        .build();

    let response: HttpBinGetResponse = client.get("/retry").send().await.unwrap();

    assert_eq!(response.url, "retried");
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn get_retries_on_timeout_using_client_policy() {
    let mock_server = MockServer::start().await;
    let attempts = Arc::new(AtomicUsize::new(0));

    Mock::given(method("GET"))
        .and(path("/timeout"))
        .respond_with({
            let attempts = attempts.clone();
            move |_request: &Request| {
                attempts.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200)
                    .set_delay(std::time::Duration::from_millis(50))
                    .set_body_json(serde_json::json!({ "url": "slow" }))
            }
        })
        .expect(2)
        .mount(&mock_server)
        .await;

    let retry_policy = rust_json_client::RetryPolicy::builder()
        .max_attempts(NonZeroU32::new(2).unwrap())
        .base_delay(std::time::Duration::from_millis(1))
        .max_delay(std::time::Duration::from_millis(5))
        .build();

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .retry_policy(retry_policy)
        .request_timeout(std::time::Duration::from_millis(10))
        .build();

    let result: Result<HttpBinGetResponse, HttpClientError> = client.get("/timeout").send().await;

    assert!(matches!(result, Err(HttpClientError::RequestError(ref err)) if err.is_timeout()));
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn get_does_not_follow_redirects_to_other_hosts() {
    let source_server = MockServer::start().await;
    let target_server = MockServer::start().await;
    let redirected_attempts = Arc::new(AtomicUsize::new(0));

    Mock::given(method("GET"))
        .and(path("/redirect"))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "location",
            format!("{}/redirect-target", target_server.uri()),
        ))
        .expect(1)
        .mount(&source_server)
        .await;

    Mock::given(method("GET"))
        .and(path("/redirect-target"))
        .respond_with({
            let redirected_attempts = redirected_attempts.clone();
            move |_request: &Request| {
                redirected_attempts.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "url": "redirected" }))
            }
        })
        .mount(&target_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&source_server.uri()).unwrap())
        .build();

    let result: Result<serde_json::Value, HttpClientError> = client.get("/redirect").send().await;

    assert!(matches!(
        result,
        Err(HttpClientError::ApiError {
            status: StatusCode::FOUND,
            ..
        })
    ));
    assert_eq!(redirected_attempts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn post_does_not_retry_without_explicit_override() {
    let mock_server = MockServer::start().await;
    let attempts = Arc::new(AtomicUsize::new(0));

    Mock::given(method("POST"))
        .and(path("/jobs"))
        .respond_with({
            let attempts = attempts.clone();
            move |_request: &Request| {
                attempts.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(503).set_body_string("temporary failure")
            }
        })
        .expect(1)
        .mount(&mock_server)
        .await;

    let retry_policy = rust_json_client::RetryPolicy::builder()
        .max_attempts(NonZeroU32::new(3).unwrap())
        .build();

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .retry_policy(retry_policy)
        .build();

    let payload = serde_json::json!({"job": "scrape"});
    let result: Result<serde_json::Value, HttpClientError> =
        client.post("/jobs", &payload).unwrap().send().await;

    assert!(matches!(
        result,
        Err(HttpClientError::ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            ..
        })
    ));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn post_retries_when_explicit_retry_policy_is_provided() {
    let mock_server = MockServer::start().await;
    let attempts = Arc::new(AtomicUsize::new(0));

    Mock::given(method("POST"))
        .and(path("/jobs"))
        .respond_with({
            let attempts = attempts.clone();
            move |_request: &Request| {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);

                if attempt == 0 {
                    ResponseTemplate::new(503).set_body_string("temporary failure")
                } else {
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "status": "accepted"
                    }))
                }
            }
        })
        .expect(2)
        .mount(&mock_server)
        .await;

    let retry_policy = rust_json_client::RetryPolicy::builder()
        .max_attempts(NonZeroU32::new(2).unwrap())
        .build();

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let payload = serde_json::json!({"job": "scrape"});
    let response: serde_json::Value = client
        .post("/jobs", &payload)
        .unwrap()
        .with_retry(retry_policy)
        .send()
        .await
        .unwrap();

    assert_eq!(response["status"], "accepted");
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn requests_include_accept_json_header() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/accept"))
        .and(header("accept", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let response: serde_json::Value = client.get("/accept").send().await.unwrap();
    assert_eq!(response["ok"], true);
}

#[tokio::test]
async fn retry_respects_retry_after_header() {
    let mock_server = MockServer::start().await;
    let attempts = Arc::new(AtomicUsize::new(0));

    Mock::given(method("GET"))
        .and(path("/rate-limited"))
        .respond_with({
            let attempts = attempts.clone();
            move |_request: &Request| {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);

                if attempt == 0 {
                    ResponseTemplate::new(429)
                        .insert_header("retry-after", "1")
                        .set_body_string("rate limited")
                } else {
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "url": "ok"
                    }))
                }
            }
        })
        .expect(2)
        .mount(&mock_server)
        .await;

    let retry_policy = rust_json_client::RetryPolicy::builder()
        .max_attempts(NonZeroU32::new(2).unwrap())
        .base_delay(std::time::Duration::from_millis(10))
        .build();

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .retry_policy(retry_policy)
        .build();

    let start = std::time::Instant::now();
    let response: HttpBinGetResponse = client.get("/rate-limited").send().await.unwrap();
    let elapsed = start.elapsed();

    assert_eq!(response.url, "ok");
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    // Retry-After: 1 means we should have waited at least 1 second
    assert!(elapsed >= std::time::Duration::from_millis(900));
}

#[tokio::test]
async fn retry_after_is_exposed_in_api_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/rate-limited-no-retry"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "30")
                .set_body_string("rate limited"),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let result: Result<serde_json::Value, HttpClientError> =
        client.get("/rate-limited-no-retry").send().await;

    match result {
        Err(HttpClientError::ApiError {
            status,
            retry_after,
            ..
        }) => {
            assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(retry_after, Some(std::time::Duration::from_secs(30)));
        }
        other => panic!("expected ApiError with retry_after, got {other:?}"),
    }
}

#[derive(Deserialize, Debug)]
struct SseChunk {
    id: u32,
    text: String,
}

#[tokio::test]
async fn send_sse_collects_all_chunks() {
    let mock_server = MockServer::start().await;

    let sse_body = "\
        data: {\"id\":1,\"text\":\"hello\"}\n\n\
        data: {\"id\":2,\"text\":\"world\"}\n\n\
        data: [DONE]\n\n";

    Mock::given(method("POST"))
        .and(path("/stream"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let payload = serde_json::json!({"stream": true});
    let chunks: Vec<SseChunk> = client
        .post("/stream", &payload)
        .unwrap()
        .send_sse()
        .await
        .unwrap();

    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].id, 1);
    assert_eq!(chunks[0].text, "hello");
    assert_eq!(chunks[1].id, 2);
    assert_eq!(chunks[1].text, "world");
}

#[tokio::test]
async fn send_sse_returns_empty_vec_on_immediate_done() {
    let mock_server = MockServer::start().await;

    let sse_body = "data: [DONE]\n\n";

    Mock::given(method("POST"))
        .and(path("/stream-empty"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let payload = serde_json::json!({"stream": true});
    let chunks: Vec<SseChunk> = client
        .post("/stream-empty", &payload)
        .unwrap()
        .send_sse()
        .await
        .unwrap();

    assert!(chunks.is_empty());
}

#[tokio::test]
async fn send_sse_ignores_comments_and_unknown_fields() {
    let mock_server = MockServer::start().await;

    let sse_body = "\
        : this is a comment\n\
        data: {\"id\":1,\"text\":\"kept\"}\n\n\
        event: heartbeat\n\
        : another comment\n\
        data: {\"id\":2,\"text\":\"also kept\"}\n\n\
        data: [DONE]\n\n";

    Mock::given(method("POST"))
        .and(path("/stream-comments"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let payload = serde_json::json!({"stream": true});
    let chunks: Vec<SseChunk> = client
        .post("/stream-comments", &payload)
        .unwrap()
        .send_sse()
        .await
        .unwrap();

    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].text, "kept");
    assert_eq!(chunks[1].text, "also kept");
}

#[tokio::test]
async fn send_sse_returns_api_error_on_non_success_status() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/stream-error"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let payload = serde_json::json!({"stream": true});
    let result: Result<Vec<SseChunk>, HttpClientError> = client
        .post("/stream-error", &payload)
        .unwrap()
        .send_sse()
        .await;

    assert!(matches!(
        result,
        Err(HttpClientError::ApiError {
            status: StatusCode::UNAUTHORIZED,
            ..
        })
    ));
}

#[tokio::test]
async fn send_sse_retries_on_transient_error() {
    let mock_server = MockServer::start().await;
    let attempts = Arc::new(AtomicUsize::new(0));

    Mock::given(method("POST"))
        .and(path("/stream-retry"))
        .respond_with({
            let attempts = attempts.clone();
            move |_request: &Request| {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);

                if attempt == 0 {
                    ResponseTemplate::new(503).set_body_string("temporary failure")
                } else {
                    ResponseTemplate::new(200)
                        .insert_header("content-type", "text/event-stream")
                        .set_body_string(
                            "data: {\"id\":1,\"text\":\"recovered\"}\n\ndata: [DONE]\n\n",
                        )
                }
            }
        })
        .expect(2)
        .mount(&mock_server)
        .await;

    let retry_policy = rust_json_client::RetryPolicy::builder()
        .max_attempts(NonZeroU32::new(2).unwrap())
        .base_delay(std::time::Duration::from_millis(10))
        .build();

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let payload = serde_json::json!({"stream": true});
    let chunks: Vec<SseChunk> = client
        .post("/stream-retry", &payload)
        .unwrap()
        .with_retry(retry_policy)
        .send_sse()
        .await
        .unwrap();

    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].text, "recovered");
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn send_sse_returns_deserialization_error_on_bad_json() {
    let mock_server = MockServer::start().await;

    let sse_body = "\
        data: {\"id\":1,\"text\":\"ok\"}\n\n\
        data: {not valid json}\n\n\
        data: [DONE]\n\n";

    Mock::given(method("POST"))
        .and(path("/stream-bad-json"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let payload = serde_json::json!({"stream": true});
    let result: Result<Vec<SseChunk>, HttpClientError> = client
        .post("/stream-bad-json", &payload)
        .unwrap()
        .send_sse()
        .await;

    assert!(matches!(
        result,
        Err(HttpClientError::DeserializationError(_))
    ));
}

#[tokio::test]
async fn send_sse_enforces_max_response_bytes() {
    let mock_server = MockServer::start().await;

    let large_text = "x".repeat(512);
    let sse_body = format!(
        "data: {{\"id\":1,\"text\":\"{large_text}\"}}\n\n\
         data: {{\"id\":2,\"text\":\"{large_text}\"}}\n\n\
         data: [DONE]\n\n"
    );

    Mock::given(method("POST"))
        .and(path("/stream-large"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .max_response_bytes(256)
        .build();

    let payload = serde_json::json!({"stream": true});
    let result: Result<Vec<SseChunk>, HttpClientError> = client
        .post("/stream-large", &payload)
        .unwrap()
        .send_sse()
        .await;

    assert!(matches!(
        result,
        Err(HttpClientError::ResponseTooLarge { limit: 256, .. })
    ));
}

#[tokio::test]
async fn send_sse_returns_incomplete_error_when_stream_ends_without_done() {
    let mock_server = MockServer::start().await;

    // Two complete chunks, but the stream closes without the `[DONE]` sentinel —
    // a truncated response (dropped connection / proxy cutoff / server crash).
    let sse_body = "\
        data: {\"id\":1,\"text\":\"first\"}\n\n\
        data: {\"id\":2,\"text\":\"second\"}\n\n";

    Mock::given(method("POST"))
        .and(path("/stream-no-done"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let payload = serde_json::json!({"stream": true});
    let result: Result<Vec<SseChunk>, HttpClientError> = client
        .post("/stream-no-done", &payload)
        .unwrap()
        .send_sse()
        .await;

    assert!(matches!(result, Err(HttpClientError::IncompleteSseStream)));
}

#[tokio::test]
async fn send_sse_returns_incomplete_error_when_stream_truncated_mid_line() {
    let mock_server = MockServer::start().await;

    // Last data line has no trailing \n and there is no `[DONE]` — simulates a
    // server closing the connection abruptly mid-stream.
    let sse_body = "data: {\"id\":1,\"text\":\"first\"}\n\ndata: {\"id\":2,\"text\":\"last\"}";

    Mock::given(method("POST"))
        .and(path("/stream-no-trailing-newline"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let payload = serde_json::json!({"stream": true});
    let result: Result<Vec<SseChunk>, HttpClientError> = client
        .post("/stream-no-trailing-newline", &payload)
        .unwrap()
        .send_sse()
        .await;

    assert!(matches!(result, Err(HttpClientError::IncompleteSseStream)));
}

#[tokio::test]
async fn send_sse_handles_data_prefix_without_space() {
    let mock_server = MockServer::start().await;

    // Some SSE implementations use "data:{json}" without a space after the colon.
    let sse_body = "data:{\"id\":1,\"text\":\"no-space\"}\n\ndata: [DONE]\n\n";

    Mock::given(method("POST"))
        .and(path("/stream-no-space"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let payload = serde_json::json!({"stream": true});
    let chunks: Vec<SseChunk> = client
        .post("/stream-no-space", &payload)
        .unwrap()
        .send_sse()
        .await
        .unwrap();

    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].text, "no-space");
}

#[tokio::test]
async fn send_sse_respects_retry_after_on_429() {
    let mock_server = MockServer::start().await;
    let attempts = Arc::new(AtomicUsize::new(0));

    Mock::given(method("POST"))
        .and(path("/stream-rate-limited"))
        .respond_with({
            let attempts = attempts.clone();
            move |_request: &Request| {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);

                if attempt == 0 {
                    ResponseTemplate::new(429)
                        .insert_header("retry-after", "1")
                        .set_body_string("rate limited")
                } else {
                    ResponseTemplate::new(200)
                        .insert_header("content-type", "text/event-stream")
                        .set_body_string("data: {\"id\":1,\"text\":\"ok\"}\n\ndata: [DONE]\n\n")
                }
            }
        })
        .expect(2)
        .mount(&mock_server)
        .await;

    let retry_policy = rust_json_client::RetryPolicy::builder()
        .max_attempts(NonZeroU32::new(2).unwrap())
        .base_delay(std::time::Duration::from_millis(10))
        .build();

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let payload = serde_json::json!({"stream": true});
    let start = std::time::Instant::now();
    let chunks: Vec<SseChunk> = client
        .post("/stream-rate-limited", &payload)
        .unwrap()
        .with_retry(retry_policy)
        .send_sse()
        .await
        .unwrap();
    let elapsed = start.elapsed();

    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].text, "ok");
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    // Retry-After: 1 means we should have waited at least 1 second
    assert!(elapsed >= std::time::Duration::from_millis(900));
}

#[tokio::test]
async fn send_sse_handles_crlf_line_endings() {
    let mock_server = MockServer::start().await;

    let sse_body = "data: {\"id\":1,\"text\":\"crlf\"}\r\n\r\ndata: [DONE]\r\n\r\n";

    Mock::given(method("POST"))
        .and(path("/stream-crlf"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let payload = serde_json::json!({"stream": true});
    let chunks: Vec<SseChunk> = client
        .post("/stream-crlf", &payload)
        .unwrap()
        .send_sse()
        .await
        .unwrap();

    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].text, "crlf");
}

#[tokio::test]
async fn send_sse_sends_accept_text_event_stream_header() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/stream-accept"))
        .and(header("accept", "text/event-stream"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("data: {\"id\":1,\"text\":\"accepted\"}\n\ndata: [DONE]\n\n"),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let payload = serde_json::json!({"stream": true});
    let chunks: Vec<SseChunk> = client
        .post("/stream-accept", &payload)
        .unwrap()
        .send_sse()
        .await
        .unwrap();

    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].text, "accepted");
}

// ─── send_text / send_bytes (raw, non-JSON response bodies) ──────────────────

#[tokio::test]
async fn send_text_returns_raw_body() {
    let mock_server = MockServer::start().await;

    let atom = "<?xml version=\"1.0\"?><feed><entry><title>hi</title></entry></feed>";

    Mock::given(method("GET"))
        .and(path("/feed.atom"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/atom+xml")
                .set_body_string(atom),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let body: String = client.get("/feed.atom").send_text().await.unwrap();

    assert_eq!(body, atom);
}

#[tokio::test]
async fn send_bytes_returns_raw_body() {
    let mock_server = MockServer::start().await;
    let raw: Vec<u8> = vec![0, 1, 2, 3, 255, 254, 200];

    Mock::given(method("GET"))
        .and(path("/binary"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(raw.clone()))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let body: Vec<u8> = client.get("/binary").send_bytes().await.unwrap();

    assert_eq!(body, raw);
}

#[tokio::test]
async fn send_text_replaces_invalid_utf8_lossily() {
    let mock_server = MockServer::start().await;
    // 0xFF can never appear in valid UTF-8; lossy decoding yields U+FFFD rather
    // than failing the whole request — a corrupt feed becomes a downstream parse
    // error (transient skip), never a hard client error.
    let invalid: Vec<u8> = vec![b'h', b'i', 0xFF];

    Mock::given(method("GET"))
        .and(path("/bad-utf8"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(invalid))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let body: String = client.get("/bad-utf8").send_text().await.unwrap();

    assert_eq!(body, "hi\u{FFFD}");
}

#[tokio::test]
async fn send_text_returns_empty_string_for_empty_body() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/empty"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    // Unlike `send`, an empty 2xx body is not an error for raw reads — it is an
    // empty string (0 entries downstream), not `EmptyResponseBody`.
    let body: String = client.get("/empty").send_text().await.unwrap();

    assert_eq!(body, "");
}

#[tokio::test]
async fn send_bytes_returns_api_error_on_non_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/missing"))
        .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let result = client.get("/missing").send_bytes().await;

    assert!(matches!(
        result,
        Err(HttpClientError::ApiError {
            status: StatusCode::NOT_FOUND,
            ..
        })
    ));
}

#[tokio::test]
async fn send_bytes_enforces_max_response_bytes() {
    let mock_server = MockServer::start().await;
    let large = "x".repeat(16 * 1024);

    Mock::given(method("GET"))
        .and(path("/large-raw"))
        .respond_with(ResponseTemplate::new(200).set_body_string(large))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .max_response_bytes(1024)
        .build();

    let result = client.get("/large-raw").send_bytes().await;

    assert!(matches!(
        result,
        Err(HttpClientError::ResponseTooLarge { limit: 1024, .. })
    ));
}

#[tokio::test]
async fn send_text_uses_accept_header_from_default_headers() {
    let mock_server = MockServer::start().await;

    // send_text must NOT hardcode an Accept header (unlike send_sse) — it honours
    // whatever the caller configured via default_headers.
    Mock::given(method("GET"))
        .and(path("/feed"))
        .and(header("accept", "application/atom+xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<feed/>"))
        .expect(1)
        .mount(&mock_server)
        .await;

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::ACCEPT,
        "application/atom+xml".parse().unwrap(),
    );

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .default_headers(headers)
        .build();

    let body: String = client.get("/feed").send_text().await.unwrap();

    assert_eq!(body, "<feed/>");
}

#[tokio::test]
async fn send_text_retries_on_transient_error_using_client_policy() {
    let mock_server = MockServer::start().await;
    let attempts = Arc::new(AtomicUsize::new(0));

    Mock::given(method("GET"))
        .and(path("/retry-text"))
        .respond_with({
            let attempts = attempts.clone();
            move |_request: &Request| {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    ResponseTemplate::new(503).set_body_string("temporary failure")
                } else {
                    ResponseTemplate::new(200).set_body_string("<feed>ok</feed>")
                }
            }
        })
        .expect(2)
        .mount(&mock_server)
        .await;

    let retry_policy = rust_json_client::RetryPolicy::builder()
        .max_attempts(NonZeroU32::new(2).unwrap())
        .base_delay(std::time::Duration::from_millis(10))
        .build();

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .retry_policy(retry_policy)
        .build();

    let body: String = client.get("/retry-text").send_text().await.unwrap();

    assert_eq!(body, "<feed>ok</feed>");
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn post_send_bytes_includes_json_body() {
    let mock_server = MockServer::start().await;
    let payload = serde_json::json!({ "q": "rust" });

    Mock::given(method("POST"))
        .and(path("/search"))
        .and(body_json(&payload))
        .respond_with(ResponseTemplate::new(200).set_body_string("<results/>"))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&mock_server.uri()).unwrap())
        .build();

    let bytes = client
        .post("/search", &payload)
        .unwrap()
        .send_bytes()
        .await
        .unwrap();

    assert_eq!(String::from_utf8_lossy(&bytes), "<results/>");
}

/// Spawns a one-shot raw HTTP/1.1 server that replies with a chunked body (no
/// `Content-Length`) that keeps streaming past `min_bytes`. wiremock always sets
/// `Content-Length`, which trips the precheck before streaming begins — this lets
/// a test drive the streamed byte-cap in `read_response_body_limited` instead.
async fn spawn_chunked_oversize_server(min_bytes: usize) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };

        // Drain the request so the client finishes writing before we respond.
        let mut buf = [0u8; 1024];
        let _ = socket.read(&mut buf).await;

        if socket
            .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
            .await
            .is_err()
        {
            return;
        }

        let chunk = "x".repeat(512);
        let frame = format!("{:x}\r\n{}\r\n", chunk.len(), chunk);
        let mut sent = 0usize;
        while sent <= min_bytes {
            // Once the client hits its cap it drops the connection; the resulting
            // write error is expected, so we just stop.
            if socket.write_all(frame.as_bytes()).await.is_err() {
                return;
            }
            sent += chunk.len();
        }
        let _ = socket.write_all(b"0\r\n\r\n").await;
    });

    format!("http://{addr}")
}

#[tokio::test]
async fn send_bytes_enforces_max_response_bytes_on_chunked_stream_without_content_length() {
    // No Content-Length means the precheck in read_checked_body cannot fire, so
    // this exercises the streamed byte-cap in read_response_body_limited directly —
    // the realistic enforcement path for a chunked feed read via send_bytes.
    let base = spawn_chunked_oversize_server(8 * 1024).await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&base).unwrap())
        .max_response_bytes(1024)
        .build();

    let result = client.get("/stream").send_bytes().await;

    match result {
        Err(HttpClientError::ResponseTooLarge { limit, received }) => {
            assert_eq!(limit, 1024);
            assert!(
                received > limit,
                "streamed cap should report received ({received}) > limit ({limit})"
            );
        }
        other => panic!("expected ResponseTooLarge from streamed cap, got {other:?}"),
    }
}

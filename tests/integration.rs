use rust_json_client::{HttpClient, HttpClientError};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("{}/redirect-target", target_server.uri())),
        )
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

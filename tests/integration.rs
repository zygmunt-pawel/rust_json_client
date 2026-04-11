use http_client_test::{HttpClient, HttpClientError};
use serde::{Deserialize, Serialize};

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
    let client = HttpClient::builder()
        .base_url("https://httpbin.org")
        .build()
        .unwrap();

    let payload = PostPayload {
        name: "test".to_string(),
        value: 42,
    };

    let response: HttpBinResponse = client.post("/post", &payload).await.unwrap();

    assert_eq!(response.json["name"], "test");
    assert_eq!(response.json["value"], 42);
}

#[tokio::test]
async fn post_to_bad_endpoint_returns_api_error() {
    let client = HttpClient::builder()
        .base_url("https://httpbin.org")
        .build()
        .unwrap();

    let payload = serde_json::json!({"foo": "bar"});

    let result: Result<serde_json::Value, HttpClientError> =
        client.post("/status/404", &payload).await;

    assert!(matches!(
        result,
        Err(HttpClientError::ApiError { status: 404, .. })
    ));
}

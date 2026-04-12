# rust_json_client

Opinionated async HTTP client for JSON APIs built on top of [reqwest](https://docs.rs/reqwest).

## Features

- Builder pattern via [bon](https://docs.rs/bon)
- Automatic retries with exponential backoff (powered by [backon](https://docs.rs/backon))
- Configurable response size limits
- Structured logging via [tracing](https://docs.rs/tracing)
- Path traversal protection
- No redirect following (safe for API clients)

## Installation

```toml
[dependencies]
rust_json_client = { git = "https://github.com/zygmunt-pawel/rust_json_client.git" }
```

## Usage

### Basic GET request

```rust
use rust_json_client::HttpClient;
use serde::Deserialize;
use url::Url;

#[derive(Deserialize)]
struct HealthResponse {
    status: String,
}

let client = HttpClient::builder()
    .base_url(Url::parse("https://api.example.com/v1")?)
    .build();

let health: HealthResponse = client.get("/health").send().await?;
```

### POST with JSON body

```rust
use rust_json_client::HttpClient;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct CreateJobRequest {
    job: String,
}

#[derive(Deserialize)]
struct CreateJobResponse {
    id: String,
}

let request = CreateJobRequest { job: "sync-products".to_string() };
let response: CreateJobResponse = client.post("/jobs", &request)?.send().await?;
```

### Retries with exponential backoff

GET requests inherit the client-level retry policy. POST requests don't retry by default — opt in per request with `.with_retry()`:

```rust
use rust_json_client::{HttpClient, RetryPolicy};
use std::num::NonZeroU32;

// Client-level policy applies to all GETs
let client = HttpClient::builder()
    .base_url(Url::parse("https://api.example.com/v1")?)
    .retry_policy(RetryPolicy::builder()
        .max_attempts(NonZeroU32::new(3).unwrap())
        .build())
    .build();

// GETs retry automatically
let health: HealthResponse = client.get("/health").send().await?;

// POSTs require explicit opt-in
let response: CreateJobResponse = client
    .post("/jobs", &request)?
    .with_retry(RetryPolicy::builder().build())
    .send()
    .await?;
```

### Client configuration

```rust
use rust_json_client::HttpClient;
use std::time::Duration;

let client = HttpClient::builder()
    .base_url(Url::parse("https://api.example.com/v1")?)
    .connect_timeout(Duration::from_secs(3))
    .request_timeout(Duration::from_secs(10))
    .max_response_bytes(1024 * 1024)       // 1 MB response limit
    .pool_idle_timeout(Duration::from_secs(30))
    .pool_max_idle_per_host(32)
    .build();
```

### Error handling

```rust
use rust_json_client::{HttpClient, HttpClientError};
use reqwest::StatusCode;

let result: Result<serde_json::Value, HttpClientError> = client
    .post("/jobs", &request)?
    .send()
    .await;

match result {
    Ok(response) => println!("ok: {response:?}"),
    Err(HttpClientError::ApiError { status: StatusCode::CONFLICT, body }) => {
        eprintln!("already exists: {body}");
    }
    Err(HttpClientError::ResponseTooLarge { limit, received }) => {
        eprintln!("response too large: {received} > {limit}");
    }
    Err(err) => return Err(err.into()),
}
```

## License

MIT

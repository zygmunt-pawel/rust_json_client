# rust_json_client

Opinionated async HTTP client for JSON APIs built on top of [reqwest](https://docs.rs/reqwest).

## Features

- Builder pattern for client and retry policy via [bon](https://docs.rs/bon)
- GET and POST with automatic JSON serialization/deserialization
- Server-Sent Events (SSE) streaming via `send_sse` — collects `data:` chunks; returns `IncompleteSseStream` when a stream is truncated (ends without `[DONE]`)
- Raw (non-JSON) response bodies via `send_text` / `send_bytes` — same status checks, retries, and size limits as `send`; `send_text` decodes UTF-8 lossily (invalid bytes → U+FFFD), `send_bytes` returns the raw bytes. `Accept` comes from `default_headers` (not overridden)
- Automatic retries with exponential backoff and jitter (powered by [backon](https://docs.rs/backon))
- GET retries inherited from client policy, POST retries opt-in per request
- Configurable retryable status codes (429, 500, 502, 503, 504 by default)
- Respects `Retry-After` header on 429 responses (uses `max(retry_after, backoff)`)
- Retries on transient transport errors (timeouts, connection failures)
- Configurable response size limits with streaming enforcement (default 10 MB)
- Truncated error body preview for non-success responses (max 8 KB)
- Safe handling of empty/whitespace-only response bodies (`()` and `Option<T>`)
- Base URL normalization with path prefix preservation
- Path traversal protection (rejects `..`, `.`, absolute URLs, fragments)
- Redirects disabled by default (safe for API clients)
- Default `Accept: application/json` header (overridable via `default_headers`)
- Scheme validation on `base_url` (only `http` and `https` accepted)
- `#[must_use]` on `RequestBuilder` — warns if `.send()` is not called
- `Send + Sync` compile-time guarantee on `HttpClient`
- Default headers support
- Configurable connect/request timeouts (5s / 30s defaults)
- Optional per-chunk SSE idle timeout (`sse_idle_timeout`) — aborts a stalled stream when the gap between chunks exceeds the bound (retryable); default off
- Configurable connection pool (idle timeout, max idle per host — default 64)
- TLS via rustls (no OpenSSL dependency)
- HTTP/2 support
- Structured logging via [tracing](https://docs.rs/tracing) with `#[instrument]` spans
- UTF-8 safe truncation of error body previews

## Installation

```toml
[dependencies]
rust_json_client = { git = "https://github.com/zygmunt-pawel/rust_json_client.git", tag = "v0.5.3" }
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
    .sse_idle_timeout(Duration::from_secs(60)) // abort an SSE stream if a chunk gap exceeds 60s
    .max_response_bytes(1024 * 1024)       // 1 MB response limit
    .pool_idle_timeout(Duration::from_secs(30))
    .pool_max_idle_per_host(32)
    .build();
```

### SSE streaming

`send_sse` consumes a `text/event-stream` response, deserializing each `data:` payload into your type and stopping at the `[DONE]` sentinel. Pair it with `sse_idle_timeout` so a stalled-but-open upstream aborts instead of hanging: the bound is the gap *between* chunks and resets on every chunk, independent of the total `request_timeout`. A stall returns the retryable `HttpClientError::SseIdleTimeout`, so a configured retry policy can recover automatically.

```rust
use rust_json_client::HttpClient;
use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
struct StreamChunk {
    text: String,
}

let client = HttpClient::builder()
    .base_url(Url::parse("https://api.example.com/v1")?)
    .sse_idle_timeout(Duration::from_secs(30)) // abort if no chunk arrives for 30s
    .build();

let chunks: Vec<StreamChunk> = client
    .post("/chat/completions", &request)?
    .send_sse()
    .await?;
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
    Err(HttpClientError::ApiError { status: StatusCode::CONFLICT, body, .. }) => {
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

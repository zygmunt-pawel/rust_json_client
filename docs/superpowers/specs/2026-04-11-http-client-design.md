# HTTP Client — Design Spec

## Overview

Reusable Rust HTTP client component (library crate) wrapping `reqwest` with a clean, simple interface. Intended for transplanting into other projects. Starts as MVP with POST-only support; future iterations add retry, timeouts, observability, streaming, and multi-service configuration.

## MVP Scope

- Send JSON POST requests with typed request/response bodies
- Builder pattern for client construction
- Custom error type with clear variants
- That's it. No retry, no timeouts, no logging, no config files.

## Architecture

Thin wrapper over `reqwest::Client`. No traits, no abstractions beyond what's needed.

### File Structure

```
src/
  lib.rs          — public re-exports
  client.rs       — HttpClient + HttpClientBuilder
  error.rs        — HttpClientError enum
  response.rs     — reserved for future response wrapper (empty for now)
```

### `HttpClient`

```rust
pub struct HttpClient {
    client: reqwest::Client,
    base_url: String,
}
```

Single public method:

```rust
pub async fn post<T: Serialize, R: DeserializeOwned>(
    &self, path: &str, body: &T,
) -> Result<R, HttpClientError>
```

Behavior:
- Joins `base_url` + `path`
- Sends JSON POST via reqwest
- On 2xx: deserializes body to `R`
- On non-2xx: returns `ApiError` with status + response body text
- On network/reqwest failure: returns `RequestError`
- On deserialization failure: returns `DeserializationError`

### `HttpClientBuilder`

```rust
HttpClient::builder()
    .base_url("https://api.example.com")
    .build()? // errors if base_url not set
```

MVP fields: `base_url` only. Future: `timeout`, `default_headers`, `retry_policy`.

### `HttpClientError`

```rust
pub enum HttpClientError {
    RequestError(reqwest::Error),
    DeserializationError(serde_json::Error),
    ApiError { status: u16, body: String },
    BuilderError(String),
}
```

Uses `thiserror` for Display/Error derives.

### Dependencies

- `reqwest` (features: `json`)
- `serde` + `serde_json`
- `thiserror`
- `tokio` (runtime, dev-dependency or brought by consumer)

## Future Iterations (out of scope for MVP)

1. Retry with backoff
2. Configurable timeouts
3. Default headers per client
4. Tracing/logging middleware
5. Metrics
6. GET, PUT, DELETE, PATCH methods
7. Streaming responses (SSE)
8. Response wrapper with access to headers/status
9. Multi-service configuration

## Non-Goals

- No config files — code-only configuration
- No trait abstractions yet — add when needed for testing
- No macros

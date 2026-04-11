# HTTP Client MVP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a minimal reusable HTTP client library wrapping reqwest with builder pattern, POST method, and typed JSON request/response.

**Architecture:** Thin wrapper over `reqwest::Client` with `HttpClient` struct holding the client and base URL. Builder pattern for construction. Custom `HttpClientError` enum using `thiserror`. No traits, no abstractions.

**Tech Stack:** Rust (edition 2024), reqwest (json feature), serde/serde_json, thiserror, tokio (dev-dependency for tests)

---

### Task 1: Set up dependencies in Cargo.toml

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Update Cargo.toml with dependencies**

Replace the current `[dependencies]` section so the full file reads:

```toml
[package]
name = "http_client_test"
version = "0.1.0"
edition = "2024"

[dependencies]
reqwest = { version = "0.12", features = ["json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"

[dev-dependencies]
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`
Expected: compiles with no errors (warnings about unused deps are fine)

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "feat: add dependencies for http client library"
```

---

### Task 2: Implement HttpClientError

**Files:**
- Create: `src/error.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write the error enum**

Create `src/error.rs`:

```rust
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
```

- [ ] **Step 2: Create lib.rs with module declaration**

Replace `src/lib.rs` contents:

```rust
mod error;

pub use error::HttpClientError;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check`
Expected: compiles with no errors

- [ ] **Step 4: Commit**

```bash
git add src/error.rs src/lib.rs
git commit -m "feat: add HttpClientError enum"
```

---

### Task 3: Implement HttpClient and HttpClientBuilder

**Files:**
- Create: `src/client.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write the test for builder — missing base_url should error**

Create the test at the bottom of `src/client.rs` (we'll write impl + test in one file):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_without_base_url_returns_error() {
        let result = HttpClient::builder().build();
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Write the test for builder — with base_url should succeed**

Add to the `tests` module:

```rust
    #[test]
    fn builder_with_base_url_succeeds() {
        let client = HttpClient::builder()
            .base_url("https://example.com")
            .build();
        assert!(client.is_ok());
    }
```

- [ ] **Step 3: Implement HttpClientBuilder and HttpClient**

Write `src/client.rs` (full file):

```rust
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::HttpClientError;

pub struct HttpClient {
    client: Client,
    base_url: String,
}

pub struct HttpClientBuilder {
    base_url: Option<String>,
}

impl HttpClient {
    pub fn builder() -> HttpClientBuilder {
        HttpClientBuilder { base_url: None }
    }

    pub async fn post<T: Serialize, R: DeserializeOwned>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<R, HttpClientError> {
        let url = format!("{}{}", self.base_url, path);

        let response = self.client.post(&url).json(body).send().await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(HttpClientError::ApiError {
                status: status.as_u16(),
                body,
            });
        }

        let bytes = response.bytes().await?;
        let parsed = serde_json::from_slice(&bytes)?;
        Ok(parsed)
    }
}

impl HttpClientBuilder {
    pub fn base_url(mut self, url: &str) -> Self {
        self.base_url = Some(url.trim_end_matches('/').to_string());
        self
    }

    pub fn build(self) -> Result<HttpClient, HttpClientError> {
        let base_url = self
            .base_url
            .ok_or_else(|| HttpClientError::BuilderError("base_url is required".to_string()))?;

        Ok(HttpClient {
            client: Client::new(),
            base_url,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_without_base_url_returns_error() {
        let result = HttpClient::builder().build();
        assert!(result.is_err());
    }

    #[test]
    fn builder_with_base_url_succeeds() {
        let client = HttpClient::builder()
            .base_url("https://example.com")
            .build();
        assert!(client.is_ok());
    }

    #[test]
    fn builder_trims_trailing_slash() {
        let client = HttpClient::builder()
            .base_url("https://example.com/")
            .build()
            .unwrap();
        assert_eq!(client.base_url, "https://example.com");
    }
}
```

- [ ] **Step 4: Update lib.rs to export client module**

Update `src/lib.rs`:

```rust
mod client;
mod error;

pub use client::HttpClient;
pub use error::HttpClientError;
```

- [ ] **Step 5: Run tests**

Run: `cargo test`
Expected: 3 tests pass

- [ ] **Step 6: Commit**

```bash
git add src/client.rs src/lib.rs
git commit -m "feat: add HttpClient with builder pattern and POST method"
```

---

### Task 4: Integration test with real HTTP call

**Files:**
- Create: `tests/integration.rs`

- [ ] **Step 1: Write an integration test using httpbin.org**

Create `tests/integration.rs`:

```rust
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
```

- [ ] **Step 2: Run integration tests**

Run: `cargo test --test integration`
Expected: 2 tests pass (requires internet)

- [ ] **Step 3: Commit**

```bash
git add tests/integration.rs
git commit -m "test: add integration tests against httpbin.org"
```

---

### Task 5: Clean up main.rs

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Remove main.rs — this is a library crate**

Delete `src/main.rs` since this is a library crate (only `lib.rs` needed):

```bash
rm src/main.rs
```

- [ ] **Step 2: Verify everything still works**

Run: `cargo test`
Expected: all 5 tests pass

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "chore: remove main.rs, this is a library crate"
```

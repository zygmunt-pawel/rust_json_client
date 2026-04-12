# SSE Streaming (`send_sse`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `send_sse::<R>()` method to `RequestBuilder` that reads SSE streams, deserializes each `data:` line to `R`, and returns `Vec<R>`.

**Architecture:** New method on `RequestBuilder` in `client.rs`, reusing existing retry/error infrastructure. SSE parsing logic lives inside the method. No new files, no new error variants, no new dependencies.

**Tech Stack:** reqwest (chunked reading), serde_json (deserialization), wiremock (testing)

---

### Task 1: Happy path — multi-chunk SSE response

**Files:**
- Test: `tests/integration.rs`
- Modify: `src/client.rs:402-447` (impl RequestBuilder)

- [ ] **Step 1: Write the failing test**

Add to `tests/integration.rs`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test send_sse_collects_all_chunks 2>&1`
Expected: compilation error — `send_sse` method does not exist

- [ ] **Step 3: Implement `send_sse` method**

Add to `src/client.rs` inside `impl<'a> RequestBuilder<'a>`, after the existing `send` method:

```rust
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
            let mut request = self.client.client.request(self.method.clone(), url.clone());

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
```

Add to `impl HttpClient` (the private methods section), after `handle_json_response`:

```rust
async fn handle_sse_response<R: DeserializeOwned>(
    response: Response,
    max_response_bytes: usize,
) -> Result<Vec<R>, HttpClientError> {
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

    let mut chunks = Vec::new();
    let mut line_buf = String::new();
    let mut received = 0usize;
    let mut response = response;

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

        let text = String::from_utf8_lossy(&chunk);
        line_buf.push_str(&text);

        while let Some(newline_pos) = line_buf.find('\n') {
            let line: String = line_buf.drain(..=newline_pos).collect();
            let line = line.trim();

            if line.is_empty() {
                continue;
            }

            if line.starts_with(':') {
                continue;
            }

            let Some(data) = line.strip_prefix("data: ").or_else(|| line.strip_prefix("data:")) else {
                continue;
            };

            let data = data.trim();

            if data == "[DONE]" {
                debug!(chunks = chunks.len(), bytes = received, "SSE stream received [DONE]");
                return Ok(chunks);
            }

            let parsed: R = serde_json::from_str(data)
                .map_err(HttpClientError::DeserializationError)?;
            chunks.push(parsed);
        }
    }

    debug!(chunks = chunks.len(), bytes = received, "SSE stream ended without [DONE]");
    Ok(chunks)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test send_sse_collects_all_chunks 2>&1`
Expected: PASS

- [ ] **Step 5: Run all existing tests to check for regressions**

Run: `cargo test 2>&1`
Expected: all tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/client.rs tests/integration.rs
git commit -m "feat: add send_sse method for streaming SSE responses"
```

---

### Task 2: Empty stream — only `[DONE]`

**Files:**
- Test: `tests/integration.rs`

- [ ] **Step 1: Write the failing test**

```rust
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
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test send_sse_returns_empty_vec_on_immediate_done 2>&1`
Expected: PASS (implementation already handles this)

- [ ] **Step 3: Commit**

```bash
git add tests/integration.rs
git commit -m "test: SSE empty stream returns empty vec"
```

---

### Task 3: SSE comments are ignored

**Files:**
- Test: `tests/integration.rs`

- [ ] **Step 1: Write the failing test**

```rust
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
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test send_sse_ignores_comments_and_unknown_fields 2>&1`
Expected: PASS (implementation already skips `:` lines and non-`data:` lines)

- [ ] **Step 3: Commit**

```bash
git add tests/integration.rs
git commit -m "test: SSE comments and unknown fields are ignored"
```

---

### Task 4: HTTP error status triggers ApiError (and retry)

**Files:**
- Test: `tests/integration.rs`

- [ ] **Step 1: Write the failing test**

```rust
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
                        .set_body_string("data: {\"id\":1,\"text\":\"recovered\"}\n\ndata: [DONE]\n\n")
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
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test send_sse_returns_api_error send_sse_retries 2>&1`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add tests/integration.rs
git commit -m "test: SSE error status and retry behavior"
```

---

### Task 5: Malformed JSON in a chunk

**Files:**
- Test: `tests/integration.rs`

- [ ] **Step 1: Write the failing test**

```rust
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
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test send_sse_returns_deserialization_error_on_bad_json 2>&1`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add tests/integration.rs
git commit -m "test: SSE malformed JSON chunk returns DeserializationError"
```

---

### Task 6: Max response bytes on SSE stream

**Files:**
- Test: `tests/integration.rs`

- [ ] **Step 1: Write the failing test**

```rust
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
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test send_sse_enforces_max_response_bytes 2>&1`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add tests/integration.rs
git commit -m "test: SSE stream enforces max response bytes"
```

---

### Task 7: Stream ends without `[DONE]`

**Files:**
- Test: `tests/integration.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn send_sse_returns_chunks_when_stream_ends_without_done() {
    let mock_server = MockServer::start().await;

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
    let chunks: Vec<SseChunk> = client
        .post("/stream-no-done", &payload)
        .unwrap()
        .send_sse()
        .await
        .unwrap();

    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].text, "first");
    assert_eq!(chunks[1].text, "second");
}
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test send_sse_returns_chunks_when_stream_ends_without_done 2>&1`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add tests/integration.rs
git commit -m "test: SSE stream without [DONE] returns collected chunks"
```

---

### Task 8: Final validation

- [ ] **Step 1: Run full test suite**

Run: `cargo test 2>&1`
Expected: all tests PASS

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1`
Expected: no warnings

- [ ] **Step 3: Run fmt check**

Run: `cargo fmt --all -- --check 2>&1`
Expected: no formatting issues

# SSE idle timeout — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an opt-in, SSE-only inter-chunk idle timeout to `rust_json_client` so a stalled-but-open stream aborts with a retryable error instead of hanging forever.

**Architecture:** A new `Option<Duration>` knob on `HttpClient` (`sse_idle_timeout`, default `None`) is threaded into `handle_sse_response`, which wraps only `response.chunk().await` in `tokio::time::timeout`. On elapse it returns a new `HttpClientError::SseIdleTimeout`, classified retryable in `RetryPolicy::is_retryable` exactly like `IncompleteSseStream`, so the existing in-call retry self-heals. No change to non-SSE read paths.

**Tech Stack:** Rust, reqwest 0.12.28 (manual SSE consumption via `Response::chunk()`), tokio 1.52.3 (`time`), bon builder, thiserror, wiremock + raw `tokio::net::TcpListener` for tests.

**Spec:** `docs/superpowers/specs/2026-06-13-sse-idle-timeout-design.md`

---

## File Structure

| File | Responsibility | Change |
|------|----------------|--------|
| `src/error.rs` | Error taxonomy | Add `SseIdleTimeout` variant |
| `src/retry.rs` | Retry classification | Add retryable arm + unit test |
| `src/client.rs` | Client config + SSE read loop | Add knob, struct field, thread into `handle_sse_response`, wrap the read; builder unit tests |
| `tests/integration.rs` | End-to-end behavior | Raw-socket stall server + idle-timeout test |
| `Cargo.toml` | Versioning | `0.5.1` → `0.5.2` |
| `README.md` | Docs | Feature bullet + config example line |

---

## Task 1: `SseIdleTimeout` error variant + retry classification

**Files:**
- Modify: `src/error.rs` (enum `HttpClientError`, after `IncompleteSseStream` at `src/error.rs:42`)
- Modify: `src/retry.rs` (`is_retryable` match arm after `src/retry.rs:59`; unit test after `incomplete_sse_stream_is_retryable`)

- [ ] **Step 1: Write the failing retry unit test**

In `src/retry.rs`, inside `mod tests`, add after the existing `incomplete_sse_stream_is_retryable` test:

```rust
    #[test]
    fn sse_idle_timeout_is_retryable() {
        let policy = RetryPolicy::builder().build();
        assert!(policy.is_retryable(&HttpClientError::SseIdleTimeout));
    }
```

- [ ] **Step 2: Run it to confirm it fails (does not compile yet)**

Run: `cargo test --lib sse_idle_timeout_is_retryable`
Expected: FAIL — compile error `no variant named SseIdleTimeout found for enum HttpClientError`.

- [ ] **Step 3: Add the error variant**

In `src/error.rs`, insert before the closing `}` of the enum (after the `IncompleteSseStream` variant at line 42):

```rust

    /// An SSE stream stalled: no chunk arrived within the configured
    /// `sse_idle_timeout` while the connection stayed open (the upstream
    /// stopped emitting mid-stream). Distinct from `IncompleteSseStream`,
    /// where the connection closed without the `[DONE]` terminator.
    #[error("SSE stream idle timeout (no chunk within configured window)")]
    SseIdleTimeout,
```

- [ ] **Step 4: Add the retryable arm**

In `src/retry.rs`, in `is_retryable`, add the arm immediately after the `IncompleteSseStream => true,` arm (line 59):

```rust
            // A stalled stream is transient — the upstream went quiet but the
            // connection is alive; a retry may land on a healthy upstream.
            HttpClientError::SseIdleTimeout => true,
```

(The match is exhaustive with no wildcard, so the compiler requires this arm — that is the intended safety net.)

- [ ] **Step 5: Run the test to confirm it passes**

Run: `cargo test --lib sse_idle_timeout_is_retryable`
Expected: PASS (1 passed).

- [ ] **Step 6: Commit**

```bash
git add src/error.rs src/retry.rs
git commit -m "feat: add SseIdleTimeout error variant, retryable

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `sse_idle_timeout` knob on `HttpClient` builder

**Files:**
- Modify: `src/client.rs` (struct `HttpClient` `src/client.rs:24-33`; `new` builder `src/client.rs:55-100`; `mod tests`)

Note: bon treats an `Option<T>` member as an optional setter defaulting to `None` (same as the existing `retry_policy: Option<RetryPolicy>` at `src/client.rs:59`) — do **not** add `#[builder(default)]`. The field is only read by tests until Task 3, so it carries `#[cfg_attr(not(test), allow(dead_code))]` here (mirroring the pool fields at `src/client.rs:29-32`); Task 3 removes that attribute when production code starts reading it.

- [ ] **Step 1: Write the failing builder unit tests**

In `src/client.rs`, inside `mod tests`, add after `builder_can_override_connection_pool_settings`:

```rust
    #[test]
    fn builder_has_no_sse_idle_timeout_by_default() {
        let base = Url::parse("https://example.com").unwrap();
        let client = HttpClient::builder().base_url(base).build();
        assert!(client.sse_idle_timeout.is_none());
    }

    #[test]
    fn builder_can_set_sse_idle_timeout() {
        let base = Url::parse("https://example.com").unwrap();
        let client = HttpClient::builder()
            .base_url(base)
            .sse_idle_timeout(Duration::from_secs(60))
            .build();
        assert_eq!(client.sse_idle_timeout, Some(Duration::from_secs(60)));
    }
```

- [ ] **Step 2: Run them to confirm they fail (does not compile yet)**

Run: `cargo test --lib sse_idle_timeout`
Expected: FAIL — compile error `no field 'sse_idle_timeout' on type 'HttpClient'` and `no method named 'sse_idle_timeout'`.

- [ ] **Step 3: Add the struct field**

In `src/client.rs`, in the `HttpClient` struct, add after `pool_max_idle_per_host: usize,` (line 32):

```rust
    // Per-chunk idle timeout for SSE streams (`send_sse`); `None` = no
    // inter-chunk bound. Only read by tests until wired into the SSE loop, so
    // it is dead in non-test builds for now (Task 3 removes this attribute).
    #[cfg_attr(not(test), allow(dead_code))]
    sse_idle_timeout: Option<Duration>,
```

- [ ] **Step 4: Add the builder parameter and construct the field**

In `src/client.rs`, in `new`, add the parameter after `request_timeout` (line 64):

```rust
        sse_idle_timeout: Option<Duration>,
```

So the signature tail reads:

```rust
        #[builder(default = Duration::from_secs(30))] request_timeout: Duration,
        sse_idle_timeout: Option<Duration>,
    ) -> Self {
```

Then in the returned `Self { ... }` (lines 92-99) add the field after `pool_max_idle_per_host,`:

```rust
            sse_idle_timeout,
```

- [ ] **Step 5: Run the tests to confirm they pass**

Run: `cargo test --lib sse_idle_timeout`
Expected: PASS (2 passed: `builder_has_no_sse_idle_timeout_by_default`, `builder_can_set_sse_idle_timeout`).

- [ ] **Step 6: Confirm a clean (warning-free) build**

Run: `cargo build`
Expected: compiles with no warnings (the `allow(dead_code)` keeps the not-yet-read field quiet).

- [ ] **Step 7: Commit**

```bash
git add src/client.rs
git commit -m "feat: add sse_idle_timeout knob to HttpClient builder

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Wrap the SSE read with the idle timeout

**Files:**
- Modify: `src/client.rs` (`handle_sse_response` signature `src/client.rs:245-248`; read loop `src/client.rs:255-279`; call site `src/client.rs:597`; remove the `allow(dead_code)` added in Task 2)
- Test: `tests/integration.rs` (new raw-socket stall server + test, appended at end of file)

- [ ] **Step 1: Write the failing integration test**

In `tests/integration.rs`, append at the end of the file:

```rust
/// Spawns a one-shot raw HTTP/1.1 server that sends SSE headers plus one
/// complete `data:` chunk, then holds the connection open without sending more
/// or closing it — simulating an upstream that stalled mid-stream (the failure
/// mode a total `request_timeout` does not catch). wiremock cannot model
/// "partial body then hang", so this is driven over a raw socket, mirroring
/// `spawn_chunked_oversize_server`.
async fn spawn_stalling_sse_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };

        // Drain the request so the client finishes writing before we respond.
        let mut buf = [0u8; 1024];
        let _ = socket.read(&mut buf).await;

        let head = "HTTP/1.1 200 OK\r\n\
                    Content-Type: text/event-stream\r\n\
                    Transfer-Encoding: chunked\r\n\r\n";
        if socket.write_all(head.as_bytes()).await.is_err() {
            return;
        }

        // One complete SSE chunk frame (no `[DONE]`), then stall.
        let data = "data: {\"id\":1,\"text\":\"partial\"}\n\n";
        let frame = format!("{:x}\r\n{}\r\n", data.len(), data);
        if socket.write_all(frame.as_bytes()).await.is_err() {
            return;
        }

        // Hold the connection open (no further chunks, no close) well past the
        // client's idle timeout.
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    });

    format!("http://{addr}")
}

#[tokio::test]
async fn send_sse_times_out_on_idle_stream() {
    let base = spawn_stalling_sse_server().await;

    let client = HttpClient::builder()
        .base_url(url::Url::parse(&base).unwrap())
        .sse_idle_timeout(std::time::Duration::from_millis(200))
        .build();

    let payload = serde_json::json!({"stream": true});

    // Outer guard: before the idle wrap exists, send_sse hangs; this turns that
    // into a fast, clear failure instead of a hung run. Once implemented, the
    // 200ms idle timeout fires far inside this 5s window.
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        client
            .post("/stream", &payload)
            .unwrap()
            .send_sse::<SseChunk>()
            .await
    })
    .await
    .expect("send_sse did not return within 5s — idle timeout not firing");

    assert!(
        matches!(result, Err(HttpClientError::SseIdleTimeout)),
        "expected SseIdleTimeout, got {result:?}"
    );
}
```

(`TcpListener`, `AsyncReadExt`/`AsyncWriteExt`, `Mock`/wiremock are already imported at the top of the file; `SseChunk` is defined at `tests/integration.rs:651`.)

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test --test integration send_sse_times_out_on_idle_stream`
Expected: FAIL in ~5s — panic `send_sse did not return within 5s — idle timeout not firing` (the knob exists but `handle_sse_response` ignores it, so the stream hangs until the outer guard elapses).

- [ ] **Step 3: Thread the timeout into `handle_sse_response` and wrap the read**

In `src/client.rs`, change the `handle_sse_response` signature (lines 245-248) to take the timeout:

```rust
    async fn handle_sse_response<R: DeserializeOwned>(
        mut response: Response,
        max_response_bytes: usize,
        sse_idle_timeout: Option<Duration>,
    ) -> Result<Vec<R>, HttpClientError> {
```

Replace the read loop (currently `while let Some(chunk) = response.chunk().await? {` at line 255 through its closing brace at line 279) with:

```rust
        loop {
            // Bound the inter-chunk gap (resets after every chunk), not the
            // total stream time: a still-open but stalled upstream aborts here
            // instead of blocking forever. `None` keeps the original behavior.
            let next = match sse_idle_timeout {
                Some(idle) => tokio::time::timeout(idle, response.chunk())
                    .await
                    .map_err(|_| HttpClientError::SseIdleTimeout)??,
                None => response.chunk().await?,
            };
            let Some(chunk) = next else { break };

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
```

(The `let Some(chunk) = next else { break };` preserves the original end-of-stream fall-through to the `IncompleteSseStream` return below the loop.)

- [ ] **Step 4: Pass the knob at the call site**

In `src/client.rs`, in `send_sse`, update the `handle_sse_response` call (line 597) to:

```rust
                HttpClient::handle_sse_response(
                    response,
                    self.client.max_response_bytes,
                    self.client.sse_idle_timeout,
                )
                .await
```

- [ ] **Step 5: Drop the temporary dead-code allow on the field**

In `src/client.rs`, the `sse_idle_timeout` struct field is now read in production. Remove the line added in Task 2:

```rust
    #[cfg_attr(not(test), allow(dead_code))]
```

so the field declaration is just its comment plus `sse_idle_timeout: Option<Duration>,`.

- [ ] **Step 6: Run the new test to confirm it passes**

Run: `cargo test --test integration send_sse_times_out_on_idle_stream`
Expected: PASS in well under 1s (the call returns `Err(SseIdleTimeout)` ~200ms in).

- [ ] **Step 7: Run the full suite to confirm no regression**

Run: `cargo test`
Expected: PASS — all existing SSE tests (which build clients with no `sse_idle_timeout`, i.e. `None`) still pass via the unchanged `response.chunk().await?` path; `cargo build` is warning-free (dead-code allow removed).

- [ ] **Step 8: Commit**

```bash
git add src/client.rs tests/integration.rs
git commit -m "feat: abort stalled SSE stream on inter-chunk idle timeout

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Version bump + docs

**Files:**
- Modify: `Cargo.toml` (`version` field)
- Modify: `README.md` (Features list ~line 27; Client configuration example ~line 118)

- [ ] **Step 1: Bump the version**

In `Cargo.toml`, change:

```toml
version = "0.5.1"
```

to:

```toml
version = "0.5.2"
```

- [ ] **Step 2: Add the Features bullet**

In `README.md`, add after the line `- Configurable connect/request timeouts (5s / 30s defaults)` (line 27):

```markdown
- Optional per-chunk SSE idle timeout (`sse_idle_timeout`) — aborts a stalled stream when the gap between chunks exceeds the bound (retryable); default off
```

- [ ] **Step 3: Add the config-example line**

In `README.md`, in the `### Client configuration` example, add after the `.request_timeout(Duration::from_secs(10))` line (line 118):

```rust
    .sse_idle_timeout(Duration::from_secs(60)) // abort an SSE stream if a chunk gap exceeds 60s
```

- [ ] **Step 4: Confirm the crate still builds and tests pass**

Run: `cargo test`
Expected: PASS (no code change in this task; confirms the version/doc edits did not break anything).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml README.md
git commit -m "chore: bump to 0.5.2, document sse_idle_timeout

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

**Spec coverage:**
- Knob `sse_idle_timeout: Option<Duration>`, default `None` → Task 2. ✓
- Wrap only `response.chunk().await` in `handle_sse_response` → Task 3 (Step 3). ✓
- New retryable variant mirroring `IncompleteSseStream` → Task 1. ✓
- Self-heal via existing `execute_with_retry` → covered by the retryable arm (Task 1); no code change needed in `send_sse`'s retry wrapper. ✓
- Tests: stall → `SseIdleTimeout` (Task 3); `None` = no regression (Task 3 Step 7 full suite); retry unit test (Task 1). ✓ Note: the spec's "healthy stream not killed" case is covered transitively by the existing passing SSE tests under the unchanged `None` path; a dedicated sub-idle-gap test is omitted as low-value (YAGNI) since the timeout logic is a single `match` with no per-gap accumulation.
- Version bump + README → Task 4. ✓
- `error.rs`/`retry.rs` additive only; non-SSE paths untouched → respected. ✓

**Placeholder scan:** No TBD/TODO; every code step shows complete code and exact commands/expected output.

**Type consistency:** `sse_idle_timeout: Option<Duration>` is identical across struct field, `new` param, `Self` construction, `handle_sse_response` param, and call site (`self.client.sse_idle_timeout`, `Duration: Copy` so no clone). Variant name `HttpClientError::SseIdleTimeout` is identical in `error.rs`, `retry.rs` arm + test, the loop's `map_err`, and the integration assertion. Builder setter `.sse_idle_timeout(Duration)` matches the bon `Option<T>` convention used by `retry_policy`.

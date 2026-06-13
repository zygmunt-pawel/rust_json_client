# SSE idle timeout — design

**Date:** 2026-06-13
**Package:** `rust_json_client` (`0.5.1` → `0.5.2`)
**Status:** approved design, pending implementation plan

## Problem

`HttpClient::send_sse` consumes the event stream chunk-by-chunk in
`handle_sse_response` (`src/client.rs:255`):

```rust
while let Some(chunk) = response.chunk().await? {
```

If the upstream stops emitting tokens mid-response but does **not** close the
connection (a stalled box / LAN hiccup), `response.chunk().await` blocks
forever. None of the existing timeouts cover this:

- `connect_timeout` (default 5s) — TCP connect only.
- `request_timeout` / reqwest `.timeout()` (default 30s) — a *total* deadline,
  not an inter-chunk one; in the field a `600s` total still let the stream hang
  far past any useful bound, and it does not distinguish "slow but progressing"
  from "dead".
- `pool_idle_timeout` (90s) — pool connection reuse, unrelated.

Downstream, the caller held a single shared concurrency permit for the whole
hang, turning a local stall into a global stall until an outer lease expired
(~40 min). The package-level defect is the missing **inter-chunk idle bound** on
the SSE read loop.

## Goal

Detect a stalled SSE stream by the **gap between chunks** (resets after every
chunk/token), not by total elapsed time, and surface it as a retryable error so
the existing retry path self-heals. Scope strictly to SSE — no behavior change
for JSON / `send_bytes` / `send_text` callers.

## Rejected alternative: client-level `read_timeout`

reqwest `0.12.28` ships `ClientBuilder::read_timeout` — a native per-read idle
timeout with exactly the right "resets after a successful read" semantics. It
was the first candidate, but rejected:

- `read_timeout` exists **only** on `Client`, not on `RequestBuilder` (verified
  in the pinned source: `RequestBuilder` has `timeout` but no `read_timeout`).
  A client-level setting applies to **every** body read on that client — JSON,
  bytes, text, SSE alike.
- This package cannot guarantee "SSE-only" that way: it would be SSE-scoped only
  if the consumer happens to keep a dedicated `HttpClient` for streaming, which
  the package neither controls nor can enforce. A shared client would impose the
  idle bound on ordinary JSON calls too.

Because the requirement is explicitly SSE-only, we scope the timeout ourselves
in the SSE loop instead.

## Design

A dedicated, opt-in idle timeout wrapped around the SSE chunk read only.

### 1. New knob — `HttpClient::new` builder

```rust
#[builder(default)] sse_idle_timeout: Option<Duration>,   // default None
```

`None` = current behavior, zero change for existing callers. Stored on
`HttpClient` alongside `max_response_bytes`. The consumer's chat client opts in
(e.g. `~60s`); ordinary clients leave it unset.

This is independent of and complementary to `request_timeout`: `request_timeout`
remains the total deadline, `sse_idle_timeout` bounds the inter-chunk gap. A
healthy long stream that emits a token within every `sse_idle_timeout` window
stays alive up to the total deadline; a stalled one dies after one idle window.

### 2. The loop — `handle_sse_response`

`handle_sse_response` takes the new `Option<Duration>` (threaded from
`self.client.sse_idle_timeout` in `send_sse`). Only the read is wrapped:

```rust
let chunk = match sse_idle_timeout {
    Some(idle) => tokio::time::timeout(idle, response.chunk())
        .await
        .map_err(|_| HttpClientError::SseIdleTimeout)??,
    None => response.chunk().await?,   // unchanged path
};
```

- The outer `?` maps `tokio::time::error::Elapsed` → `SseIdleTimeout`; the inner
  `?` propagates a genuine `reqwest::Error`.
- On elapse, the `response.chunk()` future is dropped (read cancelled). The
  function returns `Err`, so `response` drops and reqwest tears the connection
  down. Accumulated partial chunks are discarded — consistent with the existing
  `IncompleteSseStream` "never surface half an answer" philosophy.
- `tokio`'s `time` feature is already enabled (`Cargo.toml`), no new dependency.

### 3. New error variant — `error.rs`

```rust
/// An SSE stream stalled: no chunk arrived within `sse_idle_timeout`. The
/// connection was still open but the upstream stopped emitting mid-stream.
/// Distinct from `IncompleteSseStream` (connection closed without `[DONE]`).
#[error("SSE stream idle timeout (no chunk within configured window)")]
SseIdleTimeout,
```

### 4. Retry classification — `retry.rs`

`is_retryable` is an exhaustive match (no wildcard), so the new variant forces a
compile error until handled — the intended safety net. Add, mirroring
`IncompleteSseStream`:

```rust
HttpClientError::SseIdleTimeout => true,
```

A stalled stream is transient; retrying may hit a healthy upstream. Because
`send_sse` runs inside `execute_with_retry`, the idle timeout self-heals within
the same call — the retried attempt fires immediately instead of waiting on any
outer lease.

## Testing (TDD)

Harness: `wiremock 0.6.5`, matching the existing SSE edge-case tests in
`src/client.rs` (`mod tests`).

1. **Stall triggers idle timeout, retryable.** Mock responds with a partial SSE
   frame, then holds the connection open without sending `[DONE]` (a delayed /
   never-completing body). With `sse_idle_timeout = Some(short)`, `send_sse`
   returns `Err(HttpClientError::SseIdleTimeout)` in roughly the idle window
   (not the total deadline), and `RetryPolicy::is_retryable` is `true` for it.
   - If wiremock cannot model "headers + partial body then hang" cleanly, fall
     back to a hand-rolled `tokio::net::TcpListener` that writes a partial SSE
     frame and then sleeps, asserting the same outcome.
2. **`None` = no regression.** With `sse_idle_timeout = None`, existing SSE
   behavior is unchanged (a completing stream returns its chunks; a closed
   stream still yields `IncompleteSseStream`).
3. **Healthy stream is not killed.** Chunks arriving within the idle window
   complete normally even when individual gaps approach (but stay under) the
   timeout.
4. **Retry unit test.** `is_retryable(&HttpClientError::SseIdleTimeout) == true`,
   alongside the existing `incomplete_sse_stream_is_retryable` test.

## Scope of changes

- `src/client.rs` — builder arg, `HttpClient` field, thread into and wrap inside
  `handle_sse_response`; new tests in `mod tests`.
- `src/error.rs` — `SseIdleTimeout` variant.
- `src/retry.rs` — retryable arm + unit test.
- `Cargo.toml` — version `0.5.1` → `0.5.2`.
- `README` / docstrings — document the knob and its SSE-only, inter-chunk
  semantics.

No changes to `error.rs`/`retry.rs` public behavior beyond the additive variant.

## Out of scope

- Consumer-side wiring (`llm_client` lives in a separate repo; it opts in with
  one builder line after the bump).
- Client-level `read_timeout` for non-SSE reads (rejected above; can be
  revisited separately if a JSON-read idle bound is ever wanted).

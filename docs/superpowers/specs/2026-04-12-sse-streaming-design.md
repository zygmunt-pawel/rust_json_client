# SSE Streaming Support (`send_sse`)

## Problem

LLM API requests behind Cloudflare Tunnel get killed when the model takes too long to respond. The tunnel enforces a hardcoded HTTP response timeout (~30s) that cannot be configured. Streaming (SSE) solves this because tokens arrive continuously, resetting the timeout.

The client needs to support SSE responses so callers can use `"stream": true` with OpenAI-compatible APIs without the connection being cut.

## Design

### New method: `RequestBuilder::send_sse<R>()`

```rust
pub async fn send_sse<R: DeserializeOwned>(self) -> Result<Vec<R>, HttpClientError>
```

Added to `RequestBuilder` in `client.rs`, alongside the existing `send::<R>()`.

### Behavior

1. **Request phase** — identical to `send()`: builds URL, validates path, executes via `execute_with_retry` with the same retry policy, status code checking, Retry-After parsing, and max_response_bytes enforcement.

2. **Response phase** (after HTTP 200) — differs from `send()`:
   - Reads response body via `response.chunk()` in a loop
   - Maintains a line buffer to handle chunks split across network boundaries
   - Splits buffer on `\n` and processes complete lines:
     - Empty lines: skip
     - Lines starting with `:`: skip (SSE comments)
     - `data: [DONE]`: stop reading, return collected results
     - `data: <json>`: deserialize to `R`, push to `Vec<R>`
   - Tracks cumulative bytes received against `max_response_bytes`
   - Returns `Ok(Vec<R>)` when stream completes

### Error handling

No new error variants needed. Existing `HttpClientError` covers all cases:

| Scenario | Error variant |
|----------|--------------|
| HTTP 4xx/5xx | `ApiError { status, body, retry_after }` |
| Transport error / stream cut | `RequestError(reqwest::Error)` — retryable |
| Malformed JSON in chunk | `DeserializationError(serde_json::Error)` |
| Response too large | `ResponseTooLarge { limit, received }` |
| Bad URL/path | `UrlError` / `InvalidRequestPath` (unchanged) |

### Retry behavior

Same as `send()` — retry wraps the entire request. If a stream is cut mid-way, the transport error is retryable and the full request is retried from scratch.

### What does NOT change

- `send::<R>()` — untouched
- `HttpClientError` enum — no new variants
- `RetryPolicy` — no changes
- Builder API — no changes
- Existing tests — no changes

### Caller usage

```rust
#[derive(Deserialize)]
struct SseChunk {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    delta: Delta,
}

#[derive(Deserialize)]
struct Delta {
    content: Option<String>,
}

let chunks: Vec<SseChunk> = client
    .post("/chat/completions", &body)?
    .send_sse::<SseChunk>()
    .await?;

let content: String = chunks
    .iter()
    .filter_map(|c| c.choices.first()?.delta.content.as_deref())
    .collect();
```

The client handles SSE transport. The caller defines chunk shape and folds results.

## Tests

All using wiremock mock server:

1. **Happy path** — multi-chunk SSE response, verify all chunks deserialized and collected
2. **Empty stream** — only `data: [DONE]`, returns empty `Vec`
3. **Stream interrupted** — server closes connection mid-stream, returns `RequestError` (retryable)
4. **Chunk split across network boundaries** — a single SSE line arrives in two `chunk()` calls
5. **Malformed JSON in chunk** — returns `DeserializationError`
6. **SSE comments ignored** — lines starting with `:` are skipped
7. **HTTP error status** — 429/500 triggers retry, 401 returns `ApiError`
8. **Max response bytes** — stream exceeding limit returns `ResponseTooLarge`
9. **Retry on stream failure** — verify retry fires on transport error during streaming

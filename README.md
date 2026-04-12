# rust_json_client

Opinionated async HTTP client for JSON APIs built on top of [reqwest](https://docs.rs/reqwest).

## Features

- Builder pattern via [bon](https://docs.rs/bon)
- Automatic retries with exponential backoff (powered by [backon](https://docs.rs/backon))
- Configurable response size limits
- Structured logging via [tracing](https://docs.rs/tracing)
- Path traversal protection
- No redirect following (safe for API clients)

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
rust_json_client = { git = "https://github.com/zygmunt-pawel/rust_json_client.git" }
```

```rust
use rust_json_client::{HttpClient, RetryPolicy};
use url::Url;

let client = HttpClient::builder()
    .base_url(Url::parse("https://api.example.com/v1")?)
    .retry_policy(RetryPolicy::builder().build())
    .build();

let response: serde_json::Value = client.get("/health").send().await?;
```

## License

MIT

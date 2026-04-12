use rust_json_client::{HttpClient, HttpClientError, RetryPolicy};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::time::Duration;
use tracing_subscriber::EnvFilter;
use url::Url;

#[derive(Debug, Deserialize)]
struct HealthResponse {
    status: String,
}

#[derive(Debug, Serialize)]
struct CreateJobRequest {
    job: String,
}

#[derive(Debug, Deserialize)]
struct CreateJobResponse {
    id: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("rust_json_client=debug")),
        )
        .init();

    let retry_policy = RetryPolicy::builder()
        .max_attempts(NonZeroU32::new(3).unwrap())
        .retryable_status_codes(vec![
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::GATEWAY_TIMEOUT,
        ])
        .build();

    let client = HttpClient::builder()
        .base_url(Url::parse("https://api.example.com/v1")?)
        .retry_policy(retry_policy)
        .connect_timeout(Duration::from_secs(3))
        .request_timeout(Duration::from_secs(10))
        .max_response_bytes(1024 * 1024)
        .pool_idle_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(32)
        .build();

    let health: HealthResponse = client.get("/health").send().await?;
    println!("health status: {}", health.status);

    let create_job_retry = RetryPolicy::builder()
        .max_attempts(NonZeroU32::new(1).unwrap())
        .retryable_status_codes(vec![StatusCode::SERVICE_UNAVAILABLE])
        .build();

    let create_job = CreateJobRequest {
        job: "sync-products".to_string(),
    };

    let result: Result<CreateJobResponse, HttpClientError> = client
        .post("/jobs", &create_job)?
        .with_retry(create_job_retry)
        .send()
        .await;

    match result {
        Ok(response) => {
            println!("created job id={}", response.id);
            Ok(())
        }
        Err(HttpClientError::ApiError {
            status: StatusCode::CONFLICT,
            body,
            ..
        }) => {
            eprintln!("job already exists: {body}");
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

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

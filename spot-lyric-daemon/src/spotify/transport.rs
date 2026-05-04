use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    Method, Url,
};
use serde::de::DeserializeOwned;

use crate::{
    error::{DaemonError, Result},
    types::AuthSnapshot,
};

use super::discovery::ProtocolRegistry;

const MAX_NETWORK_RETRIES: usize = 3;
const MAX_RATE_LIMIT_RETRIES: usize = 4;

#[async_trait]
pub trait AuthProvider: Send + Sync {
    async fn get_access_token(&self) -> Result<String>;
    async fn get_client_token(&self) -> Result<String>;
    async fn force_refresh(&self) -> Result<AuthSnapshot>;
}

#[derive(Debug, Clone)]
pub enum TransportBody {
    Json(serde_json::Value),
    Text(String),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, Copy)]
pub enum ResponseType {
    Json,
    Text,
    Bytes,
}

#[derive(Debug, Clone)]
pub struct TransportRequest {
    pub url: Url,
    pub method: Method,
    pub headers: HashMap<String, String>,
    pub body: Option<TransportBody>,
    pub response_type: ResponseType,
    pub with_auth: bool,
}

impl TransportRequest {
    pub fn get(url: Url) -> Self {
        Self {
            url,
            method: Method::GET,
            headers: HashMap::new(),
            body: None,
            response_type: ResponseType::Json,
            with_auth: true,
        }
    }
}

#[derive(Clone)]
pub struct SpotifyTransport {
    auth: Arc<dyn AuthProvider>,
    client: reqwest::Client,
    protocol: ProtocolRegistry,
}

impl SpotifyTransport {
    pub fn new(
        auth: Arc<dyn AuthProvider>,
        client: reqwest::Client,
        protocol: ProtocolRegistry,
    ) -> Self {
        Self {
            auth,
            client,
            protocol,
        }
    }

    pub async fn access_token(&self) -> Result<String> {
        self.auth.get_access_token().await
    }

    pub async fn get_json<T: DeserializeOwned>(
        &self,
        url: impl AsRef<str>,
        headers: Option<HashMap<String, String>>,
    ) -> Result<T> {
        let mut request = TransportRequest::get(parse_url(url.as_ref())?);
        if let Some(headers) = headers {
            request.headers = headers;
        }
        self.request_json(request).await
    }

    pub async fn post_json<T: DeserializeOwned>(
        &self,
        url: impl AsRef<str>,
        body: serde_json::Value,
        headers: Option<HashMap<String, String>>,
    ) -> Result<T> {
        let mut request = TransportRequest::get(parse_url(url.as_ref())?);
        request.method = Method::POST;
        request.body = Some(TransportBody::Json(body));
        if let Some(headers) = headers {
            request.headers = headers;
        }
        self.request_json(request).await
    }

    /// POST JSON body, discard the response body (for endpoints that return empty 200).
    pub async fn post_json_no_response(
        &self,
        url: impl AsRef<str>,
        body: serde_json::Value,
        headers: Option<HashMap<String, String>>,
    ) -> Result<()> {
        let mut request = TransportRequest::get(parse_url(url.as_ref())?);
        request.method = Method::POST;
        request.body = Some(TransportBody::Json(body));
        request.response_type = ResponseType::Text;
        if let Some(headers) = headers {
            request.headers = headers;
        }
        let _ = self.execute(request, 0, false).await?;
        Ok(())
    }

    pub async fn put_json<T: DeserializeOwned>(
        &self,
        url: impl AsRef<str>,
        body: serde_json::Value,
        headers: Option<HashMap<String, String>>,
    ) -> Result<T> {
        let mut request = TransportRequest::get(parse_url(url.as_ref())?);
        request.method = Method::PUT;
        request.body = Some(TransportBody::Json(body));
        if let Some(headers) = headers {
            request.headers = headers;
        }
        self.request_json(request).await
    }

    pub async fn request_bytes(&self, mut request: TransportRequest) -> Result<Vec<u8>> {
        request.response_type = ResponseType::Bytes;
        let response = self.execute(request, 0, false).await?;
        Ok(response.bytes().await?.to_vec())
    }

    pub async fn request_json<T: DeserializeOwned>(
        &self,
        mut request: TransportRequest,
    ) -> Result<T> {
        request.response_type = ResponseType::Json;
        let url = request.url.clone();
        let response = self.execute(request, 0, false).await?;
        let status = response.status();
        let text = response.text().await?;
        serde_json::from_str::<T>(&text).map_err(|e| {
            let preview: String = text.chars().take(300).collect();
            DaemonError::InvalidResponse(format!(
                "JSON decode failed for {url} (HTTP {status}): {e} — body: {preview}"
            ))
        })
    }

    pub async fn request_optional_json<T: DeserializeOwned>(
        &self,
        mut request: TransportRequest,
    ) -> Result<Option<T>> {
        request.response_type = ResponseType::Json;
        let url = request.url.clone();
        let response = self
            .execute_without_rate_limit_retry(request, false)
            .await?;
        let status = response.status();
        if status.as_u16() == 204 {
            return Ok(None);
        }

        let text = response.text().await?;
        if text.trim().is_empty() {
            return Ok(None);
        }

        serde_json::from_str::<T>(&text).map(Some).map_err(|e| {
            let preview: String = text.chars().take(300).collect();
            DaemonError::InvalidResponse(format!(
                "JSON decode failed for {url} (HTTP {status}): {e} — body: {preview}"
            ))
        })
    }

    pub async fn request_text(&self, mut request: TransportRequest) -> Result<String> {
        request.response_type = ResponseType::Text;
        let response = self.execute(request, 0, false).await?;
        Ok(response.text().await?)
    }

    async fn execute(
        &self,
        request: TransportRequest,
        attempt: usize,
        retried_auth: bool,
    ) -> Result<reqwest::Response> {
        let prepared = self.prepare_request(&request).await?;
        let response = match self.client.execute(prepared).await {
            Ok(response) => response,
            Err(error) => {
                if is_retryable_method(&request.method)
                    && attempt < MAX_NETWORK_RETRIES
                    && is_transient_network_error(&error)
                {
                    tokio::time::sleep(network_retry_delay(attempt)).await;
                    return Box::pin(self.execute(request, attempt + 1, retried_auth)).await;
                }
                return Err(error.into());
            }
        };

        if response.status().as_u16() == 429 && attempt < MAX_RATE_LIMIT_RETRIES {
            let delay = retry_delay(&response, attempt);
            tokio::time::sleep(delay).await;
            return Box::pin(self.execute(request, attempt + 1, retried_auth)).await;
        }

        if matches!(response.status().as_u16(), 401 | 403) && !retried_auth {
            tracing::warn!(
                status = response.status().as_u16(),
                url = %request.url,
                "auth failure, refreshing tokens and retrying"
            );
            self.auth.force_refresh().await?;
            return Box::pin(self.execute(request, attempt, true)).await;
        }

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let response_text = response.text().await.unwrap_or_default();
            tracing::error!(
                status,
                url = %request.url,
                body = %response_text.chars().take(500).collect::<String>(),
                "HTTP error response"
            );
            return Err(DaemonError::HttpStatus {
                status,
                method: request.method.to_string(),
                url: request.url.to_string(),
                response_text,
            });
        }

        Ok(response)
    }

    async fn execute_without_rate_limit_retry(
        &self,
        request: TransportRequest,
        retried_auth: bool,
    ) -> Result<reqwest::Response> {
        let prepared = self.prepare_request(&request).await?;
        let response = self.client.execute(prepared).await?;

        if matches!(response.status().as_u16(), 401 | 403) && !retried_auth {
            tracing::warn!(
                status = response.status().as_u16(),
                url = %request.url,
                "auth failure, refreshing tokens and retrying"
            );
            self.auth.force_refresh().await?;
            return Box::pin(self.execute_without_rate_limit_retry(request, true)).await;
        }

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let response_text = response.text().await.unwrap_or_default();
            tracing::error!(
                status,
                url = %request.url,
                body = %response_text.chars().take(500).collect::<String>(),
                "HTTP error response"
            );
            return Err(DaemonError::HttpStatus {
                status,
                method: request.method.to_string(),
                url: request.url.to_string(),
                response_text,
            });
        }

        Ok(response)
    }

    async fn prepare_request(&self, request: &TransportRequest) -> Result<reqwest::Request> {
        let mut headers = HeaderMap::new();
        insert_header(&mut headers, "accept", "application/json")?;
        insert_header(
            &mut headers,
            "accept-language",
            &self.protocol.constants.accept_language,
        )?;
        insert_header(
            &mut headers,
            "app-platform",
            &self.protocol.constants.app_platform,
        )?;
        insert_header(
            &mut headers,
            "spotify-app-version",
            &self.protocol.constants.app_version,
        )?;
        insert_header(&mut headers, "origin", "https://open.spotify.com")?;
        insert_header(&mut headers, "referer", "https://open.spotify.com/")?;

        for (key, value) in &request.headers {
            insert_header(&mut headers, key, value)?;
        }

        if request.with_auth {
            let (access_token, client_token) =
                tokio::try_join!(self.auth.get_access_token(), self.auth.get_client_token(),)?;
            insert_header(
                &mut headers,
                "authorization",
                format!("Bearer {access_token}").as_str(),
            )?;
            insert_header(&mut headers, "client-token", &client_token)?;
        }

        let mut builder = self
            .client
            .request(request.method.clone(), request.url.clone())
            .headers(headers);
        if let Some(body) = &request.body {
            builder = match body {
                TransportBody::Json(value) => builder.json(value),
                TransportBody::Text(value) => builder.body(value.clone()),
                TransportBody::Bytes(value) => builder.body(value.clone()),
            };
        }

        Ok(builder.build()?)
    }
}

fn insert_header(headers: &mut HeaderMap, key: &str, value: &str) -> Result<()> {
    let name = HeaderName::from_bytes(key.as_bytes())
        .map_err(|_| DaemonError::InvalidArgument(format!("invalid header name: {key}")))?;
    let value = HeaderValue::from_str(value)
        .map_err(|_| DaemonError::InvalidArgument(format!("invalid header value for {key}")))?;
    headers.insert(name, value);
    Ok(())
}

fn is_retryable_method(method: &Method) -> bool {
    *method == Method::GET
}

fn is_transient_network_error(error: &reqwest::Error) -> bool {
    error.is_connect() || error.is_timeout()
}

fn network_retry_delay(attempt: usize) -> Duration {
    Duration::from_millis((300 * 2u64.pow(attempt as u32)).min(2_500))
}

fn parse_retry_after_millis(header: Option<&HeaderValue>) -> Option<u64> {
    let header = header?.to_str().ok()?;
    let seconds = header.parse::<u64>().ok()?;
    Some(seconds * 1_000)
}

fn retry_delay(response: &reqwest::Response, attempt: usize) -> Duration {
    parse_retry_after_millis(response.headers().get("retry-after"))
        .map(Duration::from_millis)
        .unwrap_or_else(|| Duration::from_millis((500 * 2u64.pow(attempt as u32)).min(10_000)))
}

fn parse_url(url: &str) -> Result<Url> {
    Url::parse(url)
        .map_err(|error| DaemonError::InvalidArgument(format!("invalid url {url}: {error}")))
}

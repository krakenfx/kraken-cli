/// HTTP clients for Kraken Spot and Futures APIs.
///
/// Each client enforces TLS-only transport (via rustls), handles request
/// signing, and transient-error retry with exponential backoff for network
/// and 5xx errors.
///
/// Rate limiting is server-authoritative: requests are sent immediately with
/// no client-side pre-throttling. When the Kraken API returns a rate limit
/// error, it is surfaced immediately as an enriched `KrakenError::RateLimit`
/// with `suggestion`, `docs_url`, and `retryable` fields so the caller
/// (agent or human) can decide how to proceed.
use std::collections::HashMap;
use std::env;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::Value;

use crate::auth;
use crate::config::{FuturesCredentials, SpotCredentials};
use crate::errors::{KrakenError, Result};
use crate::telemetry;

pub(crate) const DEFAULT_SPOT_URL: &str = "https://api.kraken.com";
pub(crate) const DEFAULT_FUTURES_URL: &str = "https://futures.kraken.com/derivatives/api/v3";
const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF_MS: u64 = 500;
const DANGER_ALLOW_ANY_URL_HOST_ENV: &str = "KRAKEN_DANGER_ALLOW_ANY_URL_HOST";

/// Spot API client with transient-error retry and server-authoritative rate limiting.
pub struct SpotClient {
    http: reqwest::Client,
    base_url: String,
}

/// Futures API client with transient-error retry and server-authoritative rate limiting.
pub struct FuturesClient {
    http: reqwest::Client,
    base_url: String,
}

fn build_default_headers() -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "X-Kraken-Client",
        HeaderValue::from_static(telemetry::CLIENT_NAME),
    );
    headers.insert(
        "X-Kraken-Client-Version",
        HeaderValue::from_static(telemetry::CLIENT_VERSION),
    );
    headers.insert(
        "X-Kraken-Agent-Client",
        HeaderValue::from_str(telemetry::agent_client())
            .map_err(|e| KrakenError::Validation(format!("Invalid agent client header: {e}")))?,
    );
    headers.insert(
        "X-Kraken-Instance-Id",
        HeaderValue::from_str(telemetry::instance_id())
            .map_err(|e| KrakenError::Validation(format!("Invalid instance ID header: {e}")))?,
    );
    headers.insert(
        "x-korigin",
        HeaderValue::from_static(telemetry::KORIGIN_REST),
    );
    Ok(headers)
}

fn build_http_client() -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .use_rustls_tls()
        .timeout(Duration::from_secs(30))
        .user_agent(telemetry::user_agent())
        .default_headers(build_default_headers()?);

    if let Ok(val) = env::var("KRAKEN_DANGER_ACCEPT_INVALID_CERTS") {
        if matches!(val.as_str(), "1" | "true" | "yes") {
            eprintln!(
                "WARNING: TLS certificate verification is DISABLED via \
                 KRAKEN_DANGER_ACCEPT_INVALID_CERTS. Connections are vulnerable to \
                 man-in-the-middle attacks. Do NOT use this in production."
            );
            builder = builder.danger_accept_invalid_certs(true);
        }
    }

    builder
        .build()
        .map_err(|e| KrakenError::Network(format!("Failed to build HTTP client: {e}")))
}

impl SpotClient {
    /// Create a new Spot client.
    pub fn new(base_url: Option<&str>) -> Result<Self> {
        Ok(Self {
            http: build_http_client()?,
            base_url: base_url.unwrap_or(DEFAULT_SPOT_URL).to_string(),
        })
    }

    /// Execute a public GET request (no auth needed), with retry on transient
    /// errors and 5xx server errors (GET is inherently idempotent).
    pub async fn public_get(
        &self,
        endpoint: &str,
        params: &[(&str, &str)],
        verbose: bool,
    ) -> Result<Value> {
        let url = format!("{}/0/public/{}", self.base_url, endpoint);
        if verbose {
            crate::output::verbose(&format!("GET {url}"));
        }

        let mut attempt = 0u32;
        loop {
            let resp = self.http.get(&url).query(params).send().await;
            match resp {
                Ok(r) if r.status().is_server_error() && attempt < MAX_RETRIES => {
                    let status = r.status();
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    if verbose {
                        crate::output::verbose(&format!(
                            "Server error {status}, retry {attempt}/{MAX_RETRIES} after {backoff}ms"
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Ok(r) => return self.parse_spot_response(r, verbose).await,
                Err(e) if is_transient(&e) && attempt < MAX_RETRIES => {
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    if verbose {
                        crate::output::verbose(&format!(
                            "Transient error, retry {attempt}/{MAX_RETRIES} after {backoff}ms"
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Execute a private POST request with Spot signing, retry, and fresh nonce per attempt.
    ///
    /// When `idempotent` is false, only transport-level errors (connect/timeout)
    /// trigger retries. 5xx server errors are not retried because the request may
    /// have been processed server-side (e.g. order placed, funds withdrawn).
    pub async fn private_post(
        &self,
        endpoint: &str,
        params: HashMap<String, String>,
        creds: &SpotCredentials,
        otp: Option<&str>,
        idempotent: bool,
        verbose: bool,
    ) -> Result<Value> {
        let uri_path = format!("/0/private/{}", endpoint);
        let url = format!("{}{}", self.base_url, uri_path);

        let mut attempt = 0u32;
        loop {
            let nonce = auth::generate_nonce()?;
            let mut attempt_params = params.clone();
            attempt_params.insert("nonce".to_string(), nonce.to_string());
            if let Some(otp_val) = otp {
                attempt_params.insert("otp".to_string(), otp_val.to_string());
            }

            let post_data = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(attempt_params.iter())
                .finish();

            let signature = auth::spot_sign(&uri_path, nonce, &post_data, &creds.api_secret)?;

            if verbose {
                crate::output::verbose(&format!("POST {url} (nonce={nonce})"));
            }

            let mut headers = HeaderMap::new();
            headers.insert(
                "API-Key",
                HeaderValue::from_str(&creds.api_key)
                    .map_err(|e| KrakenError::Auth(format!("Invalid API key header: {e}")))?,
            );
            headers.insert(
                "API-Sign",
                HeaderValue::from_str(&signature)
                    .map_err(|e| KrakenError::Auth(format!("Invalid signature header: {e}")))?,
            );

            let resp = self
                .http
                .post(&url)
                .headers(headers)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(post_data)
                .send()
                .await;

            match resp {
                Ok(r) => {
                    let status = r.status();
                    if idempotent && status.is_server_error() && attempt < MAX_RETRIES {
                        attempt += 1;
                        let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                        if verbose {
                            crate::output::verbose(&format!(
                                "Server error {status}, retry {attempt}/{MAX_RETRIES} after {backoff}ms"
                            ));
                        }
                        tokio::time::sleep(Duration::from_millis(backoff)).await;
                        continue;
                    }
                    return self.parse_spot_response(r, verbose).await;
                }
                Err(e) if is_transient(&e) && attempt < MAX_RETRIES => {
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    if verbose {
                        crate::output::verbose(&format!(
                            "Transient error, retry {attempt}/{MAX_RETRIES} after {backoff}ms"
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Execute a private POST with a JSON body and Spot signing.
    ///
    /// Some newer Kraken endpoints (e.g. Level3) expect `application/json`
    /// instead of form-encoded bodies. The signing formula is the same:
    /// `API-Sign = base64(HMAC_SHA512(uri_path + SHA256(nonce + json_body), secret))`.
    pub(crate) async fn private_post_json(
        &self,
        endpoint: &str,
        body: serde_json::Map<String, Value>,
        creds: &SpotCredentials,
        otp: Option<&str>,
        idempotent: bool,
        verbose: bool,
    ) -> Result<Value> {
        let uri_path = format!("/0/private/{}", endpoint);
        let url = format!("{}{}", self.base_url, uri_path);

        let mut attempt = 0u32;
        loop {
            let nonce = auth::generate_nonce()?;
            let mut attempt_body = body.clone();
            attempt_body.insert("nonce".to_string(), Value::Number(nonce.into()));
            if let Some(otp_val) = otp {
                attempt_body.insert("otp".to_string(), Value::String(otp_val.to_string()));
            }

            let json_str = serde_json::to_string(&attempt_body)
                .map_err(|e| KrakenError::Parse(format!("Failed to serialize JSON body: {e}")))?;

            let signature = auth::spot_sign(&uri_path, nonce, &json_str, &creds.api_secret)?;

            if verbose {
                crate::output::verbose(&format!("POST {url} [json] (nonce={nonce})"));
            }

            let mut headers = HeaderMap::new();
            headers.insert(
                "API-Key",
                HeaderValue::from_str(&creds.api_key)
                    .map_err(|e| KrakenError::Auth(format!("Invalid API key header: {e}")))?,
            );
            headers.insert(
                "API-Sign",
                HeaderValue::from_str(&signature)
                    .map_err(|e| KrakenError::Auth(format!("Invalid signature header: {e}")))?,
            );

            let resp = self
                .http
                .post(&url)
                .headers(headers)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json")
                .body(json_str)
                .send()
                .await;

            match resp {
                Ok(r) => {
                    let status = r.status();
                    if idempotent && status.is_server_error() && attempt < MAX_RETRIES {
                        attempt += 1;
                        let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                        if verbose {
                            crate::output::verbose(&format!(
                                "Server error {status}, retry {attempt}/{MAX_RETRIES} after {backoff}ms"
                            ));
                        }
                        tokio::time::sleep(Duration::from_millis(backoff)).await;
                        continue;
                    }
                    return self.parse_spot_response(r, verbose).await;
                }
                Err(e) if is_transient(&e) && attempt < MAX_RETRIES => {
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    if verbose {
                        crate::output::verbose(&format!(
                            "Transient error, retry {attempt}/{MAX_RETRIES} after {backoff}ms"
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Execute a private POST that returns raw bytes (for export retrieval).
    /// Supports OTP, retry on transient errors, and error-envelope detection.
    ///
    /// See `private_post` for the `idempotent` parameter semantics.
    pub async fn private_post_raw(
        &self,
        endpoint: &str,
        params: HashMap<String, String>,
        creds: &SpotCredentials,
        otp: Option<&str>,
        idempotent: bool,
        verbose: bool,
    ) -> Result<Vec<u8>> {
        let uri_path = format!("/0/private/{}", endpoint);
        let url = format!("{}{}", self.base_url, uri_path);

        let mut attempt = 0u32;
        loop {
            let nonce = auth::generate_nonce()?;
            let mut attempt_params = params.clone();
            attempt_params.insert("nonce".to_string(), nonce.to_string());
            if let Some(otp_val) = otp {
                attempt_params.insert("otp".to_string(), otp_val.to_string());
            }

            let post_data = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(attempt_params.iter())
                .finish();
            let signature = auth::spot_sign(&uri_path, nonce, &post_data, &creds.api_secret)?;

            if verbose {
                crate::output::verbose(&format!("POST {url} (raw download, nonce={nonce})"));
            }

            let mut headers = HeaderMap::new();
            headers.insert(
                "API-Key",
                HeaderValue::from_str(&creds.api_key)
                    .map_err(|e| KrakenError::Auth(format!("Invalid API key header: {e}")))?,
            );
            headers.insert(
                "API-Sign",
                HeaderValue::from_str(&signature)
                    .map_err(|e| KrakenError::Auth(format!("Invalid signature header: {e}")))?,
            );

            let resp = self
                .http
                .post(&url)
                .headers(headers)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(post_data)
                .send()
                .await;

            match resp {
                Ok(r) => {
                    let status = r.status();
                    if idempotent && status.is_server_error() && attempt < MAX_RETRIES {
                        attempt += 1;
                        let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                        if verbose {
                            crate::output::verbose(&format!(
                                "Server error {status}, retry {attempt}/{MAX_RETRIES} after {backoff}ms"
                            ));
                        }
                        tokio::time::sleep(Duration::from_millis(backoff)).await;
                        continue;
                    }
                    let bytes = r.bytes().await?;

                    if let Some(err) = parse_spot_error_from_body_bytes(&bytes) {
                        return Err(err);
                    }

                    if !status.is_success() {
                        return Err(KrakenError::Api {
                            category: crate::errors::ErrorCategory::Api,
                            message: format!("HTTP {status}"),
                        });
                    }

                    return Ok(bytes.to_vec());
                }
                Err(e) if is_transient(&e) && attempt < MAX_RETRIES => {
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    if verbose {
                        crate::output::verbose(&format!(
                            "Transient error, retry {attempt}/{MAX_RETRIES} after {backoff}ms"
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    async fn parse_spot_response(&self, resp: reqwest::Response, verbose: bool) -> Result<Value> {
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            if verbose {
                crate::output::verbose(&format!("Response {status}: {}", truncate(&body, 500)));
            }
            if let Ok(parsed) = serde_json::from_str::<Value>(&body) {
                if let Some(errors) = parsed.get("error").and_then(|e| e.as_array()) {
                    let errs: Vec<&str> = errors.iter().filter_map(|e| e.as_str()).collect();
                    if !errs.is_empty() {
                        return Err(KrakenError::from_kraken_error(errs[0]));
                    }
                }
            }
            return Err(KrakenError::Network(format!(
                "HTTP {status}: {}",
                truncate(&body, 200)
            )));
        }

        let body = resp.text().await?;

        if verbose {
            crate::output::verbose(&format!("Response {status}: {}", truncate(&body, 500)));
        }

        let parsed: Value = serde_json::from_str(&body)?;

        if let Some(errors) = parsed.get("error").and_then(|e| e.as_array()) {
            let errs: Vec<&str> = errors.iter().filter_map(|e| e.as_str()).collect();
            if !errs.is_empty() {
                return Err(KrakenError::from_kraken_error(errs[0]));
            }
        }

        if let Some(result) = parsed.get("result") {
            Ok(result.clone())
        } else {
            Ok(parsed)
        }
    }
}

impl FuturesClient {
    /// Create a new Futures client.
    pub fn new(base_url: Option<&str>) -> Result<Self> {
        Ok(Self {
            http: build_http_client()?,
            base_url: base_url.unwrap_or(DEFAULT_FUTURES_URL).to_string(),
        })
    }

    /// Execute a public GET request on the Futures API, with retry on
    /// transient errors and 5xx server errors (GET is inherently idempotent).
    pub async fn public_get(
        &self,
        endpoint: &str,
        params: &[(&str, &str)],
        verbose: bool,
    ) -> Result<Value> {
        let url = format!("{}/{}", self.base_url, endpoint);
        if verbose {
            crate::output::verbose(&format!("GET {url}"));
        }

        let mut attempt = 0u32;
        loop {
            let resp = self.http.get(&url).query(params).send().await;
            match resp {
                Ok(r) if r.status().is_server_error() && attempt < MAX_RETRIES => {
                    let status = r.status();
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    if verbose {
                        crate::output::verbose(&format!(
                            "Server error {status}, retry {attempt}/{MAX_RETRIES} after {backoff}ms"
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Ok(r) => return self.parse_futures_response(r, verbose).await,
                Err(e) if is_transient(&e) && attempt < MAX_RETRIES => {
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    if verbose {
                        crate::output::verbose(&format!(
                            "Transient error, retry {attempt}/{MAX_RETRIES} after {backoff}ms"
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Execute a public GET request on the Futures charts API, with retry on
    /// transient errors and 5xx server errors (GET is inherently idempotent).
    ///
    /// No auth headers.
    pub async fn public_get_charts(
        &self,
        tick_type: &str,
        symbol: &str,
        resolution: &str,
        params: &[(&str, &str)],
        verbose: bool,
    ) -> Result<Value> {
        let mut base_url = url::Url::parse(&self.base_url)
            .map_err(|e| KrakenError::Validation(format!("Invalid futures base URL: {e}")))?;
        // The charts API lives at a different path prefix than the v3 derivatives
        base_url.set_path(&format!("/api/charts/v1/{tick_type}/{symbol}/{resolution}"));
        let url = base_url.to_string();

        if verbose {
            crate::output::verbose(&format!("GET {url}"));
        }

        let mut attempt = 0u32;
        loop {
            let response = self.http.get(&url).query(params).send().await;
            match response {
                Ok(r) if r.status().is_server_error() && attempt < MAX_RETRIES => {
                    let status = r.status();
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    if verbose {
                        crate::output::verbose(&format!(
                            "Server error {status}, retry {attempt}/{MAX_RETRIES} after {backoff}ms"
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Ok(r) => return self.parse_futures_response(r, verbose).await,
                Err(e) if is_transient(&e) && attempt < MAX_RETRIES => {
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    if verbose {
                        crate::output::verbose(&format!(
                            "Transient error, retry {attempt}/{MAX_RETRIES} after {backoff}ms"
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Execute a private GET on the Futures API with auth headers and retry
    /// on transient errors and 5xx server errors (GET is inherently idempotent).
    pub async fn private_get(
        &self,
        endpoint: &str,
        params: &[(&str, &str)],
        creds: &FuturesCredentials,
        verbose: bool,
    ) -> Result<Value> {
        let url = format!("{}/{}", self.base_url, endpoint);
        let endpoint_path = format!("/api/v3/{}", endpoint);

        let mut attempt = 0u32;
        loop {
            let nonce = auth::generate_nonce()?.to_string();
            let authent = auth::futures_sign("", &nonce, &endpoint_path, &creds.api_secret)?;

            if verbose {
                crate::output::verbose(&format!("GET {url} (authenticated)"));
            }

            let resp = self
                .http
                .get(&url)
                .query(params)
                .header("APIKey", &creds.api_key)
                .header("Nonce", &nonce)
                .header("Authent", &authent)
                .send()
                .await;

            match resp {
                Ok(r) if r.status().is_server_error() && attempt < MAX_RETRIES => {
                    let status = r.status();
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    if verbose {
                        crate::output::verbose(&format!(
                            "Server error {status}, retry {attempt}/{MAX_RETRIES} after {backoff}ms"
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Ok(r) => return self.parse_futures_response(r, verbose).await,
                Err(e) if is_transient(&e) && attempt < MAX_RETRIES => {
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    if verbose {
                        crate::output::verbose(&format!(
                            "Transient error, retry {attempt}/{MAX_RETRIES} after {backoff}ms"
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Execute a private POST on the Futures API with auth headers.
    pub(crate) async fn private_post(
        &self,
        endpoint: &str,
        params: HashMap<String, String>,
        creds: &FuturesCredentials,
        verbose: bool,
    ) -> Result<Value> {
        let url = format!("{}/{}", self.base_url, endpoint);
        let endpoint_path = format!("/api/v3/{}", endpoint);

        let mut attempt = 0u32;
        loop {
            let post_data = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(params.iter())
                .finish();

            let nonce = auth::generate_nonce()?.to_string();
            let authent =
                auth::futures_sign(&post_data, &nonce, &endpoint_path, &creds.api_secret)?;

            if verbose {
                crate::output::verbose(&format!("POST {url} (authenticated)"));
                crate::output::verbose(&format!(
                    "Request body: <redacted> ({} bytes)",
                    post_data.len()
                ));
            }

            let resp = self
                .http
                .post(&url)
                .header("APIKey", &creds.api_key)
                .header("Nonce", &nonce)
                .header("Authent", &authent)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(post_data)
                .send()
                .await;

            match resp {
                Ok(r) => return self.parse_futures_response(r, verbose).await,
                Err(e) if is_transient(&e) && attempt < MAX_RETRIES => {
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    if verbose {
                        crate::output::verbose(&format!(
                            "Transient error, retry {attempt}/{MAX_RETRIES} after {backoff}ms"
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Execute a private POST with a JSON value serialized as a form field.
    ///
    /// The Kraken Futures API expects form-urlencoded bodies, even for
    /// endpoints that accept complex JSON payloads (e.g. batchorder).
    /// The JSON is passed as `json=<serialized_value>` in the form body.
    pub(crate) async fn private_post_json(
        &self,
        endpoint: &str,
        body: serde_json::Value,
        creds: &FuturesCredentials,
        verbose: bool,
    ) -> Result<Value> {
        let json_str = serde_json::to_string(&body)
            .map_err(|e| KrakenError::Parse(format!("Failed to serialize JSON body: {e}")))?;
        let mut params = HashMap::new();
        params.insert("json".to_string(), json_str);
        self.private_post(endpoint, params, creds, verbose).await
    }

    /// Execute a private PUT on the Futures API with form-urlencoded body.
    pub(crate) async fn private_put(
        &self,
        endpoint: &str,
        params: HashMap<String, String>,
        creds: &FuturesCredentials,
        verbose: bool,
    ) -> Result<Value> {
        let url = format!("{}/{}", self.base_url, endpoint);
        let endpoint_path = format!("/api/v3/{}", endpoint);

        let mut attempt = 0u32;
        loop {
            let post_data = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(params.iter())
                .finish();

            let nonce = auth::generate_nonce()?.to_string();
            let authent =
                auth::futures_sign(&post_data, &nonce, &endpoint_path, &creds.api_secret)?;

            if verbose {
                crate::output::verbose(&format!("PUT {url} (authenticated)"));
                crate::output::verbose(&format!(
                    "Request body: <redacted> ({} bytes)",
                    post_data.len()
                ));
            }

            let resp = self
                .http
                .put(&url)
                .header("APIKey", &creds.api_key)
                .header("Nonce", &nonce)
                .header("Authent", &authent)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(post_data)
                .send()
                .await;

            match resp {
                Ok(r) => return self.parse_futures_response(r, verbose).await,
                Err(e) if is_transient(&e) && attempt < MAX_RETRIES => {
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    if verbose {
                        crate::output::verbose(&format!(
                            "Transient error, retry {attempt}/{MAX_RETRIES} after {backoff}ms"
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Execute a private GET that returns raw text (for CSV endpoints).
    pub(crate) async fn private_get_raw(
        &self,
        endpoint: &str,
        params: &[(&str, &str)],
        creds: &FuturesCredentials,
        verbose: bool,
    ) -> Result<String> {
        let url = format!("{}/{}", self.base_url, endpoint);
        let endpoint_path = format!("/api/v3/{}", endpoint);

        let mut attempt = 0u32;
        loop {
            let nonce = auth::generate_nonce()?.to_string();
            let authent = auth::futures_sign("", &nonce, &endpoint_path, &creds.api_secret)?;

            if verbose {
                crate::output::verbose(&format!("GET {url} (raw, authenticated)"));
            }

            let resp = self
                .http
                .get(&url)
                .query(params)
                .header("APIKey", &creds.api_key)
                .header("Nonce", &nonce)
                .header("Authent", &authent)
                .send()
                .await;

            match resp {
                Ok(r) if r.status().is_server_error() && attempt < MAX_RETRIES => {
                    let status = r.status();
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    if verbose {
                        crate::output::verbose(&format!(
                            "Server error {status}, retry {attempt}/{MAX_RETRIES} after {backoff}ms"
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Ok(r) => {
                    let status = r.status();
                    let body = r.text().await?;
                    if verbose {
                        crate::output::verbose(&format!(
                            "Response {status}: {}",
                            truncate(&body, 500)
                        ));
                    }
                    if let Some(err) = parse_futures_error_from_body(&body) {
                        return Err(err);
                    }
                    if !status.is_success() {
                        return Err(KrakenError::Network(format!(
                            "HTTP {status}: {}",
                            truncate(&body, 200)
                        )));
                    }
                    return Ok(body);
                }
                Err(e) if is_transient(&e) && attempt < MAX_RETRIES => {
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    if verbose {
                        crate::output::verbose(&format!(
                            "Transient error, retry {attempt}/{MAX_RETRIES} after {backoff}ms"
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    async fn parse_futures_response(
        &self,
        resp: reqwest::Response,
        verbose: bool,
    ) -> Result<Value> {
        let status = resp.status();
        let body = resp.text().await?;

        if verbose {
            crate::output::verbose(&format!("Response {status}: {}", truncate(&body, 500)));
        }

        if !status.is_success() {
            if let Some(err) = parse_futures_error_from_body(&body) {
                return Err(err);
            }
            return Err(KrakenError::Network(format!(
                "HTTP {status}: {}",
                truncate(&body, 200)
            )));
        }

        let parsed: Value = serde_json::from_str(&body)?;

        if let Some(err) = parsed.get("error").and_then(|e| e.as_str()) {
            if !err.is_empty() && err != "success" {
                return Err(KrakenError::from_kraken_error(err));
            }
        }

        Ok(parsed)
    }
}

fn parse_spot_error_from_body_bytes(bytes: &[u8]) -> Option<KrakenError> {
    let parsed: Value = serde_json::from_slice(bytes).ok()?;
    let errors = parsed.get("error")?.as_array()?;
    let first = errors.iter().find_map(|e| e.as_str())?;
    if first.is_empty() {
        return None;
    }
    Some(KrakenError::from_kraken_error(first))
}

fn parse_futures_error_from_body(body: &str) -> Option<KrakenError> {
    let parsed: Value = serde_json::from_str(body).ok()?;
    let err = parsed.get("error").and_then(|e| e.as_str())?;
    if err.is_empty() || err == "success" {
        return None;
    }
    Some(KrakenError::from_kraken_error(err))
}

fn is_transient(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect()
}

/// UTF-8-safe string truncation by character count.
pub(crate) fn truncate(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

fn redact_url_authority(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut parsed) => {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.to_string()
        }
        Err(_) => "<invalid URL>".to_string(),
    }
}

fn is_trusted_override_host(host: &str) -> bool {
    let host_lc = host.to_ascii_lowercase();
    host_lc == "api.kraken.com"
        || host_lc == "futures.kraken.com"
        || host_lc == "ws.kraken.com"
        || host_lc == "ws-auth.kraken.com"
        || host_lc == "ws-l3.kraken.com"
}

fn danger_allow_any_url_host() -> bool {
    env::var(DANGER_ALLOW_ANY_URL_HOST_ENV)
        .map(|v| {
            let v_lc = v.to_ascii_lowercase();
            matches!(v_lc.as_str(), "1" | "true" | "yes")
        })
        .unwrap_or(false)
}

/// Validate that a URL uses a secure scheme (`https` or `wss`).
///
/// Rejects `http`, `ws`, and all other schemes. Error messages redact
/// any embedded credentials from the URL authority.
pub(crate) fn validate_url_scheme(url: &str) -> Result<()> {
    let parsed = url::Url::parse(url).map_err(|_| {
        KrakenError::Validation(format!("Invalid URL: {}", redact_url_authority(url)))
    })?;

    let scheme = parsed.scheme();
    if scheme == "https" || scheme == "wss" {
        if parsed.host().is_none() {
            return Err(KrakenError::Validation(
                "URL must include a host".to_string(),
            ));
        }
        Ok(())
    } else if scheme == "http" || scheme == "ws" {
        Err(KrakenError::Validation(format!(
            "Insecure URL scheme '{}' rejected for {}. Only https:// and wss:// are allowed.",
            scheme,
            redact_url_authority(url)
        )))
    } else {
        Err(KrakenError::Validation(format!(
            "Unsupported URL scheme '{}'. Only https:// and wss:// are allowed.",
            scheme
        )))
    }
}

/// Resolve a URL override with CLI flag > env var > default precedence.
///
/// When a user-provided value is present (CLI flag or env var), it is
/// validated for secure scheme before returning. When both are absent,
/// `None` is returned so the downstream constructor uses its built-in default.
pub fn resolve_url_override(
    cli_flag: Option<&str>,
    env_value: Option<&str>,
) -> Result<Option<String>> {
    match cli_flag.or(env_value) {
        Some(url) => {
            validate_url_scheme(url)?;
            let parsed = url::Url::parse(url)?;
            let host = parsed.host_str().ok_or_else(|| {
                KrakenError::Validation("URL override must include a host".to_string())
            })?;
            if !is_trusted_override_host(host) {
                if danger_allow_any_url_host() {
                    crate::output::warn(&format!(
                        "Using non-Kraken URL host '{host}' because \
                         {DANGER_ALLOW_ANY_URL_HOST_ENV} is enabled."
                    ));
                } else {
                    return Err(KrakenError::Validation(format!(
                        "Untrusted URL host '{host}'. Allowed hosts are production Kraken endpoints. \
                         To override this check, set {DANGER_ALLOW_ANY_URL_HOST_ENV}=1."
                    )));
                }
            }
            Ok(Some(url.to_string()))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_url_scheme_accepts_https() {
        assert!(validate_url_scheme("https://api.kraken.com").is_ok());
    }

    #[test]
    fn validate_url_scheme_accepts_wss() {
        assert!(validate_url_scheme("wss://ws.kraken.com/v2").is_ok());
    }

    #[test]
    fn validate_url_scheme_rejects_http() {
        let err = validate_url_scheme("http://api.kraken.com").unwrap_err();
        assert!(err.to_string().contains("Insecure"));
    }

    #[test]
    fn validate_url_scheme_rejects_ws() {
        let err = validate_url_scheme("ws://ws.kraken.com/v2").unwrap_err();
        assert!(err.to_string().contains("Insecure"));
    }

    #[test]
    fn validate_url_scheme_rejects_ftp() {
        let err = validate_url_scheme("ftp://api.kraken.com").unwrap_err();
        assert!(err.to_string().contains("Unsupported"));
    }

    #[test]
    fn validate_url_scheme_rejects_empty() {
        assert!(validate_url_scheme("").is_err());
    }

    #[test]
    fn validate_url_scheme_rejects_nonsense() {
        assert!(validate_url_scheme("not-a-url").is_err());
    }

    #[test]
    fn validate_url_scheme_rejects_scheme_only() {
        let result = validate_url_scheme("https://");
        // url::Url::parse("https://") may parse but with empty host
        // Either way, it should not pass validation
        if result.is_ok() {
            panic!("Expected scheme-only URL to be rejected");
        }
    }

    #[test]
    fn validate_url_scheme_error_redacts_credentials() {
        let err = validate_url_scheme("http://user:pass@api.kraken.com").unwrap_err();
        let msg = err.to_string();
        assert!(
            !msg.contains("user:pass"),
            "Credentials leaked in error: {msg}"
        );
        assert!(msg.contains("Insecure"));
    }

    #[test]
    fn resolve_url_override_cli_over_env() {
        let result = resolve_url_override(
            Some("https://api.kraken.com"),
            Some("https://futures.kraken.com"),
        )
        .unwrap();
        assert_eq!(result, Some("https://api.kraken.com".to_string()));
    }

    #[test]
    fn resolve_url_override_env_over_default() {
        let result = resolve_url_override(None, Some("https://futures.kraken.com")).unwrap();
        assert_eq!(result, Some("https://futures.kraken.com".to_string()));
    }

    #[test]
    fn resolve_url_override_default_when_both_absent() {
        let result = resolve_url_override(None, None).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_url_override_rejects_insecure_env() {
        let err = resolve_url_override(None, Some("http://api.kraken.com")).unwrap_err();
        assert!(err.to_string().contains("Insecure"));
    }

    #[test]
    fn resolve_url_override_rejects_insecure_cli() {
        let err = resolve_url_override(Some("http://futures.kraken.com"), None).unwrap_err();
        assert!(err.to_string().contains("Insecure"));
    }

    #[test]
    fn resolve_url_override_accepts_prod_ws_auth_host() {
        let result = resolve_url_override(Some("wss://ws-auth.kraken.com/v2"), None).unwrap();
        assert_eq!(result, Some("wss://ws-auth.kraken.com/v2".to_string()));
    }
}

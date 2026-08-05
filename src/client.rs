//! HTTP clients for Kraken Spot and Futures APIs.
//!
//! Each client enforces TLS-only transport (via rustls), handles request
//! signing, and transient-error retry with exponential backoff for network
//! and 5xx errors.
//!
//! Rate limiting is server-authoritative: requests are sent immediately with
//! no client-side pre-throttling. When the Kraken API returns a rate limit
//! error, it is surfaced immediately as an enriched `KrakenError::RateLimit`
//! with `suggestion`, `docs_url`, and `retryable` fields so the caller
//! (agent or human) can decide how to proceed.
//!
//! Diagnostics are emitted via `tracing`: request lines at `debug!`, retries at
//! `warn!`, and response bodies at `trace!` only. Bodies carry account/financial
//! data, so they are never logged above `trace!`, are truncated, and are passed
//! through [`redact_log_body`] to mask live WebSocket tokens.

use std::collections::HashMap;
use std::time::Duration;

use kraken_core::error::Error as WsError;
use kraken_ws::{BoxFuture, Minted, TokenSource};
use reqwest::header::{HeaderMap, HeaderValue};
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;

use crate::auth;
use crate::config::{Credentials, DANGER_ALLOW_ANY_URL_HOST_ENV};
use crate::errors::{KrakenError, Result};
use crate::telemetry;

pub(crate) const DEFAULT_SPOT_URL: &str = "https://api.kraken.com";
pub(crate) const DEFAULT_FUTURES_URL: &str = "https://futures.kraken.com/derivatives/api/v3";
const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF_MS: u64 = 500;

/// Spot API client with transient-error retry and server-authoritative rate limiting.
///
/// `Clone` is cheap: `reqwest::Client` is a handle over a shared connection pool,
/// so cloning reuses the pool rather than opening a new one.
/// [`AppContext::build`](crate::cli::AppContext) relies on this to clone the
/// memoized client into a REPL line's context when the line inherits unchanged
/// transport inputs, keeping the pool alive across lines.
#[derive(Clone)]
pub struct SpotClient {
    http: reqwest::Client,
    base_url: String,
}

/// Futures API client with transient-error retry and server-authoritative rate limiting.
///
/// See [`SpotClient`] for why `Clone` is cheap.
#[derive(Clone)]
pub struct FuturesClient {
    http: reqwest::Client,
    base_url: String,
}

fn build_default_headers() -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    for (name, value) in telemetry::rest_identity_headers() {
        headers.insert(
            name,
            HeaderValue::from_str(value)
                .map_err(|e| KrakenError::Validation(format!("Invalid {name} header: {e}")))?,
        );
    }
    Ok(headers)
}

fn build_http_client(accept_invalid_certs: bool) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .use_rustls_tls()
        .timeout(Duration::from_secs(30))
        .user_agent(telemetry::user_agent())
        .default_headers(build_default_headers()?);

    if accept_invalid_certs {
        tracing::warn!(
            "TLS certificate verification is DISABLED via \
             KRAKEN_DANGER_ACCEPT_INVALID_CERTS. Connections are vulnerable to \
             man-in-the-middle attacks. Do NOT use this in production."
        );
        builder = builder.danger_accept_invalid_certs(true);
    }

    builder
        .build()
        .map_err(|e| KrakenError::Network(format!("Failed to build HTTP client: {e}")))
}

impl SpotClient {
    /// `accept_invalid_certs` (resolved once in `AppContext::from_cli`)
    /// controls whether TLS verification is disabled.
    pub fn new(base_url: Option<&str>, accept_invalid_certs: bool) -> Result<Self> {
        Ok(Self {
            http: build_http_client(accept_invalid_certs)?,
            base_url: base_url.unwrap_or(DEFAULT_SPOT_URL).to_string(),
        })
    }

    /// Execute a public GET request (no auth needed), with retry on transient
    /// errors and 5xx server errors (GET is inherently idempotent).
    pub async fn public_get(&self, endpoint: &str, params: &[(&str, &str)]) -> Result<Value> {
        let url = format!("{}/0/public/{}", self.base_url, endpoint);
        tracing::debug!(method = "GET", %url, "request");

        let mut attempt = 0u32;
        loop {
            let resp = self.http.get(&url).query(params).send().await;
            match resp {
                Ok(r) if r.status().is_server_error() && attempt < MAX_RETRIES => {
                    let status = r.status();
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    tracing::warn!(
                        %status,
                        attempt,
                        max = MAX_RETRIES,
                        backoff_ms = backoff,
                        "server error; retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Ok(r) => return self.parse_spot_response(r, BodyLog::Full).await,
                Err(e) if is_transient(&e) && attempt < MAX_RETRIES => {
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    tracing::warn!(
                        attempt,
                        max = MAX_RETRIES,
                        backoff_ms = backoff,
                        "transient error; retrying"
                    );
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
        creds: &Credentials,
        otp: Option<&SecretString>,
        idempotent: bool,
    ) -> Result<Value> {
        let uri_path = format!("/0/private/{}", endpoint);
        let url = format!("{}{}", self.base_url, uri_path);

        let mut attempt = 0u32;
        loop {
            let nonce = auth::generate_nonce()?;
            let mut attempt_params = params.clone();
            attempt_params.insert("nonce".to_string(), nonce.to_string());
            if let Some(otp_val) = otp {
                attempt_params.insert("otp".to_string(), otp_val.expose_secret().to_string());
            }

            let post_data = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(attempt_params.iter())
                .finish();

            let signature = auth::spot_sign(&uri_path, nonce, &post_data, &creds.api_secret)?;

            tracing::debug!(method = "POST", %url, nonce, "request");

            let mut headers = HeaderMap::new();
            headers.insert(
                "API-Key",
                HeaderValue::from_str(creds.api_key.expose_secret())
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
                        tracing::warn!(
                            %status,
                            attempt,
                            max = MAX_RETRIES,
                            backoff_ms = backoff,
                            "server error; retrying"
                        );
                        tokio::time::sleep(Duration::from_millis(backoff)).await;
                        continue;
                    }
                    return self.parse_spot_response(r, BodyLog::SizeOnly).await;
                }
                Err(e) if is_transient(&e) && attempt < MAX_RETRIES => {
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    tracing::warn!(
                        attempt,
                        max = MAX_RETRIES,
                        backoff_ms = backoff,
                        "transient error; retrying"
                    );
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
        creds: &Credentials,
        otp: Option<&SecretString>,
        idempotent: bool,
    ) -> Result<Value> {
        let uri_path = format!("/0/private/{}", endpoint);
        let url = format!("{}{}", self.base_url, uri_path);

        let mut attempt = 0u32;
        loop {
            let nonce = auth::generate_nonce()?;
            let mut attempt_body = body.clone();
            attempt_body.insert("nonce".to_string(), Value::Number(nonce.into()));
            if let Some(otp_val) = otp {
                attempt_body.insert(
                    "otp".to_string(),
                    Value::String(otp_val.expose_secret().to_string()),
                );
            }

            let json_str = serde_json::to_string(&attempt_body)
                .map_err(|e| KrakenError::Parse(format!("Failed to serialize JSON body: {e}")))?;

            let signature = auth::spot_sign(&uri_path, nonce, &json_str, &creds.api_secret)?;

            tracing::debug!(method = "POST", %url, nonce, content_type = "json", "request");

            let mut headers = HeaderMap::new();
            headers.insert(
                "API-Key",
                HeaderValue::from_str(creds.api_key.expose_secret())
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
                        tracing::warn!(
                            %status,
                            attempt,
                            max = MAX_RETRIES,
                            backoff_ms = backoff,
                            "server error; retrying"
                        );
                        tokio::time::sleep(Duration::from_millis(backoff)).await;
                        continue;
                    }
                    return self.parse_spot_response(r, BodyLog::SizeOnly).await;
                }
                Err(e) if is_transient(&e) && attempt < MAX_RETRIES => {
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    tracing::warn!(
                        attempt,
                        max = MAX_RETRIES,
                        backoff_ms = backoff,
                        "transient error; retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Private POST returning raw bytes, for export retrieval.
    ///
    /// See `private_post` for the `idempotent` parameter semantics.
    pub async fn private_post_raw(
        &self,
        endpoint: &str,
        params: HashMap<String, String>,
        creds: &Credentials,
        otp: Option<&SecretString>,
        idempotent: bool,
    ) -> Result<Vec<u8>> {
        let uri_path = format!("/0/private/{}", endpoint);
        let url = format!("{}{}", self.base_url, uri_path);

        let mut attempt = 0u32;
        loop {
            let nonce = auth::generate_nonce()?;
            let mut attempt_params = params.clone();
            attempt_params.insert("nonce".to_string(), nonce.to_string());
            if let Some(otp_val) = otp {
                attempt_params.insert("otp".to_string(), otp_val.expose_secret().to_string());
            }

            let post_data = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(attempt_params.iter())
                .finish();
            let signature = auth::spot_sign(&uri_path, nonce, &post_data, &creds.api_secret)?;

            tracing::debug!(method = "POST", %url, nonce, raw = true, "request");

            let mut headers = HeaderMap::new();
            headers.insert(
                "API-Key",
                HeaderValue::from_str(creds.api_key.expose_secret())
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
                        tracing::warn!(
                            %status,
                            attempt,
                            max = MAX_RETRIES,
                            backoff_ms = backoff,
                            "server error; retrying"
                        );
                        tokio::time::sleep(Duration::from_millis(backoff)).await;
                        continue;
                    }
                    let bytes = r.bytes().await?;
                    // Raw export payloads are account/financial data — never log the
                    // body, only its size.
                    tracing::trace!(%status, body_bytes = bytes.len(), "response (raw, body omitted)");

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
                    tracing::warn!(
                        attempt,
                        max = MAX_RETRIES,
                        backoff_ms = backoff,
                        "transient error; retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    async fn parse_spot_response(
        &self,
        resp: reqwest::Response,
        body_log: BodyLog,
    ) -> Result<Value> {
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            tracing::debug!(%status, "error response");
            trace_response_body(status, &body, body_log);
            if let Ok(parsed) = serde_json::from_str::<Value>(&body)
                && let Some(errors) = parsed.get("error").and_then(|e| e.as_array())
            {
                let errs: Vec<&str> = errors.iter().filter_map(|e| e.as_str()).collect();
                if !errs.is_empty() {
                    return Err(KrakenError::from_kraken_error(errs[0]));
                }
            }
            return Err(KrakenError::Network(format!(
                "HTTP {status}: {}",
                truncate(&body, 200)
            )));
        }

        let body = resp.text().await?;

        tracing::debug!(%status, "response");
        trace_response_body(status, &body, body_log);

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

/// Whether a response body may be written to a trace log.
///
/// Authenticated/private endpoints return account and financial data (balances,
/// orders, trade history) that must never be logged, so those responses log only
/// their status and byte length — the same treatment the raw export/CSV paths
/// already apply. Public market-data endpoints carry no user data, so their
/// (token-redacted, truncated) body may be logged for diagnostics.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BodyLog {
    /// Public endpoint: log the redacted, truncated body at `trace`.
    Full,
    /// Private/authenticated endpoint: log only status and byte length.
    SizeOnly,
}

/// Emit a response body to a `trace` log according to `body_log`. Never logs the
/// body of a private/authenticated response, so no account or financial data can
/// reach the logs even when `kraken_cli::client=trace` is enabled.
fn trace_response_body(status: reqwest::StatusCode, body: &str, body_log: BodyLog) {
    match body_log {
        BodyLog::Full => tracing::trace!(
            %status,
            body = %truncate(redact_log_body(body).as_ref(), 500),
            "response body"
        ),
        BodyLog::SizeOnly => tracing::trace!(
            %status,
            body_bytes = body.len(),
            "response body (private, body omitted)"
        ),
    }
}

/// Mask the value of every `"token"` field in a JSON response body before it is
/// written to a trace log. WebSocket tokens (from `GetWebSocketsToken`) are live
/// credentials and must never be logged, even partially. Returns the body unchanged
/// when it is not JSON (e.g. an HTML/error page, which carries no token) or holds no
/// token, so token-free responses log byte-for-byte as before.
///
/// Note: this masks only `"token"` fields, not arbitrary account/financial data.
/// That is why response bodies are logged at `trace!` only and never at `debug!`.
fn redact_log_body(body: &str) -> std::borrow::Cow<'_, str> {
    if let Ok(mut value) = serde_json::from_str::<Value>(body)
        && redact_token_fields(&mut value)
    {
        return std::borrow::Cow::Owned(value.to_string());
    }
    std::borrow::Cow::Borrowed(body)
}

/// Recursively replace every `"token"` field value with `<redacted>`. Returns
/// whether anything was redacted so [`redact_log_body`] can leave token-free bodies
/// untouched (and unallocated).
fn redact_token_fields(value: &mut Value) -> bool {
    let mut changed = false;
    match value {
        Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                if key == "token" {
                    *val = Value::String("<redacted>".to_string());
                    changed = true;
                } else if redact_token_fields(val) {
                    changed = true;
                }
            }
        }
        Value::Array(items) => {
            for val in items.iter_mut() {
                if redact_token_fields(val) {
                    changed = true;
                }
            }
        }
        _ => {}
    }
    changed
}

impl FuturesClient {
    /// `accept_invalid_certs` (resolved once in `AppContext::from_cli`)
    /// controls whether TLS verification is disabled.
    pub fn new(base_url: Option<&str>, accept_invalid_certs: bool) -> Result<Self> {
        Ok(Self {
            http: build_http_client(accept_invalid_certs)?,
            base_url: base_url.unwrap_or(DEFAULT_FUTURES_URL).to_string(),
        })
    }

    /// Execute a public GET request on the Futures API, with retry on
    /// transient errors and 5xx server errors (GET is inherently idempotent).
    pub async fn public_get(&self, endpoint: &str, params: &[(&str, &str)]) -> Result<Value> {
        let url = format!("{}/{}", self.base_url, endpoint);
        tracing::debug!(method = "GET", %url, "request");

        let mut attempt = 0u32;
        loop {
            let resp = self.http.get(&url).query(params).send().await;
            match resp {
                Ok(r) if r.status().is_server_error() && attempt < MAX_RETRIES => {
                    let status = r.status();
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    tracing::warn!(
                        %status,
                        attempt,
                        max = MAX_RETRIES,
                        backoff_ms = backoff,
                        "server error; retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Ok(r) => return self.parse_futures_response(r, BodyLog::Full).await,
                Err(e) if is_transient(&e) && attempt < MAX_RETRIES => {
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    tracing::warn!(
                        attempt,
                        max = MAX_RETRIES,
                        backoff_ms = backoff,
                        "transient error; retrying"
                    );
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
        creds: &Credentials,
    ) -> Result<Value> {
        let url = format!("{}/{}", self.base_url, endpoint);
        let endpoint_path = format!("/api/v3/{}", endpoint);

        let mut attempt = 0u32;
        loop {
            let nonce = auth::generate_nonce()?.to_string();
            let authent = auth::futures_sign("", &nonce, &endpoint_path, &creds.api_secret)?;

            tracing::debug!(method = "GET", %url, authenticated = true, "request");

            let resp = self
                .http
                .get(&url)
                .query(params)
                .header("APIKey", creds.api_key.expose_secret())
                .header("Nonce", &nonce)
                .header("Authent", &authent)
                .send()
                .await;

            match resp {
                Ok(r) if r.status().is_server_error() && attempt < MAX_RETRIES => {
                    let status = r.status();
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    tracing::warn!(
                        %status,
                        attempt,
                        max = MAX_RETRIES,
                        backoff_ms = backoff,
                        "server error; retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Ok(r) => return self.parse_futures_response(r, BodyLog::SizeOnly).await,
                Err(e) if is_transient(&e) && attempt < MAX_RETRIES => {
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    tracing::warn!(
                        attempt,
                        max = MAX_RETRIES,
                        backoff_ms = backoff,
                        "transient error; retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Unlike the GETs, 5xx responses are never retried here: a mutating
    /// request may already have been processed server-side (e.g. order placed).
    pub(crate) async fn private_post(
        &self,
        endpoint: &str,
        params: HashMap<String, String>,
        creds: &Credentials,
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

            // Body may contain order parameters (financial data) — log only its size.
            tracing::debug!(
                method = "POST",
                %url,
                authenticated = true,
                body_bytes = post_data.len(),
                "request"
            );

            let resp = self
                .http
                .post(&url)
                .header("APIKey", creds.api_key.expose_secret())
                .header("Nonce", &nonce)
                .header("Authent", &authent)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(post_data)
                .send()
                .await;

            match resp {
                Ok(r) => return self.parse_futures_response(r, BodyLog::SizeOnly).await,
                Err(e) if is_transient(&e) && attempt < MAX_RETRIES => {
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    tracing::warn!(
                        attempt,
                        max = MAX_RETRIES,
                        backoff_ms = backoff,
                        "transient error; retrying"
                    );
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
        creds: &Credentials,
    ) -> Result<Value> {
        let json_str = serde_json::to_string(&body)
            .map_err(|e| KrakenError::Parse(format!("Failed to serialize JSON body: {e}")))?;
        let mut params = HashMap::new();
        params.insert("json".to_string(), json_str);
        self.private_post(endpoint, params, creds).await
    }

    /// 5xx is never retried, for the same reason as `private_post`.
    pub(crate) async fn private_put(
        &self,
        endpoint: &str,
        params: HashMap<String, String>,
        creds: &Credentials,
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

            tracing::debug!(
                method = "PUT",
                %url,
                authenticated = true,
                body_bytes = post_data.len(),
                "request"
            );

            let resp = self
                .http
                .put(&url)
                .header("APIKey", creds.api_key.expose_secret())
                .header("Nonce", &nonce)
                .header("Authent", &authent)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(post_data)
                .send()
                .await;

            match resp {
                Ok(r) => return self.parse_futures_response(r, BodyLog::SizeOnly).await,
                Err(e) if is_transient(&e) && attempt < MAX_RETRIES => {
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    tracing::warn!(
                        attempt,
                        max = MAX_RETRIES,
                        backoff_ms = backoff,
                        "transient error; retrying"
                    );
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
        creds: &Credentials,
    ) -> Result<String> {
        let url = format!("{}/{}", self.base_url, endpoint);
        let endpoint_path = format!("/api/v3/{}", endpoint);

        let mut attempt = 0u32;
        loop {
            let nonce = auth::generate_nonce()?.to_string();
            let authent = auth::futures_sign("", &nonce, &endpoint_path, &creds.api_secret)?;

            tracing::debug!(method = "GET", %url, raw = true, authenticated = true, "request");

            let resp = self
                .http
                .get(&url)
                .query(params)
                .header("APIKey", creds.api_key.expose_secret())
                .header("Nonce", &nonce)
                .header("Authent", &authent)
                .send()
                .await;

            match resp {
                Ok(r) if r.status().is_server_error() && attempt < MAX_RETRIES => {
                    let status = r.status();
                    attempt += 1;
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    tracing::warn!(
                        %status,
                        attempt,
                        max = MAX_RETRIES,
                        backoff_ms = backoff,
                        "server error; retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Ok(r) => {
                    let status = r.status();
                    let body = r.text().await?;
                    // Raw CSV (e.g. account log) is unredacted financial data —
                    // never log the body, only its size.
                    tracing::trace!(%status, body_bytes = body.len(), "response (raw csv, body omitted)");
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
                    tracing::warn!(
                        attempt,
                        max = MAX_RETRIES,
                        backoff_ms = backoff,
                        "transient error; retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    async fn parse_futures_response(
        &self,
        resp: reqwest::Response,
        body_log: BodyLog,
    ) -> Result<Value> {
        let status = resp.status();
        let body = resp.text().await?;

        tracing::debug!(%status, "response");
        trace_response_body(status, &body, body_log);

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

        if let Some(err) = parsed.get("error").and_then(|e| e.as_str())
            && !err.is_empty()
            && err != "success"
        {
            return Err(KrakenError::from_kraken_error(err));
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
        || host_lc == "cli-feedback.kraken.com"
        || host_lc == "cli-feedback.uat.kraken.com"
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

/// Validate a resolved URL override (secure scheme + trusted host).
///
/// clap applies the flag > env precedence upstream (via `#[arg(env = ...)]`),
/// so this receives the already-merged value. `allow_any_host` (resolved once
/// in `AppContext::from_cli`) relaxes the trusted-host check. When a value is
/// present it is validated for secure scheme and trusted host before returning;
/// when absent, `None` is returned so the downstream constructor uses its
/// built-in default.
pub fn resolve_url_override(value: Option<&str>, allow_any_host: bool) -> Result<Option<String>> {
    match value {
        Some(url) => {
            validate_url_scheme(url)?;
            let parsed = url::Url::parse(url)?;
            let host = parsed.host_str().ok_or_else(|| {
                KrakenError::Validation("URL override must include a host".to_string())
            })?;
            if !is_trusted_override_host(host) {
                if allow_any_host {
                    tracing::warn!(
                        %host,
                        "using non-Kraken URL host because {DANGER_ALLOW_ANY_URL_HOST_ENV} is enabled"
                    );
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

/// Live token source for the WS clients: signs `GetWebSocketsToken` with the spot
/// REST client. Constructed by [`AppContext::ws_config`](crate::cli::AppContext).
pub(crate) struct SpotTokenSource {
    client: std::sync::Arc<SpotClient>,
    credentials: std::sync::Arc<Credentials>,
    /// The one-time password from `--otp`, consumed by the first mint. A TOTP is valid for
    /// seconds, so replaying it on a reconnect re-mint hours later can only fail — and the
    /// server's "otp required" on the retry names the real problem, where a replayed stale
    /// code reads as a bad key. `std::sync::Mutex`: taken and released before any await.
    otp: std::sync::Mutex<Option<SecretString>>,
}

impl SpotTokenSource {
    pub(crate) fn new(
        client: std::sync::Arc<SpotClient>,
        credentials: std::sync::Arc<Credentials>,
        otp: Option<SecretString>,
    ) -> Self {
        Self {
            client,
            credentials,
            otp: std::sync::Mutex::new(otp),
        }
    }
}

impl TokenSource for SpotTokenSource {
    fn fetch(&self) -> BoxFuture<kraken_core::error::Result<Minted>> {
        // Clone the cheap handles into the future so it owns its captures and stays
        // `'static`, borrowing none of `self`.
        let client = self.client.clone();
        let credentials = self.credentials.clone();
        let otp = self
            .otp
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        Box::pin(async move {
            tracing::debug!(endpoint = "GetWebSocketsToken", "minting ws token");
            let resp = client
                .private_post(
                    "GetWebSocketsToken",
                    HashMap::new(),
                    &credentials,
                    otp.as_ref(),
                    true,
                )
                .await
                .map_err(map_token_error)?;
            let token = resp
                .get("token")
                .and_then(|t| t.as_str())
                .map(str::to_string)
                .ok_or_else(|| WsError::TokenRejected("response had no token field".to_string()))?;
            // Server-stated lifetime; the cache prefers it over its fallback.
            let expires_in = resp
                .get("expires")
                .and_then(serde_json::Value::as_u64)
                .map(std::time::Duration::from_secs);
            Ok(Minted { token, expires_in })
        })
    }
}

/// Map a REST error from the token mint into the WS error type: an auth rejection becomes
/// [`WsError::TokenRejected`], and anything else (rate limit, network, IO, parse) is carried
/// whole so its category survives to the envelope instead of collapsing into `auth`.
fn map_token_error(err: KrakenError) -> WsError {
    match err {
        // Carry the *raw* reason, not the `Display` form. `WsError::TokenRejected` is re-wrapped
        // into `KrakenError::Auth` at the WS boundary, which re-adds the "Authentication failed: "
        // prefix — so stringifying an already-`Auth` error here would double it
        // ("Authentication failed: Authentication failed: EAPI:Invalid key").
        KrakenError::Auth(reason) => WsError::TokenRejected(reason),
        other => WsError::MintFailed(Box::new(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_auth_failure_is_not_double_prefixed() {
        // Regression guard: an Auth failure is carried by its raw reason, not its Display
        // form. WsError::TokenRejected re-wraps to KrakenError::Auth, whose Display adds
        // "Authentication failed: " — stringifying here first would render it twice.
        let mapped = map_token_error(KrakenError::Auth("EAPI:Invalid key".into()));
        assert!(matches!(&mapped, WsError::TokenRejected(reason) if reason == "EAPI:Invalid key"));
        let rendered = KrakenError::from(mapped).to_string();
        assert_eq!(
            rendered.matches("Authentication failed:").count(),
            1,
            "got: {rendered}"
        );
    }

    #[test]
    fn a_throttled_mint_keeps_its_rate_limit_category() {
        // A throttled mint rides MintFailed with its category intact, so the envelope
        // says `rate_limit` (retryable) rather than degrading to `api` or `auth`.
        let mapped = map_token_error(KrakenError::from_kraken_error("EOrder:Rate limit exceeded"));
        let rendered = KrakenError::from(mapped);
        assert_eq!(
            rendered.category(),
            crate::errors::ErrorCategory::RateLimit,
            "{rendered}"
        );
    }

    #[test]
    fn a_network_mint_failure_keeps_its_category() {
        // A transient network error during the mint must round-trip with its own category —
        // automation treats `auth` as non-retryable (re-key) but `network` as retryable, so
        // collapsing it would send callers down the wrong repair path.
        let mapped = map_token_error(KrakenError::Network("connection reset".into()));
        let rendered = KrakenError::from(mapped);
        assert_eq!(
            rendered.category(),
            crate::errors::ErrorCategory::Network,
            "{rendered}"
        );
    }

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
    fn resolve_url_override_accepts_trusted_spot_host() {
        let result = resolve_url_override(Some("https://api.kraken.com"), false).unwrap();
        assert_eq!(result, Some("https://api.kraken.com".to_string()));
    }

    #[test]
    fn resolve_url_override_accepts_trusted_futures_host() {
        let result = resolve_url_override(Some("https://futures.kraken.com"), false).unwrap();
        assert_eq!(result, Some("https://futures.kraken.com".to_string()));
    }

    #[test]
    fn resolve_url_override_none_when_absent() {
        let result = resolve_url_override(None, false).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_url_override_rejects_insecure_scheme() {
        let err = resolve_url_override(Some("http://api.kraken.com"), false).unwrap_err();
        assert!(err.to_string().contains("Insecure"));
    }

    #[test]
    fn resolve_url_override_rejects_untrusted_host_by_default() {
        let err = resolve_url_override(Some("https://evil.example.com"), false).unwrap_err();
        assert!(err.to_string().contains("Untrusted URL host"));
    }

    #[test]
    fn resolve_url_override_allows_untrusted_host_when_opted_in() {
        let result = resolve_url_override(Some("https://evil.example.com"), true).unwrap();
        assert_eq!(result, Some("https://evil.example.com".to_string()));
    }

    #[test]
    fn resolve_url_override_accepts_feedback_host() {
        let result =
            resolve_url_override(Some("https://cli-feedback.kraken.com/v1/feedback"), false)
                .unwrap();
        assert_eq!(
            result.as_deref(),
            Some("https://cli-feedback.kraken.com/v1/feedback")
        );
    }

    #[test]
    fn resolve_url_override_accepts_prod_ws_auth_host() {
        let result = resolve_url_override(Some("wss://ws-auth.kraken.com/v2"), false).unwrap();
        assert_eq!(result, Some("wss://ws-auth.kraken.com/v2".to_string()));
    }

    #[test]
    fn redact_log_body_masks_token_field() {
        // The GetWebSocketsToken response shape — the token must never reach a log.
        let body = r#"{"error":[],"result":{"expires":900,"token":"LIVE-WS-TOKEN-VALUE"}}"#;
        let out = redact_log_body(body);
        assert!(!out.contains("LIVE-WS-TOKEN-VALUE"));
        assert!(out.contains(r#""token":"<redacted>""#));
    }

    #[test]
    fn redact_log_body_leaves_token_free_response_unchanged() {
        // No token field -> borrowed (no allocation), byte-for-byte identical.
        let body = r#"{"error":[],"result":{"open":{}}}"#;
        let out = redact_log_body(body);
        assert!(matches!(out, std::borrow::Cow::Borrowed(_)));
        assert_eq!(out.as_ref(), body);
    }

    #[test]
    fn redact_log_body_passes_through_non_json() {
        let body = "<html>503 Service Unavailable</html>";
        assert_eq!(redact_log_body(body).as_ref(), body);
    }

    /// Capture everything emitted at `TRACE` while `f` runs. Uses a thread-local
    /// subscriber (`with_default`) so tests running in parallel don't interfere.
    fn capture_trace(f: impl FnOnce()) -> String {
        use std::io::Write;
        use std::sync::{Arc, Mutex};

        use tracing_subscriber::fmt::MakeWriter;

        #[derive(Clone)]
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl Write for Buf {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> MakeWriter<'a> for Buf {
            type Writer = Buf;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }

        let sink = Buf(Arc::new(Mutex::new(Vec::new())));
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(sink.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let bytes = sink.0.lock().unwrap().clone();
        String::from_utf8(bytes).expect("log output is valid UTF-8")
    }

    #[test]
    fn private_response_body_is_never_logged() {
        // A Balance response carries account/financial data — the SizeOnly policy
        // must log only the byte length, never the body itself.
        let body = r#"{"error":[],"result":{"ZUSD":"123456.7890"}}"#;
        let logged = capture_trace(|| {
            trace_response_body(reqwest::StatusCode::OK, body, BodyLog::SizeOnly);
        });
        assert!(
            !logged.contains("123456.7890"),
            "private body leaked into trace logs: {logged}"
        );
        assert!(
            logged.contains("body_bytes"),
            "expected byte-length field, got: {logged}"
        );
    }

    #[test]
    fn public_response_body_is_logged_for_diagnostics() {
        // Public market data carries no user data, so its body is logged at trace.
        let body = r#"{"error":[],"result":{"XXBTZUSD":{"c":["50000.0","1.0"]}}}"#;
        let logged = capture_trace(|| {
            trace_response_body(reqwest::StatusCode::OK, body, BodyLog::Full);
        });
        assert!(
            logged.contains("50000.0"),
            "public body should be logged for diagnostics, got: {logged}"
        );
    }
}

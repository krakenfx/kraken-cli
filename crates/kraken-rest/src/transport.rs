use crate::RestError;
use reqwest::{
    RequestBuilder, StatusCode,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use serde::Deserialize;
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Its own bound so a stalled handshake fails as `is_connect`: nothing was
/// sent.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF: Duration = Duration::from_millis(500);
/// Characters of a public response body kept in a trace log.
const TRACE_BODY_CHARS: usize = 500;

/// Whether a request may be sent again after its reply is lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Reads: resent after a timeout, a dropped connection or a 5xx.
    Retryable,
    /// Writes: resent only when the connection never opened, because after a
    /// timeout or a 5xx the exchange may already have executed it.
    AtMostOnce,
}

impl Delivery {
    pub(crate) const fn once_if(at_most_once: bool) -> Self {
        if at_most_once {
            Self::AtMostOnce
        } else {
            Self::Retryable
        }
    }

    /// A malformed request would fail again, so only a failed connect is
    /// worth resending: a subset of [`never_sent`].
    fn may_resend(self, err: &reqwest::Error) -> bool {
        match self {
            Self::Retryable => err.is_timeout() || err.is_connect(),
            Self::AtMostOnce => err.is_connect(),
        }
    }

    fn retries_server_errors(self) -> bool {
        self == Self::Retryable
    }

    /// A write whose failure leaves its outcome unknown is unconfirmed, not
    /// failed: a 5xx proves nothing whatever its body names.
    fn settle<T>(
        self,
        sent: Result<Reply, RestError>,
        decode: impl FnOnce(Reply) -> Result<T, RestError>,
    ) -> Result<T, RestError> {
        let (failure, server_error) = match sent {
            Ok(reply) => {
                let server_error = reply.status.is_server_error();
                match decode(reply) {
                    Ok(decoded) => return Ok(decoded),
                    Err(failure) => (failure, server_error),
                }
            }
            Err(failure) => (failure, false),
        };
        if self == Self::AtMostOnce && (server_error || failure.may_have_reached_venue()) {
            Err(RestError::Unconfirmed(Box::new(failure)))
        } else {
            Err(failure)
        }
    }
}

/// No request byte reached the wire: the connector failed or the request never
/// built. reqwest sets `is_connect` before any byte is written.
pub(crate) fn never_sent(err: &reqwest::Error) -> bool {
    err.is_connect() || err.is_builder()
}

/// TLS certificate policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tls {
    #[default]
    Verified,
    /// For local fake venues with self-signed certificates only.
    DangerAcceptInvalidCerts,
}

/// Who is calling, sent on every request. Passed in: the crate knows nothing
/// about the binary using it.
#[derive(Debug, Clone, Default)]
pub struct Identity {
    pub user_agent: String,
    pub headers: Vec<(String, String)>,
}

/// How every venue is reached; each venue client adds only its base URL.
#[derive(Debug, Clone, Default)]
pub struct ClientOptions {
    pub tls: Tls,
    pub identity: Identity,
}

/// What a response's body may reveal in a trace log.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyLog {
    /// Public market data: the truncated body.
    Full,
    /// Account data: only its size.
    SizeOnly,
}

/// A response read to the end.
pub(crate) struct Reply {
    pub(crate) status: StatusCode,
    pub(crate) body: Vec<u8>,
}

/// Decodes `json` as `T`, naming the path to the field that failed.
pub(crate) fn parse_json<'a, T: Deserialize<'a>>(
    endpoint: &'static str,
    json: &'a [u8],
) -> Result<T, RestError> {
    let mut deserializer = serde_json::Deserializer::from_slice(json);
    serde_path_to_error::deserialize(&mut deserializer)
        .map_err(|source| RestError::Decode { endpoint, source })
}

impl Reply {
    pub(crate) fn trace(&self, policy: BodyLog) {
        match policy {
            BodyLog::Full => tracing::trace!(
                status = %self.status,
                body = %String::from_utf8_lossy(&self.body).chars().take(TRACE_BODY_CHARS).collect::<String>(),
                "response body"
            ),
            BodyLog::SizeOnly => tracing::trace!(
                status = %self.status,
                body_bytes = self.body.len(),
                "response body (private, body omitted)"
            ),
        }
    }
}

/// The timeouts, identity and TLS policy of every client.
fn client_builder(options: &ClientOptions) -> Result<reqwest::ClientBuilder, RestError> {
    let mut headers = HeaderMap::new();
    for (name, value) in &options.identity.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|e| RestError::Encode(format!("Invalid {name} header: {e}")))?;
        let value = HeaderValue::from_str(value)
            .map_err(|e| RestError::Encode(format!("Invalid {name} header: {e}")))?;
        headers.insert(name, value);
    }
    let builder = reqwest::Client::builder()
        .use_rustls_tls()
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(&options.identity.user_agent)
        .default_headers(headers);
    // The caller warns where it resolves the switch; it knows its name.
    Ok(match options.tls {
        Tls::DangerAcceptInvalidCerts => builder.danger_accept_invalid_certs(true),
        Tls::Verified => builder,
    })
}

/// The one HTTP path every endpoint of both venues shares: connection pool,
/// timeouts, identity and the retry schedule. `Clone` shares the pool.
#[derive(Debug, Clone)]
pub struct Transport {
    http: reqwest::Client,
    backoff: Duration,
}

impl Transport {
    pub fn new(options: &ClientOptions) -> Result<Self, RestError> {
        Ok(Self {
            http: client_builder(options)?.build()?,
            backoff: INITIAL_BACKOFF,
        })
    }

    /// For the browser sign-in, which this crate does not model: the same TLS
    /// policy and identity, but no redirect is followed, so an auth code or a
    /// new key's secret is never replayed to another origin.
    pub fn sign_in_http(options: &ClientOptions) -> Result<reqwest::Client, RestError> {
        Ok(client_builder(options)?
            .redirect(reqwest::redirect::Policy::none())
            .build()?)
    }

    /// Gives up on a silent venue fast and retries at once. Real time, not a
    /// paused clock: a paused clock can fire the timeout before the request
    /// reaches the venue, which would prove nothing.
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(200))
            .build()
            .expect("a client with only a timeout always builds");
        Self {
            http,
            backoff: Duration::from_millis(1),
        }
    }

    pub(crate) fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Sends the request `next` builds and decodes its reply, so every call
    /// gets `delivery`'s verdict on a failure.
    pub(crate) async fn send<T>(
        &self,
        delivery: Delivery,
        next: impl FnMut() -> Result<RequestBuilder, RestError>,
        decode: impl FnOnce(Reply) -> Result<T, RestError>,
    ) -> Result<T, RestError> {
        delivery.settle(self.deliver(delivery, next).await, decode)
    }

    /// Sends the request `next` builds until it is answered or `delivery`
    /// forbids another attempt. `next` runs once per attempt, so each carries a
    /// fresh nonce and signature.
    async fn deliver(
        &self,
        delivery: Delivery,
        mut next: impl FnMut() -> Result<RequestBuilder, RestError>,
    ) -> Result<Reply, RestError> {
        let mut attempt = 0;
        loop {
            match next()?.send().await {
                Ok(response)
                    if response.status().is_server_error()
                        && delivery.retries_server_errors()
                        && attempt < MAX_RETRIES =>
                {
                    attempt += 1;
                    back_off(self.backoff, attempt, Some(response.status())).await;
                }
                Ok(response) => {
                    let status = response.status();
                    tracing::debug!(%status, "response");
                    match response.bytes().await {
                        Ok(body) => {
                            return Ok(Reply {
                                status,
                                body: Vec::from(body),
                            });
                        }
                        // The reply was lost mid-body: a read may ask again, but the
                        // exchange has already acted on a write.
                        Err(_) if delivery.retries_server_errors() && attempt < MAX_RETRIES => {
                            attempt += 1;
                            back_off(self.backoff, attempt, None).await;
                        }
                        Err(err) => return Err(err.into()),
                    }
                }
                Err(err) if delivery.may_resend(&err) && attempt < MAX_RETRIES => {
                    attempt += 1;
                    back_off(self.backoff, attempt, None).await;
                }
                Err(err) => return Err(err.into()),
            }
        }
    }
}

async fn back_off(initial: Duration, attempt: u32, status: Option<StatusCode>) {
    let backoff = initial.saturating_mul(1 << (attempt - 1));
    let backoff_ms = u64::try_from(backoff.as_millis()).unwrap_or(u64::MAX);
    match status {
        Some(status) => tracing::warn!(
            %status,
            attempt,
            max = MAX_RETRIES,
            backoff_ms,
            "server error; retrying"
        ),
        None => tracing::warn!(
            attempt,
            max = MAX_RETRIES,
            backoff_ms,
            "transient error; retrying"
        ),
    }
    tokio::time::sleep(backoff).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    #[tokio::test]
    async fn the_sign_in_client_never_replays_a_body_to_a_redirect() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(
                ResponseTemplate::new(307)
                    .insert_header("location", format!("{}/elsewhere", server.uri())),
            )
            .mount(&server)
            .await;

        let http = Transport::sign_in_http(&ClientOptions::default()).unwrap();
        let response = http
            .post(format!("{}/oauth/token", server.uri()))
            .body("code=the-code")
            .send()
            .await
            .unwrap();

        assert_eq!(response.status().as_u16(), 307);
        let requests = server.received_requests().await.unwrap();
        assert!(requests.iter().all(|r| r.url.path() != "/elsewhere"));
    }
}

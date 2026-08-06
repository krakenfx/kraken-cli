//! [`WsConfig`]: exactly what both clients need to open a socket.

use std::sync::Arc;

use kraken_core::endpoint::Endpoint;

use super::token::{TokenCache, TokenSource};
use super::transport::TungsteniteTransport;

/// Per-endpoint URL overrides, defaulting to the production hosts.
#[derive(Debug, Clone, Default)]
pub struct Urls {
    pub public: Option<String>,
    pub auth: Option<String>,
    pub l3: Option<String>,
}

impl Urls {
    /// The configured override for `endpoint`, or its production default host.
    pub(crate) fn resolve(&self, endpoint: Endpoint) -> String {
        let override_url = match endpoint {
            Endpoint::Public => &self.public,
            Endpoint::Auth => &self.auth,
            Endpoint::L3 => &self.l3,
        };
        override_url
            .clone()
            .unwrap_or_else(|| endpoint.default_url().to_string())
    }
}

/// Everything both WS paths need to open a socket. Plain data with no invariant, so
/// it is built as a struct literal; `Default` is a public-only production client.
///
/// The reconnect schedule ([`ReconnectPolicy`](super::backoff::ReconnectPolicy)) is
/// stream-only and passed to the streaming client directly, not held here — so the
/// request path is never handed a field it must ignore.
#[derive(Clone, Default)]
pub struct WsConfig {
    /// Token fetcher, present when credentials are configured; required by the auth and
    /// L3 endpoints. Whether absence is an error is each client's policy: the streaming
    /// client skips private subscriptions (failing closed only when nothing requested
    /// remains), the request path fails with
    /// [`Error::NoTokenSource`](kraken_core::error::Error::NoTokenSource). Fetches are
    /// cached and single-flighted inside each client; reconnects re-mint.
    pub token_source: Option<Arc<dyn TokenSource>>,
    pub urls: Urls,
    /// Disables TLS verification (`--accept-invalid-certs`), for beta/UAT hosts only.
    pub accept_invalid_certs: bool,
    /// Handshake headers (client identity/telemetry), injected by the application —
    /// this crate carries no telemetry knowledge of its own.
    pub headers: Vec<(String, String)>,
}

/// Hand-written: `token_source` is a credential fetcher (`dyn` and deliberately not
/// `Debug`), and header *values* carry instance identity — names alone suffice.
impl std::fmt::Debug for WsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WsConfig")
            .field(
                "token_source",
                &self.token_source.as_ref().map(|_| "<configured>"),
            )
            .field("urls", &self.urls)
            .field("accept_invalid_certs", &self.accept_invalid_certs)
            .field(
                "headers",
                &self
                    .headers
                    .iter()
                    .map(|(k, _)| k.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl WsConfig {
    /// A transport for `endpoint`, resolving its URL override and the TLS switch.
    pub(crate) fn transport(&self, endpoint: Endpoint) -> TungsteniteTransport {
        TungsteniteTransport::new(
            self.urls.resolve(endpoint),
            self.accept_invalid_certs,
            self.headers.clone(),
        )
    }

    /// A fresh cache over `token_source` — built once per client, so its single-flight
    /// spans exactly that client's connections.
    pub(crate) fn new_token_cache(&self) -> Option<Arc<TokenCache>> {
        self.token_source
            .clone()
            .map(|source| Arc::new(TokenCache::new(source)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_prefers_an_override_then_falls_back_to_the_production_host() {
        let urls = Urls {
            public: None,
            auth: Some("wss://uat-auth.example/v2".to_string()),
            l3: None,
        };
        assert_eq!(urls.resolve(Endpoint::Auth), "wss://uat-auth.example/v2");
        assert_eq!(
            urls.resolve(Endpoint::Public),
            Endpoint::Public.default_url()
        );
        assert_eq!(urls.resolve(Endpoint::L3), Endpoint::L3.default_url());
    }
}

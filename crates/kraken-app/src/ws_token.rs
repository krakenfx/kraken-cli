//! The WS clients' token source: a signed `GetWebSocketsToken` on the Spot REST
//! client. Constructed by [`AppContext::ws_config`](crate::cli::AppContext).

use crate::{
    errors::{KrakenError, SIGN_IN_KEY_EXPIRED},
    venue::Account,
};
use kraken_auth::CredentialSource;
use kraken_core::error::Error as WsError;
use kraken_rest::spot::{SpotAccount, ws_token::WebSocketsTokenQuery};
use kraken_ws::{BoxFuture, Minted, TokenSource};
use secrecy::ExposeSecret as _;
use std::sync::{Mutex, PoisonError};

pub(crate) struct SpotTokenSource {
    /// Re-mints sign without the second factor. A TOTP is valid for seconds,
    /// so replaying it on a reconnect hours later can only fail — and the
    /// server's "otp required" on the retry names the real problem, where a
    /// replayed stale code reads as a bad key.
    renewal: SpotAccount,
    /// The account with its `--otp`, taken by the first mint.
    /// `std::sync::Mutex`: taken and released before any await.
    first: Mutex<Option<SpotAccount>>,
    /// Checked per mint, so a reconnect after the sign-in key expired is
    /// refused with the reason, not with Kraken's "Invalid key".
    source: CredentialSource,
}

impl SpotTokenSource {
    pub(crate) fn new(Account { client, source }: Account<SpotAccount>) -> Self {
        Self {
            renewal: client.clone().with_otp(None),
            first: Mutex::new(Some(client)),
            source,
        }
    }
}

impl TokenSource for SpotTokenSource {
    fn fetch(&self) -> BoxFuture<kraken_core::error::Result<Minted>> {
        if self.source.is_expired() {
            return Box::pin(async { Err(WsError::TokenRejected(SIGN_IN_KEY_EXPIRED.into())) });
        }
        // An owned account keeps the future `'static`, borrowing none of `self`.
        let account = self
            .first
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .unwrap_or_else(|| self.renewal.clone());
        Box::pin(async move {
            tracing::debug!(endpoint = "GetWebSocketsToken", "minting ws token");
            let minted = account
                .sign(&WebSocketsTokenQuery::default())
                .await
                .map_err(|e| map_token_error(e.into()))?;
            Ok(Minted {
                token: minted.token.expose_secret().to_owned(),
                // Server-stated lifetime; the cache prefers it over its fallback.
                expires_in: minted.expires.map(std::time::Duration::from_secs),
            })
        })
    }
}

/// Map a REST error from the token mint into the WS error type: an auth
/// rejection becomes [`WsError::TokenRejected`], and anything else (rate limit,
/// network, IO, parse) is carried whole so its category survives to the
/// envelope instead of collapsing into `auth`.
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
        // Regression guard: an Auth failure is carried by its raw reason, not its
        // Display form. WsError::TokenRejected re-wraps to KrakenError::Auth,
        // whose Display adds "Authentication failed: " — stringifying here
        // first would render it twice.
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
        // A transient network error during the mint must round-trip with its own
        // category — automation treats `auth` as non-retryable (re-key) but
        // `network` as retryable, so collapsing it would send callers down the
        // wrong repair path.
        let mapped = map_token_error(KrakenError::Network("connection reset".into()));
        let rendered = KrakenError::from(mapped);
        assert_eq!(
            rendered.category(),
            crate::errors::ErrorCategory::Network,
            "{rendered}"
        );
    }

    fn account(url: Option<String>, source: CredentialSource) -> Account<SpotAccount> {
        use kraken_rest::{Auth, ClientOptions, Transport, spot::SpotRest};
        use secrecy::SecretString;
        let secret = SecretString::from("dGVzdF9zZWNyZXRfa2V5X2J5dGVzXzMyX2NoYXJzISE=");
        let client = SpotRest::new(Transport::new(&ClientOptions::default()).unwrap(), url)
            .authenticate(Auth::new(SecretString::from("key"), &secret).unwrap());
        Account { client, source }
    }

    #[tokio::test]
    async fn an_expired_login_key_mints_no_token_and_says_why() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::any};

        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        let expired = CredentialSource::SignIn {
            valid_until: chrono::Utc::now() - chrono::Duration::minutes(1),
        };
        let source = SpotTokenSource::new(account(Some(server.uri()), expired));
        let Err(WsError::TokenRejected(reason)) = source.fetch().await else {
            panic!("an expired key must not mint a token");
        };
        assert_eq!(reason, SIGN_IN_KEY_EXPIRED);
    }

    #[tokio::test]
    async fn only_the_first_mint_spends_the_one_time_password() {
        use secrecy::SecretString;
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

        let server = MockServer::start().await;
        Mock::given(path("/0/private/GetWebSocketsToken"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "error": [], "result": {"token": "t", "expires": 900}
            })))
            .mount(&server)
            .await;
        let Account { client, source } = account(Some(server.uri()), CredentialSource::Env);
        let source = SpotTokenSource::new(Account {
            client: client.with_otp(Some(SecretString::from("123456"))),
            source,
        });
        source.fetch().await.unwrap();
        source.fetch().await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let otp = |i: usize| {
            let body: serde_json::Value = serde_json::from_slice(&requests[i].body).unwrap();
            body.get("otp").cloned()
        };
        assert_eq!(otp(0), Some(serde_json::json!("123456")));
        assert_eq!(otp(1), None, "a replayed TOTP can only fail");
    }
}

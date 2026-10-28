use super::{
    authorize::authorize_url,
    config::Config,
    fast_key::{self, ApiKey, CreateRequest, DeleteRequest},
    key_lifetime::KeyLifetime,
    key_name::KeyName,
    permission::Permissions,
    pkce::{State, Verifier},
    redirect::{LOGIN_TIMEOUT, Listener},
    token::{self, AccessToken},
};
use crate::error::Result;
use serde::Serialize;
use url::Url;

/// The entry point. Clones share one HTTP connection pool.
#[derive(Clone, Debug)]
pub struct AuthClient {
    config: Config,
    http: reqwest::Client,
}

impl AuthClient {
    /// The caller supplies `http`, so requests keep its TLS settings and
    /// headers.
    pub fn new(config: Config, http: reqwest::Client) -> Self {
        Self { config, http }
    }

    /// The listener is bound when this returns, so you can open the URL at
    /// once.
    pub async fn start_login(&self, permissions: Permissions) -> Result<PendingLogin> {
        let listener = Listener::bind(&self.config.redirect_ports).await?;
        let verifier = Verifier::generate()?;
        let state = State::generate()?;
        let authorize_url = authorize_url(
            self.config.endpoints.authorize(),
            &self.config.client_id,
            listener.redirect_uri(),
            &permissions.scopes(),
            &state,
            &verifier.challenge(),
        );
        tracing::debug!(redirect_uri = listener.redirect_uri(), "sign-in started");
        Ok(PendingLogin {
            client: self.clone(),
            listener,
            verifier,
            state,
            permissions,
            authorize_url,
        })
    }
}

/// The caller opens [`authorize_url`](Self::authorize_url), so a headless
/// caller can print it instead.
#[must_use = "a sign-in does nothing until `finish` is awaited"]
#[derive(Debug)]
pub struct PendingLogin {
    client: AuthClient,
    listener: Listener,
    verifier: Verifier,
    state: State,
    permissions: Permissions,
    authorize_url: Url,
}

impl PendingLogin {
    pub fn authorize_url(&self) -> &Url {
        &self.authorize_url
    }

    /// Fails with [`Error::TimedOut`](crate::Error::TimedOut) if the user does
    /// not approve within 5 minutes.
    pub async fn finish(self) -> Result<Grant> {
        let code = self.listener.wait(&self.state, LOGIN_TIMEOUT).await?;
        let config = &self.client.config;
        let form = token::code_exchange_form(
            code.expose(),
            self.listener.redirect_uri(),
            config.client_id.as_str(),
            self.verifier.expose(),
        );
        let response = self
            .client
            .http
            .post(config.endpoints.token().clone())
            .form(&form)
            .send()
            .await?;
        let (status, body) = read(response).await?;
        let token = token::parse_token_response(status, &body, &self.permissions.scopes())?;
        Ok(Grant {
            client: self.client,
            token,
            permissions: self.permissions,
        })
    }
}

/// The access token stays in memory only. It is zeroized when the grant drops.
#[derive(Debug)]
pub struct Grant {
    client: AuthClient,
    token: AccessToken,
    permissions: Permissions,
}

impl Grant {
    pub fn permissions(&self) -> &Permissions {
        &self.permissions
    }

    /// The lifetime counts from this call. A failure that can leave a key on
    /// Kraken names it in
    /// [`Error::key_on_account`](crate::Error::key_on_account).
    pub async fn create_key(&self, name: &KeyName, lifetime: KeyLifetime) -> Result<ApiKey> {
        let request = CreateRequest::new(name, &self.permissions, lifetime);
        self.send(reqwest::Method::POST, &request)
            .await
            .and_then(|(status, body)| fast_key::parse_created(status, &body, &request))
            .map_err(|error| fast_key::unconfirmed(error, name))
    }

    pub async fn delete_key(&self, name: &KeyName) -> Result<()> {
        let (status, body) = self
            .send(reqwest::Method::DELETE, &DeleteRequest::from(name))
            .await?;
        fast_key::parse_deleted(status, &body)
    }

    async fn send(&self, method: reqwest::Method, body: &impl Serialize) -> Result<(u16, Vec<u8>)> {
        let url = self.client.config.endpoints.fast_api_key().clone();
        // `bearer_auth` marks the header as sensitive, so reqwest does not log
        // it.
        let response = self
            .client
            .http
            .request(method, url)
            .bearer_auth(self.token.expose())
            .json(body)
            .send()
            .await?;
        read(response).await
    }
}

async fn read(response: reqwest::Response) -> Result<(u16, Vec<u8>)> {
    let status = response.status().as_u16();
    Ok((status, response.bytes().await?.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        error::Error,
        oauth::test_support::{
            GRANTED, client, mount_create, mount_token, permissions, redirect_browser,
        },
    };
    use base64::Engine as _;
    use chrono::Utc;
    use secrecy::ExposeSecret;
    use sha2::Digest as _;
    use std::collections::HashMap;
    use wiremock::{MockServer, Request};

    #[tokio::test]
    async fn a_full_sign_in_creates_a_restricted_expiring_key() {
        let server = MockServer::start().await;
        mount_token(&server, GRANTED).await;
        mount_create(&server, |_| {}).await;

        let login = client(&server)
            .await
            .start_login(permissions())
            .await
            .unwrap();
        redirect_browser(&login, "the-code").await;
        let grant = login.finish().await.unwrap();
        let name: KeyName = "kraken-cli-0000beef".parse().unwrap();
        let before = Utc::now();
        let key = grant
            .create_key(&name, KeyLifetime::default())
            .await
            .unwrap();
        let after = Utc::now();

        assert_eq!(key.secret().expose_secret(), "the-secret");
        let requests = server.received_requests().await.unwrap();
        let create = requests
            .iter()
            .find(|r| r.url.path() == "/oauth/fast-api-key")
            .unwrap();
        assert_eq!(create.headers["authorization"], "Bearer the-token");
        let body: serde_json::Value = serde_json::from_slice(&create.body).unwrap();
        assert_eq!(
            body["permissions"],
            serde_json::json!({ "funds_query": true, "trades_modify": true })
        );
        let requested = body["valid_until"].as_i64().unwrap();
        let lifetime = KeyLifetime::default();
        assert!(
            (lifetime.expires_at(before).timestamp()..=lifetime.expires_at(after).timestamp())
                .contains(&requested),
            "{requested}"
        );
    }

    /// Real Kraken refuses a mismatch with `invalid_grant`; the fake must too.
    #[tokio::test]
    async fn the_code_exchange_proves_the_challenge_and_sends_no_secret() {
        let server = MockServer::start().await;
        mount_token(&server, GRANTED).await;

        let login = client(&server)
            .await
            .start_login(permissions())
            .await
            .unwrap();
        redirect_browser(&login, "the-code").await;
        let sent: HashMap<String, String> =
            login.authorize_url().query_pairs().into_owned().collect();
        login.finish().await.unwrap();

        let requests: Vec<Request> = server.received_requests().await.unwrap();
        let exchange = requests
            .iter()
            .find(|r| r.url.path() == "/oauth/token")
            .unwrap();
        assert!(!exchange.headers.contains_key("authorization"));
        let form: HashMap<String, String> = url::form_urlencoded::parse(&exchange.body)
            .into_owned()
            .collect();
        assert_eq!(form["redirect_uri"], sent["redirect_uri"]);
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(form["code_verifier"].as_bytes()));
        assert_eq!(challenge, sent["code_challenge"]);
    }

    #[tokio::test]
    async fn a_narrowed_grant_fails_before_any_key_exists() {
        let server = MockServer::start().await;
        mount_token(
            &server,
            "account.fast-api-key:write account.fast-api-key:multiple",
        )
        .await;

        let login = client(&server)
            .await
            .start_login(permissions())
            .await
            .unwrap();
        redirect_browser(&login, "the-code").await;
        let error = login.finish().await.unwrap_err();

        assert!(matches!(error, Error::ScopeNotGranted { .. }));
        let requests = server.received_requests().await.unwrap();
        assert!(
            requests
                .iter()
                .all(|r| r.url.path() != "/oauth/fast-api-key")
        );
    }
}

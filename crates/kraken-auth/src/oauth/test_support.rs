//! A mock Kraken and a fake browser, so a test can reach a real `Grant`.
use super::{
    client::{AuthClient, Grant, PendingLogin},
    config::{Config, Endpoints, RedirectPorts},
    permission::{Permission, Permissions},
};
use std::{collections::HashMap, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{body_string_contains, method, path},
};

/// Answers a key creation as Kraken does: with the requested name and expiry,
/// and the one-time secret. `change` then edits the `result`.
pub(crate) async fn mount_create(
    server: &MockServer,
    change: impl Fn(&mut serde_json::Value) + Send + Sync + 'static,
) {
    Mock::given(method("POST"))
        .and(path("/oauth/fast-api-key"))
        .respond_with(move |request: &Request| {
            let sent: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            let mut result = serde_json::json!({
                "api_key_name": sent["api_key_name"], "api_key": "the-key",
                "secret": "the-secret", "valid_until": sent["valid_until"],
                "permissions": sent["permissions"],
            });
            change(&mut result);
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "error": [], "result": result }))
        })
        .expect(1)
        .mount(server)
        .await;
}

/// Answers a key creation with `reply` as it is.
pub(crate) async fn mount_create_reply(server: &MockServer, reply: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/oauth/fast-api-key"))
        .respond_with(reply)
        .expect(1)
        .mount(server)
        .await;
}

pub(crate) async fn mount_delete(server: &MockServer) {
    mount_delete_reply(
        server,
        ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({ "error": [], "result": true })),
    )
    .await;
}

/// Answers a key deletion with `reply` as it is.
pub(crate) async fn mount_delete_reply(server: &MockServer, reply: ResponseTemplate) {
    Mock::given(method("DELETE"))
        .and(path("/oauth/fast-api-key"))
        .respond_with(reply)
        .expect(1)
        .mount(server)
        .await;
}

pub(crate) const GRANTED: &str = "account.fast-api-key:write account.fast-api-key:multiple \
                                  account.fast-api-key:funds-query \
                                  account.fast-api-key:trades-modify";

pub(crate) fn permissions() -> Permissions {
    [Permission::FundsQuery, Permission::TradesModify]
        .into_iter()
        .collect()
}

/// Follows no redirects, as `Transport::sign_in_http` does in production.
fn sign_in_http() -> reqwest::ClientBuilder {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
}

pub(crate) async fn client(server: &MockServer) -> AuthClient {
    client_with(server, sign_in_http()).await
}

async fn client_with(server: &MockServer, http: reqwest::ClientBuilder) -> AuthClient {
    let base = Url::parse(&server.uri()).unwrap();
    let port = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = Config::new("testclient".parse().unwrap())
        .with_endpoints(Endpoints::new(base.join("/oauth/authorize").unwrap(), base).unwrap())
        .with_redirect_ports(RedirectPorts::new([port]).unwrap());
    AuthClient::new(config, http.build().unwrap())
}

/// Acts as the browser: it follows the redirect that Kraken sends.
pub(crate) async fn redirect_browser(login: &PendingLogin, code: &str) {
    let query: HashMap<_, _> = login.authorize_url().query_pairs().into_owned().collect();
    let callback = Url::parse(&query["redirect_uri"]).unwrap();
    let port = callback.port().unwrap();
    let state = query["state"].clone();
    let code = code.to_string();
    tokio::spawn(async move {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let request =
            format!("GET /callback?code={code}&state={state} HTTP/1.1\r\nHost: localhost\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut page = String::new();
        stream.read_to_string(&mut page).await.unwrap();
    });
}

pub(crate) async fn mount_token(server: &MockServer, scope: &str) {
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code=the-code"))
        .and(body_string_contains("client_id=testclient"))
        .and(body_string_contains("code_verifier="))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "the-token", "token_type": "bearer",
            "expires_in": 14400, "scope": scope,
        })))
        .expect(1)
        .mount(server)
        .await;
}

/// A finished sign-in against `server`, with every scope granted.
pub(crate) async fn grant(server: &MockServer) -> Grant {
    grant_from(server, client(server).await).await
}

/// A grant whose every request gives up after `timeout`.
pub(crate) async fn grant_timing_out(server: &MockServer, timeout: Duration) -> Grant {
    grant_from(
        server,
        client_with(server, sign_in_http().timeout(timeout)).await,
    )
    .await
}

async fn grant_from(server: &MockServer, client: AuthClient) -> Grant {
    mount_token(server, GRANTED).await;
    let login = client.start_login(permissions()).await.unwrap();
    redirect_browser(&login, "the-code").await;
    login.finish().await.unwrap()
}

use super::{
    key_lifetime::KeyLifetime,
    key_name::KeyName,
    permission::{Permission, Permissions},
};
use crate::{
    credentials::has_expired,
    error::{Error, KeyMismatch, Result},
};
use chrono::{DateTime, Utc};
use secrecy::SecretString;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::BTreeMap;

/// Kraken returns the secret one time only, in this value.
#[derive(Debug)]
pub struct ApiKey {
    name: KeyName,
    key: SecretString,
    secret: SecretString,
    valid_until: DateTime<Utc>,
}

impl ApiKey {
    pub fn name(&self) -> &KeyName {
        &self.name
    }

    pub fn key(&self) -> &SecretString {
        &self.key
    }

    pub fn secret(&self) -> &SecretString {
        &self.secret
    }

    pub fn valid_until(&self) -> DateTime<Utc> {
        self.valid_until
    }
}

#[derive(Serialize)]
pub(crate) struct CreateRequest {
    api_key_name: KeyName,
    permissions: Permissions,
    /// Never the epoch: zero means "no expiry".
    #[serde(with = "chrono::serde::ts_seconds")]
    valid_until: DateTime<Utc>,
}

impl CreateRequest {
    pub(crate) fn new(name: &KeyName, permissions: &Permissions, lifetime: KeyLifetime) -> Self {
        Self {
            api_key_name: name.clone(),
            permissions: permissions.clone(),
            valid_until: lifetime.expires_at(Utc::now()),
        }
    }

    /// A key that differs from this request is refused, so a server defect
    /// cannot leave a wider or longer-lived key than the user chose.
    fn mismatch(&self, created: &CreatedKey) -> Option<KeyMismatch> {
        if created.api_key_name != self.api_key_name {
            return Some(KeyMismatch::Name(created.api_key_name.clone()));
        }
        // Kraken stores whole seconds, so the request's own expiry rounds down.
        if created.valid_until.timestamp() > self.valid_until.timestamp()
            || has_expired(created.valid_until)
        {
            return Some(KeyMismatch::Expiry(created.valid_until));
        }
        let unrequested: Vec<String> = created
            .permissions
            .iter()
            .filter(|&(flag, &granted)| {
                granted
                    && !Permission::from_flag(flag).is_some_and(|p| self.permissions.contains(p))
            })
            .map(|(flag, _)| flag.clone())
            .collect();
        (!unrequested.is_empty()).then_some(KeyMismatch::Permissions(unrequested))
    }
}

#[derive(Serialize)]
pub(crate) struct DeleteRequest {
    api_key_name: KeyName,
}

impl From<&KeyName> for DeleteRequest {
    fn from(name: &KeyName) -> Self {
        Self {
            api_key_name: name.clone(),
        }
    }
}

#[derive(Deserialize)]
struct Envelope<T> {
    #[serde(default)]
    error: Vec<String>,
    result: Option<T>,
}

#[derive(Deserialize)]
struct CreatedKey {
    api_key_name: KeyName,
    api_key: Option<String>,
    secret: Option<String>,
    #[serde(with = "chrono::serde::ts_seconds")]
    valid_until: DateTime<Utc>,
    /// The key's flags. Kraken can omit the ones that are off.
    #[serde(default)]
    permissions: BTreeMap<String, bool>,
}

fn envelope<T: DeserializeOwned>(status: u16, body: &[u8]) -> Result<Option<T>> {
    let success = (200..300).contains(&status);
    match serde_json::from_slice::<Envelope<T>>(body) {
        Ok(envelope) if !envelope.error.is_empty() => Err(Error::KrakenApi {
            status,
            codes: envelope.error,
        }),
        Ok(envelope) if success => Ok(envelope.result),
        Err(error) if success => Err(error.into()),
        _ => Err(Error::Rejected {
            status,
            code: "unexpected response".to_string(),
        }),
    }
}

pub(crate) fn parse_created(status: u16, body: &[u8], request: &CreateRequest) -> Result<ApiKey> {
    let created: CreatedKey = envelope(status, body)?.ok_or(Error::Incomplete("result"))?;
    if let Some(mismatch) = request.mismatch(&created) {
        return Err(Error::UnexpectedKey {
            name: created.api_key_name,
            mismatch,
        });
    }
    Ok(ApiKey {
        name: created.api_key_name,
        key: created.api_key.ok_or(Error::Incomplete("api_key"))?.into(),
        secret: created.secret.ok_or(Error::Incomplete("secret"))?.into(),
        valid_until: created.valid_until,
    })
}

/// A create that failed after the request may have left the key on Kraken:
/// the reply was lost, unreadable, a server error, or an envelope code whose
/// outcome is unknown. Such a failure names the key, so the caller can delete
/// it. A connect failure sent nothing, and any other code or a 4xx is a
/// refusal.
pub(crate) fn unconfirmed(error: Error, requested: &KeyName) -> Error {
    let may_exist = match &error {
        Error::Http(error) => !(error.is_connect() || error.is_builder()),
        Error::Decode(_) | Error::Incomplete(_) => true,
        Error::Rejected { status, .. } => *status >= 500,
        Error::KrakenApi { status, codes } => {
            *status >= 500 || codes.iter().any(|code| is_unknown_outcome(code))
        }
        _ => false,
    };
    if may_exist {
        Error::Unconfirmed {
            name: requested.clone(),
            cause: Box::new(error),
        }
    } else {
        error
    }
}

/// Service and internal errors leave the outcome open: the key can exist.
fn is_unknown_outcome(code: &str) -> bool {
    code.starts_with("EService:") || code == "EGeneral:Internal error"
}

pub(crate) fn parse_deleted(status: u16, body: &[u8]) -> Result<()> {
    envelope::<serde_json::Value>(status, body).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    /// The response shape for `POST /oauth/fast-api-key`.
    const CREATED: &[u8] = br#"{"error":[],"result":{
        "id":"AABBCC","api_key_name":"kraken-cli-0a1b2c3d",
        "api_key":"the-key","secret":"the-secret",
        "nonce":0,"nonce_window":0,"permissions":{},"valid_until":4000000000,
        "query_from":0,"query_to":0,"ip_allowlist":[],"last_used":null,
        "oauth_client":"0123456789abcdef0123456789abcdef"}}"#;

    /// The request that `CREATED` answers. Built by hand: its expiry is past
    /// what `CreateRequest::new` accepts today.
    fn parse(status: u16, body: &[u8]) -> Result<ApiKey> {
        let name: KeyName = "kraken-cli-0a1b2c3d".parse().unwrap();
        let permissions: Permissions = [Permission::FundsQuery, Permission::TradesModify]
            .into_iter()
            .collect();
        let request = CreateRequest {
            api_key_name: name,
            permissions,
            valid_until: DateTime::from_timestamp(4_000_000_000, 0).unwrap(),
        };
        parse_created(status, body, &request)
    }

    fn created_with(change: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
        let mut body: serde_json::Value = serde_json::from_slice(CREATED).unwrap();
        change(&mut body["result"]);
        serde_json::to_vec(&body).unwrap()
    }

    #[test]
    fn a_created_key_carries_its_one_time_secret() {
        let key = parse(200, CREATED).unwrap();
        assert_eq!(key.name().as_str(), "kraken-cli-0a1b2c3d");
        assert_eq!(key.key().expose_secret(), "the-key");
        assert_eq!(key.secret().expose_secret(), "the-secret");
        assert_eq!(key.valid_until().timestamp(), 4_000_000_000);
    }

    #[test]
    fn debug_never_prints_the_key_or_the_secret() {
        let rendered = format!("{:?}", parse(200, CREATED).unwrap());
        assert!(!rendered.contains("the-key") && !rendered.contains("the-secret"));
    }

    #[test]
    fn a_response_without_the_secret_is_incomplete() {
        let body = created_with(|result| {
            result.as_object_mut().unwrap().remove("secret");
        });
        assert!(matches!(
            parse(200, &body),
            Err(Error::Incomplete("secret"))
        ));
    }

    #[test]
    fn a_key_wider_longer_or_renamed_is_unexpected() {
        let cases = [
            (
                created_with(|r| r["permissions"] = serde_json::json!({ "funds_withdraw": true })),
                KeyMismatch::Permissions(vec!["funds_withdraw".into()]),
            ),
            (
                created_with(|r| r["valid_until"] = 4_000_000_001_i64.into()),
                KeyMismatch::Expiry(DateTime::from_timestamp(4_000_000_001, 0).unwrap()),
            ),
            (
                created_with(|r| r["valid_until"] = 0.into()),
                KeyMismatch::Expiry(DateTime::UNIX_EPOCH),
            ),
            (
                created_with(|r| r["api_key_name"] = "kraken-cli-ffffffff".into()),
                KeyMismatch::Name("kraken-cli-ffffffff".parse().unwrap()),
            ),
        ];
        for (body, expected) in cases {
            assert!(
                matches!(parse(200, &body), Err(Error::UnexpectedKey { mismatch, .. }) if mismatch == expected),
                "{expected}"
            );
        }
    }

    #[test]
    fn an_echo_of_the_requested_flags_is_expected() {
        let echoed = created_with(|r| {
            r["permissions"] = serde_json::json!({
                "funds_query": true, "trades_modify": true, "funds_withdraw": false,
            });
        });
        assert!(parse(200, &echoed).is_ok());
    }

    #[test]
    fn an_api_error_is_a_rejection() {
        let body = br#"{"error":["EOAuth:Max fast API keys reached"]}"#;
        assert!(matches!(
            parse(400, body),
            Err(Error::KrakenApi { status: 400, codes }) if codes == ["EOAuth:Max fast API keys reached"]
        ));
    }

    #[test]
    fn only_a_failure_after_the_request_can_leave_a_key() {
        let name: KeyName = "kraken-cli-0a1b2c3d".parse().unwrap();
        let api = |status, code: &str| Error::KrakenApi {
            status,
            codes: vec![code.into()],
        };
        for error in [
            api(200, "EService:Unavailable"),
            api(200, "EGeneral:Internal error"),
            api(500, "EAPI:Bad request"),
            Error::Incomplete("secret"),
        ] {
            assert!(matches!(
                unconfirmed(error, &name),
                Error::Unconfirmed { .. }
            ));
        }
        for error in [
            api(200, "EOAuth:Max fast API keys reached"),
            Error::LifetimeOutOfRange,
        ] {
            assert!(!matches!(
                unconfirmed(error, &name),
                Error::Unconfirmed { .. }
            ));
        }
    }

    #[tokio::test]
    async fn a_create_that_never_connected_leaves_nothing_to_delete() {
        // Bound and dropped at once, so nothing listens on the port.
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let error = http
            .get(format!("http://{closed}"))
            .send()
            .await
            .unwrap_err();
        assert!(error.is_connect(), "{error}");

        let name: KeyName = "kraken-cli-0a1b2c3d".parse().unwrap();
        assert!(matches!(unconfirmed(error.into(), &name), Error::Http(_)));
    }
}

//! A REST venue as one context reaches it: the public client, and the account
//! it is signed in to when credentials resolved.

use crate::{
    config::StoredKeys,
    errors::{KrakenError, Result, SIGN_IN_KEY_EXPIRED},
};
use kraken_auth::{CredentialSource, Credentials, Engine};
use kraken_rest::{
    Auth, RestError,
    futures::{FuturesAccount, FuturesRest},
    spot::{SpotAccount, SpotRest},
};
use secrecy::SecretString;
use std::sync::Arc;

/// A venue's public client, able to sign in to an account.
pub(crate) trait SignIn: Clone {
    type Account: Clone;
    const ENGINE: Engine;

    fn sign_in(&self, auth: Arc<Auth>) -> Self::Account;
    fn key(account: &Self::Account) -> &Arc<Auth>;
}

impl SignIn for SpotRest {
    type Account = SpotAccount;
    const ENGINE: Engine = Engine::Spot;

    fn sign_in(&self, auth: Arc<Auth>) -> SpotAccount {
        self.authenticate(auth)
    }

    fn key(account: &SpotAccount) -> &Arc<Auth> {
        account.auth()
    }
}

impl SignIn for FuturesRest {
    type Account = FuturesAccount;
    const ENGINE: Engine = Engine::Futures;

    fn sign_in(&self, auth: Arc<Auth>) -> FuturesAccount {
        self.authenticate(auth)
    }

    fn key(account: &FuturesAccount) -> &Arc<Auth> {
        account.auth()
    }
}

#[derive(Clone)]
pub(crate) struct Venue<R: SignIn> {
    rest: R,
    account: std::result::Result<Account<R::Account>, Unsigned>,
}

/// A signed-in client, and where its key came from.
#[derive(Clone)]
pub(crate) struct Account<A> {
    pub(crate) client: A,
    pub(crate) source: CredentialSource,
}

/// Why a venue has no account. Raised only when a private command asks, so a
/// public command still runs beside a broken secret.
#[derive(Clone)]
enum Unsigned {
    Missing,
    Malformed {
        source: CredentialSource,
        reason: String,
    },
}

impl<R: SignIn> Venue<R> {
    pub(crate) fn new(rest: R, credentials: Option<Credentials>) -> Self {
        let account = match credentials {
            Some(credentials) => sign_in(&rest, credentials),
            None => Err(Unsigned::Missing),
        };
        Self { rest, account }
    }

    /// A flag or env key of this line, else the `session`'s, else the stored
    /// key. A stored key is never carried over: it is read again, so a reset
    /// or a switch takes effect at once.
    pub(crate) fn resolve(
        rest: R,
        supplied: Option<Credentials>,
        session: Option<&Self>,
        stored: &StoredKeys,
    ) -> Self {
        let session = session.filter(|session| {
            session
                .key_source()
                .is_some_and(CredentialSource::is_override)
        });
        match (supplied, session) {
            (Some(credentials), _) => Self::new(rest, Some(credentials)),
            (None, Some(session)) => session.moved_to(rest),
            (None, None) => Self::new(rest, stored.credentials(R::ENGINE)),
        }
    }

    pub(crate) fn rest(&self) -> &R {
        &self.rest
    }

    pub(crate) fn account(&self) -> Result<&R::Account> {
        self.signed().map(|account| &account.client)
    }

    pub(crate) fn source(&self) -> Result<CredentialSource> {
        self.signed().map(|account| account.source)
    }

    /// The key that wins resolution, malformed or not, so a broken env key is
    /// still reported as the override. `None` without a key, or when it is an
    /// expired sign-in key.
    pub(crate) fn source_in_use(&self) -> Option<CredentialSource> {
        self.key_source().filter(|source| !source.is_expired())
    }

    /// For a stream that runs without an account: only its private feeds would
    /// notice a broken secret, so it is reported here. An expired key is kept:
    /// its token mint names the reason.
    pub(crate) fn optional_account(&self) -> Option<&Account<R::Account>> {
        match &self.account {
            Ok(account) => Some(account),
            Err(Unsigned::Missing) => None,
            Err(Unsigned::Malformed { reason, .. }) => {
                tracing::warn!(
                    engine = R::ENGINE.label(),
                    reason,
                    "API secret unusable; private feeds are unavailable"
                );
                None
            }
        }
    }

    /// The key's source, whether or not it can sign.
    fn key_source(&self) -> Option<CredentialSource> {
        match &self.account {
            Ok(account) => Some(account.source),
            Err(Unsigned::Malformed { source, .. }) => Some(*source),
            Err(Unsigned::Missing) => None,
        }
    }

    /// This venue's key on `rest`, a REPL line's own client.
    fn moved_to(&self, rest: R) -> Self {
        let account = match &self.account {
            Ok(account) => Ok(Account {
                client: rest.sign_in(Arc::clone(R::key(&account.client))),
                source: account.source,
            }),
            Err(unsigned) => Err(unsigned.clone()),
        };
        Self { rest, account }
    }

    fn signed(&self) -> Result<&Account<R::Account>> {
        match &self.account {
            Ok(account) if account.source.is_expired() => {
                Err(KrakenError::Auth(SIGN_IN_KEY_EXPIRED.into()))
            }
            Ok(account) => Ok(account),
            Err(Unsigned::Missing) => Err(missing_credentials(R::ENGINE)),
            Err(Unsigned::Malformed { reason, .. }) => Err(KrakenError::Auth(reason.clone())),
        }
    }
}

impl Venue<SpotRest> {
    /// Every private request carries `otp`, Spot's static second factor.
    /// A sign-in key has no 2FA, so it never sends the password.
    pub(crate) fn with_otp(mut self, otp: Option<SecretString>) -> Self {
        self.account = self.account.map(|account| match account.source {
            CredentialSource::SignIn { .. } => account,
            _ => Account {
                client: account.client.with_otp(otp),
                source: account.source,
            },
        });
        self
    }

    #[cfg(test)]
    pub(crate) fn otp(&self) -> Option<SecretString> {
        self.account.as_ref().ok()?.client.otp().cloned()
    }
}

fn missing_credentials(engine: Engine) -> KrakenError {
    let (key_env, secret_env) = engine.env_names();
    let login = match engine {
        Engine::Spot => "Run `kraken auth login`, use",
        Engine::Futures => "Use",
    };
    KrakenError::Auth(format!(
        "No {} API credentials found. {login} `kraken auth set`, or set {key_env} / {secret_env} \
         env vars.",
        engine.label()
    ))
}

fn sign_in<R: SignIn>(
    rest: &R,
    credentials: Credentials,
) -> std::result::Result<Account<R::Account>, Unsigned> {
    let Credentials {
        api_key,
        api_secret,
        source,
    } = credentials;
    let auth = Auth::new(api_key, &api_secret).map_err(|err| Unsigned::Malformed {
        source,
        reason: match err {
            RestError::Signing(reason) => reason,
            other => other.to_string(),
        },
    })?;
    Ok(Account {
        client: rest.sign_in(Arc::new(auth)),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use chrono::{Duration, Utc};
    use kraken_auth::{KeyPair, StoredAuth};
    use kraken_rest::{ClientOptions, Transport};

    fn spot(url: &str) -> SpotRest {
        let transport = Transport::new(&ClientOptions::default()).unwrap();
        SpotRest::new(transport, Some(url.into()))
    }

    fn credentials(secret: &str, source: CredentialSource) -> Credentials {
        Credentials {
            api_key: SecretString::from("key"),
            api_secret: SecretString::from(secret.to_owned()),
            source,
        }
    }

    fn secret() -> String {
        base64::engine::general_purpose::STANDARD.encode(b"secret")
    }

    /// A store holding only your API key, so no test reads the real file.
    fn stored_own_key() -> StoredKeys {
        let mut stored = StoredAuth::default();
        stored.set_own(KeyPair::new("own-key", secret()));
        StoredKeys::from(stored)
    }

    #[test]
    fn a_malformed_secret_fails_only_the_private_call() {
        let venue = Venue::new(
            spot("https://a"),
            Some(credentials("not base64!", CredentialSource::Env)),
        );
        let Err(err) = venue.account() else {
            panic!("an undecodable secret cannot sign");
        };
        assert!(err.to_string().contains("decode API secret"), "{err}");
        assert!(venue.optional_account().is_none());
    }

    #[test]
    fn a_malformed_env_key_is_still_the_key_in_use() {
        let venue = Venue::new(
            spot("https://a"),
            Some(credentials("not base64!", CredentialSource::Env)),
        );
        assert_eq!(venue.source_in_use(), Some(CredentialSource::Env));
    }

    #[test]
    fn a_line_without_credentials_keeps_the_sessions_env_key_and_otp() {
        let session = Venue::new(
            spot("https://a"),
            Some(credentials(&secret(), CredentialSource::Env)),
        )
        .with_otp(Some(SecretString::from("123456")));
        let line = Venue::resolve(spot("https://b"), None, Some(&session), &stored_own_key())
            .with_otp(session.otp());
        assert!(matches!(line.source().unwrap(), CredentialSource::Env));
        assert!(Arc::ptr_eq(
            line.account().unwrap().auth(),
            session.account().unwrap().auth()
        ));
        assert!(line.otp().is_some());
    }

    #[test]
    fn a_login_key_never_sends_the_one_time_password() {
        let login = CredentialSource::SignIn {
            valid_until: Utc::now() + Duration::days(1),
        };
        let venue = Venue::new(spot("https://a"), Some(credentials(&secret(), login)))
            .with_otp(Some(SecretString::from("123456")));
        assert!(venue.otp().is_none());
    }

    #[test]
    fn a_sessions_malformed_env_key_never_falls_back_to_the_stored_key() {
        let session = Venue::new(
            spot("https://a"),
            Some(credentials("not base64!", CredentialSource::Env)),
        );
        let line = Venue::resolve(spot("https://b"), None, Some(&session), &stored_own_key());
        assert!(line.account().is_err());
    }

    #[test]
    fn a_session_never_carries_over_a_stored_key() {
        let session = Venue::new(
            spot("https://a"),
            Some(credentials(&secret(), CredentialSource::Config)),
        );
        let line = Venue::resolve(
            spot("https://b"),
            None,
            Some(&session),
            &StoredKeys::from(StoredAuth::default()),
        );
        assert!(line.account().is_err(), "a reset key must not survive");
    }

    #[test]
    fn an_expired_login_key_is_refused_with_the_reason() {
        let expired = CredentialSource::SignIn {
            valid_until: Utc::now() - Duration::minutes(1),
        };
        let venue = Venue::new(spot("https://a"), Some(credentials(&secret(), expired)));
        let Err(err) = venue.account() else {
            panic!("an expired key must not sign");
        };
        assert_eq!(
            err.to_string(),
            KrakenError::Auth(SIGN_IN_KEY_EXPIRED.into()).to_string()
        );
        assert!(venue.source_in_use().is_none());
        assert!(
            venue.optional_account().is_some(),
            "its mint names the reason"
        );
    }

    #[test]
    fn no_credentials_names_the_engines_env_vars() {
        let venue = Venue::new(spot("https://a"), None);
        let Err(err) = venue.account() else {
            panic!("no credentials, no account");
        };
        assert!(err.to_string().contains("KRAKEN_API_KEY"), "{err}");
        assert!(err.to_string().contains("kraken auth login"), "{err}");
    }
}

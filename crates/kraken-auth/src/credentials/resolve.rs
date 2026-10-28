use super::{CredentialSource, Credentials, Engine, KeyPair, KeySlot, StoredAuth};
use secrecy::{ExposeSecret, SecretString};

/// A key and a secret as one source supplies them, before the check that both
/// are present.
#[derive(Clone, Default)]
pub struct Supplied {
    pub key: Option<String>,
    pub secret: Option<SecretString>,
}

impl Supplied {
    /// Both halves are set, by the same rule as the env tier of
    /// [`resolve_supplied`].
    pub fn is_complete(&self) -> bool {
        self.key.as_deref().is_some_and(is_set)
            && self
                .secret
                .as_ref()
                .is_some_and(|secret| is_set(secret.expose_secret()))
    }

    fn without_empty(self) -> Self {
        Self {
            key: self.key.filter(|key| is_set(key)),
            secret: self.secret.filter(|secret| is_set(secret.expose_secret())),
        }
    }
}

/// The key from a flag, else from the env vars. Both beat
/// [`resolve_stored`]. Each tier needs both halves. A half-set tier gives a
/// warning and the next tier applies.
pub fn resolve_supplied(engine: Engine, flag: Supplied, env: Supplied) -> Option<Credentials> {
    let suffix = match engine {
        Engine::Spot => "",
        Engine::Futures => " for Futures",
    };
    if let Some(pair) = complete(flag, |missing| match missing {
        Half::Secret => format!("--api-key without --api-secret{suffix}: the flags are ignored."),
        Half::Key => format!("--api-secret without --api-key{suffix}: the flags are ignored."),
    }) {
        return Some(Credentials::new(&pair, CredentialSource::Flag));
    }

    let (key_env, secret_env) = engine.env_names();
    if let Some(pair) = complete(env.without_empty(), |missing| match missing {
        Half::Secret => {
            format!("{key_env} is set but {secret_env} is not: the env vars are ignored.")
        }
        Half::Key => format!("{secret_env} is set but {key_env} is not: the env vars are ignored."),
    }) {
        return Some(Credentials::new(&pair, CredentialSource::Env));
    }
    None
}

/// The stored key for `engine`: the futures key, or the active Spot key.
pub fn resolve_stored(engine: Engine, stored: &StoredAuth) -> Option<Credentials> {
    match engine {
        Engine::Futures => stored
            .futures()
            .map(|pair| Credentials::new(pair, CredentialSource::Config)),
        Engine::Spot => active_spot_key(stored),
    }
}

/// An expired sign-in key still resolves to itself, never to the other key: the
/// CLI never signs with a key that the user did not choose. The caller refuses
/// it with [`CredentialSource::is_expired`] when it needs a key.
fn active_spot_key(stored: &StoredAuth) -> Option<Credentials> {
    match stored.active()? {
        KeySlot::ApiKey => stored
            .own()
            .map(|pair| Credentials::new(pair, CredentialSource::Config)),
        KeySlot::SignIn => stored.login().map(Credentials::login),
    }
}

#[derive(Clone, Copy)]
enum Half {
    Key,
    Secret,
}

fn complete(supplied: Supplied, warning: impl Fn(Half) -> String) -> Option<KeyPair> {
    match (supplied.key, supplied.secret) {
        (Some(key), Some(secret)) => Some(KeyPair::new(key, secret)),
        (Some(_), None) => {
            tracing::warn!("{}", warning(Half::Secret));
            None
        }
        (None, Some(_)) => {
            tracing::warn!("{}", warning(Half::Key));
            None
        }
        (None, None) => None,
    }
}

/// Plugin hosts pass an empty string for an unset field. Only the literal
/// empty string counts as unset: whitespace reaches the API, which reports it.
fn is_set(value: &str) -> bool {
    !value.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::LoginKey;
    use chrono::{Duration, Utc};

    fn supplied(key: Option<&str>, secret: Option<&str>) -> Supplied {
        Supplied {
            key: key.map(Into::into),
            secret: secret.map(SecretString::from),
        }
    }

    fn none() -> Supplied {
        Supplied::default()
    }

    fn login_key(valid_for: Duration) -> LoginKey {
        LoginKey {
            pair: KeyPair::new("login-key", "bG9naW4="),
            name: "kraken-cli-0000beef".parse().unwrap(),
            valid_until: Utc::now() + valid_for,
        }
    }

    fn both_keys() -> StoredAuth {
        let mut stored = StoredAuth::default();
        stored.set_own(KeyPair::new("own-key", "b3du"));
        stored.set_login(login_key(Duration::days(1)));
        stored
    }

    /// The CLI's order: a supplied key, then the stored key.
    fn resolve(
        engine: Engine,
        flag: Supplied,
        env: Supplied,
        stored: &StoredAuth,
    ) -> Option<Credentials> {
        resolve_supplied(engine, flag, env).or_else(|| resolve_stored(engine, stored))
    }

    fn source(
        engine: Engine,
        flag: Supplied,
        env: Supplied,
        stored: &StoredAuth,
    ) -> Option<CredentialSource> {
        resolve(engine, flag, env, stored).map(|c| c.source)
    }

    #[test]
    fn a_flag_pair_beats_an_env_pair_and_the_stored_key() {
        let both = supplied(Some("k"), Some("s"));
        assert_eq!(
            source(Engine::Spot, both.clone(), both, &both_keys()),
            Some(CredentialSource::Flag)
        );
    }

    #[test]
    fn an_env_pair_beats_the_active_stored_key() {
        let env = supplied(Some("k"), Some("s"));
        let mut stored = both_keys();
        stored.activate(KeySlot::SignIn).unwrap();
        assert_eq!(
            source(Engine::Spot, none(), env, &stored),
            Some(CredentialSource::Env)
        );
    }

    #[test]
    fn the_chosen_login_key_is_used_over_your_own_key() {
        let mut stored = both_keys();
        stored.activate(KeySlot::SignIn).unwrap();
        let credentials = resolve(Engine::Spot, none(), none(), &stored).unwrap();
        assert_eq!(credentials.api_key.expose_secret(), "login-key");
    }

    #[test]
    fn an_expired_active_login_key_never_falls_back_to_your_own_key() {
        let mut stored = both_keys();
        stored.activate(KeySlot::SignIn).unwrap();
        stored.set_login(login_key(-Duration::days(1)));
        let credentials = resolve(Engine::Spot, none(), none(), &stored).unwrap();
        assert!(matches!(
            credentials.source,
            CredentialSource::SignIn { .. }
        ));
        assert!(credentials.source.is_expired());
    }

    #[test]
    fn futures_never_uses_a_spot_key() {
        assert!(resolve(Engine::Futures, none(), none(), &both_keys()).is_none());
    }

    #[test]
    fn a_half_set_tier_falls_through_to_the_next() {
        let half = supplied(Some("k"), None);
        assert_eq!(
            source(Engine::Spot, half, none(), &both_keys()),
            Some(CredentialSource::Config)
        );
    }

    #[test]
    fn an_empty_env_var_counts_as_unset_but_whitespace_does_not() {
        assert!(!supplied(Some(""), Some("s")).is_complete());
        assert!(supplied(Some("   "), Some("s")).is_complete());
    }
}

use super::{KeyPair, LoginKey};
use chrono::{DateTime, Utc};
use secrecy::SecretString;
use std::fmt;

/// The key pair that a request is signed with, and where it came from.
#[derive(Clone, Debug)]
pub struct Credentials {
    pub api_key: SecretString,
    pub api_secret: SecretString,
    pub source: CredentialSource,
}

impl Credentials {
    pub(crate) fn new(pair: &KeyPair, source: CredentialSource) -> Self {
        Self {
            api_key: pair.key.clone(),
            api_secret: pair.secret.clone(),
            source,
        }
    }

    pub(crate) fn login(key: &LoginKey) -> Self {
        Self::new(
            &key.pair,
            CredentialSource::SignIn {
                valid_until: key.valid_until,
            },
        )
    }
}

/// Where the active credentials came from. A flag beats an env var, and both
/// beat the stored keys; between `Config` and `SignIn`, the active choice
/// decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialSource {
    Flag,
    Env,
    /// The key that the user set with `kraken auth set`.
    Config,
    /// The sign-in key that `kraken auth login` created. Only this key
    /// expires.
    SignIn {
        valid_until: DateTime<Utc>,
    },
}

impl CredentialSource {
    /// A flag or an env var wins over the active stored key.
    pub fn is_override(self) -> bool {
        matches!(self, Self::Flag | Self::Env)
    }

    /// Checked at each use, so a long-lived context refuses a key that expired
    /// after it started.
    pub fn is_expired(self) -> bool {
        matches!(self, Self::SignIn { valid_until } if has_expired(valid_until))
    }
}

pub(crate) fn has_expired(valid_until: DateTime<Utc>) -> bool {
    valid_until <= Utc::now()
}

impl fmt::Display for CredentialSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Flag => "command-line flag",
            Self::Env => "environment variable",
            Self::Config => "config file",
            Self::SignIn { .. } => "browser sign-in",
        })
    }
}

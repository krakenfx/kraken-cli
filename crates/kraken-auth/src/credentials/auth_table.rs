//! The on-disk shape of the `[auth]` table, and the policy for entries this
//! build cannot read.
use super::{KeyPair, LoginKey, StoredAuth, stored::Choice};
use crate::{error::Result, oauth::KeyName};
use chrono::{DateTime, Utc};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::DeserializeOwned};
use serde_with::skip_serializing_none;

pub(super) const AUTH_TABLE: &str = "auth";

/// Removes `[auth]` from `document` and reads it. Nothing is cloned, so the
/// secrets stay in one place.
pub(super) fn take(document: &mut toml::Table) -> Result<StoredAuth> {
    document
        .remove(AUTH_TABLE)
        .map_or_else(|| Ok(StoredAuth::default()), parse)
}

/// Reads `[auth]` and leaves `document` whole, for a caller that writes it
/// back.
pub(super) fn read(document: &toml::Table) -> Result<StoredAuth> {
    document
        .get(AUTH_TABLE)
        .map_or_else(|| Ok(StoredAuth::default()), |table| parse(table.clone()))
}

fn parse(table: toml::Value) -> Result<StoredAuth> {
    Ok(table.try_into::<AuthTable>()?.into())
}

/// The flat key fields, read strictly: a bad one fails the whole load.
const KEY_PAIR_FIELDS: [&str; 4] = [
    "api_key",
    "api_secret",
    "futures_api_key",
    "futures_api_secret",
];

/// Merges into the `[auth]` table read from disk. An entry this build cannot
/// read or does not know stays, because a newer release or a damaged sign-in
/// key may still name a key on Kraken. The key pairs are always read, so they
/// are written as held: a cleared pair leaves the file.
pub(super) fn merge(document: &mut toml::Table, auth: &StoredAuth) -> Result<()> {
    let mut table = match document.remove(AUTH_TABLE) {
        Some(toml::Value::Table(table)) => table,
        _ => toml::Table::new(),
    };
    for field in KEY_PAIR_FIELDS {
        table.remove(field);
    }
    if let toml::Value::Table(known) = toml::Value::try_from(AuthTable::from(auth))? {
        table.extend(known);
    }
    document.insert(AUTH_TABLE.into(), table.into());
    Ok(())
}

/// The flat key fields are the format that `auth set` has always written, so a
/// config file from an older release loads unchanged.
#[skip_serializing_none]
#[derive(Default, Deserialize, Serialize)]
struct AuthTable {
    #[serde(serialize_with = "exposed")]
    api_key: Option<SecretString>,
    #[serde(serialize_with = "exposed")]
    api_secret: Option<SecretString>,
    #[serde(serialize_with = "exposed")]
    futures_api_key: Option<SecretString>,
    #[serde(serialize_with = "exposed")]
    futures_api_secret: Option<SecretString>,
    /// The CLI writes the choice and the sign-in key. A damaged one is ignored
    /// with a warning, so it cannot cost the key that you set yourself. A
    /// damaged choice is never written back, so it stays on disk.
    #[serde(
        default,
        deserialize_with = "damaged_choice_is_unreadable",
        serialize_with = "chosen_slot",
        skip_serializing_if = "has_no_slot"
    )]
    active: Choice,
    #[serde(
        rename = "sign-in",
        default,
        deserialize_with = "damaged_login_is_none"
    )]
    login: Option<LoginTable>,
}

#[derive(Deserialize, Serialize)]
struct LoginTable {
    #[serde(serialize_with = "exposed")]
    api_key: Option<SecretString>,
    #[serde(serialize_with = "exposed")]
    api_secret: Option<SecretString>,
    name: KeyName,
    valid_until: DateTime<Utc>,
}

/// `secrecy` has no `Serialize`, on purpose. The 0600 config file is the one
/// place where the CLI writes a secret.
fn exposed<S: Serializer>(
    secret: &Option<SecretString>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    match secret {
        Some(secret) => serializer.serialize_some(secret.expose_secret()),
        None => serializer.serialize_none(),
    }
}

/// A half-written pair is ignored: a key without its secret cannot sign.
fn pair(key: Option<SecretString>, secret: Option<SecretString>) -> Option<KeyPair> {
    Some(KeyPair {
        key: key?,
        secret: secret?,
    })
}

fn damaged_choice_is_unreadable<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Choice, D::Error> {
    Ok(lenient(deserializer, "the active key choice")?.map_or(Choice::Unreadable, Choice::Slot))
}

fn chosen_slot<S: Serializer>(
    choice: &Choice,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    choice.slot().serialize(serializer)
}

fn has_no_slot(choice: &Choice) -> bool {
    choice.slot().is_none()
}

fn damaged_login_is_none<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<LoginTable>, D::Error> {
    lenient(deserializer, "the sign-in key")
}

fn lenient<'de, D: Deserializer<'de>, T: DeserializeOwned>(
    deserializer: D,
    what: &str,
) -> std::result::Result<Option<T>, D::Error> {
    match toml::Value::deserialize(deserializer)?.try_into() {
        Ok(parsed) => Ok(Some(parsed)),
        Err(error) => {
            tracing::warn!(
                entry = what,
                error = error.message(),
                "ignored a damaged entry in config.toml"
            );
            Ok(None)
        }
    }
}

impl From<AuthTable> for StoredAuth {
    fn from(table: AuthTable) -> Self {
        Self {
            own: pair(table.api_key, table.api_secret),
            futures: pair(table.futures_api_key, table.futures_api_secret),
            login: table.login.and_then(|login| {
                Some(LoginKey {
                    pair: pair(login.api_key, login.api_secret)?,
                    name: login.name,
                    valid_until: login.valid_until,
                })
            }),
            chosen: table.active,
        }
    }
}

impl From<&StoredAuth> for AuthTable {
    fn from(auth: &StoredAuth) -> Self {
        let halves = |pair: Option<&KeyPair>| {
            pair.map_or((None, None), |p| {
                (Some(p.key.clone()), Some(p.secret.clone()))
            })
        };
        let (api_key, api_secret) = halves(auth.own.as_ref());
        let (futures_api_key, futures_api_secret) = halves(auth.futures.as_ref());
        Self {
            api_key,
            api_secret,
            futures_api_key,
            futures_api_secret,
            active: auth.chosen,
            login: auth.login.as_ref().map(|login| LoginTable {
                api_key: Some(login.pair.key.clone()),
                api_secret: Some(login.pair.secret.clone()),
                name: login.name.clone(),
                valid_until: login.valid_until,
            }),
        }
    }
}

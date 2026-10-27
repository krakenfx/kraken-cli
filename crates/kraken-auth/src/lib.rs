#![doc = include_str!("../README.md")]
// Tests use unwrap, expect and panic. The workspace lints deny them elsewhere.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod credentials;
mod error;
mod login;
mod oauth;

pub use credentials::{
    CredentialSource, CredentialStore, Credentials, Engine, KeyPair, KeySlot, LoginKey,
    SecretSource, StoredAuth, Supplied, read_secret, resolve_stored, resolve_supplied,
};
pub use error::{Error, KeyMismatch, Result};
pub use login::{ReplacedKey, SavedLogin, save_login_key};
pub use oauth::{
    ApiKey, AuthClient, ClientId, Config, Endpoints, Grant, KeyLifetime, KeyName,
    PRODUCTION_SIGN_IN_HOST, PendingLogin, Permission, Permissions,
};

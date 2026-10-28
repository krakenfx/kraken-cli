//! Browser sign-in: a public Kraken Connect client creates a restricted API
//! key. The functional core (PKCE, URLs, wire types) is pure; `client` and
//! `redirect` are the only parts that do I/O.

mod authorize;
mod callback;
mod client;
mod config;
mod fast_key;
mod key_lifetime;
mod key_name;
mod permission;
mod pkce;
mod redirect;
#[cfg(test)]
pub(crate) mod test_support;
mod token;

pub use client::{AuthClient, Grant, PendingLogin};
pub use config::{ClientId, Config, Endpoints, PRODUCTION_SIGN_IN_HOST};
pub use fast_key::ApiKey;
pub use key_lifetime::KeyLifetime;
pub use key_name::KeyName;
pub use permission::{Permission, Permissions};

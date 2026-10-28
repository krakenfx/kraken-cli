//! Kraken's REST APIs, typed. Every endpoint is a type that fixes its path,
//! its response and whether a lost reply may be resent; one transport carries
//! them all.
//!
//! Spot follows Kraken's API description
//! (<https://docs.kraken.com/openapi/spot-rest.yaml>), Futures its public
//! reference. This crate owns the calls and the venue's vocabulary; the
//! front-end owns presentation and the published error categories. A response
//! type's `Serialize` is the shape the front-end publishes, so changing it is a
//! contract change.

// Tests assert with unwrap/panic by design; the workspace lints deny them elsewhere.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod auth;
mod decimal;
mod error;
pub mod futures;
mod nonce;
mod primitives;
pub mod spot;
#[cfg(test)]
mod test_venue;
mod transport;

pub use auth::Auth;
pub use error::RestError;
pub use primitives::{Asset, CommaList, Id, KeyedRows, Pair, UnixTime};
pub use transport::{ClientOptions, Delivery, Identity, Tls, Transport};

//! Kraken v2 WebSocket transport over [`kraken_core`]'s wire model.
//!
//! [`StreamClient`] reconnects per endpoint; [`RequestClient`] uses single-use
//! sockets. Both share one connection layer and an abstract [`TokenSource`].

#![warn(missing_debug_implementations)]

//! [`Event`] is exhaustive and configuration fields are public while this crate
//! remains workspace-private.

// Tests assert with unwrap/panic by design; the workspace lints deny them elsewhere.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

mod backoff;
mod config;
mod connection;
#[cfg(test)]
mod connection_tests;
#[cfg(test)]
mod mock_transport;
mod request_client;
mod stream_client;
mod token;
mod transport;

pub use backoff::ReconnectPolicy;
pub use config::{Urls, WsConfig};
pub use connection::{AbortCause, Event};
pub use request_client::{ReplyBatch, RequestClient};
pub use stream_client::{EventStream, StreamClient};
pub use token::{BoxFuture, Minted, TokenSource};
pub use transport::dangerous_tls_connector;

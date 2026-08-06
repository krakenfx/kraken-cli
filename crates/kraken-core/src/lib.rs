//! Typed wire model and error taxonomy for the Kraken Spot WebSocket v2 protocol.
//!
//! Source: <https://docs.kraken.com/api/docs/websocket-v2>. Split into the outgoing
//! `request` methods and their `wire` framing, the incoming `response` reply model
//! and `inbound` frame classification, the [`subscribe`] channel catalogue and
//! payloads, the [`endpoint`] socket catalogue, the [`error`] type, and the
//! `primitives` shared by all of them.
//!
//! Layer 0 by construction: pure data — parsing, validation, serialization — with no
//! I/O, no async runtime, and no transport dependencies.
//!
//! Precision bound: inbound JSON numbers decode into `Decimal` through `f64` (~17
//! significant digits) — `arbitrary_precision` cannot ride the flattened channel
//! envelope (see the manifest note). Outgoing request params do not share the bound:
//! their `Decimal` fields serialize digit-exact.
//!
//! The root re-exports below are the public surface; only the modules external code
//! actually paths into stay `pub`.

#![warn(missing_debug_implementations)]
// Tests assert with unwrap/panic by design; the workspace lints deny them elsewhere.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

pub mod endpoint;
pub mod error;
mod inbound;
pub mod name;
mod primitives;
mod request;
mod response;
pub mod subscribe;
mod wire;

pub use endpoint::Endpoint;
pub use error::Error;
pub use inbound::Inbound;
pub use primitives::{OrderSide, OrderType};
pub use request::{
    AddOrderParams, AddOrderResult, AmendOrderParams, AmendOrderResult, BatchAddParams,
    BatchCancelParams, BatchOrderParams, CancelAllOrdersAfterParams, CancelAllOrdersAfterResult,
    CancelAllParams, CancelOrderParams, CancelOrderResult, ConditionalClose, CountResult,
    FeePreference, PingParams, Pong, PriceType, StpType, TimeInForce, TriggerReference, Triggers,
};
pub use response::{Method, MethodReply, MethodResponse};
pub use subscribe::{
    BookDepth, Channel, ChannelData, ChannelMessage, EventTrigger, Level3Depth, OhlcInterval,
    SubscribableChannel, Users, WsSubscription,
};
pub use wire::{Request, Wire};

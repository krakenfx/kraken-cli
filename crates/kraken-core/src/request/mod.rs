//! Strictly-typed params and results for the request/response methods — everything
//! that implements [`Request`](crate::Request).
//!
//! Source: the order-management methods under
//! <https://docs.kraken.com/api/docs/websocket-v2> (each submodule links its own page).
//!
//! Each method lives in its own submodule, pairing the request params with the result
//! it decodes into:
//!
//! - [`add_order`] / [`amend_order`] / [`cancel_order`]
//! - [`cancel_all`] / [`cancel_all_orders_after`]
//! - [`batch_add`] / [`batch_cancel`]
//! - [`ping`]
//!
//! Params serialize via serde with `skip_serializing_if`, so the command layer
//! builds typed structs rather than hand-built maps; the auth token is flattened in
//! at send time by [`Wire`](crate::Wire). Each params type implements
//! [`Request`](crate::Request) to bind its wire `method`. Order methods ride the
//! authenticated endpoint; `ping` overrides the defaults to ride the public one.

mod add_order;
mod amend_order;
mod batch_add;
mod batch_cancel;
mod cancel_all;
mod cancel_all_orders_after;
mod cancel_order;
mod decimal_wire;
mod ping;
mod value;

pub use add_order::{AddOrderParams, AddOrderResult, ConditionalClose, Triggers};
pub use amend_order::{AmendOrderParams, AmendOrderResult};
pub use batch_add::{BatchAddParams, BatchOrderParams};
pub use batch_cancel::BatchCancelParams;
pub use cancel_all::{CancelAllParams, CountResult};
pub use cancel_all_orders_after::{CancelAllOrdersAfterParams, CancelAllOrdersAfterResult};
pub use cancel_order::{CancelOrderParams, CancelOrderResult};
pub use ping::{PingParams, Pong};
pub use value::{FeePreference, PriceType, StpType, TimeInForce, TriggerReference};

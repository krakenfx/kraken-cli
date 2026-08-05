//! Everything on the publish/subscribe side of the v2 protocol.
//!
//! Source: <https://docs.kraken.com/api/docs/websocket-v2>.
//!
//! - [`channel`] — the channel catalogue ([`SubscribableChannel`] / `Channel`) and the
//!   validated depth/interval newtypes that parameterize a subscription.
//! - [`subscription`] — [`WsSubscription`], the strictly-typed `subscribe` params.
//! - [`message`] — [`ChannelMessage`], the typed inbound channel-data frames.
//!
//! Outgoing subscribe frames are wrapped by [`Wire`](crate::Wire); inbound
//! frames are classified by [`Inbound`](crate::Inbound)'s `FromStr`.

mod channel;
mod subscription;
// `pub` so the feature-gated recorder can reach the typed payload entries by path
// (`subscribe::message::TickerData`, …) without flattening record-only types into
// the shared `domain` namespace.
pub mod message;

pub use channel::{BookDepth, Channel, Level3Depth, OhlcInterval, SubscribableChannel};
pub use message::{ChannelData, ChannelMessage};
pub use subscription::{EventTrigger, Users, WsSubscription};

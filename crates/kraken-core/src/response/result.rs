//! Typed method-reply bodies, dispatched by serde on the wire `method` tag —
//! the request/response mirror of the channel side's
//! [`ChannelData`](crate::ChannelData).

use crate::request::{
    AddOrderResult, AmendOrderResult, CancelAllOrdersAfterResult, CancelOrderResult, CountResult,
};
use serde::Deserialize;
use serde_with::DeserializeFromStr;
use strum::{Display, EnumDiscriminants, EnumIter, EnumString, EnumTryAs};

/// The wire `method` name paired with its typed `result` body — serde
/// dispatches on the frame's own `method` field, so the method names its result
/// schema and a body that no longer deserializes is a hard error; there is no
/// verbatim fallback. `result` is legitimately absent on rejections, so each
/// variant wraps an `Option`; `ping`/`pong` carry no body at all, so they are
/// unit variants and a body on one fails the frame.
///
/// [`Method`] — the v2 `method` vocabulary, the reply side's discriminant as
/// [`Channel`](crate::Channel) is the channel side's — is derived from this
/// enum, so the two can never drift. It is deliberately strict: a name outside
/// the catalogue fails the frame, so a new server method surfaces as a logged
/// error to fix, never as silently reinterpreted data.
#[derive(Debug, Clone, Deserialize, EnumDiscriminants, EnumTryAs)]
#[serde(tag = "method", content = "result", rename_all = "snake_case")]
#[strum_discriminants(
    name(Method),
    vis(pub),
    derive(Display, EnumIter, EnumString, DeserializeFromStr),
    strum(serialize_all = "snake_case")
)]
pub enum MethodReply {
    AddOrder(Option<AddOrderResult>),
    AmendOrder(Option<AmendOrderResult>),
    BatchAdd(Option<Vec<AddOrderResult>>),
    BatchCancel(Option<CountResult>),
    CancelAll(Option<CountResult>),
    CancelAllOrdersAfter(Option<CancelAllOrdersAfterResult>),
    CancelOrder(Option<CancelOrderResult>),
    /// Inbound only as a rejection: the server echoes the *request's* method on
    /// `success: false`, so a refused ping answers as `ping` — success answers
    /// as [`Self::Pong`].
    Ping,
    Pong,
    Subscribe(Option<SubscribeAck>),
    Unsubscribe(Option<SubscribeAck>),
}

/// The typed body of a subscribe/unsubscribe ack: the granted subscription's
/// echo. Only `warnings` is read by consumers; the remaining echo fields are
/// tolerated and dropped.
#[derive(Debug, Clone, Deserialize)]
pub struct SubscribeAck {
    /// The acked pair, echoed under `result` — closes that symbol's rejection
    /// incident on the streaming path.
    pub symbol: Option<String>,
    pub warnings: Option<Vec<String>>,
}

impl MethodReply {
    /// The [`Method`] this reply answers — mirroring
    /// [`ChannelData::channel`](crate::ChannelData::channel).
    pub fn method(&self) -> Method {
        self.into()
    }
}

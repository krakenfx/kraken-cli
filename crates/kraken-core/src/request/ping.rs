//! `ping` — application-level keepalive / latency probe.
//!
//! Source: <https://docs.kraken.com/api/docs/websocket-v2/ping>. The request is just
//! `{method: "ping"}` (no params); the server answers `{method: "pong"}`,
//! echoing `req_id` when one was sent. Unlike other acks, `pong` omits
//! `success` — absence reads as success.

use crate::{MethodReply, Request, endpoint::Endpoint};
use serde::Serialize;

/// `ping` request params — none; the field-less struct serializes to an omitted
/// `params`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PingParams {}

impl Request for PingParams {
    const METHOD: &'static str = "ping";
    type Response = Pong;

    fn endpoint(&self) -> Endpoint {
        Endpoint::Public
    }

    /// `pong` carries no result body on the wire
    /// (<https://docs.kraken.com/api/docs/websocket-v2/ping>) — the variant itself is
    /// the whole reply.
    fn extract(reply: MethodReply) -> Option<Pong> {
        matches!(reply, MethodReply::Pong).then_some(Pong {})
    }
}

/// The `pong` reply. Carries no fields; serializes to `{"method":"pong"}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "method", rename = "pong")]
pub struct Pong {}

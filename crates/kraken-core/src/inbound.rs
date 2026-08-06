//! Inbound frame classification: one text frame in, one typed [`Inbound`] out.

use serde::Deserialize;

use super::response::{Method, MethodResponse};
use super::subscribe::{Channel, ChannelMessage};
use crate::error::{Error, Result};

/// What a decoded inbound frame is. Exhaustive over the wire: a frame that fits none of
/// these is a parse error, never silently dropped.
#[derive(Debug)]
pub enum Inbound {
    /// A channel-data frame (market data or an account stream).
    Channel(ChannelMessage),
    /// A method reply: an order reply, a subscribe/unsubscribe ack, an error, or
    /// `pong`. Routed by `req_id` to an awaiting caller, or surfaced as an event.
    Method(MethodResponse),
    /// A server keepalive (`{"channel":"heartbeat"}`). Deliberately payload-less on the
    /// wire (<https://docs.kraken.com/api/docs/websocket-v2/heartbeat>), so a unit
    /// variant *is* its full type; its arrival is the connection-level liveness signal
    /// the live monitor watches to tell a stalled socket from a merely quiet market.
    Heartbeat,
}

impl std::str::FromStr for Inbound {
    type Err = Error;

    /// Classify one inbound text frame — the single source of truth for what a frame *is*.
    /// Deliberately strict: an unknown discriminant, a payload off its schema, or a frame
    /// with neither key fails, so a surprise on the wire is a logged error to fix, never
    /// silently reinterpreted. The one tolerance is vocabulary inside a payload: an
    /// unrecognized enum token decodes to that enum's `Unknown` variant, degrading one
    /// field, not the frame.
    fn from_str(text: &str) -> Result<Self> {
        /// A channel frame carries `channel`, a method reply carries `method`. Both parse
        /// straight into their typed vocabularies; a name outside either catalogue fails
        /// the frame here.
        #[derive(Deserialize)]
        struct Routing {
            channel: Option<Channel>,
            method: Option<Method>,
        }

        // The probe skips every non-discriminant field, and the winning side decodes
        // straight from the text: two linear scans, no intermediate
        // `serde_json::Value` DOM and none of its per-key/value String clones. (The
        // flattened envelope still buffers through serde's internal `Content`; what
        // this removes is the extra Value tree on top of it, once per frame.)
        let Routing { channel, method } = serde_json::from_str(text)?;
        match (channel, method) {
            // A frame claiming to be both is outside the protocol; guessing a side
            // would mask the drift.
            (Some(_), Some(_)) => Err(Error::protocol("frame carries both `channel` and `method`")),
            (Some(Channel::Heartbeat), None) => Ok(Self::Heartbeat),
            (Some(_), None) => Ok(Self::Channel(serde_json::from_str::<ChannelMessage>(text)?)),
            (None, Some(_)) => Ok(Self::Method(serde_json::from_str::<MethodResponse>(text)?)),
            (None, None) => Err(Error::protocol(
                "frame carries neither `channel` nor `method`",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_channel_data() {
        let frame = r#"{"channel":"trade","type":"update","data":[{
            "symbol":"BTC/USD","side":"buy","price":1.0,"qty":1.0,
            "ord_type":"market","trade_id":1,"timestamp":"TS"}]}"#;
        assert!(matches!(
            frame.parse::<Inbound>().unwrap(),
            Inbound::Channel(_)
        ));
    }

    #[test]
    fn heartbeat_is_classified_for_liveness() {
        assert!(matches!(
            r#"{"channel":"heartbeat"}"#.parse::<Inbound>().unwrap(),
            Inbound::Heartbeat
        ));
    }

    #[test]
    fn a_frame_with_no_discriminant_is_a_hard_error() {
        assert!(r#"{"unexpected":"shape"}"#.parse::<Inbound>().is_err());
    }

    #[test]
    fn a_frame_with_both_discriminants_is_a_hard_error() {
        assert!(r#"{"channel":"trade","method":"pong"}"#.parse::<Inbound>().is_err());
    }

    #[test]
    fn a_drifted_method_result_is_a_hard_error() {
        let reply = r#"{"method":"add_order","success":true,"result":{"order_id":42},"req_id":9}"#;
        assert!(reply.parse::<Inbound>().is_err());
    }

    #[test]
    fn an_unknown_channel_is_a_hard_error() {
        let frame = r#"{"channel":"brand_new_feed","type":"update","data":[{"x":1}]}"#;
        assert!(frame.parse::<Inbound>().is_err());
    }

    #[test]
    fn an_unknown_method_is_a_hard_error() {
        let reply = r#"{"method":"brand_new_method","success":true}"#;
        assert!(reply.parse::<Inbound>().is_err());
    }
}

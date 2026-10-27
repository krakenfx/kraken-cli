//! The method-reply envelope every reply shares, mirroring the channel side's
//! `DataMessage` envelope.

use super::result::{Method, MethodReply};
use crate::{
    error::{Error, Result},
    wire::Request,
};
use serde::Deserialize;

/// A method reply envelope — order replies, `pong`, and subscribe/unsubscribe
/// acks all share this shape. The method-tagged result body is flattened in, so
/// serde decodes it straight into its typed [`MethodReply`] at ingest;
/// [`Self::into_result`] extracts the one a request statically expects.
#[derive(Debug, Clone, Deserialize)]
pub struct MethodResponse {
    #[serde(flatten)]
    pub reply: MethodReply,
    /// Some replies (e.g. `pong`) omit `success`; absent reads as success.
    pub success: Option<bool>,
    pub error: Option<String>,
    /// On a *rejected* subscribe, the server names the offending pair at the
    /// top level (captured live: `{"error":"Currency pair not supported
    /// BOGUS/NOPE","method": "subscribe","success":false,"symbol":"BOGUS/
    /// NOPE",...}`) — a successful ack nests its echo under `result`
    /// instead. The connection actor evicts by this.
    pub symbol: Option<String>,
    pub req_id: Option<u64>,
    /// When the server received the request on the wire (RFC3339). Present on
    /// subscribe/method acks; absent on `pong`. Surfaced for the live monitor's
    /// server-side latency and clock-offset estimate — never required by the
    /// trade path.
    pub time_in: Option<String>,
    /// When the server sent the acknowledgement on the wire (RFC3339). See
    /// [`Self::time_in`].
    pub time_out: Option<String>,
}

impl MethodResponse {
    pub fn method(&self) -> Method {
        self.reply.method()
    }

    /// A reply is a failure only if it explicitly carries `success: false`.
    pub fn is_success(&self) -> bool {
        self.success != Some(false)
    }

    /// Extract the typed result a request statically expects. A correlated
    /// server rejection becomes [`Error::ServerRejected`]; an id-less one —
    /// the server refused the whole frame before correlating — becomes
    /// [`Error::FrameRejected`], so batch callers can tell the two apart.
    /// Anything but the expected result variant — absent when one is
    /// required, or a different method's — is a parse error.
    pub fn into_result<P: Request>(self) -> Result<P::Response> {
        if self.success == Some(false) {
            let message = self.error.unwrap_or_else(|| "unknown error".to_string());
            return Err(match self.req_id {
                None => Error::FrameRejected(message),
                Some(_) => Error::ServerRejected(message),
            });
        }
        P::extract(self.reply)
            .ok_or_else(|| Error::protocol(format!("reply did not carry a {} result", P::METHOD)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AddOrderParams, Inbound};

    #[test]
    fn a_captured_subscribe_rejection_names_its_symbol_at_the_top_level() {
        let frame = r#"{"error":"Currency pair not supported BOGUS/NOPE","method":"subscribe","req_id":7,"success":false,"symbol":"BOGUS/NOPE","time_in":"2026-07-07T17:23:59.172238Z","time_out":"2026-07-07T17:23:59.172284Z"}"#;
        let Inbound::Method(resp) = frame.parse::<Inbound>().unwrap() else {
            panic!("a subscribe ack classifies as a method reply");
        };
        assert!(!resp.is_success());
        assert_eq!(resp.symbol.as_deref(), Some("BOGUS/NOPE"));
        assert!(matches!(resp.reply, MethodReply::Subscribe(None)));
        assert_eq!(resp.req_id, Some(7));
    }

    #[test]
    fn a_captured_subscribe_success_nests_its_echo_under_result() {
        let frame = r#"{"method":"subscribe","req_id":7,"result":{"channel":"ticker","event_trigger":"trades","snapshot":true,"symbol":"BTC/USD"},"success":true,"time_in":"2026-07-07T17:23:59.172238Z","time_out":"2026-07-07T17:23:59.172284Z"}"#;
        let Inbound::Method(resp) = frame.parse::<Inbound>().unwrap() else {
            panic!("a subscribe ack classifies as a method reply");
        };
        assert!(resp.is_success());
        assert!(resp.symbol.is_none());
        assert!(matches!(resp.reply, MethodReply::Subscribe(Some(_))));
    }

    #[test]
    fn classifies_method_reply_and_projects_result() {
        let reply =
            r#"{"method":"add_order","success":true,"result":{"order_id":"OXXX"},"req_id":9}"#;
        match reply.parse::<Inbound>().unwrap() {
            Inbound::Method(resp) => {
                assert_eq!(resp.req_id, Some(9));
                assert!(resp.is_success());
                let r = resp.into_result::<AddOrderParams>().unwrap();
                assert_eq!(r.order_id.as_deref(), Some("OXXX"));
            }
            _ => panic!("expected a method reply"),
        }
    }

    #[test]
    fn server_rejection_becomes_a_server_error() {
        let reply = r#"{"method":"add_order","success":false,"error":"EOrder:Bad","req_id":9}"#;
        let Inbound::Method(resp) = reply.parse::<Inbound>().unwrap() else {
            panic!("expected a method reply");
        };
        let err = resp.into_result::<AddOrderParams>().unwrap_err();
        assert!(matches!(err, Error::ServerRejected(m) if m == "EOrder:Bad"));
    }

    #[test]
    fn an_id_less_rejection_brands_as_a_frame_rejection() {
        // No `req_id` echo: the server refused the frame before correlating, so the
        // rejection answers the whole request, not one id — batch callers key on this.
        let reply = r#"{"method":"add_order","success":false,"error":"EOrder:Bad"}"#;
        let Inbound::Method(resp) = reply.parse::<Inbound>().unwrap() else {
            panic!("expected a method reply");
        };
        let err = resp.into_result::<AddOrderParams>().unwrap_err();
        assert!(matches!(err, Error::FrameRejected(m) if m == "EOrder:Bad"));
    }

    #[test]
    fn subscribe_ack_parses_wire_timestamps() {
        let ack = r#"{"method":"subscribe","success":true,"result":{"channel":"ticker"},
            "time_in":"2026-06-27T17:26:08.596889Z","time_out":"2026-06-27T17:26:08.596933Z"}"#;
        let Inbound::Method(resp) = ack.parse::<Inbound>().unwrap() else {
            panic!("expected a method reply");
        };
        assert_eq!(resp.time_in.as_deref(), Some("2026-06-27T17:26:08.596889Z"));
        assert_eq!(
            resp.time_out.as_deref(),
            Some("2026-06-27T17:26:08.596933Z")
        );
    }

    #[test]
    fn an_absent_result_decodes_to_none_not_an_error() {
        let reply = r#"{"method":"cancel_order","success":false,"error":"EOrder:Unknown order"}"#;
        let Inbound::Method(resp) = reply.parse::<Inbound>().unwrap() else {
            panic!("expected a method reply");
        };
        assert!(matches!(resp.reply, MethodReply::CancelOrder(None)));
    }

    #[test]
    fn a_result_body_on_pong_is_a_hard_error() {
        let reply = r#"{"method":"pong","result":{"unexpected":true},"req_id":1}"#;
        assert!(reply.parse::<Inbound>().is_err());
    }

    /// [`Method`] is derived from [`MethodReply`], but its wire strings come
    /// from strum's snake_case while the reply tags come from serde's — two
    /// casing algorithms that could diverge on a future name. Pin their
    /// agreement here. A bare `{method, req_id}` frame is every method's
    /// minimal reply (rejections omit `result`; `ping`/`pong` never carry
    /// one), so all eleven must decode from it.
    #[test]
    fn every_method_in_the_catalogue_dispatches_to_a_reply() {
        use strum::IntoEnumIterator;
        for method in Method::iter() {
            let raw = format!(r#"{{"method":"{method}","req_id":1}}"#);
            let Ok(Inbound::Method(resp)) = raw.parse::<Inbound>() else {
                panic!("method `{method}` has no MethodReply counterpart");
            };
            assert_eq!(
                resp.method(),
                method,
                "reply → method mapping must round-trip for `{method}`"
            );
        }
    }
}

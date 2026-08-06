//! `cancel_order` — cancel by order id, client order id, or user reference.
//!
//! Source: <https://docs.kraken.com/api/docs/websocket-v2/cancel_order>. Exactly one
//! identifier kind per request; the reply carries `order_id` (and echoed `cl_ord_id`)
//! per cancelled order.

use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};

use crate::{MethodReply, Request};

/// `cancel_order` request params: one identifier list per the v2 contract, so the
/// mixed-kind state is unrepresentable. When several ids are given the engine streams
/// one reply per id, all sharing the request id; the request path gathers them all
/// before the server closes the socket — see
/// [`expected_replies`](CancelOrderParams::expected_replies). Untagged: each variant
/// serializes as its own wire key (`order_id`, `cl_ord_id`, `order_userref`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum CancelOrderParams {
    OrderIds { order_id: Vec<String> },
    ClOrdIds { cl_ord_id: Vec<String> },
    UserRefs { order_userref: Vec<i32> },
}

impl CancelOrderParams {
    /// Identifiers in the populated list — the number of individual replies Kraken
    /// streams back for this cancel.
    fn id_count(&self) -> usize {
        match self {
            Self::OrderIds { order_id } => order_id.len(),
            Self::ClOrdIds { cl_ord_id } => cl_ord_id.len(),
            Self::UserRefs { order_userref } => order_userref.len(),
        }
    }
}

impl Request for CancelOrderParams {
    const METHOD: &'static str = "cancel_order";
    type Response = CancelOrderResult;

    fn extract(reply: MethodReply) -> Option<CancelOrderResult> {
        reply.try_as_cancel_order().flatten()
    }

    /// One reply per identifier. The CLI guarantees a non-empty list; fall back to one
    /// defensively rather than panicking on a hand-built empty variant.
    fn expected_replies(&self) -> NonZeroUsize {
        NonZeroUsize::new(self.id_count()).unwrap_or(NonZeroUsize::MIN)
    }
}

/// `cancel_order` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CancelOrderResult {
    pub order_id: Option<String>,
    pub cl_ord_id: Option<String>,
    pub warnings: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_identifier_kind_serializes_as_its_own_wire_key() {
        let ids = CancelOrderParams::OrderIds {
            order_id: vec!["O1".into(), "O2".into()],
        };
        assert_eq!(
            serde_json::to_value(&ids).unwrap(),
            serde_json::json!({"order_id": ["O1", "O2"]})
        );
        let refs = CancelOrderParams::UserRefs {
            order_userref: vec![-7, 9],
        };
        assert_eq!(
            serde_json::to_value(&refs).unwrap(),
            serde_json::json!({"order_userref": [-7, 9]})
        );
    }
}

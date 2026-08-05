//! `cancel_all_orders_after` — the dead-man's switch.
//!
//! Source: <https://docs.kraken.com/api/docs/websocket-v2/cancel_after> (the doc page
//! slug is `cancel_after`, though the wire method is `cancel_all_orders_after`). The reply
//! echoes the engine's `currentTime` and the `triggerTime` at which the sweep will fire.

use serde::{Deserialize, Serialize};

use crate::{MethodReply, Request};

/// `cancel_all_orders_after` request params (the dead-man's switch). `timeout` is in
/// seconds (max 86400); `0` cancels the timer without touching any orders.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CancelAllOrdersAfterParams {
    pub timeout: u64,
}

impl Request for CancelAllOrdersAfterParams {
    const METHOD: &'static str = "cancel_all_orders_after";
    type Response = CancelAllOrdersAfterResult;

    fn extract(reply: MethodReply) -> Option<CancelAllOrdersAfterResult> {
        reply.try_as_cancel_all_orders_after().flatten()
    }
}

/// `cancel_all_orders_after` result. Both timestamps are RFC3339. Unusually for v2 the
/// wire keys are camelCase (`currentTime`/`triggerTime`), hence the explicit renames.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CancelAllOrdersAfterResult {
    #[serde(rename = "currentTime")]
    pub current_time: String,
    #[serde(rename = "triggerTime")]
    pub trigger_time: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Inbound;

    /// The crate's only camelCase renames — its most drift-prone serde spot — pinned
    /// against the docs' reply shape in both directions.
    #[test]
    fn a_docs_shaped_reply_round_trips_its_camel_case_keys() {
        let frame = r#"{"method":"cancel_all_orders_after","req_id":6,"result":{"currentTime":"2023-09-21T12:03:14Z","triggerTime":"2023-09-21T12:04:14Z"},"success":true}"#;
        let Inbound::Method(resp) = frame.parse::<Inbound>().unwrap() else {
            panic!("a cancel_all_orders_after ack classifies as a method reply");
        };
        let result = resp
            .into_result::<CancelAllOrdersAfterParams>()
            .expect("the docs-shaped result decodes");
        assert_eq!(result.current_time, "2023-09-21T12:03:14Z");
        assert_eq!(result.trigger_time, "2023-09-21T12:04:14Z");

        let out = serde_json::to_value(&result).unwrap();
        assert_eq!(out["currentTime"], "2023-09-21T12:03:14Z");
        assert_eq!(out["triggerTime"], "2023-09-21T12:04:14Z");
    }
}

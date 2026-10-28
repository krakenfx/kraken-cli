//! `batch_cancel` — cancel many orders by id in a single frame. Replies with
//! the shared [`CountResult`].
//!
//! Source: <https://docs.kraken.com/api/docs/websocket-v2/batch_cancel>. `orders` holds
//! Kraken `order_id`s or numeric `order_userref`s (the docs allow 2–50 per
//! batch); the optional `cl_ord_id` list cancels by client id.

use super::cancel_all::CountResult;
use crate::{MethodReply, Request};
use serde::Serialize;

/// `batch_cancel` request params (see the module schema).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct BatchCancelParams {
    pub orders: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cl_ord_id: Vec<String>,
}

impl Request for BatchCancelParams {
    const METHOD: &'static str = "batch_cancel";
    type Response = CountResult;

    fn extract(reply: MethodReply) -> Option<CountResult> {
        reply.try_as_batch_cancel().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Inbound, Wire};

    #[test]
    fn batch_cancel_frame_pins_the_wire_shape() {
        let params = BatchCancelParams {
            orders: vec!["OZQNQ1-ABC12-XYZ789".into(), "42".into()],
            cl_ord_id: vec![],
        };
        let json = serde_json::to_string(&Wire::new(
            BatchCancelParams::METHOD,
            Some("tok-1"),
            &params,
            Some(3),
        ))
        .expect("serializes");
        assert_eq!(
            json,
            r#"{"method":"batch_cancel","params":{"token":"tok-1","orders":["OZQNQ1-ABC12-XYZ789","42"]},"req_id":3}"#
        );
    }

    /// The docs' success reply: the cancelled count nested under `result`.
    #[test]
    fn a_docs_shaped_batch_cancel_reply_decodes_its_count() {
        let reply = r#"{"method":"batch_cancel","req_id":1234567890,"result":{"count":3},"success":true,"time_in":"2022-06-13T08:09:10.123456Z","time_out":"2022-06-13T08:09:10.789012Z"}"#;
        let Inbound::Method(resp) = reply.parse::<Inbound>().expect("the frame decodes") else {
            panic!("expected a method reply");
        };
        let result = resp
            .into_result::<BatchCancelParams>()
            .expect("the reply projects into the count result");
        assert_eq!(result.count, 3);
    }
}

//! `cancel_all` — cancel every open order. Owns [`CountResult`], the count-only
//! reply shared with [`batch_cancel`](super::batch_cancel).
//!
//! Source: <https://docs.kraken.com/api/docs/websocket-v2/cancel_all>. The request carries
//! only the auth token (flattened in by [`Wire`](crate::Wire)), so its params
//! struct is empty; the reply is a `count` of orders cancelled plus optional `warnings`.

use serde::{Deserialize, Serialize};

use crate::{MethodReply, Request};

/// `cancel_all` request params (only the token, flattened in at send time).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CancelAllParams {}

impl Request for CancelAllParams {
    const METHOD: &'static str = "cancel_all";
    type Response = CountResult;

    fn extract(reply: MethodReply) -> Option<CountResult> {
        reply.try_as_cancel_all().flatten()
    }
}

/// `cancel_all` / `batch_cancel` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CountResult {
    pub count: u64,
    pub warnings: Option<Vec<String>>,
}

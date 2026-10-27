//! Outgoing framing: the [`Wire`] envelope every method rides, and the
//! [`Request`] contract binding typed params to their method name and typed
//! response.

use super::response::MethodReply;
use crate::endpoint::Endpoint;
use serde::Serialize;
use serde_with::skip_serializing_none;
use std::num::NonZeroUsize;

pub trait Request: Serialize {
    type Response: Serialize;

    /// The wire `method` name for a request carrying these params.
    const METHOD: &'static str;

    /// This request's typed result, or `None` when the reply answers a
    /// different method or omits a required body — the caller turns that
    /// into a protocol error.
    fn extract(reply: MethodReply) -> Option<Self::Response>;

    /// How many correlated replies the server streams for this request. One for
    /// every method except multi-id `cancel_order` (one per identifier);
    /// the caller compares it against what arrived, so a truncated batch
    /// fails instead of passing silently.
    fn expected_replies(&self) -> NonZeroUsize {
        NonZeroUsize::MIN
    }

    /// The socket this request rides. Defaults to the authenticated endpoint,
    /// where every order method lives.
    fn endpoint(&self) -> Endpoint {
        Endpoint::Auth
    }

    /// Whether the frame must carry an auth token. Derived from
    /// [`endpoint`](Self::endpoint) — only the public socket is tokenless —
    /// so a request can never declare an endpoint and a contradicting token
    /// requirement. (Mirrors `WsSubscription::requires_token`.)
    fn requires_token(&self) -> bool {
        self.endpoint().requires_token()
    }
}

/// A params object with the auth `token` flattened alongside its own fields —
/// the shape v2 expects (the token sits *inside* `params`). The `token` key is
/// dropped when absent (every public request: market-data subscriptions,
/// `ping`).
#[skip_serializing_none]
#[derive(Serialize)]
struct Authenticated<'a, P: Serialize> {
    token: Option<&'a str>,
    #[serde(flatten)]
    params: P,
}

/// The `{method, params, req_id}` frame shared by every outgoing method —
/// orders and channel subscriptions. The token (when any) is flattened into
/// `params`.
///
/// An empty `params` object is never emitted: the v2 server rejects `params:
/// {}` on paramless methods like `ping`, and a token-less, field-less method
/// has nothing to send. This is enforced once, structurally, in
/// [`Serialize`](Wire#impl-Serialize) — no method opts out, so `ping` needs no
/// special case.
pub struct Wire<'a, P: Serialize> {
    method: &'static str,
    params: Authenticated<'a, P>,
    req_id: Option<u64>,
}

/// Hand-written: `params` flattens the auth token in; a derived `Debug` would
/// print a live credential.
impl<P: Serialize> std::fmt::Debug for Wire<'_, P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wire")
            .field("method", &self.method)
            .field("req_id", &self.req_id)
            .finish_non_exhaustive()
    }
}

impl<'a, P: Serialize> Wire<'a, P> {
    pub fn new(
        method: &'static str,
        token: Option<&'a str>,
        params: P,
        req_id: Option<u64>,
    ) -> Self {
        Self {
            method,
            params: Authenticated { token, params },
            req_id,
        }
    }
}

impl<P: Serialize> Serialize for Wire<'_, P> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::{Error, SerializeMap};

        // Serialize the params (token already flattened in) up front so we can drop the
        // key entirely when it collapses to an empty object. `RawValue` preserves the
        // params' own field order, so every other method's wire bytes are unchanged.
        let params = serde_json::value::to_raw_value(&self.params).map_err(Error::custom)?;
        let has_params = params.get() != "{}";

        let len = 1 + usize::from(has_params) + usize::from(self.req_id.is_some());
        let mut map = serializer.serialize_map(Some(len))?;
        map.serialize_entry("method", self.method)?;
        if has_params {
            map.serialize_entry("params", &params)?;
        }
        if let Some(req_id) = self.req_id {
            map.serialize_entry("req_id", &req_id)?;
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CancelAllParams, PingParams};

    #[test]
    fn empty_params_object_is_dropped_from_the_frame() {
        let v = serde_json::to_value(Wire::new("ping", None, &PingParams {}, Some(7))).unwrap();
        assert_eq!(v["method"], "ping");
        assert!(v.get("params").is_none());
        assert_eq!(v["req_id"], 7);
    }

    #[test]
    fn token_only_params_object_is_kept() {
        let v = serde_json::to_value(Wire::new(
            "cancel_all",
            Some("tok-1"),
            &CancelAllParams {},
            Some(3),
        ))
        .unwrap();
        assert_eq!(v["params"]["token"], "tok-1");
    }
}

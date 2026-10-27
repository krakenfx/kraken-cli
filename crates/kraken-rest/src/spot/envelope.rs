//! Spot's response envelope, `{"error": [..], "result": ..}`, shared by every
//! endpoint.

use super::{Access as _, Endpoint};
use crate::{
    RestError,
    transport::{BodyLog, Reply, parse_json},
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::value::RawValue;

#[derive(Deserialize)]
struct Envelope<'a> {
    #[serde(default)]
    error: Vec<String>,
    #[serde(borrow)]
    result: Option<&'a RawValue>,
}

/// Kraken's first error wins over the HTTP status: it reports a rejection with
/// a 200 as often as with a 4xx.
pub(super) fn decode<E: Endpoint>(reply: &Reply) -> Result<E::Response, RestError> {
    decode_as(E::PATH, E::Access::BODY_LOG, reply)
}

fn decode_as<T: DeserializeOwned>(
    endpoint: &'static str,
    body_log: BodyLog,
    reply: &Reply,
) -> Result<T, RestError> {
    reply.trace(body_log);
    let envelope = match parse_json::<Envelope<'_>>(endpoint, &reply.body) {
        Ok(envelope) => envelope,
        Err(_) if !reply.status.is_success() => {
            return Err(RestError::status(reply.status, &reply.body));
        }
        Err(e) => return Err(e),
    };
    if let Some(first) = envelope.error.into_iter().next() {
        return Err(RestError::Rejected(first));
    }
    if !reply.status.is_success() {
        return Err(RestError::status(reply.status, &reply.body));
    }
    let result = envelope.result.map_or("null", RawValue::get);
    parse_json(endpoint, result.as_bytes())
}

/// A download's body is the file itself; only a failure is an envelope.
pub(super) fn file(reply: Reply) -> Result<Vec<u8>, RestError> {
    reply.trace(BodyLog::SizeOnly);
    if let Ok(envelope) = serde_json::from_slice::<Envelope<'_>>(&reply.body)
        && let Some(first) = envelope.error.into_iter().next()
    {
        return Err(RestError::Rejected(first));
    }
    if !reply.status.is_success() {
        return Err(RestError::status(reply.status, &reply.body));
    }
    Ok(reply.body)
}

/// Decodes a captured envelope the way a live reply is decoded, for fixture
/// tests.
#[cfg(test)]
pub(crate) fn decode_fixture<T: DeserializeOwned>(path: &str) -> Result<T, RestError> {
    let reply = crate::test_venue::fixture_reply("spot", path);
    decode_as("fixture", BodyLog::SizeOnly, &reply)
}

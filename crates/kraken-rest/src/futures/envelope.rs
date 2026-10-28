//! Futures replies carry their outcome inline: `"result": "error"` with an
//! `error` code, or a list of `errors`.

use super::{Access as _, Endpoint};
use crate::{
    RestError,
    transport::{BodyLog, Reply, parse_json},
};
use serde::{Deserialize, de::DeserializeOwned};

#[derive(Deserialize)]
struct Outcome {
    error: Option<String>,
    #[serde(default)]
    errors: Vec<serde_json::Value>,
}

impl Outcome {
    /// The first error Kraken named, if it named one.
    fn rejection(self) -> Option<String> {
        let first = self.error.filter(|e| !e.is_empty() && e != "success");
        first.or_else(|| {
            self.errors.into_iter().next().map(|e| match e {
                serde_json::Value::String(code) => code,
                other => other.to_string(),
            })
        })
    }
}

pub(super) fn decode<E: Endpoint>(reply: &Reply) -> Result<E::Response, RestError> {
    decode_as(E::PATH, E::Access::BODY_LOG, reply)
}

fn decode_as<T: DeserializeOwned>(
    endpoint: &'static str,
    body_log: BodyLog,
    reply: &Reply,
) -> Result<T, RestError> {
    reply.trace(body_log);
    rejection_or_status(reply)?;
    parse_json(endpoint, &reply.body)
}

/// A CSV download: the body is the file; only a failure is JSON.
pub(super) fn text<E: Endpoint>(reply: Reply) -> Result<String, RestError> {
    reply.trace(BodyLog::SizeOnly);
    rejection_or_status(&reply)?;
    String::from_utf8(reply.body).map_err(|source| RestError::NotText {
        endpoint: E::PATH,
        source,
    })
}

/// Kraken's named error wins over the HTTP status; a body that is not an
/// outcome falls back to the status.
fn rejection_or_status(reply: &Reply) -> Result<(), RestError> {
    if let Ok(outcome) = serde_json::from_slice::<Outcome>(&reply.body)
        && let Some(code) = outcome.rejection()
    {
        return Err(RestError::Rejected(code));
    }
    if !reply.status.is_success() {
        return Err(RestError::status(reply.status, &reply.body));
    }
    Ok(())
}

/// Decodes a captured reply the way a live one is decoded, for fixture tests.
#[cfg(test)]
pub(crate) fn decode_fixture<T: DeserializeOwned>(path: &str) -> Result<T, RestError> {
    let reply = crate::test_venue::fixture_reply("futures", path);
    decode_as("fixture", BodyLog::SizeOnly, &reply)
}

/// A captured live reply decodes and republishes exactly: no field dropped,
/// no number reshaped. Compared as JSON values, so key order is free.
#[cfg(test)]
pub(crate) fn assert_round_trips<T: DeserializeOwned + serde::Serialize>(path: &str) {
    let reply = crate::test_venue::fixture_reply("futures", path);
    let wire: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    let decoded: T =
        decode_as("fixture", BodyLog::SizeOnly, &reply).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert_eq!(serde_json::to_value(&decoded).unwrap(), wire, "{path}");
}

//! The one way building a request from arguments can fail.

/// Arguments that parsed individually but do not form a valid request —
/// mutually exclusive identifiers supplied together, a batch outside the
/// venue's bounds, a malformed JSON payload.
///
/// One variant deliberately, carrying the explanation: every caller turns this
/// into the same refusal, so splitting it per shape would buy no decision and
/// cost every front-end a match.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct InvalidRequest(String);

impl InvalidRequest {
    /// Takes `impl Into<String>` so callers pass a literal or a `format!`
    /// without an extra allocation decision at each site.
    pub fn new(explanation: impl Into<String>) -> Self {
        Self(explanation.into())
    }
}

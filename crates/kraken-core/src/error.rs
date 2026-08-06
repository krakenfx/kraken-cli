//! The protocol layer's error type.
//!
//! Every layer up to the application boundary speaks [`Error`]; the binary maps it
//! into its own edge taxonomy (envelope categories, exit codes) there.

use crate::endpoint::Endpoint;

/// A protocol-layer result.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong inside the WebSocket client.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The endpoint's session ended without a terminal event. On the request path this
    /// always surfaces wrapped in [`ReplyLost`](Self::ReplyLost) — it is a cause, not a verdict.
    #[error("{0} connection closed")]
    ConnectionClosed(Endpoint),

    /// The dial budget elapsed before a request could be sent — named for the phase
    /// that timed out, not the missing response. Retry-safe: nothing reached the wire.
    #[error("timed out before the request was sent; nothing reached the exchange")]
    DialTimeout,

    /// The frame was written but no reply ever arrived — the reply window elapsed, or
    /// the server closed without answering. Distinct from
    /// [`DialTimeout`](Self::DialTimeout), which covers the phases *before* the send: here the
    /// request may have reached the exchange and executed, so a caller must verify its
    /// state before retrying — a blind retry can double-submit an order.
    #[error(
        "no reply arrived; the request may have reached the exchange — verify \
         its state before retrying"
    )]
    Unacknowledged,

    /// The session failed *after* the request frame was sent; any reply was lost with
    /// it. Carries the underlying failure, with the same verify-before-retrying
    /// semantics as [`Unacknowledged`](Self::Unacknowledged).
    #[error(
        "the session failed after the request was sent; the request may have reached \
         the exchange — verify its state before retrying"
    )]
    ReplyLost(#[source] Box<Error>),

    /// A private endpoint was used without a configured token source.
    #[error("credentials required for the {0} endpoint")]
    NoTokenSource(Endpoint),

    /// Minting an auth token was rejected (a credentials problem); the app-layer wiring
    /// maps a REST auth failure into this.
    #[error("token fetch failed: {0}")]
    TokenRejected(String),

    /// Minting an auth token failed for a non-credentials reason (rate limit, network,
    /// IO, parse). Carries the application's original error opaquely — this crate is a
    /// pipe for it, not a classifier — so the failure keeps its full class through to
    /// the caller's envelope: a retryable network blip during a mint must not surface
    /// as a non-retryable `{"error":"auth"}` telling the user to re-key.
    #[error("token mint failed")]
    MintFailed(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The token mint outlived its budget — a wedged REST endpoint, not a credentials
    /// problem; surfaced as a timeout so the operator isn't misdirected to a socket
    /// that was never dialed.
    #[error("the token mint did not complete within {0:?}")]
    MintTimeout(std::time::Duration),

    /// The engine rejected a method (`success: false`); carries the server's error
    /// code so the host can classify it.
    #[error("{0}")]
    ServerRejected(String),

    /// The engine rejected the request frame as a whole: one id-less `success: false`
    /// reply, no `req_id` echo. Distinct from [`Self::ServerRejected`] so a batch
    /// caller can tell "the one reply that failed" from "the only reply there will
    /// ever be" — the per-id replies a whole-frame rejection leaves unanswered were
    /// never owed.
    #[error("{0}")]
    FrameRejected(String),

    /// A value failed validation against its allow-list (book depth, OHLC interval, …).
    #[error("{0}")]
    Validation(String),

    /// JSON (de)serialization failed (also covers a malformed frame).
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// A transport-level failure (connect, send, or stream error).
    #[error("transport error: {0}")]
    Transport(String),

    /// One dial attempt (TCP + TLS + WebSocket handshake) outlived the transport's
    /// per-attempt bound. Retry-safe: nothing was sent, and the dial-retry loops treat
    /// it like any other failed attempt.
    #[error("connect attempt timed out after {0:?}")]
    ConnectTimeout(std::time::Duration),

    /// A frame write outlived the transport's send bound — a wedged socket. Bytes may
    /// have partially left the process, so this carries the same verify-before-retrying
    /// semantics as [`Unacknowledged`](Self::Unacknowledged): the frame may still have
    /// reached the exchange, and a blind retry can double-submit an order.
    #[error(
        "send timed out after {0:?}; the frame may have reached the exchange — verify \
         its state before retrying"
    )]
    SendTimeout(std::time::Duration),

    /// The close handshake outlived the transport's bound. Dropping the writer after this
    /// closes the TCP abruptly, so callers tearing down anyway trace it and move on.
    #[error("close handshake timed out after {0:?}")]
    CloseTimeout(std::time::Duration),
}

impl Error {
    /// Whether the request may have reached the exchange: a blind retry risks a
    /// double submit, so order state must be verified first. The typed form of the
    /// "verify its state before retrying" prose these variants' `Display` carries.
    pub fn is_verify_first(&self) -> bool {
        matches!(
            self,
            Self::Unacknowledged | Self::SendTimeout(_) | Self::ReplyLost(_)
        )
    }

    /// Whether the venue rejected the session token itself (`EAPI:Invalid token`) — as
    /// opposed to rejecting the request the token authorized. Distinct from
    /// [`TokenRejected`](Self::TokenRejected), which is a mint-time credentials failure:
    /// this is a live token going dead, so callers drop their cached token and re-mint.
    pub fn is_token_rejection(&self) -> bool {
        matches!(
            self,
            Self::ServerRejected(message) | Self::FrameRejected(message)
                if message.contains("EAPI:Invalid token")
        )
    }

    /// A frame that violates the protocol's shape: a result where none belongs, a result
    /// from a different method, a frame with no discriminant. Classified as a parse
    /// error, the category every shape mismatch carries.
    pub fn protocol(message: impl std::fmt::Display) -> Self {
        Self::Json(<serde_json::Error as serde::de::Error>::custom(message))
    }

    /// A socket-level failure, stringified. The constructor the transport crate uses
    /// where a `From` impl would be an orphan (its error types are foreign here).
    pub fn transport(err: impl std::fmt::Display) -> Self {
        Self::Transport(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_first_marks_exactly_the_sent_but_unconfirmed_variants() {
        assert!(Error::Unacknowledged.is_verify_first());
        assert!(Error::SendTimeout(std::time::Duration::from_secs(5)).is_verify_first());
        assert!(Error::ReplyLost(Box::new(Error::Transport("boom".into()))).is_verify_first());
        // Pre-send failures must stay blindly retryable.
        assert!(!Error::DialTimeout.is_verify_first());
        assert!(!Error::Transport("refused".into()).is_verify_first());
        assert!(!Error::ConnectionClosed(Endpoint::Auth).is_verify_first());
    }

    #[test]
    fn only_the_venues_invalid_token_reply_is_a_token_rejection() {
        assert!(Error::FrameRejected("EAPI:Invalid token".into()).is_token_rejection());
        assert!(Error::ServerRejected("EAPI:Invalid token".into()).is_token_rejection());
        // An order rejection keeps the token; a mint failure never had a live one.
        assert!(!Error::ServerRejected("EOrder:Unknown order".into()).is_token_rejection());
        assert!(!Error::TokenRejected("EAPI:Invalid key".into()).is_token_rejection());
    }
}

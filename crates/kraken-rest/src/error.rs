/// Characters of a non-Kraken error body kept for the message.
const BODY_EXCERPT_CHARS: usize = 200;

/// Why a REST call failed: what happened, with Kraken's own code when it
/// rejected the request. Mapping that onto a published error category is the
/// caller's contract, not this crate's. Exhaustive on purpose: a new failure
/// mode must reach every caller's mapping as a compile error.
#[derive(Debug, thiserror::Error)]
pub enum RestError {
    /// The exchange never answered: connect, TLS, timeout, or a broken body.
    #[error(transparent)]
    Transport(#[from] reqwest::Error),

    /// Kraken rejected the request. Carries the first error code it sent, e.g.
    /// `EOrder:Insufficient funds`.
    #[error("{0}")]
    Rejected(String),

    /// A failure status whose body carried no Kraken error (a gateway page).
    #[error("HTTP {status}: {excerpt}")]
    Status {
        status: reqwest::StatusCode,
        excerpt: String,
    },

    /// The response did not match the endpoint's schema. The source names the
    /// failing field.
    #[error("{endpoint}: {source}")]
    Decode {
        endpoint: &'static str,
        #[source]
        source: serde_path_to_error::Error<serde_json::Error>,
    },

    /// A text download whose body is not UTF-8.
    #[error("{endpoint}: body is not UTF-8: {source}")]
    NotText {
        endpoint: &'static str,
        #[source]
        source: std::string::FromUtf8Error,
    },

    /// A write that may have executed although no usable reply proves it, so
    /// it must be verified, never resent. The source says what went wrong.
    #[error("outcome unknown")]
    Unconfirmed(#[source] Box<RestError>),

    /// The request could not be encoded: a parameter or an identity header
    /// that is not valid on the wire.
    #[error("{0}")]
    Encode(String),

    /// The request could not be signed: a malformed key or secret, or a clock
    /// outside the nonce range.
    #[error("{0}")]
    Signing(String),
}

impl RestError {
    /// A failed connect or build sent nothing, and a rejection or gateway
    /// refusal is final; any other failure leaves the exchange's side unknown.
    pub(crate) fn may_have_reached_venue(&self) -> bool {
        match self {
            Self::Transport(err) => !crate::transport::never_sent(err),
            Self::Decode { .. } => true,
            Self::Rejected(_)
            | Self::Status { .. }
            | Self::NotText { .. }
            | Self::Unconfirmed(_)
            | Self::Encode(_)
            | Self::Signing(_) => false,
        }
    }

    pub(crate) fn status(status: reqwest::StatusCode, body: &[u8]) -> Self {
        let body = String::from_utf8_lossy(body);
        Self::Status {
            status,
            excerpt: body.chars().take(BODY_EXCERPT_CHARS).collect(),
        }
    }
}

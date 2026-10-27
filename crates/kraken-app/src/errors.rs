//! Stable error taxonomy for JSON envelopes and exit handling.
//!
//! Protocol-specific errors are normalized here so every surface exposes the
//! same categories.

use serde::Serialize;
use strum::{Display, EnumIter};

/// One order a cancel sweep could not close, named so the caller can retry it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelFailure {
    pub txid: String,
    pub error: String,
    /// The cancel may have executed, so check the order before resending it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub verify_first: bool,
}

/// The structured face of a live-workspace gate refusal — one type both
/// renderers consume. The `refusal` tag plus its fields land verbatim in the
/// JSON envelope; [`Refusal::detail_lines`] renders the same facts as rows.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "refusal", rename_all = "snake_case")]
pub enum Refusal {
    /// The order spends beyond what the allocation ledger holds free of
    /// reservations.
    AllocationBudget {
        workspace: String,
        /// The asset the order spends (a pair leg, not necessarily the
        /// allocation currency).
        funding: String,
        #[serde(with = "rust_decimal::serde::str")]
        available: rust_decimal::Decimal,
        #[serde(with = "rust_decimal::serde::str")]
        required: rust_decimal::Decimal,
        /// False when the ledger could not be fully confirmed against the
        /// venue, so `available` may be stale.
        reconciled: bool,
    },
    /// Live claims exceed what the account holds.
    OverAllocated {
        workspace: String,
        backing: kraken_workspace::backing::Backing,
    },
    /// The account balance could not be read, so the claim is uncheckable.
    Unverified {
        workspace: String,
        backing: kraken_workspace::backing::Backing,
    },
}

impl Refusal {
    /// The envelope facts as aligned rows for the table-mode error render —
    /// same style as the cancel sweep's detail lines.
    fn detail_lines(&self) -> Vec<String> {
        match self {
            Self::AllocationBudget {
                workspace,
                funding,
                available,
                required,
                reconciled,
            } => {
                let mut lines = vec![
                    "  refused:    allocation budget".to_string(),
                    format!("  workspace:  {workspace}"),
                    format!("  available:  {available} {funding}"),
                    format!("  required:   {required} {funding}"),
                ];
                if !reconciled {
                    lines.push(
                        "  note:       ledger unconfirmed by the venue; available may be stale"
                            .to_string(),
                    );
                }
                lines
            }
            Self::OverAllocated { workspace, backing }
            | Self::Unverified { workspace, backing } => {
                let kind = match self {
                    Self::Unverified { .. } => "backing unverified",
                    _ => "backing over-allocated",
                };
                vec![
                    format!("  refused:    {kind}"),
                    format!("  workspace:  {workspace}"),
                    format!("  allocation: {} {}", backing.allocation, backing.asset),
                    format!(
                        "  account:    {} {}",
                        backing
                            .account
                            .map_or_else(|| "unread".to_string(), |held| held.to_string()),
                        backing.asset
                    ),
                    format!(
                        "  elsewhere:  {} {} claimed by other live workspaces",
                        backing.allocated_elsewhere, backing.asset
                    ),
                ]
            }
        }
    }
}

/// Kraken's reply for a transaction ID its trading engine does not hold open.
const UNKNOWN_ORDER_CODE: &str = "EOrder:Unknown order";

/// The one-line summary of a mixed cancel sweep. The middle clause is dropped
/// when nothing was already gone, so the common failure keeps its old wording.
fn cancel_summary(
    cancelled: &[String],
    already_gone: &[String],
    failed: &[CancelFailure],
) -> String {
    let total = cancelled.len() + already_gone.len() + failed.len();
    let gone = if already_gone.is_empty() {
        String::new()
    } else {
        format!("; {} already gone at the venue", already_gone.len())
    };
    format!(
        "cancelled {}/{total} order(s){gone}; {} failed",
        cancelled.len(),
        failed.len()
    )
}

/// Top-level error categories used in JSON error envelopes and exit codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Display, EnumIter)]
#[strum(serialize_all = "snake_case")]
pub enum ErrorCategory {
    Api,
    Auth,
    Network,
    RateLimit,
    Validation,
    Config,
    // snake_case would render "web_socket"; the envelope code is "websocket".
    #[strum(to_string = "websocket")]
    WebSocket,
    Io,
    Timeout,
    Parse,
}

/// The primary error type for all CLI operations.
#[derive(Debug, thiserror::Error)]
pub enum KrakenError {
    #[error("{message}")]
    Api {
        category: ErrorCategory,
        message: String,
    },

    #[error("Authentication failed: {0}")]
    Auth(String),

    /// A promotion evaluated but refused; the checklist rides the envelope
    /// so an agent reads what is missing instead of guessing.
    #[error("{message}")]
    NotPromotable {
        message: String,
        checklist: serde_json::Value,
    },

    /// Some orders closed at the venue and some did not. Every list rides the
    /// envelope: the cancels that succeeded are already real, and the caller
    /// needs the failures by name to retry. Fails the command so a flatten
    /// that only partly worked can never read as success.
    ///
    /// `already_gone` counts toward the total but not the failures: the venue
    /// holds no such order, so there was nothing left to cancel.
    #[error("{}", cancel_summary(cancelled, already_gone, failed))]
    PartialCancel {
        cancelled: Vec<String>,
        already_gone: Vec<String>,
        failed: Vec<CancelFailure>,
    },

    /// The venue holds no order with the transaction ID asked about. Typed
    /// apart from [`Self::Api`] because a cancel reads it as "nothing left to
    /// cancel there" — but never as proof the order did not fill: Kraken
    /// answers the same for an order that closed a moment ago.
    #[error("{0}")]
    UnknownOrder(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("{message}")]
    RateLimit {
        message: String,
        suggestion: String,
        retryable: bool,
        docs_url: String,
    },

    #[error("Validation error: {0}")]
    Validation(String),

    /// A pre-venue refusal from a live-workspace gate, structured so both
    /// output modes carry the same facts: typed envelope fields for agents,
    /// detail rows for humans. `message` keeps the full prose, so the
    /// rendered text is identical to the plain `Validation` form it replaced.
    #[error("Validation error: {message}")]
    Refused {
        message: String,
        refusal: Box<Refusal>,
    },

    #[error("Configuration error: {0}")]
    Config(String),

    /// `verify_first`: the request may have reached the exchange, so a blind
    /// retry risks a double submit — surfaced in the JSON envelope for
    /// agents.
    #[error("WebSocket error: {message}")]
    WebSocket { message: String, verify_first: bool },

    /// A REST write that may have executed without proof — `verify_first` in
    /// the envelope. The cause, the failure that hid the outcome, shapes the
    /// rest of it.
    #[error("{0}; the request may have executed — verify its state before retrying")]
    Unconfirmed(Box<KrakenError>),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Background task failed: {0}")]
    Task(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

impl KrakenError {
    /// Returns the error category for JSON envelope output.
    pub fn category(&self) -> ErrorCategory {
        match self {
            Self::Api { category, .. } => *category,
            Self::Unconfirmed(cause) => cause.category(),
            Self::Auth(_) => ErrorCategory::Auth,
            Self::Network(_) => ErrorCategory::Network,
            Self::RateLimit { .. } => ErrorCategory::RateLimit,
            Self::Validation(_) | Self::Refused { .. } | Self::NotPromotable { .. } => {
                ErrorCategory::Validation
            }
            Self::Config(_) => ErrorCategory::Config,
            Self::WebSocket { .. } => ErrorCategory::WebSocket,
            Self::Io(_) => ErrorCategory::Io,
            // A dead background task is a local runtime failure — routed like other in-process I/O.
            Self::Task(_) => ErrorCategory::Io,
            Self::Timeout(_) => ErrorCategory::Timeout,
            Self::Parse(_) => ErrorCategory::Parse,
            Self::Other(_) | Self::PartialCancel { .. } | Self::UnknownOrder(_) => {
                ErrorCategory::Api
            }
        }
    }

    /// A connection-level WebSocket failure making no claim about send phase —
    /// the constructor for every site outside the typed spot request path.
    pub(crate) fn websocket(message: impl Into<String>) -> Self {
        Self::WebSocket {
            message: message.into(),
            verify_first: false,
        }
    }

    /// The request may have reached the exchange, so a blind retry risks a
    /// double submit.
    pub(crate) fn is_verify_first(&self) -> bool {
        match self {
            Self::Unconfirmed(_) => true,
            Self::WebSocket { verify_first, .. } => *verify_first,
            Self::PartialCancel { failed, .. } => failed.iter().any(|f| f.verify_first),
            _ => false,
        }
    }

    /// A validation error that names the flag and env var the bad value came
    /// from.
    pub(crate) fn labelled(self, label: &str) -> Self {
        let inner = match self {
            Self::Validation(message) => message,
            other => other.to_string(),
        };
        Self::Validation(format!("{label}: {inner}"))
    }

    /// Constructs an API error from a Kraken error string (e.g. "EAPI:Invalid
    /// key").
    pub(crate) fn from_kraken_error(msg: &str) -> Self {
        if msg.starts_with("EAPI:Rate limit") {
            return Self::RateLimit {
                message: format!("Spot REST API rate limit exceeded ({msg})."),
                suggestion: "Wait 5-15 seconds before retrying. Reduce request frequency. Use \
                             WebSocket streaming instead of REST polling for real-time data. \
                             Batch order operations where possible (up to 15 per batch). Current \
                             limits depend on your account verification tier (Starter: 15 calls \
                             max / 0.33/s decay, Intermediate: 20 / 0.5/s, Pro: 20 / 1.0/s). \
                             Ledger and trade history calls cost 2 each, other calls cost 1. \
                             AddOrder and CancelOrder use a separate trading engine limiter."
                    .to_string(),
                retryable: true,
                docs_url: SPOT_REST_RATE_LIMITS_DOCS.to_string(),
            };
        }
        if msg.starts_with("EService:Throttled") || msg.starts_with("EService: Throttled") {
            return Self::RateLimit {
                message: format!("Too many concurrent requests ({msg})."),
                suggestion: "Reduce the number of parallel in-flight requests. This is a \
                             concurrency throttle, not a rate counter. Serialize requests or add \
                             a small delay between concurrent calls."
                    .to_string(),
                retryable: true,
                docs_url: SPOT_REST_RATE_LIMITS_DOCS.to_string(),
            };
        }
        if msg.starts_with("EOrder:Rate limit") {
            return Self::RateLimit {
                message: format!("Trading engine rate limit exceeded ({msg})."),
                suggestion: "The matching engine has per-pair rate limits separate from the REST \
                             API. Cancelling orders within 5 seconds costs +8 per order. Amending \
                             within 5 seconds costs +3. Let orders rest longer before cancelling \
                             or amending. Use batch orders to reduce per-order cost. Thresholds: \
                             Starter 60, Intermediate 125, Pro 180 per pair."
                    .to_string(),
                retryable: true,
                docs_url: "https://docs.kraken.com/api/docs/guides/spot-ratelimits".to_string(),
            };
        }
        if msg == "apiLimitExceeded" {
            return Self::RateLimit {
                message: "Futures API rate limit exceeded.".to_string(),
                suggestion: "Futures uses cost-based budgets: /derivatives endpoints have a \
                             budget of 500 per 10 seconds (sendorder costs 10, editorder costs \
                             10, cancelorder costs 10, batchorder costs 9 + batch size, \
                             cancelallorders costs 25, accounts costs 2). /history endpoints have \
                             a separate pool of 100 tokens replenishing at 100 per 10 minutes. \
                             Reduce request frequency or use batch orders to lower per-order cost."
                    .to_string(),
                retryable: true,
                docs_url: "https://docs.kraken.com/api/docs/guides/futures-rate-limits/"
                    .to_string(),
            };
        }
        if msg.starts_with("EGeneral:Permission")
            || msg.starts_with("EAPI:Invalid key")
            || msg == "authenticationError"
            || msg == "insufficientPrivileges"
        {
            return Self::Auth(msg.to_string());
        }
        if msg.starts_with(UNKNOWN_ORDER_CODE) {
            return Self::UnknownOrder(msg.to_string());
        }
        Self::Api {
            category: ErrorCategory::Api,
            message: msg.to_string(),
        }
    }

    /// Per-order detail for the human path, printed under the summary line.
    ///
    /// The JSON envelope carries these as typed arrays, but the summary line
    /// has room for neither the transaction ID nor the venue's reason — the
    /// only two facts a caller needs to retry. Empty for every other error.
    pub(crate) fn detail_lines(&self) -> Vec<String> {
        match self {
            Self::PartialCancel {
                cancelled,
                already_gone,
                failed,
            } => {
                // Failures first: they are the set the caller has to act on.
                let mut lines: Vec<String> = failed
                    .iter()
                    .map(|f| format!("  failed:       {} — {}", f.txid, f.error))
                    .collect();
                lines.extend(
                    already_gone.iter().map(|txid| {
                        format!("  already gone: {txid} — the venue holds no such order")
                    }),
                );
                lines.extend(
                    cancelled
                        .iter()
                        .map(|txid| format!("  cancelled:    {txid}")),
                );
                lines
            }
            Self::Refused { refusal, .. } => refusal.detail_lines(),
            Self::Unconfirmed(cause) => cause.detail_lines(),
            _ => Vec::new(),
        }
    }

    /// Builds the JSON error envelope.
    ///
    /// Rate limit errors include additional fields that LLM agents can use
    /// to adapt their strategy: `suggestion`, `retryable`, and `docs_url`.
    pub fn to_json_envelope(&self) -> serde_json::Value {
        let mut envelope = self.envelope_fields();
        if self.is_verify_first() {
            envelope["verify_first"] = true.into();
        }
        envelope
    }

    fn envelope_fields(&self) -> serde_json::Value {
        match self {
            Self::RateLimit {
                message,
                suggestion,
                retryable,
                docs_url,
            } => serde_json::json!({
                "error": "rate_limit",
                "message": message,
                "suggestion": suggestion,
                "retryable": retryable,
                "docs_url": docs_url,
            }),
            Self::Unconfirmed(cause) => {
                let mut envelope = cause.envelope_fields();
                envelope["message"] = self.to_string().into();
                envelope
            }
            Self::NotPromotable { message, checklist } => serde_json::json!({
                "error": "validation",
                "message": message,
                "checklist": checklist,
            }),
            Self::Refused { refusal, .. } => {
                let mut envelope = serde_json::json!({
                    "error": "validation",
                    "message": self.to_string(),
                });
                // The refusal's tag and fields land flat beside `message`, so
                // an agent reads `.refusal`/`.available` without re-parsing
                // prose; serialization of our own enum cannot fail. `extend`
                // overwrites on key collision, so no Refusal field may ever be
                // named `error` or `message`.
                if let (Some(envelope), Ok(serde_json::Value::Object(fields))) = (
                    envelope.as_object_mut(),
                    serde_json::to_value(refusal.as_ref()),
                ) {
                    envelope.extend(fields);
                }
                envelope
            }
            Self::PartialCancel {
                cancelled,
                already_gone,
                failed,
            } => serde_json::json!({
                "error": "api",
                "message": self.to_string(),
                "cancelled": cancelled,
                "already_gone_at_venue": already_gone,
                "failed": failed,
            }),
            _ => serde_json::json!({
                "error": self.category().to_string(),
                "message": self.to_string(),
            }),
        }
    }
}

/// Render `err` with its full source chain (`failure: cause: root`). The
/// protocol error's `Display` names only the failure phase — what killed a
/// `ReplyLost` session lives on `source()` — and the envelope has one
/// `message` field to carry both.
fn with_source_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

/// Arguments that parsed but do not form a valid request. Always `validation`:
/// the user typed a combination the venue's contract forbids, which is the same
/// class of refusal clap itself produces.
impl From<kraken_contract::error::InvalidRequest> for KrakenError {
    fn from(err: kraken_contract::error::InvalidRequest) -> Self {
        Self::Validation(err.to_string())
    }
}

impl From<kraken_core::Error> for KrakenError {
    fn from(err: kraken_core::Error) -> Self {
        use kraken_core::Error;
        match err {
            // A server rejection is classified exactly like a REST error so callers
            // route on the same stable categories (rate_limit, auth, api, ...).
            Error::ServerRejected(message) | Error::FrameRejected(message) => {
                KrakenError::from_kraken_error(&message)
            }
            Error::NoTokenSource(_) => KrakenError::Auth(err.to_string()),
            Error::TokenRejected(message) => KrakenError::Auth(message),
            // The mint failure carries this binary's own error whole; recover it so
            // nothing — category or rate-limit guidance — is lost crossing the WS layer.
            Error::MintFailed(source) => match source.downcast::<KrakenError>() {
                Ok(err) => *err,
                Err(source) => KrakenError::Api {
                    category: ErrorCategory::Api,
                    message: format!("token mint failed: {source}"),
                },
            },
            Error::MintTimeout(_) => KrakenError::Timeout(err.to_string()),
            Error::Validation(message) => KrakenError::Validation(message),
            Error::Json(e) => KrakenError::Parse(e.to_string()),
            // Everything left is a connection/transport-level fault. The sent-but-
            // unconfirmed phase crosses as the typed `verify_first` flag (surfaced in
            // the envelope); the Display prose restates it for humans.
            Error::ConnectionClosed(_)
            | Error::DialTimeout
            | Error::Unacknowledged
            | Error::ReplyLost(_)
            | Error::Transport(_)
            | Error::ConnectTimeout(_)
            | Error::SendTimeout(_)
            | Error::CloseTimeout(_) => KrakenError::WebSocket {
                verify_first: err.is_verify_first(),
                message: with_source_chain(&err),
            },
        }
    }
}

impl From<reqwest::Error> for KrakenError {
    fn from(err: reqwest::Error) -> Self {
        // reqwest's Display stops at "error sending request"; the cause
        // (timed out, connection refused) is only on the source chain.
        Self::Network(with_source_chain(&err))
    }
}

const SPOT_REST_RATE_LIMITS_DOCS: &str =
    "https://docs.kraken.com/api/docs/guides/spot-rest-ratelimits/";

/// Why a command refuses the active sign-in key, on REST and on the
/// WebSocket token mint alike.
pub(crate) const SIGN_IN_KEY_EXPIRED: &str = "The sign-in key expired. Run `kraken auth login`, \
                                              or `kraken auth switch api-key` to use the key from \
                                              `kraken auth set`.";

impl From<kraken_auth::Error> for KrakenError {
    fn from(err: kraken_auth::Error) -> Self {
        use kraken_auth::Error;
        let message = err.to_string();
        match err {
            // The user acts on the cause; `save_login_key` already tried to
            // delete the key.
            Error::Unconfirmed { cause, .. } => Self::from(*cause),
            // The category that every other Kraken call gives the same code.
            Error::KrakenApi { codes, .. } => Self::from_kraken_error(&codes.join(", ")),
            Error::Rejected { status: 429, .. } => Self::RateLimit {
                message,
                suggestion: "Wait a minute, then sign in again.".to_string(),
                retryable: true,
                docs_url: SPOT_REST_RATE_LIMITS_DOCS.to_string(),
            },
            Error::TimedOut(_) => Self::Timeout(message),
            Error::UnexpectedKey { .. } => Self::Api {
                category: ErrorCategory::Api,
                message,
            },
            Error::Denied
            | Error::AuthorizationFailed(_)
            | Error::ScopeNotGranted { .. }
            | Error::ScopeNotRequested { .. }
            | Error::Rejected {
                status: 400 | 401 | 403,
                ..
            }
            | Error::EmptySecretStdin
            | Error::EmptySecretFile(_) => Self::Auth(message),
            Error::Rejected { .. } => Self::Api {
                category: ErrorCategory::Api,
                message,
            },
            Error::NoFreePort { .. } | Error::Io(_) => Self::Io(std::io::Error::other(message)),
            Error::InsecureEndpoint(_)
            | Error::ProductionCodeOffKraken(_)
            | Error::Invalid { .. }
            | Error::LifetimeOutOfRange
            | Error::KeyNotStored(_) => Self::Validation(message),
            Error::Http(source) => source.into(),
            Error::Decode(_) | Error::Incomplete(_) => Self::Parse(message),
            Error::Random(_) | Error::ConfigSyntax(_) | Error::ConfigWrite(_) => {
                Self::Config(message)
            }
        }
    }
}

impl From<serde_json::Error> for KrakenError {
    fn from(err: serde_json::Error) -> Self {
        Self::Parse(err.to_string())
    }
}

/// The REST crate reports what happened; the category, remedy and retry
/// verdict for Kraken's own codes are decided here, once.
impl From<kraken_rest::RestError> for KrakenError {
    fn from(err: kraken_rest::RestError) -> Self {
        use kraken_rest::RestError;
        match err {
            RestError::Transport(e) => e.into(),
            RestError::Rejected(code) => Self::from_kraken_error(&code),
            e @ RestError::Status { .. } => Self::Network(e.to_string()),
            e @ (RestError::Decode { .. } | RestError::NotText { .. }) => {
                Self::Parse(e.to_string())
            }
            RestError::Unconfirmed(cause) => match Self::from(*cause) {
                // A refusal by name executed nothing, whatever the HTTP status.
                refused @ (Self::RateLimit { .. } | Self::Auth(_) | Self::UnknownOrder(_)) => {
                    refused
                }
                lost => Self::Unconfirmed(Box::new(lost)),
            },
            RestError::Encode(reason) => Self::Validation(reason),
            RestError::Signing(reason) => Self::Auth(reason),
        }
    }
}

impl From<tokio::task::JoinError> for KrakenError {
    fn from(err: tokio::task::JoinError) -> Self {
        Self::Task(err.to_string())
    }
}

impl From<url::ParseError> for KrakenError {
    fn from(err: url::ParseError) -> Self {
        Self::Validation(format!("Invalid URL: {err}"))
    }
}

impl From<tokio_tungstenite::tungstenite::Error> for KrakenError {
    fn from(err: tokio_tungstenite::tungstenite::Error) -> Self {
        // tungstenite's own Display already says what failed (handshake, IO, protocol,
        // closed); surface it verbatim under the `websocket` category so the `?`
        // operator can carry connect/send/handshake failures with no per-site wrapping.
        Self::websocket(err.to_string())
    }
}

impl From<tokio_tungstenite::tungstenite::http::header::InvalidHeaderValue> for KrakenError {
    fn from(err: tokio_tungstenite::tungstenite::http::header::InvalidHeaderValue) -> Self {
        Self::websocket(format!("Invalid header value: {err}"))
    }
}

/// The recording crate's envelope mapping: each of its failure modes lands on a
/// stable category (`agents/error-catalog.json`), so agent retry routing is
/// independent of where storage lives.
impl From<kraken_recording::Error> for KrakenError {
    fn from(err: kraken_recording::Error) -> Self {
        match err {
            kraken_recording::Error::Rejected(what) => KrakenError::Validation(what),
            kraken_recording::Error::Damaged(what) => KrakenError::Parse(what),
            // Local store, local I/O — `io`, without leaking engine internals
            // into the envelope's category vocabulary.
            #[cfg(feature = "record-duckdb")]
            kraken_recording::Error::Engine(e) => KrakenError::Io(std::io::Error::other(e)),
            kraken_recording::Error::Io(e) => KrakenError::Io(e),
        }
    }
}

/// The paper engine's envelope mapping. `Journal` delegates to the recording
/// bridge above, so every storage failure mode keeps exactly one category
/// authority — no re-mapping drift between layers.
impl From<kraken_paper::PaperError> for KrakenError {
    fn from(err: kraken_paper::PaperError) -> Self {
        match err {
            kraken_paper::PaperError::Rejected(what) => KrakenError::Validation(what),
            kraken_paper::PaperError::Incompatible(what) => KrakenError::Config(what),
            kraken_paper::PaperError::Journal(e) => e.into(),
        }
    }
}

/// The lab layer's envelope mapping. `Replay` delegates to the replay bridge
/// below — every failure mode keeps exactly one category authority.
impl From<kraken_lab::LabError> for KrakenError {
    fn from(err: kraken_lab::LabError) -> Self {
        use kraken_lab::LabError;
        match err {
            e @ (LabError::NoEpoch { .. }
            | LabError::ExperimentNotFound(_)
            | LabError::ExperimentExists(_)
            | LabError::Spec(_)) => KrakenError::Validation(e.to_string()),
            LabError::Damaged(what) => KrakenError::Parse(what),
            LabError::Store(e) => KrakenError::Io(e),
            LabError::Recording(e) => e.into(),
            LabError::Session(e) => e.into(),
            LabError::Replay(e) => e.into(),
        }
    }
}

/// The recipe layer's envelope mapping: refusals and absence are agent
/// guidance (`validation`), a newer schema is `config`, a broken artifact is
/// `parse`, and `Recording` delegates to the recording bridge above — one
/// category authority per storage failure mode.
impl From<kraken_recipe::RecipeError> for KrakenError {
    fn from(err: kraken_recipe::RecipeError) -> Self {
        use kraken_recipe::RecipeError;
        match err {
            e @ (RecipeError::NotFound(_) | RecipeError::Exists(_) | RecipeError::Spec(_)) => {
                KrakenError::Validation(e.to_string())
            }
            e @ RecipeError::Incompatible { .. } => KrakenError::Config(e.to_string()),
            RecipeError::Damaged(what) => KrakenError::Parse(what),
            RecipeError::Store(e) => KrakenError::Io(e),
            RecipeError::Recording(e) => e.into(),
        }
    }
}

/// The workspace layer's envelope mapping: refusals are agent guidance
/// (`validation`), store failures are local machine faults.
impl From<kraken_workspace::WorkspaceError> for KrakenError {
    fn from(err: kraken_workspace::WorkspaceError) -> Self {
        use kraken_workspace::WorkspaceError;
        match err {
            e @ (WorkspaceError::NotFound(_)
            | WorkspaceError::Exists(_)
            | WorkspaceError::Spec(_)
            | WorkspaceError::CapitalManaged(_)
            | WorkspaceError::PairNotAllowed { .. }
            | WorkspaceError::Unsupported { .. }
            | WorkspaceError::FuturesUnscoped { .. }
            | WorkspaceError::SimulationOnLiveLedger { .. }
            | WorkspaceError::NotReproducibleOnLiveLedger { .. }
            | WorkspaceError::SessionActive { .. }
            | WorkspaceError::NoActiveSession
            | WorkspaceError::SessionNotFound { .. }
            | WorkspaceError::LabelTaken { .. }) => KrakenError::Validation(e.to_string()),
            e @ WorkspaceError::NotPromotable { .. } => {
                let message = e.to_string();
                let WorkspaceError::NotPromotable { checklist, .. } = e else {
                    unreachable!("matched NotPromotable above");
                };
                KrakenError::NotPromotable {
                    message,
                    checklist: serde_json::to_value(*checklist).unwrap_or_default(),
                }
            }
            e @ WorkspaceError::Incompatible { .. } => KrakenError::Config(e.to_string()),
            e @ WorkspaceError::Damaged { .. } => KrakenError::Parse(e.to_string()),
            WorkspaceError::Paper(e) => e.into(),
            WorkspaceError::Recording(e) => e.into(),
            WorkspaceError::Session(e) => e.into(),
            WorkspaceError::Store(e) => KrakenError::Io(e),
        }
    }
}

/// The replay layer's envelope mapping: the not-stopped refusal is agent
/// guidance (`validation`), sink and render failures are local machine
/// faults.
impl From<kraken_replay::ReplayError> for KrakenError {
    fn from(err: kraken_replay::ReplayError) -> Self {
        use kraken_replay::ReplayError;
        match err {
            e @ ReplayError::NotStopped { .. } => KrakenError::Validation(e.to_string()),
            e @ ReplayError::NewerJournal { .. } => KrakenError::Config(e.to_string()),
            e @ ReplayError::NoEpoch { .. } => KrakenError::Validation(e.to_string()),
            ReplayError::Source(what) => KrakenError::Validation(what),
            ReplayError::Emit(e) => KrakenError::Io(e),
            ReplayError::Render(e) => KrakenError::Parse(e.to_string()),
        }
    }
}

/// The session layer's envelope mapping. `Recording` delegates to the
/// recording bridge above — one category authority per storage failure mode.
impl From<kraken_session::SessionError> for KrakenError {
    fn from(err: kraken_session::SessionError) -> Self {
        use kraken_session::SessionError;
        match err {
            SessionError::Rejected(what) => KrakenError::Validation(what),
            e @ SessionError::IncompatibleManifest { .. } => KrakenError::Config(e.to_string()),
            SessionError::Damaged(what) => KrakenError::Parse(what),
            SessionError::Recording(e) => e.into(),
            SessionError::Io(e) => KrakenError::Io(e),
        }
    }
}

pub type Result<T> = std::result::Result<T, KrakenError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_throttled_sign_in_is_a_retryable_rate_limit() {
        let throttled = kraken_auth::Error::Rejected {
            status: 429,
            code: "unexpected response".into(),
        };
        assert!(matches!(
            KrakenError::from(throttled),
            KrakenError::RateLimit {
                retryable: true,
                ..
            }
        ));
    }

    #[test]
    fn a_rate_limited_key_request_stays_a_retryable_rate_limit() {
        let rejected = kraken_auth::Error::KrakenApi {
            status: 200,
            codes: vec!["EAPI:Rate limit exceeded".into()],
        };
        assert!(matches!(
            KrakenError::from(rejected),
            KrakenError::RateLimit {
                retryable: true,
                ..
            }
        ));
    }
    use strum::IntoEnumIterator;

    /// The category codes are the CLI's stable JSON envelope contract; a new
    /// variant must be added here deliberately, with its wire string.
    #[test]
    fn category_codes_match_the_envelope_contract() {
        let codes: Vec<String> = ErrorCategory::iter().map(|c| c.to_string()).collect();
        assert_eq!(
            codes,
            [
                "api",
                "auth",
                "network",
                "rate_limit",
                "validation",
                "config",
                "websocket",
                "io",
                "timeout",
                "parse",
            ]
        );
    }

    /// A gate refusal's envelope carries its facts as typed fields beside the
    /// prose — the agent-facing half of the output contract. Field names and
    /// the `refusal` tag values are pinned: agents route on them.
    #[test]
    fn a_refusal_envelope_carries_typed_fields_beside_the_message() {
        use rust_decimal_macros::dec;
        let err = KrakenError::Refused {
            message: "Insufficient USD in live workspace 'prod': available 3.4844, required 4."
                .into(),
            refusal: Box::new(Refusal::AllocationBudget {
                workspace: "prod".into(),
                funding: "USD".into(),
                available: dec!(3.4844),
                required: dec!(4),
                reconciled: true,
            }),
        };
        let envelope = err.to_json_envelope();
        assert_eq!(envelope["error"], "validation");
        assert_eq!(envelope["refusal"], "allocation_budget");
        assert_eq!(envelope["workspace"], "prod");
        assert_eq!(envelope["funding"], "USD");
        assert_eq!(envelope["available"], "3.4844");
        assert_eq!(envelope["required"], "4");
        assert_eq!(envelope["reconciled"], true);
        assert!(
            envelope["message"]
                .as_str()
                .is_some_and(|m| m.starts_with("Validation error: Insufficient USD")),
            "the prose stays byte-compatible with the plain form it replaced"
        );
    }

    /// The backing refusals carry the whole backing object, so an agent reads
    /// the same shape a successful command stamps.
    #[test]
    fn a_backing_refusal_envelope_carries_the_backing_object() {
        use rust_decimal_macros::dec;
        let backing = kraken_workspace::backing::Backing::evaluate(
            "USD",
            dec!(100),
            kraken_workspace::backing::Claims::known(dec!(0)),
            None,
        );
        let err = KrakenError::Refused {
            message: "cannot verify".into(),
            refusal: Box::new(Refusal::Unverified {
                workspace: "prod".into(),
                backing,
            }),
        };
        let envelope = err.to_json_envelope();
        assert_eq!(envelope["refusal"], "unverified");
        assert_eq!(envelope["backing"]["status"], "unverified");
        assert_eq!(envelope["backing"]["allocation"], "100");
        assert!(envelope["backing"]["account"].is_null());
    }

    /// The table render shows the same facts as rows under the error line —
    /// the human-facing half of the same contract.
    #[test]
    fn a_refusal_renders_its_facts_as_detail_rows() {
        use rust_decimal_macros::dec;
        let err = KrakenError::Refused {
            message: "whatever".into(),
            refusal: Box::new(Refusal::AllocationBudget {
                workspace: "prod".into(),
                funding: "USD".into(),
                available: dec!(3.4844),
                required: dec!(4),
                reconciled: false,
            }),
        };
        let lines = err.detail_lines().join("\n");
        assert!(lines.contains("allocation budget"), "{lines}");
        assert!(lines.contains("workspace:  prod"), "{lines}");
        assert!(lines.contains("available:  3.4844 USD"), "{lines}");
        assert!(lines.contains("required:   4 USD"), "{lines}");
        assert!(lines.contains("may be stale"), "{lines}");
    }

    /// The two backing refusals share one detail arm; each must name itself,
    /// not the other — an unverified outage mislabelled "over-allocated"
    /// sends the user moving funds instead of fixing credentials.
    #[test]
    fn each_backing_refusal_names_its_own_kind() {
        use rust_decimal_macros::dec;
        let backing = |account| {
            kraken_workspace::backing::Backing::evaluate(
                "USD",
                dec!(100),
                kraken_workspace::backing::Claims::known(dec!(0)),
                account,
            )
        };
        let unverified = KrakenError::Refused {
            message: String::new(),
            refusal: Box::new(Refusal::Unverified {
                workspace: "prod".into(),
                backing: backing(None),
            }),
        };
        assert!(
            unverified
                .detail_lines()
                .join("\n")
                .contains("backing unverified"),
            "{:?}",
            unverified.detail_lines()
        );
        let over = KrakenError::Refused {
            message: String::new(),
            refusal: Box::new(Refusal::OverAllocated {
                workspace: "prod".into(),
                backing: backing(Some(dec!(50))),
            }),
        };
        assert!(
            over.detail_lines()
                .join("\n")
                .contains("backing over-allocated"),
            "{:?}",
            over.detail_lines()
        );
    }

    /// One row per `PaperError` variant: the envelope category each failure
    /// mode of the paper crate lands on is a pinned agent-routing contract.
    #[test]
    fn paper_error_categories_are_pinned() {
        use kraken_paper::PaperError;
        let cases: [(KrakenError, ErrorCategory); 4] = [
            (
                PaperError::Rejected("volume must be a positive number".into()).into(),
                ErrorCategory::Validation,
            ),
            (
                PaperError::Incompatible("record version 2 is newer".into()).into(),
                ErrorCategory::Config,
            ),
            (
                PaperError::Journal(kraken_recording::Error::Rejected("in use".into())).into(),
                ErrorCategory::Validation,
            ),
            (
                PaperError::Journal(std::io::Error::other("disk gone").into()).into(),
                ErrorCategory::Io,
            ),
        ];
        for (err, category) in cases {
            assert_eq!(err.category(), category, "{err}");
        }
    }

    /// One row per `LabError` variant: pinned agent-routing contract; the
    /// nested replay failure keeps the replay bridge's category, and the
    /// not-found hint — the agent skill's retry cue — stays byte-stable.
    #[test]
    fn lab_error_categories_are_pinned() {
        use kraken_lab::LabError;
        let not_found: KrakenError = LabError::ExperimentNotFound("m1".into()).into();
        assert_eq!(
            not_found.to_string(),
            "Validation error: experiment 'm1' not found; run 'kraken lab new m1' first"
        );
        let cases: [(KrakenError, ErrorCategory); 7] = [
            (not_found, ErrorCategory::Validation),
            (
                LabError::ExperimentExists("m1".into()).into(),
                ErrorCategory::Validation,
            ),
            (
                LabError::Spec("an experiment needs a hypothesis".into()).into(),
                ErrorCategory::Validation,
            ),
            (
                LabError::NoEpoch {
                    session: "s1".into(),
                }
                .into(),
                ErrorCategory::Validation,
            ),
            (
                LabError::Damaged("experiment file tampered".into()).into(),
                ErrorCategory::Parse,
            ),
            (
                LabError::Store(std::io::Error::other("disk gone")).into(),
                ErrorCategory::Io,
            ),
            (
                LabError::Replay(kraken_replay::ReplayError::NotStopped {
                    session: "s1".into(),
                })
                .into(),
                ErrorCategory::Validation,
            ),
        ];
        for (err, category) in cases {
            assert_eq!(err.category(), category, "{err}");
        }
    }

    /// One row per `RecipeError` variant: pinned agent-routing contract; the
    /// nested recording failure keeps the recording bridge's category, and
    /// the not-found hint — the agent skill's retry cue — stays byte-stable.
    #[test]
    fn recipe_error_categories_are_pinned() {
        use kraken_recipe::RecipeError;
        let not_found: KrakenError = RecipeError::NotFound("btc-levels".into()).into();
        assert_eq!(
            not_found.to_string(),
            "Validation error: recipe 'btc-levels' not found; list local recipes with 'kraken \
             recipe list'"
        );
        let cases: [(KrakenError, ErrorCategory); 7] = [
            (not_found, ErrorCategory::Validation),
            (
                RecipeError::Exists("btc-levels".into()).into(),
                ErrorCategory::Validation,
            ),
            (
                RecipeError::Spec("session 's1' has no strategy label".into()).into(),
                ErrorCategory::Validation,
            ),
            (
                RecipeError::Incompatible {
                    found: "2.0".into(),
                    expected: "1.0".into(),
                }
                .into(),
                ErrorCategory::Config,
            ),
            (
                RecipeError::Damaged("recipe 'x' cannot be decoded".into()).into(),
                ErrorCategory::Parse,
            ),
            (
                RecipeError::Store(std::io::Error::other("disk gone")).into(),
                ErrorCategory::Io,
            ),
            (
                RecipeError::Recording(kraken_recording::Error::Rejected("tape is in use".into()))
                    .into(),
                ErrorCategory::Validation,
            ),
        ];
        for (err, category) in cases {
            assert_eq!(err.category(), category, "{err}");
        }
    }

    /// One row per `WorkspaceError` variant: pinned agent-routing contract;
    /// the not-found hint — the agent skill's retry cue — stays byte-stable.
    #[test]
    fn workspace_error_categories_are_pinned() {
        use kraken_workspace::WorkspaceError;
        let not_found: KrakenError = WorkspaceError::NotFound("btc-momentum".into()).into();
        assert_eq!(
            not_found.to_string(),
            "Validation error: Workspace 'btc-momentum' not found. Create it with 'kraken \
             workspace create btc-momentum --capital <amount> --mode paper'."
        );
        let unsupported: KrakenError = WorkspaceError::Unsupported {
            what: "withdraw".into(),
            workspace: "w1".into(),
            mode: kraken_workspace::WorkspaceMode::Paper,
        }
        .into();
        assert!(
            unsupported.to_string().contains("workspace promote"),
            "the refusal names the promotion path: {unsupported}"
        );
        let cases: [(KrakenError, ErrorCategory); 11] = [
            (not_found, ErrorCategory::Validation),
            (
                WorkspaceError::PairNotAllowed {
                    pair: "SOL/USD".into(),
                    workspace: "scalper".into(),
                    allowed: "BTC/USD".into(),
                }
                .into(),
                ErrorCategory::Validation,
            ),
            (unsupported, ErrorCategory::Validation),
            (
                WorkspaceError::FuturesUnscoped {
                    workspace: "w1".into(),
                }
                .into(),
                ErrorCategory::Validation,
            ),
            (
                WorkspaceError::Exists("btc-momentum".into()).into(),
                ErrorCategory::Validation,
            ),
            (
                WorkspaceError::Spec("Invalid workspace name".into()).into(),
                ErrorCategory::Validation,
            ),
            (
                WorkspaceError::CapitalManaged("capital is fixed at create".into()).into(),
                ErrorCategory::Validation,
            ),
            (
                WorkspaceError::Incompatible {
                    found: "2.0".into(),
                    expected: "1.0".into(),
                }
                .into(),
                ErrorCategory::Config,
            ),
            (
                WorkspaceError::Damaged {
                    name: "w1".into(),
                    what: "not json".into(),
                }
                .into(),
                ErrorCategory::Parse,
            ),
            (
                WorkspaceError::Paper(kraken_paper::PaperError::Rejected("no epoch".into())).into(),
                ErrorCategory::Validation,
            ),
            (
                WorkspaceError::Store(std::io::Error::other("disk gone")).into(),
                ErrorCategory::Io,
            ),
        ];
        for (err, category) in cases {
            assert_eq!(err.category(), category, "{err}");
        }
    }

    /// One row per `ReplayError` variant, and the not-stopped hint — the
    /// agent skill's retry cue — stays byte-stable across the bridge.
    #[test]
    fn replay_error_categories_are_pinned() {
        use kraken_replay::ReplayError;
        let not_stopped: KrakenError = ReplayError::NotStopped {
            session: "s1".into(),
        }
        .into();
        assert_eq!(not_stopped.category(), ErrorCategory::Validation);
        assert_eq!(
            not_stopped.to_string(),
            "Validation error: session 's1' has no final summary to anchor on; run 'kraken \
             session stop' first"
        );

        let newer_journal: KrakenError = ReplayError::NewerJournal { skipped: 2 }.into();
        assert_eq!(newer_journal.category(), ErrorCategory::Config);

        let no_epoch: KrakenError = ReplayError::NoEpoch {
            session: "s1".into(),
        }
        .into();
        assert_eq!(no_epoch.category(), ErrorCategory::Validation);

        let emit: KrakenError = ReplayError::Emit(std::io::Error::other("sink gone")).into();
        assert_eq!(emit.category(), ErrorCategory::Io);

        let render_failure = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        let render: KrakenError = ReplayError::Render(render_failure).into();
        assert_eq!(render.category(), ErrorCategory::Parse);
    }

    /// One row per `SessionError` variant: pinned agent-routing contract, and
    /// the incompatible-manifest message stays byte-stable across the bridge.
    #[test]
    fn session_error_categories_are_pinned() {
        use kraken_session::SessionError;
        let cases: [(KrakenError, ErrorCategory); 5] = [
            (
                SessionError::Rejected("session x not found".into()).into(),
                ErrorCategory::Validation,
            ),
            (
                SessionError::IncompatibleManifest {
                    found: "2.0".into(),
                    expected: "1.0".into(),
                }
                .into(),
                ErrorCategory::Config,
            ),
            (
                SessionError::Damaged("expected value at line 1".into()).into(),
                ErrorCategory::Parse,
            ),
            (
                SessionError::Recording(kraken_recording::Error::Damaged("bad line".into())).into(),
                ErrorCategory::Parse,
            ),
            (
                SessionError::Io(std::io::Error::other("disk gone")).into(),
                ErrorCategory::Io,
            ),
        ];
        for (err, category) in cases {
            assert_eq!(err.category(), category, "{err}");
        }
        let incompatible: KrakenError = SessionError::IncompatibleManifest {
            found: "2.0".into(),
            expected: "1.0".into(),
        }
        .into();
        assert_eq!(
            incompatible.to_string(),
            "Configuration error: session manifest version 2.0 is incompatible with this build \
             (expects 1.0)"
        );
    }

    /// `ReplyLost`'s Display names only the failure phase; the envelope's one
    /// `message` must still carry the cause from the source chain.
    #[test]
    fn reply_lost_envelope_message_names_the_underlying_cause() {
        let err = kraken_core::Error::ReplyLost(Box::new(kraken_core::Error::Transport(
            "socket reset".into(),
        )));
        let mapped = KrakenError::from(err);
        assert!(mapped.to_string().contains("socket reset"), "got: {mapped}");
    }

    #[test]
    fn from_kraken_error_spot_rest_rate_limit() {
        let err = KrakenError::from_kraken_error("EAPI:Rate limit exceeded");
        assert_eq!(err.category(), ErrorCategory::RateLimit);
        if let KrakenError::RateLimit {
            retryable,
            docs_url,
            suggestion,
            ..
        } = &err
        {
            assert!(retryable);
            assert!(docs_url.contains("spot-rest-ratelimits"));
            assert!(suggestion.contains("Ledger"));
        } else {
            panic!("Expected RateLimit variant, got {err:?}");
        }
    }

    #[test]
    fn from_kraken_error_eservice_throttled() {
        let err = KrakenError::from_kraken_error("EService:Throttled");
        assert_eq!(err.category(), ErrorCategory::RateLimit);
        if let KrakenError::RateLimit { suggestion, .. } = &err {
            assert!(suggestion.contains("concurrency"));
        } else {
            panic!("Expected RateLimit variant");
        }
    }

    #[test]
    fn from_kraken_error_eservice_throttled_with_space() {
        let err = KrakenError::from_kraken_error("EService: Throttled: 1741500000");
        assert_eq!(err.category(), ErrorCategory::RateLimit);
    }

    #[test]
    fn from_kraken_error_trading_engine() {
        let err = KrakenError::from_kraken_error("EOrder:Rate limit exceeded");
        assert_eq!(err.category(), ErrorCategory::RateLimit);
        if let KrakenError::RateLimit {
            docs_url,
            suggestion,
            ..
        } = &err
        {
            assert!(docs_url.contains("spot-ratelimits"));
            assert!(suggestion.contains("per-pair"));
        } else {
            panic!("Expected RateLimit variant");
        }
    }

    /// Automation retries `network` and `rate_limit` and re-keys on `auth`, so
    /// a gateway page must never read as `parse` nor a throttle as `api`.
    #[test]
    fn a_rest_failure_keeps_the_category_automation_routes_on() {
        use kraken_rest::RestError;
        let category = |err: RestError| KrakenError::from(err).category();
        assert_eq!(
            category(RestError::Status {
                status: reqwest::StatusCode::BAD_GATEWAY,
                excerpt: "<html>".into(),
            }),
            ErrorCategory::Network
        );
        assert_eq!(
            category(RestError::Rejected("apiLimitExceeded".into())),
            ErrorCategory::RateLimit
        );
        assert_eq!(
            category(RestError::Rejected("EService:Throttled".into())),
            ErrorCategory::RateLimit
        );
        assert_eq!(
            category(RestError::Rejected("authenticationError".into())),
            ErrorCategory::Auth
        );
        assert_eq!(
            category(RestError::Signing("bad secret".into())),
            ErrorCategory::Auth
        );
        assert_eq!(
            category(RestError::Encode("Invalid X-Agent header".into())),
            ErrorCategory::Validation
        );
        assert_eq!(
            category(RestError::NotText {
                endpoint: "accountlogcsv",
                source: String::from_utf8(vec![0xff]).unwrap_err(),
            }),
            ErrorCategory::Parse
        );
    }

    /// A rate limit behind a 5xx was refused, not lost: it keeps its own
    /// envelope and needs no state check.
    #[test]
    fn an_unconfirmed_rate_limit_leads_as_a_rate_limit() {
        let cause = kraken_rest::RestError::Rejected("EAPI:Rate limit exceeded".into());
        let err = KrakenError::from(kraken_rest::RestError::Unconfirmed(Box::new(cause)));

        let envelope = err.to_json_envelope();

        assert_eq!(envelope["error"], "rate_limit");
        assert_eq!(envelope["retryable"], true);
        assert!(envelope.get("verify_first").is_none());
    }

    /// A cancel sweep reads this as "nothing left to cancel there".
    #[test]
    fn an_unconfirmed_unknown_order_stays_an_unknown_order() {
        let cause = kraken_rest::RestError::Rejected("EOrder:Unknown order".into());
        let err = KrakenError::from(kraken_rest::RestError::Unconfirmed(Box::new(cause)));

        assert!(matches!(err, KrakenError::UnknownOrder(_)), "{err:?}");
    }

    /// One unconfirmed cancel in a sweep flags the whole envelope; a plain
    /// failure does not.
    #[test]
    fn a_cancel_sweep_surfaces_an_unconfirmed_failure() {
        let sweep = |verify_first| KrakenError::PartialCancel {
            cancelled: vec!["OA".into()],
            already_gone: Vec::new(),
            failed: vec![CancelFailure {
                txid: "OB".into(),
                error: "timed out".into(),
                verify_first,
            }],
        };

        let unconfirmed = sweep(true).to_json_envelope();
        let plain = sweep(false).to_json_envelope();

        assert_eq!(unconfirmed["verify_first"], true);
        assert_eq!(unconfirmed["failed"][0]["verify_first"], true);
        assert!(plain.get("verify_first").is_none());
        assert!(plain["failed"][0].get("verify_first").is_none());
    }

    /// An accepted write with an unreadable reply keeps its cause's category
    /// and tells the agent to verify, not resend.
    #[test]
    fn an_unconfirmed_rest_write_carries_verify_first() {
        let cause = kraken_rest::RestError::Status {
            status: reqwest::StatusCode::BAD_GATEWAY,
            excerpt: "<html>".into(),
        };
        let err = KrakenError::from(kraken_rest::RestError::Unconfirmed(Box::new(cause)));

        let envelope = err.to_json_envelope();

        assert_eq!(envelope["error"], "network");
        assert_eq!(envelope["verify_first"], true);
        let message = envelope["message"].as_str().unwrap();
        assert!(message.starts_with("Network error: HTTP 502"), "{message}");
        assert!(
            message.ends_with("verify its state before retrying"),
            "{message}"
        );
    }

    #[test]
    fn from_kraken_error_futures_api_limit() {
        let err = KrakenError::from_kraken_error("apiLimitExceeded");
        assert_eq!(err.category(), ErrorCategory::RateLimit);
        if let KrakenError::RateLimit {
            docs_url,
            suggestion,
            ..
        } = &err
        {
            assert!(docs_url.contains("futures-rate-limits"));
            assert!(suggestion.contains("500 per 10 seconds"));
        } else {
            panic!("Expected RateLimit variant");
        }
    }

    #[test]
    fn from_kraken_error_auth() {
        let err = KrakenError::from_kraken_error("EAPI:Invalid key");
        assert_eq!(err.category(), ErrorCategory::Auth);
    }

    #[test]
    fn from_kraken_error_permission() {
        let err = KrakenError::from_kraken_error("EGeneral:Permission denied");
        assert_eq!(err.category(), ErrorCategory::Auth);
    }

    #[test]
    fn from_kraken_error_futures_authentication_error() {
        let err = KrakenError::from_kraken_error("authenticationError");
        assert_eq!(err.category(), ErrorCategory::Auth);
    }

    #[test]
    fn from_kraken_error_futures_insufficient_privileges() {
        let err = KrakenError::from_kraken_error("insufficientPrivileges");
        assert_eq!(err.category(), ErrorCategory::Auth);
    }

    #[test]
    fn from_kraken_error_unknown_falls_to_api() {
        let err = KrakenError::from_kraken_error("EGeneral:Unknown order");
        assert_eq!(err.category(), ErrorCategory::Api);
    }

    #[test]
    fn rate_limit_envelope_has_all_fields() {
        let err = KrakenError::RateLimit {
            message: "test".to_string(),
            suggestion: "wait".to_string(),
            retryable: true,
            docs_url: "https://example.com".to_string(),
        };
        let envelope = err.to_json_envelope();
        assert_eq!(envelope["error"], "rate_limit");
        assert_eq!(envelope["message"], "test");
        assert_eq!(envelope["suggestion"], "wait");
        assert_eq!(envelope["retryable"], true);
        assert_eq!(envelope["docs_url"], "https://example.com");
    }

    #[test]
    fn non_rate_limit_envelope_has_error_and_message() {
        let err = KrakenError::Auth("bad key".to_string());
        let envelope = err.to_json_envelope();
        assert_eq!(envelope["error"], "auth");
        assert!(envelope["message"].as_str().unwrap().contains("bad key"));
        assert!(envelope.get("suggestion").is_none());
    }

    #[test]
    fn the_sent_but_unconfirmed_phase_crosses_typed() {
        // The agent contract must not hang on Display prose: the phase crosses the
        // crate boundary as the typed flag.
        let sent: KrakenError = kraken_core::Error::Unacknowledged.into();
        assert!(matches!(
            sent,
            KrakenError::WebSocket {
                verify_first: true,
                ..
            }
        ));
        let pre_send: KrakenError = kraken_core::Error::Transport("refused".into()).into();
        assert!(matches!(
            pre_send,
            KrakenError::WebSocket {
                verify_first: false,
                ..
            }
        ));
    }

    #[test]
    fn verify_first_surfaces_in_the_websocket_envelope() {
        let sent: KrakenError = kraken_core::Error::Unacknowledged.into();
        let envelope = sent.to_json_envelope();
        assert_eq!(envelope["error"], "websocket");
        assert_eq!(envelope["verify_first"], true);

        let pre_send: KrakenError = kraken_core::Error::Transport("refused".into()).into();
        assert!(
            pre_send.to_json_envelope().get("verify_first").is_none(),
            "pre-send failures make no claim"
        );
    }

    /// reqwest's own Display says only "builder error" or "error sending
    /// request"; what went wrong is on the source chain.
    #[tokio::test]
    async fn a_reqwest_error_message_names_its_cause() {
        let err = reqwest::Client::builder()
            .build()
            .unwrap()
            .get("no-scheme")
            .send()
            .await
            .unwrap_err();

        let message = KrakenError::from(err).to_string();

        assert!(message.contains("relative URL without a base"), "{message}");
    }
}

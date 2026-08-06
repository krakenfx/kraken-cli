//! Stable error taxonomy for JSON envelopes and exit handling.
//!
//! Protocol-specific errors are normalized here so every surface exposes the
//! same categories.

use strum::{Display, EnumIter};

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

    #[error("Configuration error: {0}")]
    Config(String),

    /// `verify_first`: the request may have reached the exchange, so a blind retry
    /// risks a double submit — surfaced in the JSON envelope for agents.
    #[error("WebSocket error: {message}")]
    WebSocket { message: String, verify_first: bool },

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
            Self::Auth(_) => ErrorCategory::Auth,
            Self::Network(_) => ErrorCategory::Network,
            Self::RateLimit { .. } => ErrorCategory::RateLimit,
            Self::Validation(_) | Self::NotPromotable { .. } => ErrorCategory::Validation,
            Self::Config(_) => ErrorCategory::Config,
            Self::WebSocket { .. } => ErrorCategory::WebSocket,
            Self::Io(_) => ErrorCategory::Io,
            // A dead background task is a local runtime failure — routed like other in-process I/O.
            Self::Task(_) => ErrorCategory::Io,
            Self::Timeout(_) => ErrorCategory::Timeout,
            Self::Parse(_) => ErrorCategory::Parse,
            Self::Other(_) => ErrorCategory::Api,
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

    /// Constructs an API error from a Kraken error string (e.g. "EAPI:Invalid key").
    pub(crate) fn from_kraken_error(msg: &str) -> Self {
        if msg.starts_with("EAPI:Rate limit") {
            return Self::RateLimit {
                message: format!("Spot REST API rate limit exceeded ({msg})."),
                suggestion: "Wait 5-15 seconds before retrying. Reduce request frequency. \
                    Use WebSocket streaming instead of REST polling for real-time data. \
                    Batch order operations where possible (up to 15 per batch). \
                    Current limits depend on your account verification tier \
                    (Starter: 15 calls max / 0.33/s decay, \
                    Intermediate: 20 / 0.5/s, Pro: 20 / 1.0/s). \
                    Ledger and trade history calls cost 2 each, other calls cost 1. \
                    AddOrder and CancelOrder use a separate trading engine limiter."
                    .to_string(),
                retryable: true,
                docs_url: "https://docs.kraken.com/api/docs/guides/spot-rest-ratelimits/"
                    .to_string(),
            };
        }
        if msg.starts_with("EService:Throttled") || msg.starts_with("EService: Throttled") {
            return Self::RateLimit {
                message: format!("Too many concurrent requests ({msg})."),
                suggestion: "Reduce the number of parallel in-flight requests. \
                    This is a concurrency throttle, not a rate counter. \
                    Serialize requests or add a small delay between concurrent calls."
                    .to_string(),
                retryable: true,
                docs_url: "https://docs.kraken.com/api/docs/guides/spot-rest-ratelimits/"
                    .to_string(),
            };
        }
        if msg.starts_with("EOrder:Rate limit") {
            return Self::RateLimit {
                message: format!("Trading engine rate limit exceeded ({msg})."),
                suggestion:
                    "The matching engine has per-pair rate limits separate from the REST API. \
                    Cancelling orders within 5 seconds costs +8 per order. \
                    Amending within 5 seconds costs +3. \
                    Let orders rest longer before cancelling or amending. \
                    Use batch orders to reduce per-order cost. \
                    Thresholds: Starter 60, Intermediate 125, Pro 180 per pair."
                        .to_string(),
                retryable: true,
                docs_url: "https://docs.kraken.com/api/docs/guides/spot-ratelimits".to_string(),
            };
        }
        if msg == "apiLimitExceeded" {
            return Self::RateLimit {
                message: "Futures API rate limit exceeded.".to_string(),
                suggestion:
                    "Futures uses cost-based budgets: /derivatives endpoints have a budget \
                    of 500 per 10 seconds (sendorder costs 10, editorder costs 10, \
                    cancelorder costs 10, batchorder costs 9 + batch size, \
                    cancelallorders costs 25, accounts costs 2). \
                    /history endpoints have a separate pool of 100 tokens replenishing \
                    at 100 per 10 minutes. \
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
        Self::Api {
            category: ErrorCategory::Api,
            message: msg.to_string(),
        }
    }

    /// Builds the JSON error envelope.
    ///
    /// Rate limit errors include additional fields that LLM agents can use
    /// to adapt their strategy: `suggestion`, `retryable`, and `docs_url`.
    pub(crate) fn to_json_envelope(&self) -> serde_json::Value {
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
            Self::WebSocket {
                verify_first: true, ..
            } => serde_json::json!({
                "error": "websocket",
                "message": self.to_string(),
                "verify_first": true,
            }),
            Self::NotPromotable { message, checklist } => serde_json::json!({
                "error": "validation",
                "message": message,
                "checklist": checklist,
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
        Self::Network(err.to_string())
    }
}

impl From<serde_json::Error> for KrakenError {
    fn from(err: serde_json::Error) -> Self {
        Self::Parse(err.to_string())
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

impl From<toml::de::Error> for KrakenError {
    fn from(err: toml::de::Error) -> Self {
        Self::Config(format!("TOML parse error: {err}"))
    }
}

impl From<base64::DecodeError> for KrakenError {
    fn from(err: base64::DecodeError) -> Self {
        Self::Auth(format!("Base64 decode error: {err}"))
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
            | WorkspaceError::LiveUnavailable { .. }
            | WorkspaceError::PairNotAllowed { .. }
            | WorkspaceError::Unsupported { .. }
            | WorkspaceError::FuturesUnscoped { .. }
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
    use strum::IntoEnumIterator;

    use super::*;

    /// The category codes are the CLI's stable JSON envelope contract; a new variant
    /// must be added here deliberately, with its wire string.
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

    /// One row per `WorkspaceError` variant: pinned agent-routing contract;
    /// the not-found hint — the agent skill's retry cue — stays byte-stable.
    #[test]
    fn workspace_error_categories_are_pinned() {
        use kraken_workspace::WorkspaceError;
        let not_found: KrakenError = WorkspaceError::NotFound("btc-momentum".into()).into();
        assert_eq!(
            not_found.to_string(),
            "Validation error: Workspace 'btc-momentum' not found. Create it with \
             'kraken workspace create btc-momentum --capital <amount> --mode paper'."
        );
        let live: KrakenError = WorkspaceError::LiveUnavailable {
            name: "scalper".into(),
        }
        .into();
        assert!(
            live.to_string().contains("scoped credentials"),
            "the live refusal names the safety blocker: {live}"
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
        let cases: [(KrakenError, ErrorCategory); 12] = [
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
            (live, ErrorCategory::Validation),
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
            "Validation error: session 's1' has no final summary to anchor on; \
             run 'kraken session stop' first"
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
}

/// Unified error types for the Kraken CLI.
///
/// Error categories cover API responses, authentication failures, network
/// issues, rate-limiting, validation, configuration, WebSocket errors,
/// I/O, and response parsing problems.
use std::fmt;

/// Top-level error categories used in JSON error envelopes and exit codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    Api,
    Auth,
    Network,
    RateLimit,
    Validation,
    Config,
    WebSocket,
    Io,
    Parse,
}

impl fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Api => "api",
            Self::Auth => "auth",
            Self::Network => "network",
            Self::RateLimit => "rate_limit",
            Self::Validation => "validation",
            Self::Config => "config",
            Self::WebSocket => "websocket",
            Self::Io => "io",
            Self::Parse => "parse",
        };
        f.write_str(s)
    }
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

    #[error("WebSocket error: {0}")]
    WebSocket(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

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
            Self::Validation(_) => ErrorCategory::Validation,
            Self::Config(_) => ErrorCategory::Config,
            Self::WebSocket(_) => ErrorCategory::WebSocket,
            Self::Io(_) => ErrorCategory::Io,
            Self::Parse(_) => ErrorCategory::Parse,
            Self::Other(_) => ErrorCategory::Api,
        }
    }

    /// Constructs an API error from a Kraken error string (e.g. "EAPI:Invalid key").
    pub fn from_kraken_error(msg: &str) -> Self {
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
    pub fn to_json_envelope(&self) -> serde_json::Value {
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
            _ => serde_json::json!({
                "error": self.category().to_string(),
                "message": self.to_string(),
            }),
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

pub type Result<T> = std::result::Result<T, KrakenError>;

#[cfg(test)]
mod tests {
    use super::*;

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
}

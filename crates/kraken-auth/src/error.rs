use std::time::Duration;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sign-in timed out after {} minutes", .0.as_secs() / 60)]
    TimedOut(Duration),

    #[error("sign-in was declined in the browser")]
    Denied,

    /// The redirect carries an OAuth error, not a code.
    #[error("Kraken refused the sign-in: {0}")]
    AuthorizationFailed(String),

    #[error("Kraken did not grant: {}", .missing.join(", "))]
    ScopeNotGranted { missing: Vec<String> },

    /// A grant wider than the consent that kraken-cli asked for is refused.
    #[error("Kraken granted scopes that kraken-cli did not request: {}", .extra.join(", "))]
    ScopeNotRequested { extra: Vec<String> },

    /// Kraken created a key that differs from the request. `name` is the key
    /// that it created, so the caller can delete it.
    #[error("Kraken created key {name} with {mismatch}, which kraken-cli did not request")]
    UnexpectedKey {
        name: crate::KeyName,
        mismatch: KeyMismatch,
    },

    /// A create failed after the request, so Kraken can hold key `name`.
    /// Shows the cause's message: it is the failure the user acts on.
    #[error("{cause}")]
    Unconfirmed {
        name: crate::KeyName,
        cause: Box<Error>,
    },

    /// An OAuth error (RFC 6749 §5.2), or a reply that is not an envelope.
    #[error("Kraken rejected the request (HTTP {status}): {code}")]
    Rejected { status: u16, code: String },

    /// The `error` list of a Kraken API envelope, such as
    /// `EAPI:Rate limit exceeded`.
    #[error("Kraken rejected the request (HTTP {status}): {}", .codes.join(", "))]
    KrakenApi { status: u16, codes: Vec<String> },

    #[error("no redirect port is free (tried {ports:?})")]
    NoFreePort {
        ports: Vec<u16>,
        #[source]
        source: std::io::Error,
    },

    #[error("endpoint {0} must use https (http is allowed for loopback addresses only)")]
    InsecureEndpoint(String),

    #[error("the production sign-in sends its code to https://api.kraken.com only, not to {0}")]
    ProductionCodeOffKraken(String),

    #[error("invalid {what}: {value:?}")]
    Invalid { what: &'static str, value: String },

    #[error(
        "key lifetime must be between 1m and {}d",
        crate::KeyLifetime::MAX_DAYS
    )]
    LifetimeOutOfRange,

    #[error("Kraken's response has no {0}")]
    Incomplete(&'static str),

    #[error("the OS random number generator failed: {0}")]
    Random(getrandom::Error),

    #[error("no {} is stored", .0.label())]
    KeyNotStored(crate::KeySlot),

    // These two messages predate the crate: they are the published envelope
    // text, so they keep their casing.
    #[error("Empty secret received from stdin — did you forget to pipe input?")]
    EmptySecretStdin,

    #[error("Empty secret read from file: {}", .0.display())]
    EmptySecretFile(std::path::PathBuf),

    /// The parser's message only. `toml::de::Error` keeps the whole file for
    /// its `Debug` and quotes the bad line in its `Display`, and the file
    /// holds secrets, so it is never kept.
    #[error("the config file is not valid TOML: {0}")]
    ConfigSyntax(String),

    #[error("could not write the config file: {0}")]
    ConfigWrite(#[from] toml::ser::Error),

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error("unexpected response from Kraken: {0}")]
    Decode(#[from] serde_json::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Error {
    /// The key that Kraken can hold after this failure, which the caller
    /// should delete.
    pub fn key_on_account(&self) -> Option<&crate::KeyName> {
        match self {
            Self::UnexpectedKey { name, .. } | Self::Unconfirmed { name, .. } => Some(name),
            _ => None,
        }
    }
}

/// How a created key differs from its request.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum KeyMismatch {
    #[error("the name {0}")]
    Name(crate::KeyName),
    #[error("the expiry {0}")]
    Expiry(chrono::DateTime<chrono::Utc>),
    /// Flag names as Kraken sent them: one that kraken-cli never requests has
    /// no `Permission`.
    #[error("the permissions {}", .0.join(", "))]
    Permissions(Vec<String>),
}

impl From<toml::de::Error> for Error {
    fn from(error: toml::de::Error) -> Self {
        Self::ConfigSyntax(error.message().to_string())
    }
}

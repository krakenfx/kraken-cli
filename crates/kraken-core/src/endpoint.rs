//! Which socket a channel or method rides, and how that resolves to a URL.
//!
//! [`Endpoint`] is the pure classifier — public market data, authenticated
//! account/trading, or the Level 3 order book — and the single source of truth for
//! whether a token is required. Mapping an endpoint to a concrete URL is the only
//! transport concern here: [`Endpoint::default_url`] gives the production host, and
//! the client's per-endpoint overrides (used for beta/UAT) take precedence.

use strum::Display;

/// Production Kraken v2 WebSocket hosts, used when no override is configured.
const DEFAULT_PUBLIC_URL: &str = "wss://ws.kraken.com/v2";
const DEFAULT_AUTH_URL: &str = "wss://ws-auth.kraken.com/v2";
const DEFAULT_L3_URL: &str = "wss://ws-l3.kraken.com/v2";

/// Which socket endpoint a channel or method is served on. Doubles as the "which
/// socket" identity reported on lifecycle events, and decides whether an auth token
/// is required (every endpoint except [`Endpoint::Public`] needs one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display)]
#[strum(serialize_all = "lowercase")]
pub enum Endpoint {
    /// Public market data — no credentials.
    Public,
    /// Authenticated account/trading traffic.
    Auth,
    /// Level 3 order book — authenticated, on its own host.
    L3,
}

impl Endpoint {
    /// Whether this endpoint requires an auth token. Only the public market-data
    /// socket does not.
    pub const fn requires_token(self) -> bool {
        !matches!(self, Endpoint::Public)
    }

    /// Whether frames on this endpoint carry the account's own data (balances, order
    /// flow), so raw payloads must never be logged — even debug-gated. Narrower than
    /// [`requires_token`](Self::requires_token): L3 authenticates, but its frames are
    /// venue-wide book data, not the account's.
    pub const fn carries_account_data(self) -> bool {
        matches!(self, Endpoint::Auth)
    }

    /// The production host for this endpoint, used when the client has no override.
    pub const fn default_url(self) -> &'static str {
        match self {
            Endpoint::Public => DEFAULT_PUBLIC_URL,
            Endpoint::Auth => DEFAULT_AUTH_URL,
            Endpoint::L3 => DEFAULT_L3_URL,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_is_tokenless() {
        assert!(!Endpoint::Public.requires_token());
        assert!(Endpoint::Auth.requires_token());
        assert!(Endpoint::L3.requires_token());
    }

    /// The raw-payload logging scrub keys on this: a new private/account endpoint must
    /// classify itself here or its frames may be logged raw.
    #[test]
    fn only_auth_frames_carry_account_data() {
        assert!(Endpoint::Auth.carries_account_data());
        assert!(!Endpoint::Public.carries_account_data());
        assert!(!Endpoint::L3.carries_account_data());
    }

    #[test]
    fn default_urls_are_the_production_hosts() {
        assert_eq!(Endpoint::Public.default_url(), "wss://ws.kraken.com/v2");
        assert_eq!(Endpoint::Auth.default_url(), "wss://ws-auth.kraken.com/v2");
        assert_eq!(Endpoint::L3.default_url(), "wss://ws-l3.kraken.com/v2");
    }
}

//! The token the authenticated WebSocket feeds take in place of a signature.

use crate::{
    Delivery,
    spot::{Endpoint, Private, sealed::Sealed},
};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};

/// `POST /0/private/GetWebSocketsToken`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct WebSocketsTokenQuery {}

/// A live credential: it never reaches a log, and its `Debug` is redacted.
#[derive(Debug, Deserialize)]
pub struct WebSocketsToken {
    pub token: SecretString,
    /// Seconds the token stays valid if no connection uses it.
    pub expires: Option<u64>,
}

impl Sealed for WebSocketsTokenQuery {}
impl Endpoint for WebSocketsTokenQuery {
    const PATH: &'static str = "GetWebSocketsToken";
    type Access = Private;
    type Response = WebSocketsToken;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spot::envelope::decode_fixture;
    use secrecy::ExposeSecret as _;

    #[test]
    fn the_token_decodes_and_never_prints() {
        let minted: WebSocketsToken = decode_fixture("GetWebSocketsToken.json").unwrap();
        assert_eq!(minted.expires, Some(900));
        assert!(!format!("{minted:?}").contains(minted.token.expose_secret()));
    }
}

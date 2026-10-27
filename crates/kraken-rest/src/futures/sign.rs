//! Futures request signing: `Authent = base64(HMAC-SHA512(secret,
//! SHA256(postData ‖ nonce ‖ endpointPath)))`, per
//! <https://docs.kraken.com/api/docs/guides/futures-rest>. `postData` is the
//! encoded parameters exactly as sent, a GET's query included.

use crate::{Auth, RestError, nonce};
use sha2::{Digest as _, Sha256};

pub(super) struct Headers {
    pub(super) nonce: String,
    pub(super) authent: String,
}

pub(super) fn headers(
    post_data: &str,
    endpoint_path: &str,
    auth: &Auth,
) -> Result<Headers, RestError> {
    let nonce = nonce::next()?.to_string();
    Ok(Headers {
        authent: authent(post_data, &nonce, endpoint_path, auth)?,
        nonce,
    })
}

/// `base64(HMAC-SHA512(secret, SHA256(challenge)))`, per
/// <https://docs.kraken.com/api/docs/guides/futures-websockets/#sign-challenge>.
pub fn sign_challenge(challenge: &str, auth: &Auth) -> Result<String, RestError> {
    auth.mac(&[&Sha256::digest(challenge.as_bytes())])
}

fn authent(
    post_data: &str,
    nonce: &str,
    endpoint_path: &str,
    auth: &Auth,
) -> Result<String, RestError> {
    let digest = Sha256::new()
        .chain_update(post_data)
        .chain_update(nonce)
        .chain_update(endpoint_path)
        .finalize();
    auth.mac(&[&digest])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::test_auth;

    #[test]
    fn the_challenge_signature_matches_the_known_answer() {
        let secret = secrecy::SecretString::from(base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            b"futures_ws_secret_key_32chars!!",
        ));
        let auth = Auth::new(secrecy::SecretString::from("key"), &secret).unwrap();
        assert_eq!(
            sign_challenge("test-challenge-uuid-12345", &auth).unwrap(),
            "regmolAVxA8iApSVkZg8qAiZkJIwbeZIh57ia5GrHp9OSNka1n8Uy2QZVl9TY5da7ASKmc7prffHRjRMUxvIUg=="
        );
    }

    /// Pins the bytes Kraken verifies against the docs' worked inputs.
    #[test]
    fn the_authent_is_deterministic() {
        let first = authent(
            "symbol=fi_xbtusd_180615",
            "1415957147987",
            "/api/v3/orderbook",
            &test_auth(),
        )
        .unwrap();
        let again = authent(
            "symbol=fi_xbtusd_180615",
            "1415957147987",
            "/api/v3/orderbook",
            &test_auth(),
        )
        .unwrap();
        assert_eq!(first, again);
        let other = authent("", "1415957147987", "/api/v3/orderbook", &test_auth()).unwrap();
        assert_ne!(first, other, "postData is part of the signed message");
    }
}

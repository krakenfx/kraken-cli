use crate::RestError;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use hmac::{Hmac, KeyInit as _, Mac as _};
use secrecy::{ExposeSecret as _, SecretSlice, SecretString};
use sha2::Sha512;

/// An API key pair able to sign requests. The secret is decoded once, here, so
/// a malformed secret fails before any request leaves.
pub struct Auth {
    key: SecretString,
    secret: SecretSlice<u8>,
}

impl Auth {
    pub fn new(key: SecretString, secret: &SecretString) -> Result<Self, RestError> {
        let secret = BASE64
            .decode(secret.expose_secret())
            .map_err(|e| RestError::Signing(format!("Failed to decode API secret: {e}")))?;
        Ok(Self {
            key,
            secret: secret.into(),
        })
    }

    pub fn key(&self) -> &str {
        self.key.expose_secret()
    }

    /// `base64(HMAC-SHA512(secret, parts…))`, the MAC every Kraken signature is
    /// built on, the Futures WebSocket challenge included.
    pub fn mac(&self, parts: &[&[u8]]) -> Result<String, RestError> {
        let mut mac = Hmac::<Sha512>::new_from_slice(self.secret.expose_secret())
            .map_err(|e| RestError::Signing(format!("HMAC key error: {e}")))?;
        for part in parts {
            mac.update(part);
        }
        Ok(BASE64.encode(mac.finalize().into_bytes()))
    }
}

/// Never prints the key or the secret.
impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Auth").finish_non_exhaustive()
    }
}

#[cfg(test)]
pub(crate) fn test_auth() -> Auth {
    let secret = SecretString::from(BASE64.encode(b"test_secret_key_bytes_32_chars!!"));
    Auth::new(SecretString::from("test-api-key"), &secret).unwrap()
}

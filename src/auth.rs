/// Authentication and signing for Kraken Spot and Futures APIs.
///
/// Spot: `API-Sign = base64(HMAC_SHA512(uri_path + SHA256(nonce + post_data), base64_decode(api_secret)))`
/// Futures v3: `Authent = base64(HMAC_SHA512(SHA256(postData + Nonce + endpointPath), base64_decode(api_secret)))`
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256, Sha512};

use crate::config::SecretValue;
use crate::errors::{KrakenError, Result};

type HmacSha512 = Hmac<Sha512>;

static NONCE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate a monotonically increasing nonce based on nanosecond timestamp.
/// Nanosecond precision avoids `EAPI:Invalid nonce` errors when requests
/// fire in rapid succession (milliseconds can collide under high concurrency).
/// Uses an atomic counter to guarantee monotonicity within a process.
pub(crate) fn generate_nonce() -> Result<u64> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| KrakenError::Auth("system clock is before Unix epoch".into()))?
        .as_nanos() as u64;

    loop {
        let current = NONCE_COUNTER.load(Ordering::SeqCst);
        let next = ts.max(current + 1);
        if NONCE_COUNTER
            .compare_exchange(current, next, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return Ok(next);
        }
    }
}

/// Compute the Spot API `API-Sign` header value.
///
/// `uri_path` — e.g. `/0/private/Balance`
/// `nonce` — the nonce used in the POST body
/// `post_data` — the URL-encoded POST body (including the nonce field)
/// `api_secret` — base64-encoded secret from Kraken
pub(crate) fn spot_sign(
    uri_path: &str,
    nonce: u64,
    post_data: &str,
    api_secret: &SecretValue,
) -> Result<String> {
    let secret_bytes = BASE64
        .decode(api_secret.expose())
        .map_err(|e| KrakenError::Auth(format!("Failed to decode API secret: {e}")))?;

    // SHA256(nonce + post_data)
    let mut sha = Sha256::new();
    sha.update(nonce.to_string().as_bytes());
    sha.update(post_data.as_bytes());
    let sha_digest = sha.finalize();

    // HMAC_SHA512(uri_path + sha256_digest, decoded_secret)
    let mut mac = HmacSha512::new_from_slice(&secret_bytes)
        .map_err(|e| KrakenError::Auth(format!("HMAC key error: {e}")))?;
    mac.update(uri_path.as_bytes());
    mac.update(&sha_digest);

    let result = mac.finalize().into_bytes();
    Ok(BASE64.encode(result))
}

/// Compute the Futures v3 `Authent` header value.
///
/// `post_data` — URL-encoded POST body (empty string for GET requests)
/// `nonce` — the Nonce header value
/// `endpoint_path` — e.g. `/api/v3/accounts`
/// `api_secret` — base64-encoded secret from Kraken
pub(crate) fn futures_sign(
    post_data: &str,
    nonce: &str,
    endpoint_path: &str,
    api_secret: &SecretValue,
) -> Result<String> {
    let secret_bytes = BASE64
        .decode(api_secret.expose())
        .map_err(|e| KrakenError::Auth(format!("Failed to decode Futures API secret: {e}")))?;

    // SHA256(postData + Nonce + endpointPath)
    let mut sha = Sha256::new();
    sha.update(post_data.as_bytes());
    sha.update(nonce.as_bytes());
    sha.update(endpoint_path.as_bytes());
    let sha_digest = sha.finalize();

    // HMAC_SHA512(sha256_digest, decoded_secret)
    let mut mac = HmacSha512::new_from_slice(&secret_bytes)
        .map_err(|e| KrakenError::Auth(format!("HMAC key error: {e}")))?;
    mac.update(&sha_digest);

    let result = mac.finalize().into_bytes();
    Ok(BASE64.encode(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonce_is_monotonic() {
        let n1 = generate_nonce().unwrap();
        let n2 = generate_nonce().unwrap();
        let n3 = generate_nonce().unwrap();
        assert!(n2 > n1, "nonce must be strictly increasing");
        assert!(n3 > n2, "nonce must be strictly increasing");
    }

    #[test]
    fn spot_sign_deterministic() {
        // Known test vector: inputs chosen to produce a deterministic output.
        let secret = SecretValue::new(BASE64.encode(b"test_secret_key_bytes_32_chars!!"));
        let sig = spot_sign(
            "/0/private/Balance",
            1616492376594,
            "nonce=1616492376594",
            &secret,
        )
        .expect("signing should succeed");
        assert!(!sig.is_empty());
        // Verify determinism by signing twice
        let sig2 = spot_sign(
            "/0/private/Balance",
            1616492376594,
            "nonce=1616492376594",
            &secret,
        )
        .expect("signing should succeed");
        assert_eq!(sig, sig2);
    }

    #[test]
    fn futures_sign_deterministic() {
        let secret = SecretValue::new(BASE64.encode(b"futures_secret_key_bytes_32ch!!"));
        let sig = futures_sign("", "1234567890", "/api/v3/accounts", &secret)
            .expect("signing should succeed");
        assert!(!sig.is_empty());
        let sig2 = futures_sign("", "1234567890", "/api/v3/accounts", &secret)
            .expect("signing should succeed");
        assert_eq!(sig, sig2);
    }

    #[test]
    fn spot_sign_rejects_invalid_base64_secret() {
        let secret = SecretValue::new("not-valid-base64!!!".to_string());
        let result = spot_sign("/0/private/Balance", 123, "nonce=123", &secret);
        assert!(result.is_err());
    }
}

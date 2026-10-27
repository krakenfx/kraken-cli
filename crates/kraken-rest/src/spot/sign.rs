//! Spot request signing: `API-Sign = base64(HMAC-SHA512(secret, path ‖
//! SHA256(nonce ‖ body)))`, per <https://docs.kraken.com/api/docs/guides/spot-rest-auth>.

use crate::{Auth, RestError, nonce};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

/// The request body every private endpoint takes: its own parameters plus the
/// nonce and, for accounts with a static second factor, the OTP. Both sit
/// inside the signed body.
#[derive(Serialize)]
struct SignedBody<'a, T> {
    nonce: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    otp: Option<&'a str>,
    #[serde(flatten)]
    request: &'a T,
}

pub(super) struct Signed {
    pub(super) nonce: u64,
    pub(super) body: String,
    pub(super) signature: String,
}

pub(super) fn sign<T: Serialize>(
    path: &str,
    request: &T,
    auth: &Auth,
    otp: Option<&str>,
) -> Result<Signed, RestError> {
    let nonce = nonce::next()?;
    let body = serde_json::to_string(&SignedBody {
        nonce,
        otp,
        request,
    })
    .map_err(|e| RestError::Encode(format!("Failed to serialize request: {e}")))?;
    Ok(Signed {
        nonce,
        signature: signature(path, nonce, &body, auth)?,
        body,
    })
}

fn signature(path: &str, nonce: u64, body: &str, auth: &Auth) -> Result<String, RestError> {
    let digest = Sha256::new()
        .chain_update(nonce.to_string())
        .chain_update(body)
        .finalize();
    auth.mac(&[path.as_bytes(), &digest])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::test_auth;

    /// Pins the exact bytes, so a hashing-crate bump can never silently change
    /// what Kraken verifies.
    #[test]
    fn the_signature_matches_the_known_answer() {
        let sig = signature(
            "/0/private/Balance",
            1616492376594,
            "nonce=1616492376594",
            &test_auth(),
        )
        .unwrap();
        assert_eq!(
            sig,
            "4X7nFc5U3rLzrjRGXsNIB/JXCS90F60ILkC/Xg9iGTvULGGPsq9GZd9UbcOK+FFNznOehsW5l/\
             LIv4Rm5+caIg=="
        );
    }

    #[test]
    fn the_nonce_and_parameters_share_one_body() {
        #[derive(Serialize)]
        struct Params {
            asset: &'static str,
        }
        let signed = sign(
            "/0/private/TradeBalance",
            &Params { asset: "ZUSD" },
            &test_auth(),
            None,
        )
        .unwrap();
        let body: serde_json::Value = serde_json::from_str(&signed.body).unwrap();
        assert_eq!(body["nonce"], signed.nonce);
        assert_eq!(body["asset"], "ZUSD");
        assert!(body.get("otp").is_none());
    }

    #[test]
    fn the_second_factor_is_signed_inside_the_body() {
        let signed = sign("/0/private/Balance", &(), &test_auth(), Some("123456")).unwrap();
        let body: serde_json::Value = serde_json::from_str(&signed.body).unwrap();
        assert_eq!(body["otp"], "123456");
    }
}

use crate::error::{Error, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};

/// 32 bytes encode to 43 base64url characters, the RFC 7636 §4.1 minimum.
const VERIFIER_BYTES: usize = 32;
/// 128 bits, so a forged callback cannot guess the state.
const STATE_BYTES: usize = 16;

/// The PKCE secret of one login. Only its hash leaves the process before the
/// code exchange.
#[derive(Debug)]
pub(crate) struct Verifier(SecretString);

impl Verifier {
    pub(crate) fn generate() -> Result<Self> {
        Ok(Self(random_token::<VERIFIER_BYTES>()?.into()))
    }

    pub(crate) fn challenge(&self) -> Challenge {
        Challenge(s256(self.expose()))
    }

    pub(crate) fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

#[derive(Debug)]
pub(crate) struct Challenge(String);

impl Challenge {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Connects the browser redirect to the login that this process started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct State(String);

impl State {
    pub(crate) fn generate() -> Result<Self> {
        Ok(Self(random_token::<STATE_BYTES>()?))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg(test)]
    pub(crate) fn fixed(value: &str) -> Self {
        Self(value.to_string())
    }
}

fn s256(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn random_token<const N: usize>() -> Result<String> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes).map_err(Error::Random)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn challenge_matches_the_rfc_7636_appendix_b_vector() {
        assert_eq!(
            s256("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn two_logins_never_share_a_verifier_or_a_state() {
        assert_ne!(
            Verifier::generate().unwrap().expose(),
            Verifier::generate().unwrap().expose()
        );
        assert_ne!(State::generate().unwrap(), State::generate().unwrap());
    }

    #[test]
    fn debug_never_prints_the_verifier() {
        let verifier = Verifier::generate().unwrap();
        assert!(!format!("{verifier:?}").contains(verifier.expose()));
    }

    proptest! {
        #[test]
        fn every_verifier_is_a_valid_rfc_7636_verifier(_ in 0..64_u8) {
            let verifier = Verifier::generate().unwrap();
            let v = verifier.expose();
            prop_assert!((43..=128).contains(&v.len()));
            prop_assert!(v.chars().all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c)));
        }
    }
}

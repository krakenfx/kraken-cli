//! The one rule a stored API secret must pass, shared by `kraken setup` and
//! `kraken auth set` so neither saves a secret the other would refuse.

use secrecy::SecretString;

/// Whether the request signer can decode `secret`. Checked when the secret is
/// entered, not on the first signed request; the decoder's own error is left
/// out because it describes the secret.
pub(crate) fn is_decodable_secret(secret: &str) -> bool {
    kraken_rest::Auth::new(
        SecretString::from(String::new()),
        &SecretString::from(secret.to_owned()),
    )
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_that_is_not_base64_is_refused() {
        assert!(!is_decodable_secret("not base64 !!"));
    }

    #[test]
    fn a_secret_in_the_format_kraken_issues_is_accepted() {
        // 64 bytes, standard base64: the shape of a Kraken private key.
        let secret = "BCEDEGNKX3v+RWNfmNcZTO/\
                      TG0aCpqSk5WyF0yZVLlZzwKBWw9O0MUJxw+Q1BTH+ymsEoqFaGu7gLJDOn+H4qQ==";
        assert!(is_decodable_secret(secret));
    }
}

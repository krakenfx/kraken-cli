use super::{
    config::ClientId,
    pkce::{Challenge, State},
};
use url::Url;

pub(crate) fn authorize_url(
    base: &Url,
    client_id: &ClientId,
    redirect_uri: &str,
    scopes: &[&str],
    state: &State,
    challenge: &Challenge,
) -> Url {
    let mut url = base.clone();
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id.as_str())
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", &scopes.join(" "))
        .append_pair("state", state.as_str())
        .append_pair("code_challenge", challenge.as_str())
        .append_pair("code_challenge_method", "S256");
    url
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::pkce::Verifier;
    use std::collections::HashMap;

    #[test]
    fn carries_every_parameter_a_public_pkce_client_needs() {
        let verifier = Verifier::generate().unwrap();
        let url = authorize_url(
            &Url::parse("https://www.kraken.com/oauth/authorize").unwrap(),
            &"abc123".parse().unwrap(),
            "http://localhost:27461/callback",
            &[
                "account.fast-api-key:write",
                "account.fast-api-key:funds-query",
            ],
            &State::fixed("xyz"),
            &verifier.challenge(),
        );
        let query: HashMap<_, _> = url.query_pairs().into_owned().collect();

        assert_eq!(query["response_type"], "code");
        assert_eq!(query["client_id"], "abc123");
        assert_eq!(query["redirect_uri"], "http://localhost:27461/callback");
        assert_eq!(
            query["scope"],
            "account.fast-api-key:write account.fast-api-key:funds-query"
        );
        assert_eq!(query["state"], "xyz");
        assert_eq!(query["code_challenge"], verifier.challenge().as_str());
        assert_eq!(query["code_challenge_method"], "S256");
        assert!(
            !url.as_str().contains(verifier.expose()),
            "the verifier must stay local"
        );
    }
}

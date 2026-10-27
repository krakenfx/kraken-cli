use crate::error::{Error, Result};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::collections::BTreeSet;

/// Valid for 4 hours, with no refresh token. The CLI uses it to create one key.
#[derive(Debug)]
pub(crate) struct AccessToken(SecretString);

impl AccessToken {
    pub(crate) fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

/// RFC 6749 §5.1.
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    /// Space-separated. It can be narrower than the request (RFC 6749 §3.3).
    /// Absent means the server granted the requested scope.
    scope: Option<String>,
}

/// RFC 6749 §5.2.
#[derive(Deserialize)]
struct TokenError {
    error: String,
}

/// A public client sends `client_id` in the body and no `Authorization` header,
/// because it has no secret.
pub(crate) fn code_exchange_form<'a>(
    code: &'a str,
    redirect_uri: &'a str,
    client_id: &'a str,
    verifier: &'a str,
) -> [(&'static str, &'a str); 5] {
    [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", verifier),
    ]
}

/// Checks that Kraken granted exactly the `required` scopes. Then a narrow
/// grant fails here, not on the first trade, and a wider one never runs.
pub(crate) fn parse_token_response(
    status: u16,
    body: &[u8],
    required: &[&str],
) -> Result<AccessToken> {
    if !(200..300).contains(&status) {
        let code = serde_json::from_slice::<TokenError>(body)
            .map_or_else(|_| "unexpected response".to_string(), |e| e.error);
        return Err(Error::Rejected { status, code });
    }
    let response: TokenResponse = serde_json::from_slice(body)?;
    let Some(scope) = &response.scope else {
        return Ok(AccessToken(response.access_token.into()));
    };
    let granted: BTreeSet<&str> = scope.split_whitespace().collect();
    let missing: Vec<String> = required
        .iter()
        .filter(|scope| !granted.contains(*scope))
        .map(ToString::to_string)
        .collect();
    if !missing.is_empty() {
        return Err(Error::ScopeNotGranted { missing });
    }
    let extra: Vec<String> = granted
        .into_iter()
        .filter(|scope| !required.contains(scope))
        .map(ToString::to_string)
        .collect();
    if !extra.is_empty() {
        return Err(Error::ScopeNotRequested { extra });
    }
    Ok(AccessToken(response.access_token.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUIRED: [&str; 2] = [
        "account.fast-api-key:write",
        "account.fast-api-key:funds-query",
    ];

    #[test]
    fn a_full_grant_yields_the_token() {
        let body = br#"{"access_token":"tok","token_type":"bearer","expires_in":14400,
            "scope":"account.fast-api-key:funds-query account.fast-api-key:write"}"#;
        let token = parse_token_response(200, body, &REQUIRED).unwrap();
        assert_eq!(token.expose(), "tok");
    }

    #[test]
    fn an_omitted_scope_means_the_requested_scope() {
        let body = br#"{"access_token":"tok","token_type":"bearer","expires_in":14400}"#;
        assert!(parse_token_response(200, body, &REQUIRED).is_ok());
    }

    #[test]
    fn an_empty_scope_grants_nothing() {
        let body = br#"{"access_token":"tok","token_type":"bearer","scope":""}"#;
        let error = parse_token_response(200, body, &REQUIRED).unwrap_err();
        assert!(matches!(error, Error::ScopeNotGranted { missing } if missing.len() == 2));
    }

    #[test]
    fn a_narrowed_grant_names_what_is_missing() {
        let body = br#"{"access_token":"tok","token_type":"bearer","expires_in":14400,
            "scope":"account.fast-api-key:write"}"#;
        let error = parse_token_response(200, body, &REQUIRED).unwrap_err();
        assert!(matches!(
            error,
            Error::ScopeNotGranted { missing } if missing == ["account.fast-api-key:funds-query"]
        ));
    }

    #[test]
    fn a_grant_wider_than_the_request_is_refused() {
        let body = br#"{"access_token":"tok","scope":"account.fast-api-key:write account.fast-api-key:funds-query account.fast-api-key:funds-withdraw"}"#;
        let error = parse_token_response(200, body, &REQUIRED).unwrap_err();
        assert!(matches!(
            error,
            Error::ScopeNotRequested { extra } if extra == ["account.fast-api-key:funds-withdraw"]
        ));
    }

    #[test]
    fn an_oauth_error_carries_its_code() {
        let error =
            parse_token_response(400, br#"{"error":"invalid_grant"}"#, &REQUIRED).unwrap_err();
        assert!(matches!(error, Error::Rejected { status: 400, code } if code == "invalid_grant"));
    }

    #[test]
    fn debug_never_prints_the_token() {
        let body = br#"{"access_token":"tok-value","scope":"account.fast-api-key:write account.fast-api-key:funds-query"}"#;
        let token = parse_token_response(200, body, &REQUIRED).unwrap();
        assert!(!format!("{token:?}").contains("tok-value"));
    }
}

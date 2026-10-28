//! Which URLs an override may point at: secure schemes only, and Kraken's own
//! hosts unless the user opts out.

use crate::{
    config::DANGER_ALLOW_ANY_URL_HOST_ENV,
    errors::{KrakenError, Result},
};

fn redact_url_authority(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut parsed) => {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.to_string()
        }
        Err(_) => "<invalid URL>".to_string(),
    }
}

fn is_trusted_override_host(host: &str) -> bool {
    let host_lc = host.to_ascii_lowercase();
    host_lc == "api.kraken.com"
        || host_lc == "futures.kraken.com"
        || host_lc == "ws.kraken.com"
        || host_lc == "ws-auth.kraken.com"
        || host_lc == "ws-l3.kraken.com"
        || host_lc == "cli-feedback.kraken.com"
}

/// Validate that a URL uses a secure scheme (`https` or `wss`).
///
/// Rejects `http`, `ws`, and all other schemes. Error messages redact
/// any embedded credentials from the URL authority.
pub(crate) fn validate_url_scheme(url: &str) -> Result<()> {
    let parsed = url::Url::parse(url).map_err(|_| {
        KrakenError::Validation(format!("Invalid URL: {}", redact_url_authority(url)))
    })?;

    let scheme = parsed.scheme();
    if scheme == "https" || scheme == "wss" {
        if parsed.host().is_none() {
            return Err(KrakenError::Validation(
                "URL must include a host".to_string(),
            ));
        }
        Ok(())
    } else if scheme == "http" || scheme == "ws" {
        Err(KrakenError::Validation(format!(
            "Insecure URL scheme '{}' rejected for {}. Only https:// and wss:// are allowed.",
            scheme,
            redact_url_authority(url)
        )))
    } else {
        Err(KrakenError::Validation(format!(
            "Unsupported URL scheme '{}'. Only https:// and wss:// are allowed.",
            scheme
        )))
    }
}

/// Validate a resolved URL override (secure scheme + trusted host).
///
/// clap applies the flag > env precedence upstream (via `#[arg(env = ...)]`),
/// so this receives the already-merged value. `allow_any_host` (resolved once
/// in `AppContext::from_cli`) relaxes the trusted-host check. When a value is
/// present it is validated for secure scheme and trusted host before returning;
/// when absent, `None` is returned so the downstream constructor uses its
/// built-in default.
pub(crate) fn resolve_url_override(
    value: Option<&str>,
    allow_any_host: bool,
) -> Result<Option<String>> {
    resolve_override(value, allow_any_host, is_trusted_override_host)
}

/// The sign-in page is not an API endpoint, so it has its own allowlist: no
/// signed request may go to it without the danger flag.
pub(crate) fn resolve_authorize_url_override(
    value: Option<&str>,
    allow_any_host: bool,
) -> Result<Option<String>> {
    resolve_override(value, allow_any_host, |host| {
        host.eq_ignore_ascii_case(kraken_auth::PRODUCTION_SIGN_IN_HOST)
    })
}

fn resolve_override(
    value: Option<&str>,
    allow_any_host: bool,
    is_trusted: impl Fn(&str) -> bool,
) -> Result<Option<String>> {
    match value {
        Some(url) => {
            validate_url_scheme(url)?;
            let parsed = url::Url::parse(url)?;
            let host = parsed.host_str().ok_or_else(|| {
                KrakenError::Validation("URL override must include a host".to_string())
            })?;
            if !is_trusted(host) {
                if allow_any_host {
                    tracing::warn!(
                        %host,
                        "using non-Kraken URL host because {DANGER_ALLOW_ANY_URL_HOST_ENV} is enabled"
                    );
                } else {
                    return Err(KrakenError::Validation(format!(
                        "Untrusted URL host '{host}'. Allowed hosts are production Kraken \
                         endpoints. To override this check, set {DANGER_ALLOW_ANY_URL_HOST_ENV}=1."
                    )));
                }
            }
            Ok(Some(url.to_string()))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_url_scheme_rejects_ws() {
        let err = validate_url_scheme("ws://ws.kraken.com/v2").unwrap_err();
        assert!(err.to_string().contains("Insecure"));
    }

    #[test]
    fn validate_url_scheme_rejects_ftp() {
        let err = validate_url_scheme("ftp://api.kraken.com").unwrap_err();
        assert!(err.to_string().contains("Unsupported"));
    }

    #[test]
    fn validate_url_scheme_rejects_empty() {
        assert!(validate_url_scheme("").is_err());
    }

    #[test]
    fn validate_url_scheme_rejects_nonsense() {
        assert!(validate_url_scheme("not-a-url").is_err());
    }

    #[test]
    fn validate_url_scheme_rejects_scheme_only() {
        let result = validate_url_scheme("https://");
        // url::Url::parse("https://") may parse but with empty host
        // Either way, it should not pass validation
        if result.is_ok() {
            panic!("Expected scheme-only URL to be rejected");
        }
    }

    #[test]
    fn validate_url_scheme_error_redacts_credentials() {
        let err = validate_url_scheme("http://user:pass@api.kraken.com").unwrap_err();
        let msg = err.to_string();
        assert!(
            !msg.contains("user:pass"),
            "Credentials leaked in error: {msg}"
        );
        assert!(msg.contains("Insecure"));
    }

    #[test]
    fn resolve_url_override_accepts_trusted_spot_host() {
        let result = resolve_url_override(Some("https://api.kraken.com"), false).unwrap();
        assert_eq!(result, Some("https://api.kraken.com".to_string()));
    }

    #[test]
    fn the_sign_in_host_is_not_trusted_for_signed_requests() {
        assert!(resolve_url_override(Some("https://www.kraken.com"), false).is_err());
        assert!(
            resolve_authorize_url_override(Some("https://www.kraken.com/oauth/authorize"), false)
                .is_ok()
        );
        assert!(
            resolve_authorize_url_override(Some("https://api.kraken.com/oauth/authorize"), false)
                .is_err()
        );
    }

    #[test]
    fn resolve_url_override_accepts_trusted_futures_host() {
        let result = resolve_url_override(Some("https://futures.kraken.com"), false).unwrap();
        assert_eq!(result, Some("https://futures.kraken.com".to_string()));
    }

    #[test]
    fn resolve_url_override_none_when_absent() {
        let result = resolve_url_override(None, false).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_url_override_rejects_insecure_scheme() {
        let err = resolve_url_override(Some("http://api.kraken.com"), false).unwrap_err();
        assert!(err.to_string().contains("Insecure"));
    }

    #[test]
    fn resolve_url_override_rejects_untrusted_host_by_default() {
        let err = resolve_url_override(Some("https://evil.example.com"), false).unwrap_err();
        assert!(err.to_string().contains("Untrusted URL host"));
    }

    #[test]
    fn resolve_url_override_allows_untrusted_host_when_opted_in() {
        let result = resolve_url_override(Some("https://evil.example.com"), true).unwrap();
        assert_eq!(result, Some("https://evil.example.com".to_string()));
    }

    #[test]
    fn resolve_url_override_accepts_feedback_host() {
        let result =
            resolve_url_override(Some("https://cli-feedback.kraken.com/v1/feedback"), false)
                .unwrap();
        assert_eq!(
            result.as_deref(),
            Some("https://cli-feedback.kraken.com/v1/feedback")
        );
    }

    #[test]
    fn resolve_url_override_accepts_prod_ws_auth_host() {
        let result = resolve_url_override(Some("wss://ws-auth.kraken.com/v2"), false).unwrap();
        assert_eq!(result, Some("wss://ws-auth.kraken.com/v2".to_string()));
    }
}

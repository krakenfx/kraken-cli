/// Built-in MCP (Model Context Protocol) server for kraken-cli.
///
/// Exposes CLI commands as MCP tools over stdio, enabling direct integration
/// with MCP clients without subprocess wrappers.
pub(crate) mod registry;
pub(crate) mod schema;
pub(crate) mod server;

use crate::errors::{KrakenError, Result};

const VALID_SERVICES: &[&str] = &[
    "market",
    "account",
    "trade",
    "funding",
    "earn",
    "subaccount",
    "futures",
    "websocket",
    "paper",
    "auth",
];

// futures-ws tools are excluded because the group is absent from VALID_SERVICES,
// so they never pass the service filter in registry build. Only websocket needs
// explicit post-filter exclusion here.
const STREAMING_GROUPS: &[&str] = &["websocket"];

pub(crate) fn parse_services(input: &str) -> Result<Vec<String>> {
    let trimmed = input.trim();
    if trimmed.eq_ignore_ascii_case("all") {
        return Ok(VALID_SERVICES.iter().map(|s| s.to_string()).collect());
    }

    let tokens: Vec<String> = trimmed
        .split(',')
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();

    if tokens.is_empty() {
        return Err(KrakenError::Validation(
            "At least one service must be specified".into(),
        ));
    }

    for token in &tokens {
        if token == "utility" {
            return Err(KrakenError::Validation(
                "Service 'utility' is not available in MCP mode (interactive only)".into(),
            ));
        }
        if !VALID_SERVICES.contains(&token.as_str()) {
            return Err(KrakenError::Validation(format!(
                "Unknown service: '{token}'. Valid services: {}",
                VALID_SERVICES.join(", ")
            )));
        }
    }

    Ok(tokens)
}

pub(crate) fn apply_exclusions(services: &[String]) -> Vec<String> {
    services
        .iter()
        .filter(|s| !STREAMING_GROUPS.contains(&s.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_all_returns_all_valid_services() {
        let result = parse_services("all").unwrap();
        assert_eq!(result.len(), VALID_SERVICES.len());
        for svc in VALID_SERVICES {
            assert!(result.contains(&svc.to_string()));
        }
    }

    #[test]
    fn parse_explicit_list() {
        let result = parse_services("market,trade").unwrap();
        assert_eq!(result, vec!["market", "trade"]);
    }

    #[test]
    fn parse_normalizes_case_and_whitespace() {
        let result = parse_services(" Market , TRADE ").unwrap();
        assert_eq!(result, vec!["market", "trade"]);
    }

    #[test]
    fn parse_rejects_unknown_token() {
        let err = parse_services("market,bogus").unwrap_err().to_string();
        assert!(err.contains("Unknown service: 'bogus'"));
    }

    #[test]
    fn parse_rejects_utility() {
        let err = parse_services("utility").unwrap_err().to_string();
        assert!(err.contains("not available in MCP mode"));
    }

    #[test]
    fn parse_rejects_empty_input() {
        let err = parse_services("").unwrap_err().to_string();
        assert!(err.contains("At least one service"));
    }

    #[test]
    fn exclusions_remove_streaming_groups() {
        let input: Vec<String> = vec!["market".into(), "websocket".into(), "trade".into()];
        let result = apply_exclusions(&input);
        assert_eq!(result, vec!["market", "trade"]);
    }

    #[test]
    fn exclusions_preserve_rest_groups() {
        let input: Vec<String> = vec!["market".into(), "account".into()];
        let result = apply_exclusions(&input);
        assert_eq!(result, vec!["market", "account"]);
    }
}

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
    "workspace",
    "auth",
    "feedback",
    // 17 REST commands, no credential required, exposed as MCP tools and
    // scoped per workspace like spot paper.
    "futures-paper",
];

/// Service groups MCP v1 cannot serve, with the reason to report. One table
/// keeps `parse_services`, the filter, and the caller's error message from
/// drifting apart. `futures-ws` appears here only so a `-s websocket` error can
/// mention it: the group is absent from `VALID_SERVICES`, so it can never be
/// requested and never needs filtering.
const STREAMING_GROUPS: &[&str] = &["websocket", "futures-ws"];

/// Why a streaming group yields no tools. A live stream cannot be hosted inside
/// a request/response tool, so one reason covers every entry above.
pub(crate) const STREAMING_REASON: &str = "streaming groups are excluded in MCP v1";

pub(crate) fn exclusion_reason(group: &str) -> Option<&'static str> {
    STREAMING_GROUPS.contains(&group).then_some(STREAMING_REASON)
}

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
        .filter(|s| exclusion_reason(s).is_none())
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

    #[test]
    fn parse_accepts_a_group_with_no_auth_requirement() {
        // `futures-paper` needs no credentials, so a mixed `-s` list must parse
        // rather than be rejected as a typo.
        assert_eq!(
            parse_services("market,futures-paper").unwrap(),
            vec!["market", "futures-paper"]
        );
    }

    #[test]
    fn futures_paper_is_served_not_excluded() {
        // 17 REST commands with no credential requirement, exposed as MCP tools
        // and scoped per workspace like spot paper.
        let input: Vec<String> = vec!["futures-paper".into()];
        assert_eq!(apply_exclusions(&input), vec!["futures-paper"]);
        assert_eq!(exclusion_reason("futures-paper"), None);
    }

    #[test]
    fn exclusion_reason_is_the_streaming_reason_for_streaming_groups() {
        assert_eq!(exclusion_reason("websocket"), Some(STREAMING_REASON));
        // Named for the error message only; absent from VALID_SERVICES.
        assert_eq!(exclusion_reason("futures-ws"), Some(STREAMING_REASON));
    }

    #[test]
    fn exclusion_reason_is_none_for_served_groups() {
        for group in [
            "market",
            "account",
            "paper",
            "workspace",
            "futures",
            "futures-paper",
        ] {
            assert_eq!(exclusion_reason(group), None, "{group} should be served");
        }
    }
}

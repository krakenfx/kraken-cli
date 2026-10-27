//! Which service groups a server process exposes.
//!
//! The groups themselves are the contract's ([`Service`]), so a new group
//! reaches the CLI, the catalog and this filter from one declaration. What is
//! decided here is only which of them a request/response server can serve.

use crate::error::{McpError, Result};
use kraken_contract::tool::{Service, Surface};
use std::{collections::BTreeSet, sync::LazyLock};

/// The groups holding at least one callable command.
///
/// Derived, not listed: "servable" is not a judgment this crate gets to make. A
/// group whose every command streams or is interactive has no tool to offer, so
/// a new one of either kind is excluded without touching this file.
fn servable() -> &'static BTreeSet<Service> {
    static SERVABLE: LazyLock<BTreeSet<Service>> = LazyLock::new(|| {
        Surface::shared()
            .iter()
            .filter(|entry| entry.reach.is_tool())
            .map(|entry| entry.group)
            .collect()
    });
    &SERVABLE
}

/// The groups an MCP server can expose. The single list the instructions
/// briefing advertises, so a new group cannot silently vanish from it.
pub(crate) fn exposable() -> impl Iterator<Item = Service> {
    Service::all().filter(|service| servable().contains(service))
}

/// The groups excluded from every server, named in the refusal so a caller who
/// asked for only these learns which they were.
fn unservable() -> impl Iterator<Item = Service> {
    Service::all().filter(|service| !servable().contains(service))
}

/// Parses a `--services` list into the groups this server will expose. `all`
/// means every exposable group, and an unservable group named explicitly is
/// dropped here rather than by a second call a caller could forget.
pub fn parse_services(input: &str) -> Result<Vec<Service>> {
    let trimmed = input.trim();
    if trimmed.eq_ignore_ascii_case("all") {
        return Ok(exposable().collect());
    }

    let mut services = Vec::new();
    for token in trimmed.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        let service = <Service as clap::ValueEnum>::from_str(token, true).map_err(|_| {
            McpError::Validation(format!(
                "Unknown service: '{token}'. Valid services: {}",
                exposable()
                    .map(|service| service.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        if servable().contains(&service) {
            services.push(service);
        }
    }

    if services.is_empty() {
        return Err(McpError::Validation(format!(
            "No REST-eligible service remains. Streaming and interactive groups ({}) cannot be \
             served over MCP.",
            unservable()
                .map(|service| service.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    Ok(services)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_all_returns_every_exposable_group() {
        let parsed = parse_services("all").unwrap();
        assert_eq!(parsed.len(), exposable().count());
        assert!(!parsed.contains(&Service::Websocket));
    }

    #[test]
    fn parse_normalizes_case_and_whitespace() {
        assert_eq!(
            parse_services(" Market , TRADE ").unwrap(),
            vec![Service::Market, Service::Trade]
        );
    }

    #[test]
    fn parse_rejects_unknown_token() {
        let err = parse_services("market,bogus").unwrap_err().to_string();
        assert!(err.contains("Unknown service: 'bogus'"), "{err}");
    }

    #[test]
    fn parse_rejects_empty_input() {
        let err = parse_services("").unwrap_err().to_string();
        assert!(err.contains("No REST-eligible service"), "{err}");
    }

    /// A streaming group named explicitly is dropped rather than honoured: a
    /// request/response tool call has nowhere to put a stream.
    #[test]
    fn parse_drops_streaming_groups_named_explicitly() {
        assert_eq!(
            parse_services("market,websocket,trade").unwrap(),
            vec![Service::Market, Service::Trade]
        );
    }

    /// A selection of nothing but streaming groups is a refusal, not an empty
    /// server that advertises no tools.
    #[test]
    fn parse_rejects_a_selection_of_only_streaming_groups() {
        assert!(parse_services("websocket,futures-ws").is_err());
    }

    /// The groups that cannot be served are exactly the streaming and
    /// interactive ones. Pinned as an outcome rather than a list: a group that
    /// gained its first callable command should start being exposed, and one
    /// that lost its last should stop, both without an edit here.
    #[test]
    fn the_unservable_groups_are_the_streaming_and_interactive_ones() {
        assert_eq!(
            unservable().collect::<Vec<_>>(),
            vec![Service::Websocket, Service::FuturesWs, Service::Utility]
        );
    }
}

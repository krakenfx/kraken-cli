//! The published command catalog (`agents/tool-catalog.json`), generated.
//!
//! It used to be written by hand and held in step with the code by a dozen
//! tests. Now it is emitted from [`Surface`] — the command tree's own
//! classification of every leaf, with each schema derived from the leaf type.
//! The contract's `every_command_is_classified_exactly_once` is what makes that
//! one surface sufficient: it proves the surface and the clap tree are the same
//! set, so the catalog does not walk clap a second time to find out.
//!
//! Every command is classified, callable or not, so the catalog still says that
//! `ws cancel-all` cancels real orders even though only the CLI can run it.
//! `reach` says whether a client can call it, and `tool` names the tool when
//! one can.

use kraken_contract::tool::{Credentials, Effect, Reach, Service, Surface, ToolName};
use serde::Serialize;
use std::collections::BTreeMap;

/// The catalog's own version. Bumped when an entry's shape changes, which is a
/// breaking change for anything reading it.
const SCHEMA_VERSION: &str = "2.0.0";

#[derive(Serialize)]
pub struct Catalog {
    schema_version: &'static str,
    cli_version: String,
    description: &'static str,
    groups: BTreeMap<String, String>,
    commands: Vec<Published<'static>>,
}

/// One command, as the catalog publishes it.
#[derive(Serialize)]
struct Published<'a> {
    /// The catalog's own identifier: the command path, hyphenated.
    name: String,
    /// What an operator would type.
    command: &'a str,
    /// The same invocation with its positional arguments, in the order the
    /// command line expects them. `input_schema` names and types the arguments;
    /// only this records their order, which an agent driving the CLI as a
    /// subprocess needs.
    usage: &'a str,
    /// The MCP tool that runs it. Absent when no client can call it.
    #[serde(skip_serializing_if = "Option::is_none")]
    tool: Option<&'a ToolName>,
    /// Whether a client can call it, and when not, why not.
    reach: Reach,
    group: Service,
    description: &'a str,
    credentials: Credentials,
    /// Whether a call must be acknowledged. Derived from `effect`, published
    /// because a client routes on it. `effect: local_or_live` is the one shape
    /// it understates: such a command writes local state until an argument
    /// points it at the real account, which needs the same confirmation.
    dangerous: bool,
    /// Whether a paper workspace relaxes that acknowledgment.
    paper_safe: bool,
    effect: Effect,
    /// The arguments the command takes, generated from the leaf's own type.
    /// Published for the CLI-only commands too: an agent driving `kraken
    /// record` or `session start` as a subprocess validates its arguments
    /// against this the same way a tool call does.
    input_schema: &'a schemars::Schema,
    /// One invocation an operator could paste.
    #[serde(skip_serializing_if = "Option::is_none")]
    example: Option<&'static str>,
    /// Cross-field requirements no schema keyword states portably: which
    /// arguments exclude each other, and which cannot be omitted together. The
    /// CLI enforces these through clap, a tool call through the same
    /// declaration.
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    constraints: &'a [String],
    /// Why this command is not a tool, when it is not.
    #[serde(skip_serializing_if = "Option::is_none")]
    why: Option<&'static str>,
}

impl Catalog {
    /// Generates the catalog from the command tree.
    pub fn generate(cli_version: String) -> Self {
        let mut commands: Vec<Published<'static>> = Surface::shared()
            .iter()
            .map(|entry| Published {
                name: entry.path().to_string().replace(' ', "-"),
                command: &entry.command,
                usage: &entry.usage,
                tool: entry.reach.is_tool().then_some(&entry.name),
                reach: entry.reach,
                group: entry.group,
                description: &entry.description,
                credentials: entry.credentials,
                dangerous: entry.effect.is_dangerous(),
                paper_safe: entry.effect.is_paper_safe(),
                effect: entry.effect,
                input_schema: &entry.input_schema,
                example: entry.example,
                constraints: &entry.constraints,
                why: entry.reach.why_not(),
            })
            .collect();
        // Published in the order an operator would scan: by the command they
        // would type, not by the tool name derived from it.
        commands.sort_by(|a, b| a.command.cmp(b.command));

        Self {
            schema_version: SCHEMA_VERSION,
            cli_version,
            description: "Every command kraken-cli can run. `tool` names the MCP tool that calls \
                          it, `usage` gives the positional order, and `input_schema` is generated \
                          from the command's own Rust type, so it cannot drift from the arguments \
                          the command accepts. Its property names are the argument names a tool \
                          call takes. On the command line, the arguments `usage` lists are \
                          positional and every other property is a flag with hyphens for \
                          underscores (`min_fills` is `--min-fills`), except where `--help` \
                          spells it otherwise: `trade_type` and `ledger_type` are `--type`, \
                          `window` is `--for`, and `sessions` is `--session`. `constraints` \
                          states the cross-field requirements no schema keyword carries. Confirm \
                          any command with `dangerous: true`, and any `effect: local_or_live` one \
                          whose live argument is set (`--mode live`, `--real-account`): those \
                          reach the real account too. Route errors on the `error` field, never on \
                          message text (see agents/error-catalog.json).",
            groups: groups(),
            commands,
        }
    }
}

/// The service groups, described by the enum that defines them.
fn groups() -> BTreeMap<String, String> {
    Service::all()
        .map(|service| (service.to_string(), service.description()))
        .collect()
}

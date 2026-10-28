//! The tools this server exposes: the command tree's own surface
//! ([`kraken_contract::tool`]), filtered to the active service groups and
//! rendered as rmcp definitions.
//!
//! Nothing here decides *what* a tool is — the contract does, from the leaf
//! types themselves. This module decides which of them a given server process
//! offers, and how the acknowledgment gate shows up in what a client caches.

use crate::error::{McpError, Result};
use kraken_contract::{
    command::Command,
    tool::{Effect, Entry, Service, Surface},
};
use rmcp::model::{Tool, ToolAnnotations};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// Whether a tool is destructive under `mode`: a paper-safe verb is relaxed
/// only inside a paper workspace.
///
/// The single implementation of the gate. Discovery (which shapes the schema a
/// client caches) and execution both route through here, so a cached annotation
/// and a live call cannot disagree except about the mode itself.
const fn armed_under(effect: Effect, mode: GateMode) -> bool {
    effect.is_dangerous() && !(effect.is_paper_safe() && matches!(mode, GateMode::Paper))
}

/// The gate a command executes under, derived from the active workspace.
///
/// Resolved at startup for tool discovery and again on every call for the
/// acknowledgment gate: a workspace can be promoted while the server runs, and
/// the cached discovery value would then be a stale relaxation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateMode {
    /// No workspace — the real Kraken account. A named live workspace also
    /// gates like Master: fail-closed.
    Master,
    /// A paper workspace: simulated fills, no real money reachable through the
    /// mode-routed verbs.
    Paper,
}

/// One registered tool: what the contract declares, plus the definition
/// discovery serves.
#[derive(Debug, Clone)]
pub struct ToolEntry {
    declared: Arc<Entry>,
    pub(crate) tool: Tool,
}

impl ToolEntry {
    /// Whether this call needs acknowledgment under `mode`. Asked per call, not
    /// cached: the effect is the contract's, only the mode moves.
    pub fn armed_under(&self, mode: GateMode) -> bool {
        armed_under(self.declared.effect, mode)
    }

    pub fn name(&self) -> &str {
        self.tool.name.as_ref()
    }

    pub fn description(&self) -> &str {
        self.tool.description.as_deref().unwrap_or("")
    }

    /// The invocation an operator would type (`kraken order buy`) — what a
    /// confirmation prompt names, because a wire tool name is not what the
    /// person approving it recognises.
    pub fn command(&self) -> &str {
        &self.declared.command
    }

    /// The advertised input schema, so a host can pin contract details without
    /// depending on rmcp types. Borrowed: a caller that indexes it
    /// (`["required"]`) has no use for a copy of the whole map.
    pub fn input_schema(&self) -> &Map<String, Value> {
        &self.tool.input_schema
    }

    /// Whether discovery annotates this tool as destructive.
    pub fn destructive_hint(&self) -> bool {
        self.tool
            .annotations
            .as_ref()
            .and_then(|a| a.destructive_hint)
            .unwrap_or(false)
    }

    /// What the call does to the world, as the contract classifies it.
    pub fn effect(&self) -> Effect {
        self.declared.effect
    }

    /// The service group this tool registered under.
    pub fn group(&self) -> Service {
        self.declared.group
    }

    /// The typed command this call describes, or why it cannot be one.
    pub fn call(&self, arguments: Map<String, Value>) -> kraken_contract::tool::Result<Command> {
        self.declared.command(arguments)
    }

    /// The narrowing arguments this tool advertises, named in the refusal when
    /// a call returns more than the seam will carry.
    pub fn narrowing_arguments(&self) -> Vec<&str> {
        // `all` is not here: it widens a call rather than narrowing it, and the
        // refusal handles it separately by telling the caller to drop it.
        // `asset` is here for `assets`, whose unnarrowed form returns the whole
        // venue — the shape of call this refusal exists for.
        const NARROWING: &[&str] = &[
            "asset", "count", "depth", "end", "interval", "limit", "offset", "pair", "pairs",
            "since", "start", "symbol", "symbols",
        ];
        let advertised: BTreeSet<&str> = self.declared.arguments().collect();
        NARROWING
            .iter()
            .copied()
            .filter(|name| advertised.contains(name))
            .collect()
    }
}

/// The active tools, by wire name. One container: the name is already inside
/// each entry, so a parallel index could only ever disagree with it. `Surface`
/// yields tool-name order, which this preserves.
#[derive(Debug)]
pub struct ToolRegistry(BTreeMap<String, ToolEntry>);

impl ToolRegistry {
    /// Registers every tool in `surface` whose group is active.
    ///
    /// `mode` shapes only the advertised schema — whether it asks for
    /// `acknowledged` up front. The gate itself is re-derived per call.
    pub fn build(
        surface: &Surface,
        active: &[Service],
        allow_dangerous: bool,
        mode: GateMode,
    ) -> Result<Self> {
        let tools: BTreeMap<String, ToolEntry> = surface
            .iter()
            // A command the contract classifies but no client can call — a
            // stream, a prompt — is still in the surface: the catalog documents
            // it. It is not a tool.
            .filter(|declared| declared.reach.is_tool() && active.contains(&declared.group))
            .map(|declared| {
                let entry = register(Arc::clone(declared), allow_dangerous, mode);
                (entry.tool.name.to_string(), entry)
            })
            .collect();

        if tools.is_empty() {
            return Err(McpError::Validation(
                "No tools available after service filtering. Ensure at least one REST-eligible \
                 service group is specified."
                    .into(),
            ));
        }
        Ok(Self(tools))
    }

    pub fn tools(&self) -> impl ExactSizeIterator<Item = &ToolEntry> {
        self.0.values()
    }

    pub fn get_by_name(&self, name: &str) -> Option<&ToolEntry> {
        self.0.get(name)
    }

    /// The discovery payload as advertised to clients — what `tools/list`
    /// serves, and what the snapshot tests pin as the schema contract.
    pub fn tool_definitions(&self) -> Vec<Tool> {
        self.0.values().map(|e| e.tool.clone()).collect()
    }
}

/// Renders one declaration as the definition a client caches.
fn register(declared: Arc<Entry>, allow_dangerous: bool, mode: GateMode) -> ToolEntry {
    let armed = armed_under(declared.effect, mode);
    let mut schema = declared
        .input_schema
        .as_object()
        .cloned()
        .unwrap_or_default();
    if !allow_dangerous {
        match (armed, declared.effect) {
            (true, _) => declare_acknowledgment(&mut schema, Acknowledgment::Required),
            (false, Effect::LocalOrLive) => {
                declare_acknowledgment(&mut schema, Acknowledgment::Offered);
            }
            _ => {}
        }
    }

    let tool = Tool::new(
        declared.name.to_string(),
        describe(&declared, armed),
        Arc::new(schema),
    )
    .with_title(declared.command.clone())
    .with_annotations(annotations(declared.effect, armed));

    ToolEntry { declared, tool }
}

/// rmcp's behaviour hints, from the contract's judgment under the gate that
/// applies now.
///
/// `destructive` follows `armed`, not the raw effect: inside a paper workspace
/// a mode-routed order fills against the simulator, so a client routing on
/// annotations must not prompt a human for it — the same relaxation the danger
/// prefix and the acknowledgment already get.
///
/// A free function, not `From`: both types are foreign to this crate, so an
/// impl would break the orphan rule (E0117).
fn annotations(effect: Effect, armed: bool) -> ToolAnnotations {
    match effect {
        Effect::ReadOnly => ToolAnnotations::new().read_only(true),
        _ if !armed => ToolAnnotations::new().read_only(false),
        _ => ToolAnnotations::new()
            .read_only(false)
            .destructive(true)
            .idempotent(false),
    }
}

/// The description a client sees.
///
/// The danger prefix is part of discovery, not decoration: a client that
/// ignores annotations still reads the text. The leaf's cross-field rules are
/// part of it too — a caller that reads "give exactly one of `pairs`, `all`"
/// never sends the shape the seam would refuse, and no JSON Schema keyword
/// states that portably.
fn describe(declared: &Entry, armed: bool) -> String {
    let mut text = if armed {
        format!(
            "[DANGEROUS: requires human confirmation] {}",
            declared.description
        )
    } else {
        declared.description.clone()
    };
    if !declared.constraints.is_empty() {
        text.push_str(&format!(
            " Required shape: {}.",
            declared.constraints.join("; ")
        ));
    }
    text
}

/// How firmly discovery asks for `acknowledged`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Acknowledgment {
    /// The call is armed: a client cannot invoke the tool without stating
    /// intent.
    Required,
    /// The call is harmless in its usual shape but not in all of them —
    /// `workspace create --mode live` allocates real money, and the execution
    /// gate refuses that shape unconfirmed, so discovery says so up front.
    Offered,
}

impl Acknowledgment {
    const fn description(self) -> &'static str {
        match self {
            Self::Required => {
                "Set to true to state that you intend this operation. It is not the confirmation: \
                 where the client can reach the user, the user is asked and their answer decides."
            }
            Self::Offered => {
                "Required (true) when a mode or target names the real account: that allocates or \
                 moves real money."
            }
        }
    }
}

/// Adds `acknowledged` to the advertised schema, required or merely offered.
///
/// Advertised whatever the client can do, because discovery is built before any
/// client has handshaked and a client caches what it is served: a schema that
/// appeared or vanished with the elicitation capability would be a schema the
/// client could hold a stale copy of. The argument is harmless to a client that
/// never needs it, and it is what the gate falls back to for one that cannot
/// reach a person.
fn declare_acknowledgment(schema: &mut Map<String, Value>, kind: Acknowledgment) {
    let Some(properties) = schema
        .entry("properties")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
    else {
        return;
    };
    properties.insert(
        "acknowledged".to_owned(),
        serde_json::json!({ "type": "boolean", "description": kind.description() }),
    );

    if kind == Acknowledgment::Required
        && let Some(names) = schema
            .entry("required")
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
    {
        names.push(Value::String("acknowledged".to_owned()));
    }
}

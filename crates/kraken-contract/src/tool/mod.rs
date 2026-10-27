//! The agent-facing surface: every callable leaf of [`Command`], derived from
//! the command tree's own types.
//!
//! A tool's name, description, arguments and schema all come from the leaf that
//! declares it, so renaming a field reshapes the tool automatically and the two
//! cannot drift. What no schema can state — whether a call moves money, which
//! credentials it needs, which service group it belongs to — each leaf declares
//! once through [`Classified`], on the line next to the struct it describes.
//!
//! Execution is one conversion: [`Entry::command`]. There is no argv, no second
//! parse, and no per-leaf dispatch.
//!
//! What a schema cannot state, a leaf declares beside its classification: which
//! arguments exclude or require each other ([`Rule`]), because clap's own
//! `conflicts_with` and `required_unless_present` are attributes only the
//! command line reads.

use crate::command::Command;
use schemars::{Schema, generate::SchemaSettings};
use serde_json::{Map, Value};
#[cfg(test)]
use std::collections::BTreeSet;
use std::{borrow::Borrow, collections::BTreeMap, fmt, sync::Arc};

mod coerce;
mod rule;
mod schema;

pub use rule::Rule;

/// A tool call's result, which fails only in the ways [`CallError`] names.
pub type Result<T> = std::result::Result<T, CallError>;

/// Tag chains clap hides. `#[command(flatten)]` on a variant makes the group a
/// Rust type but not a CLI verb, so `kraken ticker` runs
/// `Command::Market(MarketCommand::Ticker(..))` — the tag chain keeps a
/// `market` segment the command line never shows. Pinned against the real clap
/// tree by `kraken-cli`'s surface tests, which is where `Cli` lives.
const TRANSPARENT: &[&[&str]] = &[
    &["market"],
    &["account"],
    &["streaming"],
    &["futures", "market"],
    &["futures", "trading"],
    &["paper", "trade"],
    &["session", "request"],
    &["streaming", "ws", "cmd"],
    &["streaming", "ws", "cmd", "channel"],
    &["streaming", "ws", "cmd", "method"],
];

/// Where a leaf sits in the tree: its family's tag chain plus its own tag,
/// which together are the keys a call's arguments nest under —
/// `{"order": {"buy": {…}}}` for `["order"]` and `"buy"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommandPath {
    groups: &'static [&'static str],
    tag: &'static str,
}

impl CommandPath {
    pub const fn new(groups: &'static [&'static str], tag: &'static str) -> Self {
        Self { groups, tag }
    }

    /// The full tag chain, outermost first.
    pub fn tags(self) -> impl DoubleEndedIterator<Item = &'static str> {
        self.groups.iter().copied().chain(std::iter::once(self.tag))
    }

    /// Nest `arguments` under the chain, producing the shape [`Command`]
    /// deserializes from.
    fn nest(self, arguments: Value) -> Value {
        self.tags().rev().fold(arguments, |inner, tag| {
            Value::Object(Map::from_iter([(tag.to_owned(), inner)]))
        })
    }
}

impl fmt::Display for CommandPath {
    /// The CLI spelling — `order buy`, or `ticker` for a transparent group.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut written = 0;
        for (i, group) in self.groups.iter().enumerate() {
            if TRANSPARENT.contains(&&self.groups[..=i]) {
                continue;
            }
            if written > 0 {
                f.write_str(" ")?;
            }
            f.write_str(group)?;
            written += 1;
        }
        if written > 0 {
            f.write_str(" ")?;
        }
        f.write_str(self.tag)
    }
}

/// The name a client calls a tool by: `kraken_order_buy`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub struct ToolName(String);

impl From<CommandPath> for ToolName {
    fn from(path: CommandPath) -> Self {
        Self(format!(
            "kraken_{}",
            path.to_string().replace([' ', '-'], "_")
        ))
    }
}

impl fmt::Display for ToolName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// So a [`Surface`] lookup takes the wire string without building a `ToolName`.
impl Borrow<str> for ToolName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// What a call does to the world — the judgment no schema carries.
///
/// Flat on purpose: a paper-safe call that is not otherwise dangerous cannot be
/// expressed, because it does not exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// Reads only.
    ReadOnly,
    /// Writes local state and never the venue: scaffolding, a paper workspace.
    Local,
    /// Writes local state in its ordinary shape, but one argument points it at
    /// the real account — `workspace create --mode live` allocates real money,
    /// `recipe run --real-account` trades it. The execution gate refuses that
    /// shape unacknowledged, so discovery offers `acknowledged` without
    /// demanding it.
    LocalOrLive,
    /// Moves money or changes account state, with no simulated equivalent.
    Dangerous,
    /// Moves money against the real account, but the same verb runs simulated
    /// inside a paper workspace — so a paper call needs no acknowledgment.
    PaperSafe,
}

impl Effect {
    /// Whether a call must be acknowledged before it runs.
    pub const fn is_dangerous(self) -> bool {
        matches!(self, Self::Dangerous | Self::PaperSafe)
    }

    /// Whether a paper workspace relaxes that acknowledgment.
    pub const fn is_paper_safe(self) -> bool {
        matches!(self, Self::PaperSafe)
    }
}

/// Which credentials a call needs. Richer than a bool, because an agent can act
/// on *which* key is missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Credentials {
    /// Public data.
    None,
    /// The spot REST key.
    Spot,
    /// The futures key, which is a separate credential.
    Futures,
}

/// A service group. `ValueEnum`, so `kraken-mcp --services market,paper` parses
/// into these instead of being matched as strings.
///
/// Each variant's doc comment is its published description: clap's derive lifts
/// it into the variant's `PossibleValue` help, and [`Service::description`]
/// reads it back, so the catalog cannot describe a group differently from the
/// enum that defines it.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    clap::ValueEnum,
    strum::Display,
    serde::Serialize,
)]
#[strum(serialize_all = "kebab-case")]
#[serde(rename_all = "kebab-case")]
pub enum Service {
    /// Public market data; no credentials.
    Market,
    /// Private account data: balances, orders, history.
    Account,
    /// Spot order placement and management.
    Trade,
    /// Spot paper trading against live prices.
    Paper,
    /// Workspaces, sessions, tapes, recipes, extensions.
    Workspace,
    /// Futures market data and trading.
    Futures,
    /// Futures paper trading.
    FuturesPaper,
    /// Deposits, withdrawals, transfers.
    Funding,
    /// Staking and earn allocations.
    Earn,
    /// Browser sign-in and credential management.
    Auth,
    /// Subaccount management.
    Subaccount,
    /// Product feedback.
    Feedback,
    /// WebSocket streaming; CLI only.
    Websocket,
    /// Futures WebSocket streaming; CLI only.
    FuturesWs,
    /// The interactive setup wizard; CLI only.
    Utility,
}

impl Service {
    /// The group's own description, as its doc comment states it.
    pub fn description(self) -> String {
        <Self as clap::ValueEnum>::to_possible_value(&self)
            .and_then(|value| value.get_help().map(ToString::to_string))
            .unwrap_or_default()
    }

    /// Every group the contract declares, in declaration order.
    pub fn all() -> impl Iterator<Item = Self> {
        <Self as clap::ValueEnum>::value_variants().iter().copied()
    }
}

/// How far a command reaches: what a client can do with it, and when it cannot,
/// why. Every command is classified either way — the catalog says what each one
/// does to the world whether or not a client can call it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reach {
    /// A tool call runs it and returns its value.
    Tool,
    /// Writes its own output as it runs: a frame stream, or the replies to one
    /// WebSocket request. A request/response call has nowhere to put it.
    Stream,
    /// Needs a terminal: a secret typed in, or a prompt answered. A stateless
    /// tool call cannot carry either.
    Interactive,
    /// Deliberately not offered — local credential surgery with no remote use.
    Withheld,
}

impl Reach {
    /// Whether a client can call it.
    pub const fn is_tool(self) -> bool {
        self.why_not().is_none()
    }

    /// Why a client cannot call it, or `None` when it can. The reason is
    /// published in the catalog, so it lives with the variant rather than with
    /// whichever consumer happens to render it.
    pub const fn why_not(self) -> Option<&'static str> {
        match self {
            Self::Tool => None,
            Self::Stream => Some("prints its own output as it runs; run it through the CLI"),
            Self::Interactive => Some("needs a terminal: a secret typed in, or a prompt answered"),
            Self::Withheld => Some("not offered: local credential surgery with no remote use"),
        }
    }
}

/// A command leaf, as an agent sees it.
///
/// Every leaf declares this — including the ones that stream — so the published
/// catalog can say what each command does to the world whether or not a client
/// can call it. [`Reach`] is the axis that decides callability;
/// classification is not an exposure decision.
pub trait Classified: schemars::JsonSchema + clap::Args {
    /// Where the leaf sits in the tree. Checked against the derived schema, so
    /// a wrong chain fails a test rather than a call.
    const PATH: CommandPath;
    const SERVICE: Service;
    const EFFECT: Effect;
    const CREDENTIALS: Credentials;
    const REACH: Reach;
    /// Cross-field validity clap states as attributes, which a tool call never
    /// reaches. Empty for the leaves whose arguments are independent — most of
    /// them.
    const RULES: &'static [Rule] = &[];
    /// One invocation an operator could paste, published in the catalog. The
    /// arguments are generated; a worked example is the one thing they cannot
    /// be.
    const EXAMPLE: Option<&'static str> = None;
}

/// The full invocation: the command plus its positionals, or just the command
/// when it takes none.
fn usage(path: CommandPath, positionals: &str) -> String {
    if positionals.is_empty() {
        format!("kraken {path}")
    } else {
        format!("kraken {path} {positionals}")
    }
}

/// The leaf's positional arguments as an operator would type them: `<PAIR>` for
/// one that is required, `[PAIRS...]` for an optional list.
///
/// Read from clap rather than restated, because the published template is how
/// an agent driving the CLI as a subprocess learns the argument *order* — which
/// no JSON Schema records.
fn positionals<T: clap::Args>() -> String {
    let command = T::augment_args(clap::Command::new("leaf"));
    let mut rendered = Vec::new();
    for arg in command.get_positionals() {
        let name = arg
            .get_value_names()
            .and_then(|names| names.first())
            .map(|name| name.to_string())
            .unwrap_or_else(|| arg.get_id().as_str().to_ascii_uppercase());
        let many = arg
            .get_num_args()
            .is_some_and(|range| range.max_values() > 1);
        let ellipsis = if many { "..." } else { "" };
        rendered.push(if arg.is_required_set() {
            format!("<{name}{ellipsis}>")
        } else {
            format!("[{name}{ellipsis}]")
        });
    }
    rendered.join(" ")
}

/// The doc comments one command enum's variants carry, by serde tag.
///
/// A leaf's own description is not on its own type: `/// Place a buy order.`
/// sits on `OrderCommand::Buy`, so schemars puts it on that branch of
/// `OrderCommand`'s schema and it never appears in `Buy`'s. Reading one level
/// of the family enum's `oneOf` is therefore the whole lookup, and the family
/// enum is named at the declaration site — no descent through the tree, and no
/// deciding whether a nested `oneOf` is a subcommand or a documented value enum
/// (guessing that wrong is what once lost `workspace create`, `feedback` and
/// `session note` from the surface).
///
/// A tag the enum does not have yields no description, and
/// `every_tool_describes_itself` fails on that.
pub struct Descriptions(BTreeMap<String, String>);

impl Descriptions {
    /// Reads the variants of one command enum. Called once per family by
    /// [`tools!`](crate::tools), not once per leaf.
    pub fn of<T: schemars::JsonSchema>() -> Self {
        let schema = variants_of::<T>();
        let branches = schema.get("oneOf").and_then(Value::as_array);
        Self(
            branches
                .into_iter()
                .flatten()
                .filter_map(|branch| {
                    // Externally tagged: each branch is a single-key object whose
                    // key is the tag. A unit variant serializes as a bare string
                    // and has no `properties` — it also has no arguments type, so
                    // it cannot be a classified leaf.
                    let tag = branch.get("properties")?.as_object()?.keys().next()?;
                    let description = branch.get("description")?.as_str()?;
                    // `variants_of` skips `portable`, so this prose still
                    // carries the source's hard wraps.
                    Some((tag.clone(), schema::unwrap_prose(description).into_owned()))
                })
                .collect(),
        )
    }

    fn of_tag(&self, tag: &str) -> String {
        self.0.get(tag).cloned().unwrap_or_default()
    }
}

/// A schema generated the way a tool call reads it: the deserialize contract,
/// inlined so a client never meets a `$ref` it cannot resolve, and with no
/// `$schema` keyword — this is embedded in a tool definition, not served as a
/// document. This is what a leaf publishes as its arguments.
fn schema_of<T: schemars::JsonSchema>() -> Schema {
    let mut settings = SchemaSettings::draft2020_12().for_deserialize();
    settings.inline_subschemas = true;
    settings.meta_schema = None;
    let mut generated = settings.into_generator().root_schema_for::<T>();
    schema::portable(&mut generated);
    generated
}

/// A family enum's own schema, read for its variants' tags and prose.
///
/// Deliberately not [`schema_of`]: inlining would expand every leaf's arguments
/// inside its family — a throwaway second copy of the whole surface — and the
/// two keywords this reads, the tag and the variant's `description`, sit on the
/// branch object either way. Nothing here is published, so it skips
/// [`schema::portable`] too.
fn variants_of<T: schemars::JsonSchema>() -> Schema {
    let mut settings = SchemaSettings::draft2020_12().for_deserialize();
    settings.meta_schema = None;
    settings.into_generator().root_schema_for::<T>()
}

/// One tool, as discovery serves it. Serializes as the published catalog entry.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Entry {
    pub name: ToolName,
    /// The invocation an operator would type, without its arguments.
    pub command: String,
    /// The same invocation with its positional arguments in the order the
    /// command line expects them — the one thing `input_schema` cannot
    /// record, and what an agent driving the CLI as a subprocess needs.
    pub usage: String,
    pub group: Service,
    /// The leaf's own doc comment, read from the generated schema.
    pub description: String,
    pub credentials: Credentials,
    pub effect: Effect,
    pub reach: Reach,
    pub input_schema: Schema,
    /// One invocation an operator could paste. Absent only where no leaf
    /// declares one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<&'static str>,
    /// The cross-field rules a caller must satisfy, as prose. Published because
    /// a client that reads them never sends the shape that breaks one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<String>,
    #[serde(skip)]
    path: CommandPath,
    #[serde(skip)]
    rules: &'static [Rule],
}

impl Entry {
    /// Reads one leaf's whole declaration, taking its prose from its family's
    /// variants. Called by [`tools!`](crate::tools).
    pub fn of<T: Classified>(described: &Descriptions) -> Self {
        let mut input_schema = schema_of::<T>();
        // A list clap marks `required` reaches `serde` as an empty `Vec`, so the
        // generated schema alone never says it must carry a value. The rule that
        // refuses the empty call is the declaration; this publishes it.
        let required: Vec<&str> = T::RULES
            .iter()
            .filter_map(|rule| match rule {
                Rule::NonEmpty(field) => Some(*field),
                _ => None,
            })
            .collect();
        schema::require(&mut input_schema, &required);

        Self {
            name: ToolName::from(T::PATH),
            command: format!("kraken {}", T::PATH),
            usage: usage(T::PATH, &positionals::<T>()),
            group: T::SERVICE,
            description: described.of_tag(T::PATH.tag),
            credentials: T::CREDENTIALS,
            effect: T::EFFECT,
            reach: T::REACH,
            input_schema,
            example: T::EXAMPLE,
            constraints: T::RULES.iter().map(Rule::requirement).collect(),
            path: T::PATH,
            rules: T::RULES,
        }
    }

    /// Where this tool sits in the tree.
    pub fn path(&self) -> CommandPath {
        self.path
    }

    /// The arguments as the leaf will read them, or why it cannot.
    ///
    /// Three checks, in the order that gives a caller the most useful refusal:
    /// every argument must be one the schema names — the check that stops a
    /// caller smuggling in a flag the tool never advertised, such as a
    /// credential override; then the values are read the way the command
    /// line would read them; then the leaf's own [`Rule`]s, which are the
    /// validity clap enforces through attributes no tool call reaches.
    fn accepted(&self, mut arguments: Map<String, Value>) -> Result<Map<String, Value>> {
        if !self.reach.is_tool() {
            return Err(CallError::NotATool {
                command: self.path,
                reach: self.reach,
            });
        }
        let accepted = self.properties();
        if let Some(argument) = arguments
            .keys()
            .find(|key| !accepted.is_some_and(|properties| properties.contains_key(*key)))
        {
            return Err(CallError::UnknownArgument {
                command: self.path,
                argument: argument.clone(),
                accepted: self.arguments().collect::<Vec<_>>().join(", "),
            });
        }

        if let Some(properties) = schema::properties(&self.input_schema) {
            coerce::to_schema(&mut arguments, properties);
        }

        // After coercion: `"all": "true"` is an argument that was given, and the
        // rules judge what a caller meant, not how they spelled it.
        if let Some(why) = self
            .rules
            .iter()
            .find_map(|rule| rule.violation(&arguments))
        {
            return Err(CallError::Shape {
                command: self.path,
                why,
            });
        }

        Ok(arguments)
    }

    /// The command this call describes, refusals and all: the one door, and the
    /// only place a `serde` failure can be attributed to an argument, because
    /// it is where the schema is in reach.
    pub fn command(&self, arguments: Map<String, Value>) -> Result<Command> {
        read(self.path, self.accepted(arguments)?, &self.input_schema)
    }

    /// The argument names the leaf accepts.
    pub fn arguments(&self) -> impl Iterator<Item = &str> {
        self.properties()
            .into_iter()
            .flat_map(|properties| properties.keys().map(String::as_str))
    }

    /// The advertised arguments, as the schema holds them. One resolution of
    /// where argument names live, so the anti-injection check in
    /// [`Self::accepted`] and the names it reports back cannot disagree.
    fn properties(&self) -> Option<&Map<String, Value>> {
        schema::properties(&self.input_schema)
    }
}

/// Every command the contract classifies, by tool name. The callable ones are
/// tools; the rest the catalog documents, and says why.
#[derive(Debug)]
pub struct Surface(BTreeMap<ToolName, Arc<Entry>>);

impl Surface {
    /// Assembles the surface: one entry per classified leaf, each already
    /// carrying its own schema and its own prose.
    ///
    /// Every leaf yields an entry — the doc comment that used to promise this
    /// also described a `filter_map` that quietly dropped three of them. Only
    /// the description can come up empty, and `every_tool_describes_itself`
    /// fails on that rather than a tool going missing.
    fn new() -> Self {
        Self(
            classifications()
                .into_iter()
                .map(|entry| (entry.name.clone(), Arc::new(entry)))
                .collect(),
        )
    }

    /// The process-wide surface, and the only way to one: deriving it generates
    /// a schema per leaf and one per family, so every consumer — the
    /// server, the catalog, the tests — shares this rather than paying for
    /// it again.
    pub fn shared() -> &'static Self {
        static SHARED: std::sync::LazyLock<Surface> = std::sync::LazyLock::new(Surface::new);
        &SHARED
    }

    pub fn get(&self, name: &str) -> Option<&Arc<Entry>> {
        self.0.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<Entry>> {
        self.0.values()
    }

    /// Resolves a wire name, then the arguments: the whole journey from a
    /// client's JSON to the command the CLI parses.
    pub fn command(&self, tool: &str, arguments: Map<String, Value>) -> Result<Command> {
        self.get(tool)
            .ok_or_else(|| CallError::UnknownTool(tool.to_owned()))?
            .command(arguments)
    }
}

/// Reads the arguments as the command they describe.
///
/// `advertised` only improves a refusal: `serde` loses its field path through a
/// `#[serde(flatten)]` group — which is every trading leaf — and the schema can
/// say which argument was not the type it declares.
fn read(path: CommandPath, arguments: Map<String, Value>, advertised: &Schema) -> Result<Command> {
    let nested = path.nest(Value::Object(arguments));
    // Deserialized through `serde_path_to_error`: the bare message names only the
    // type it wanted, and `invalid type: string "0.5", expected f64` leaves a
    // caller guessing which of two dozen arguments to fix.
    serde_path_to_error::deserialize(&nested).map_err(|error| CallError::Arguments {
        command: path,
        argument: match argument_at(&error, path) {
            named if !named.is_empty() => named,
            _ => leaf_arguments(&nested, path)
                .map(|arguments| schema::type_mismatches(advertised, arguments).join(", "))
                .unwrap_or_default(),
        },
        source: error.into_inner(),
    })
}

/// The leaf's own arguments, back out of the nested shape the deserializer
/// read.
fn leaf_arguments(nested: &Value, path: CommandPath) -> Option<&Map<String, Value>> {
    path.tags()
        .try_fold(nested, |level, tag| level.get(tag))?
        .as_object()
}

/// The failing argument's name, as the deserializer's path reports it minus the
/// tag chain that led to the leaf (`order.buy.volume` → `volume`). Empty when
/// the failure is the leaf itself rather than one of its arguments.
fn argument_at(error: &serde_path_to_error::Error<serde_json::Error>, path: CommandPath) -> String {
    let tags: Vec<&str> = path.tags().collect();
    error
        .path()
        .iter()
        .map(|segment| segment.to_string())
        .filter(|segment| !tags.contains(&segment.as_str()))
        .collect::<Vec<_>>()
        .join(".")
}

/// Why a tool call could not become a command.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("no tool named `{0}`")]
    UnknownTool(String),
    #[error("`{command}` is not callable ({reach:?}); run it through the CLI")]
    NotATool { command: CommandPath, reach: Reach },
    #[error("`{command}` takes no argument named `{argument}`; it accepts: {accepted}")]
    UnknownArgument {
        command: CommandPath,
        argument: String,
        /// What it does accept, so the refusal tells a caller what to try —
        /// the schema says so too, but the message is what a model reads.
        accepted: String,
    },
    #[error("`{command}` needs a different shape: {why}")]
    Shape {
        command: CommandPath,
        /// What to change, in the words a model reads.
        why: String,
    },
    #[error("invalid arguments for `{command}`: {}{source}", prefix(argument))]
    Arguments {
        command: CommandPath,
        /// The argument the deserializer stopped on, so the refusal names what
        /// to fix rather than only what was expected.
        argument: String,
        #[source]
        source: serde_json::Error,
    },
}

/// `` `volume`: `` — the failing argument, when the deserializer named one.
fn prefix(argument: &str) -> String {
    if argument.is_empty() {
        String::new()
    } else {
        format!("`{argument}`: ")
    }
}

/// Every family's classifications, in tree order. One entry per
/// [`tools!`](crate::tools) invocation; a family missing here is a family with
/// no tools.
fn classifications() -> Vec<Entry> {
    [
        crate::account::account(),
        crate::auth::auth(),
        crate::command::root(),
        crate::earn::earn(),
        crate::explain::explain(),
        crate::funding::deposit(),
        crate::funding::withdrawal(),
        crate::futures::futures_market(),
        crate::futures::futures_trading(),
        crate::futures::futures_trading_order(),
        crate::futures_paper::futures_paper(),
        crate::futures_ws::futures_ws(),
        crate::lab::lab(),
        crate::market::market(),
        crate::order::order(),
        crate::paper::paper(),
        crate::paper::paper_trade(),
        crate::plugin::plugin(),
        crate::recipe::recipe(),
        crate::session::session(),
        crate::session::session_request(),
        crate::session::session_request_state(),
        crate::streamd::streaming_streamd(),
        crate::streaming::streaming(),
        crate::subaccount::subaccount(),
        crate::tape::tape(),
        crate::workspace::workspace(),
        crate::ws::streaming_ws_cmd_channel(),
        crate::ws::streaming_ws_cmd_method(),
    ]
    .concat()
}

/// Declares one command family's tools, as a table: one line per command, in
/// the file that defines it.
///
/// Everything an agent sees is derived from the types — the tool's name, its
/// arguments, its schema, its prose. A line here supplies only the four
/// judgments no type can state, and locates the leaf in the tree:
///
/// ```text
/// crate::tools! {
///     /// The `kraken order` commands.
///     fn order in ["order"] of OrderCommand {
///         Buy       as "buy"        => Trade, PaperSafe, Spot, Tool
///             example "kraken order buy BTCUSD 0.5 --type limit --price 50000";
///         Batch     as "batch"      => Trade, Dangerous, Spot, Tool;
///         CancelAll as "cancel-all" => Trade, PaperSafe, Spot, Tool;
///     }
/// }
/// //  ↑ leaf type    ↑ serde tag      ↑ Service, Effect, Credentials, Reach
/// ```
///
/// # The optional clauses
///
/// `where` declares the cross-field validity clap states as attributes. A tool
/// call never reaches clap — its arguments deserialize straight into the leaf —
/// so `conflicts_with`, `required_unless_present` and `requires` have no effect
/// on one, and the same rule is restated here as a [`Rule`]:
///
/// ```text
/// Ticker as "ticker" => Market, ReadOnly, None, Tool
///     where ExactlyOne(&["pairs", "all"])
///     example "kraken ticker BTCUSD ETHUSD";
/// ```
///
/// `a_shape_the_command_line_refuses_is_refused_of_a_tool_call` fails if either
/// side of that pair goes missing, so the two cannot drift apart.
///
/// `example` is one invocation an operator could paste. Everything else about a
/// command is generated; a worked example is the one thing that cannot be.
///
/// # The two coordinates
///
/// `in` is the family's **tag chain** and the leaf's tag extends it, giving the
/// keys a call's arguments nest under: `["order"]` + `"buy"` is
/// `{"order": {"buy": {…}}}`, the shape [`Command`] deserializes from. The tag
/// is written out rather than derived from the type name because it is a wire
/// key — renaming it breaks callers, so it belongs in sight.
///
/// `of` names the **enum whose variants those tags are**, because a leaf's
/// description is not on the leaf. `/// Place a buy order.` is a doc comment on
/// `OrderCommand::Buy`, so it appears in `OrderCommand`'s schema and never in
/// `Buy`'s — see [`Descriptions`], which reads one level of it, once per
/// family.
///
/// # What a line produces
///
/// For `Buy as "buy"` in the family above:
///
/// ```text
/// tool name     kraken_order_buy       from the tag chain
/// command       kraken order buy       the same chain, minus groups clap hides
/// description   Place a buy order.     the doc comment on OrderCommand::Buy
/// input_schema  {pair, volume, type…}  schemars over `struct Buy` itself
/// gate posture  armed, relaxed in a paper workspace   from Effect::PaperSafe
/// ```
///
/// # Adding a command
///
/// Add the variant and its arguments struct, then one line here. Nothing else:
/// no schema to write, no catalog to edit, no dispatch arm.
///
/// Three tests hold the table honest, and each fails in a way that names the
/// mistake:
///
/// - a command with no line, or a line naming a command the CLI does not have,
///   fails `every_command_is_classified_exactly_once`
/// - a tag that is not the variant's serde tag — or a wrong `of` enum —
///   resolves to no description and fails `every_tool_describes_itself`
/// - a money verb classified `ReadOnly` or `Local` fails
///   `mutating_commands_are_classified_dangerous` in `kraken-cli`
///
/// # What it expands to
///
/// An `impl` [`Classified`] per leaf (the five consts, verbatim), and a
/// `pub(crate) fn <family>() -> Vec<Entry>` that reads the enum's
/// descriptions once and pairs them with each leaf's own schema.
/// [`classifications`] calls every family function; a family absent from that
/// list is a family with no tools.
#[macro_export]
macro_rules! tools {
    (
        $(#[$meta:meta])*
        fn $family:ident in $groups:tt of $enum:ty {
            $($leaf:ident as $tag:literal =>
                $service:ident, $effect:ident, $credentials:ident, $reach:ident
                $(where $($rule:ident($($rule_arg:tt)*)),+ $(,)?)?
                $(example $example:literal)?;)*
        }
    ) => {
        $(
            impl $crate::tool::Classified for $leaf {
                const PATH: $crate::tool::CommandPath =
                    $crate::tool::CommandPath::new(&$groups, $tag);
                const SERVICE: $crate::tool::Service = $crate::tool::Service::$service;
                const EFFECT: $crate::tool::Effect = $crate::tool::Effect::$effect;
                const CREDENTIALS: $crate::tool::Credentials =
                    $crate::tool::Credentials::$credentials;
                const REACH: $crate::tool::Reach = $crate::tool::Reach::$reach;
                $(const RULES: &'static [$crate::tool::Rule] =
                    &[$($crate::tool::Rule::$rule($($rule_arg)*)),+];)?
                $(const EXAMPLE: Option<&'static str> = Some($example);)?
            }
        )*

        $(#[$meta])*
        pub(crate) fn $family() -> Vec<$crate::tool::Entry> {
            // One read of the family enum, shared by its leaves.
            let described = $crate::tool::Descriptions::of::<$enum>();
            vec![$($crate::tool::Entry::of::<$leaf>(&described),)*]
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Parser, Subcommand};

    /// The clap tree, built from the same enum the surface is derived from.
    fn clap_tree() -> clap::Command {
        Command::augment_subcommands(clap::Command::new("kraken"))
    }

    /// Every CLI leaf path, as clap spells it.
    fn clap_leaves() -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        walk_clap_leaves(&clap_tree(), &mut Vec::new(), &mut |_, path| {
            out.insert(path.join(" "));
        });
        out
    }

    /// Every command must be classified, and every classification must name a
    /// command the CLI actually has. An exact bijection: there is no allowlist
    /// of exceptions, because every leaf of the tree carries an arguments
    /// type.
    ///
    /// This replaces the old failure mode outright — a command missing from one
    /// of two hand-kept surfaces used to register no tool at all, silently.
    #[test]
    fn every_command_is_classified_exactly_once() {
        let surface = Surface::shared();
        let classified: BTreeSet<String> =
            surface.iter().map(|tool| tool.path().to_string()).collect();
        let leaves = clap_leaves();

        let unclassified: Vec<&String> = leaves.difference(&classified).collect();
        assert!(
            unclassified.is_empty(),
            "commands with no `tools!` line: {unclassified:?}"
        );

        let phantom: Vec<&String> = classified.difference(&leaves).collect();
        assert!(
            phantom.is_empty(),
            "classifications whose command the CLI does not have: {phantom:?}"
        );
    }

    /// A documented value enum generates a `oneOf` exactly like a nested
    /// subcommand does — one branch per variant, each with its own description.
    /// Reading the keyword instead of the branch shape cost `workspace create`,
    /// `feedback` and `session note` their place in the surface, so the
    /// distinction is pinned on the argument that caused it.
    #[test]
    fn a_value_enum_argument_is_not_a_nested_subcommand() {
        let surface = Surface::shared();
        let create = surface
            .get("kraken_workspace_create")
            .expect("workspace create is a command");
        let properties = create
            .input_schema
            .get("properties")
            .and_then(Value::as_object)
            .expect("it takes arguments");
        assert!(
            properties.contains_key("mode"),
            "`--mode` is an argument of `workspace create`, not a level below it"
        );
        assert!(
            !create.description.is_empty(),
            "a leaf reached by the wrong tag chain loses its description"
        );
    }

    /// A command no client can call is still classified — that is what lets the
    /// published catalog say `ws cancel-all` cancels real orders even though no
    /// tool runs it.
    #[test]
    fn a_command_that_is_not_a_tool_is_still_classified() {
        let surface = Surface::shared();
        let cancel_all = surface
            .iter()
            .find(|tool| tool.command == "kraken ws cancel-all")
            .expect("ws cancel-all is a command");
        assert_eq!(cancel_all.reach, Reach::Stream);
        assert!(
            cancel_all.effect.is_dangerous(),
            "it cancels every open order on the real account"
        );
        assert!(
            surface.command("kraken_ws_cancel_all", Map::new()).is_err(),
            "classified is not callable"
        );
    }

    /// A command with no description is a family that lost its `JsonSchema`
    /// derive, or a declared tag the tree spells differently — both of which
    /// would ship a tool an agent cannot understand.
    #[test]
    fn every_tool_describes_itself() {
        let surface = Surface::shared();
        let mute: Vec<String> = surface
            .iter()
            .filter(|tool| tool.description.is_empty())
            .map(|tool| tool.command.clone())
            .collect();
        assert!(mute.is_empty(), "tools with no description: {mute:?}");
        assert!(
            surface.iter().next().is_some(),
            "the surface must not be empty, or the check above is vacuous"
        );
    }

    /// An argument the command line lets you omit must not be required of a
    /// tool call. Whether the two then agree on the default *value* is a
    /// separate question, asked by
    /// [`a_default_the_command_line_supplies_is_the_one_a_tool_call_supplies`].
    ///
    /// The narrower version of this test only looked at `default_value`, so it
    /// said nothing about the 32 boolean flags: absent-means-false on the
    /// command line, but `missing field 'validate'` to an agent. This asks
    /// the question clap actually answers — is the argument optional? — so
    /// the whole class is covered, defaults and flags alike.
    #[test]
    fn an_optional_command_line_argument_is_never_required_of_a_tool_call() {
        let mut wrong = Vec::new();
        each_clap_leaf(|command, cli, tool| {
            let required = required_arguments(tool);
            for arg in command.get_arguments() {
                let id = arg.get_id().as_str();
                if arg.is_required_set() || !required.contains(&id) {
                    continue;
                }
                wrong.push(format!("{cli} --{id}"));
            }
        });
        assert!(
            wrong.is_empty(),
            "optional on the command line, required of a tool call — each needs \
             `#[serde(default)]`: {wrong:#?}"
        );
    }

    /// Where the command line supplies a default, a tool call must supply the
    /// same one.
    ///
    /// This is the one fact in the tree stated twice: `#[arg(default_value)]`
    /// for clap, `#[serde(default)]` for a tool call. Only the second
    /// reaches the published schema, so a divergence is invisible from
    /// either side — the same invocation would mean one thing typed and
    /// another called, and on an order verb that is a differently priced
    /// trade.
    ///
    /// Asked of clap's own defaults rather than a written list, so a new one
    /// declared on only one side fails here.
    #[test]
    fn a_default_the_command_line_supplies_is_the_one_a_tool_call_supplies() {
        let mut wrong = Vec::new();
        let mut agreed = 0usize;
        each_clap_leaf(|command, cli, tool| {
            let advertised = schema::properties(&tool.input_schema);
            for arg in command.get_arguments() {
                let typed: Vec<&str> = arg
                    .get_default_values()
                    .iter()
                    .filter_map(|value| value.to_str())
                    .collect();
                if typed.is_empty() {
                    continue;
                }
                let id = arg.get_id().as_str();
                match advertised
                    .and_then(|properties| properties.get(id))
                    .and_then(|schema| schema.get("default"))
                {
                    Some(published) if same_default(published, &typed) => agreed += 1,
                    Some(published) => wrong.push(format!(
                        "{cli} --{id}: command line {typed:?}, tool call {published}"
                    )),
                    None => wrong.push(format!(
                        "{cli} --{id}: command line {typed:?}, tool call advertises none"
                    )),
                }
            }
        });
        assert!(
            wrong.is_empty(),
            "a default that differs between the two declarations — each needs `#[serde(default)]` \
             spelling the same value as `#[arg(default_value)]`: {wrong:#?}"
        );
        // A walk that stops matching leaves would agree about nothing and pass.
        assert!(
            agreed > 0,
            "no default was compared at all — the clap walk found no tool"
        );
    }

    /// Whether the schema's default is the value clap would have parsed.
    /// Compared as values, not as text: clap holds every default as the
    /// string a user would have typed, so `1.0` and `"1"` are the same
    /// default and only the spelling differs.
    fn same_default(published: &Value, typed: &[&str]) -> bool {
        match published {
            // A list default reaches clap either as one delimited value or as
            // several, and both spell the same list.
            Value::Array(values) => {
                let flat: Vec<&str> = match typed {
                    [one] => one.split(',').collect(),
                    many => many.to_vec(),
                };
                values.len() == flat.len()
                    && values
                        .iter()
                        .zip(flat)
                        .all(|(value, text)| same_scalar(value, text))
            }
            scalar => matches!(typed, [one] if same_scalar(scalar, one)),
        }
    }

    fn same_scalar(published: &Value, typed: &str) -> bool {
        match published {
            Value::String(text) => text == typed,
            Value::Number(number) => number
                .as_f64()
                .zip(typed.parse::<f64>().ok())
                .is_some_and(|(published, typed)| published == typed),
            // No `bool` arm: a flag's default is its absence, so no clap
            // `default_value` in the tree is a boolean. One added here would
            // fail with both spellings named, which says what to add.
            _ => false,
        }
    }

    /// The hole this whole mechanism exists to close: a shape the command line
    /// refuses must not be a call a tool accepts.
    ///
    /// Generic on purpose — it asks clap the question rather than listing the
    /// leaves, so a new `required_unless_present` pair added without a matching
    /// [`Rule`] fails here instead of shipping. `kraken ticker` with no
    /// arguments used to return an empty body, and `futures cancel` with
    /// neither ID used to reach the venue.
    #[test]
    fn a_shape_the_command_line_refuses_is_refused_of_a_tool_call() {
        let mut accepted = Vec::new();
        each_clap_leaf(|command, cli, tool| {
            if !tool.reach.is_tool() {
                return;
            }
            // An empty command line: whatever clap demands of it, a tool call
            // giving nothing must be refused for the same reason.
            let refused_by_clap = command
                .clone()
                .try_get_matches_from([command.get_name().to_owned()])
                .is_err();
            let accepted_as_a_call = tool.command(Map::new()).is_ok();
            if refused_by_clap && accepted_as_a_call {
                accepted.push(cli.to_owned());
            }

            // A list clap marks `required` must carry a value. Through `serde` it
            // arrives as an empty `Vec`, which is the same call with nothing in
            // it — only a `NonEmpty` rule refuses that.
            for arg in command.get_arguments().filter(|arg| {
                arg.is_required_set()
                    && arg
                        .get_num_args()
                        .is_some_and(|range| range.max_values() > 1)
            }) {
                let empty =
                    Map::from_iter([(arg.get_id().as_str().to_owned(), Value::Array(Vec::new()))]);
                if tool.command(empty).is_ok() {
                    accepted.push(format!("{cli} (empty --{})", arg.get_id()));
                }
            }
        });
        assert!(
            accepted.is_empty(),
            "the command line refuses these with no arguments, a tool call accepts them — each \
             needs a `where` rule on its `tools!` line: {accepted:#?}"
        );
    }

    /// Every argument clap declares in conflict with another is covered by a
    /// declared rule. `get_arg_conflicts_with` is the one constraint clap
    /// exposes, so this half of the duplication is policed by clap itself.
    #[test]
    fn every_conflicting_argument_pair_is_declared() {
        let mut undeclared = Vec::new();
        each_clap_leaf(|command, cli, tool| {
            if !tool.reach.is_tool() {
                return;
            }
            for arg in command.get_arguments() {
                let id = arg.get_id().as_str();
                for other in command.get_arg_conflicts_with(arg) {
                    let other_id = other.get_id().as_str();
                    let declared = tool.rules.iter().any(|rule| {
                        rule.mentions().contains(&id) && rule.mentions().contains(&other_id)
                    });
                    if !declared {
                        undeclared.push(format!("{cli}: {id} vs {other_id}"));
                    }
                }
            }
        });
        assert!(
            undeclared.is_empty(),
            "clap says these arguments conflict; no rule tells a tool call so: {undeclared:#?}"
        );
    }

    /// clap's `requires` gets no [`Rule`] counterpart, and cannot get one
    /// generically: clap exposes no getter for it, so — unlike
    /// `conflicts_with`,
    /// which `every_conflicting_argument_pair_is_declared` reads back off the
    /// tree — nothing can ask whether a pair was restated here. What *can* be
    /// held is that the attribute only ever sits where it costs nothing: on a
    /// leaf no client can call, where clap is the only parser a value meets.
    ///
    /// All five of today's sit on `auth set` (interactive), `recipe run` and
    /// `session start` (both streams). When this fails, either one of those
    /// became callable or a callable leaf grew a `requires` — and the
    /// requirement then needs restating as a `Rule::Requires`, which does
    /// not exist yet because nothing has needed it. An unstated one is the
    /// hole [`Rule`] exists to close: a tool call never reaches clap, so
    /// the attribute would simply not apply.
    ///
    /// Read per module, not across the crate: leaf names repeat between
    /// families — four declare a `Ticker`, two of them streams — so a
    /// crate-wide index by name lets one family's classification answer for
    /// another's. A leaf whose own module does not classify it (flattened
    /// into one, or classified by the root table in `command.rs`) is
    /// reported rather than assumed safe.
    #[test]
    fn a_clap_requires_never_sits_on_a_callable_leaf() {
        let mut carrying = 0;
        for (module, source) in contract_sources() {
            let reach = declared_reach(&source);
            for leaf in structs_with_a_clap_requires(&source) {
                carrying += 1;
                let Some(declared) = reach.get(&leaf) else {
                    panic!(
                        "`{leaf}` ({module}) carries a clap `requires` and its own module does \
                         not classify it — check the leaf it is flattened into, or the table that \
                         does"
                    );
                };
                assert_ne!(
                    declared, "Tool",
                    "`{leaf}` ({module}) is callable and carries a clap `requires`, which a tool \
                     call never reaches — restate it as a `Rule`"
                );
            }
        }
        // A scan that matched nothing would agree with everything.
        assert!(
            carrying > 0,
            "no clap `requires` found at all — the scan has stopped matching"
        );
    }

    /// The leaves one module's [`tools!`](crate::tools) table declares, with
    /// the reach each declares.
    ///
    /// Source-read because the tables name Rust *types* and nothing at runtime
    /// maps a type back to the tool derived from it — [`Surface`] is keyed by
    /// command path. Both facts the test compares come from this one module, so
    /// they cannot be read out of step with each other.
    fn declared_reach(source: &str) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for line in source.lines() {
            // `Leaf as "tag" => Service, Effect, Credentials, Reach`
            let Some((leaf, rest)) = line.trim().split_once(" as ") else {
                continue;
            };
            if !rest.starts_with('"') {
                continue; // an `as` cast, not a table line
            }
            let Some((_, classification)) = rest.split_once("=> ") else {
                continue;
            };
            // The line may continue with `where`/`example` on the next one, so it
            // does not always end in `;`.
            let fields: Vec<&str> = classification
                .trim_end_matches(';')
                .split(',')
                .map(str::trim)
                .collect();
            if let [_service, _effect, _credentials, reach] = fields[..] {
                // The tables align their columns, so the leaf carries the padding
                // that alignment adds.
                out.insert(leaf.trim_end().to_owned(), reach.to_owned());
            }
        }
        out
    }

    /// The argument structs in one module carrying a clap `requires`.
    fn structs_with_a_clap_requires(source: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut current = String::new();
        for line in source.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("pub struct ") {
                current = rest
                    .trim_end_matches(['{', ' ', '(', ';'])
                    .trim()
                    .to_owned();
            }
            if line.starts_with("#[arg(") && line.contains("requires = \"") {
                out.insert(current.clone());
            }
        }
        out
    }

    /// The contract crate's own modules, by file name.
    fn contract_sources() -> Vec<(String, String)> {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&src).expect("the contract's src is readable") {
            let path = entry.expect("a readable dir entry").path();
            if path.extension().is_some_and(|ext| ext == "rs") {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_owned();
                out.push((
                    name,
                    std::fs::read_to_string(&path).expect("a readable module"),
                ));
            }
        }
        assert!(!out.is_empty(), "no contract sources found");
        out
    }

    /// Every command carries a worked example. Everything else about a command
    /// is generated from its type; this is the one thing a person has to
    /// write, so a new command that skips it fails here.
    #[test]
    fn every_command_offers_an_example() {
        let missing: Vec<&str> = Surface::shared()
            .iter()
            .filter(|tool| tool.example.is_none())
            .map(|tool| tool.command.as_str())
            .collect();
        assert!(
            missing.is_empty(),
            "commands with no `example` on their `tools!` line: {missing:#?}"
        );
    }

    /// The mirror of
    /// `an_optional_command_line_argument_is_never_required_of_a_tool_call`:
    /// an argument the command line insists on must be `required` in the
    /// schema, or a client believes it can omit one and learns otherwise
    /// from a failed call.
    #[test]
    fn a_required_command_line_argument_is_required_of_a_tool_call() {
        let mut silent = Vec::new();
        each_clap_leaf(|command, cli, tool| {
            if !tool.reach.is_tool() {
                return;
            }
            let required = required_arguments(tool);
            for arg in command.get_arguments().filter(|arg| arg.is_required_set()) {
                let id = arg.get_id().as_str();
                if !required.contains(&id) {
                    silent.push(format!("{cli} <{id}>"));
                }
            }
        });
        assert!(
            silent.is_empty(),
            "the command line requires these; the advertised schema does not say so: {silent:#?}"
        );
    }

    /// A rule that names an argument the tool does not advertise can never
    /// fire.
    #[test]
    fn every_rule_names_an_advertised_argument() {
        let surface = Surface::shared();
        let mut phantom = Vec::new();
        for tool in surface.iter() {
            for field in tool.rules.iter().flat_map(Rule::mentions) {
                if !schema::advertises(&tool.input_schema, field) {
                    phantom.push(format!("{}: {field}", tool.command));
                }
            }
        }
        assert!(
            phantom.is_empty(),
            "rules naming arguments no tool advertises: {phantom:#?}"
        );
    }

    /// The two shapes that made this necessary, pinned by name.
    #[test]
    fn ticker_refuses_both_pairs_and_all() {
        let surface = Surface::shared();
        let err = surface
            .command(
                "kraken_ticker",
                serde_json::json!({"pairs": ["XBTUSD"], "all": true})
                    .as_object()
                    .cloned()
                    .expect("an object"),
            )
            .expect_err("`--all` conflicts with pairs on the command line");
        assert!(matches!(err, CallError::Shape { .. }), "{err}");
    }

    #[test]
    fn ticker_refuses_neither_pairs_nor_all() {
        let surface = Surface::shared();
        let err = surface
            .command("kraken_ticker", Map::new())
            .expect_err("pairs is required unless --all");
        assert!(matches!(err, CallError::Shape { .. }), "{err}");
    }

    /// A stringified number is what an LLM client sends; the command line would
    /// have parsed it, so the seam does too.
    #[test]
    fn a_stringified_argument_reaches_the_same_command() {
        let surface = Surface::shared();
        let coerced = surface
            .command(
                "kraken_ohlc",
                serde_json::json!({"pair": "XBTUSD", "interval": "60"})
                    .as_object()
                    .cloned()
                    .expect("an object"),
            )
            .expect("a stringified integer is read as one");
        let typed = surface
            .command(
                "kraken_ohlc",
                serde_json::json!({"pair": "XBTUSD", "interval": 60})
                    .as_object()
                    .cloned()
                    .expect("an object"),
            )
            .expect("the typed form parses");
        assert_eq!(
            serde_json::to_value(&coerced).expect("serialize"),
            serde_json::to_value(&typed).expect("serialize")
        );
    }

    /// A refusal an agent can act on names the argument, not just the type.
    #[test]
    fn a_deserialize_failure_names_the_argument() {
        let err = call_command(
            "kraken_ohlc",
            serde_json::json!({"pair": "XBTUSD", "since": 5}),
        )
        .expect_err("since is a string");
        assert!(err.to_string().contains("`since`"), "{err}");
    }

    /// `serde` loses its field path through a flattened arg group, which is the
    /// shape of every trading leaf — the schema names the argument instead.
    #[test]
    fn a_flattened_leaf_still_names_the_argument() {
        let err = call_command(
            "kraken_order_buy",
            serde_json::json!({"pair": "XBTUSD", "volume": "1", "userref": "not-a-number"}),
        )
        .expect_err("a non-numeric userref cannot deserialize");
        assert!(err.to_string().contains("`userref`"), "{err}");
    }

    fn call_command(tool: &str, arguments: Value) -> Result<Command> {
        Surface::shared().command(tool, arguments.as_object().cloned().expect("an object"))
    }

    /// The published usage carries the positional order, which no schema
    /// records.
    #[test]
    fn the_published_usage_shows_the_positional_order() {
        let surface = Surface::shared();
        let buy = surface.get("kraken_order_buy").expect("order buy");
        assert_eq!(buy.usage, "kraken order buy <PAIR> <VOLUME>");
        let balance = surface.get("kraken_balance").expect("balance");
        assert_eq!(
            balance.usage, "kraken balance",
            "a command with no positionals is its own usage"
        );
    }

    /// Every clap leaf with the entry derived from it.
    ///
    /// The index is built once: matching each leaf by scanning the surface
    /// makes the walk quadratic, five times over. Callable and streaming
    /// leaves alike — both carry a schema, and a caller that only means the
    /// callable ones asks `reach.is_tool()` itself.
    fn each_clap_leaf(mut visit: impl FnMut(&clap::Command, &str, &Entry)) {
        let by_path: BTreeMap<String, &Entry> = Surface::shared()
            .iter()
            .map(|tool| (tool.path().to_string(), tool.as_ref()))
            .collect();
        walk_clap_leaves(&clap_tree(), &mut Vec::new(), &mut |command, path| {
            let cli = path.join(" ");
            if let Some(tool) = by_path.get(&cli) {
                visit(command, &cli, tool);
            }
        });
    }

    /// The arguments the advertised schema insists on.
    fn required_arguments(tool: &Entry) -> Vec<&str> {
        tool.input_schema
            .get("required")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default()
    }

    fn walk_clap_leaves(
        command: &clap::Command,
        path: &mut Vec<String>,
        visit: &mut impl FnMut(&clap::Command, &[String]),
    ) {
        let mut children = 0;
        for sub in command.get_subcommands() {
            if sub.get_name() == "help" {
                continue;
            }
            children += 1;
            path.push(sub.get_name().to_owned());
            walk_clap_leaves(sub, path, visit);
            path.pop();
        }
        if children == 0 && !path.is_empty() {
            visit(command, path);
        }
    }

    /// One command, reachable two ways: as a tool call and as a command line.
    struct Parity {
        tool: &'static str,
        arguments: fn() -> Value,
        argv: &'static [&'static str],
    }

    /// The leaves where a divergence would misprice or misroute real money, so
    /// each is pinned rather than trusted to the generic checks above.
    const PARITY: &[Parity] = &[
        Parity {
            tool: "kraken_order_buy",
            arguments: || serde_json::json!({"pair": "BTCUSD", "volume": "0.5", "type": "limit", "price": "50000"}),
            argv: &[
                "order", "buy", "BTCUSD", "0.5", "--type", "limit", "--price", "50000",
            ],
        },
        Parity {
            tool: "kraken_order_sell",
            arguments: || serde_json::json!({"pair": "ETHUSD", "volume": "2", "type": "market", "reduce_only": true}),
            argv: &[
                "order",
                "sell",
                "ETHUSD",
                "2",
                "--type",
                "market",
                "--reduce-only",
            ],
        },
        Parity {
            tool: "kraken_order_cancel",
            arguments: || serde_json::json!({"txids": ["OQCLML-BW3P3-BUCMWZ"]}),
            argv: &["order", "cancel", "OQCLML-BW3P3-BUCMWZ"],
        },
        // Omitting everything optional is the case that proves the defaults
        // agree: `--type` is `limit` on the command line and must be `limit`
        // here too, or the same call places a different order through each door.
        Parity {
            tool: "kraken_order_buy",
            arguments: || serde_json::json!({"pair": "BTCUSD", "volume": "0.5", "price": "50000"}),
            argv: &["order", "buy", "BTCUSD", "0.5", "--price", "50000"],
        },
        Parity {
            tool: "kraken_paper_buy",
            arguments: || serde_json::json!({"pair": "BTCUSD", "volume": "0.01"}),
            argv: &["paper", "buy", "BTCUSD", "0.01"],
        },
        Parity {
            tool: "kraken_paper_init",
            arguments: || serde_json::json!({}),
            argv: &["paper", "init"],
        },
        Parity {
            tool: "kraken_futures_paper_init",
            arguments: || serde_json::json!({}),
            argv: &["futures", "paper", "init"],
        },
        Parity {
            tool: "kraken_futures_order_buy",
            arguments: || serde_json::json!({"symbol": "PF_XBTUSD", "size": "1"}),
            argv: &["futures", "order", "buy", "PF_XBTUSD", "1"],
        },
        Parity {
            tool: "kraken_paper_sell",
            arguments: || serde_json::json!({"pair": "BTCUSD", "volume": "0.01"}),
            argv: &["paper", "sell", "BTCUSD", "0.01"],
        },
        Parity {
            tool: "kraken_withdraw",
            arguments: || serde_json::json!({"asset": "XBT", "key": "cold-storage", "amount": "0.1"}),
            argv: &["withdraw", "XBT", "cold-storage", "0.1"],
        },
        Parity {
            tool: "kraken_workspace_create",
            arguments: || serde_json::json!({"name": "sandbox", "capital": "10000", "mode": "paper"}),
            argv: &[
                "workspace",
                "create",
                "sandbox",
                "--capital",
                "10000",
                "--mode",
                "paper",
            ],
        },
    ];

    /// The whole point: a tool call's arguments are the leaf's own fields, so
    /// reading them yields the command the CLI would have parsed — defaults,
    /// value enums and all.
    #[test]
    fn a_tool_call_becomes_the_command_the_cli_parses() {
        /// A root to parse against — the real one lives in the executing crate.
        #[derive(Parser)]
        struct Root {
            #[command(subcommand)]
            command: Command,
        }

        let surface = Surface::shared();
        for case in PARITY {
            let arguments = (case.arguments)();
            let from_tool = surface
                .command(
                    case.tool,
                    arguments.as_object().cloned().expect("an object"),
                )
                .unwrap_or_else(|why| panic!("{}: {why}", case.tool));

            let argv = std::iter::once("kraken").chain(case.argv.iter().copied());
            let from_cli = Root::try_parse_from(argv)
                .unwrap_or_else(|why| panic!("{}: the fixture must parse: {why}", case.tool))
                .command;

            assert_eq!(
                serde_json::to_value(&from_tool).expect("serialize"),
                serde_json::to_value(&from_cli).expect("serialize"),
                "{}: a tool call and a command line must reach the same command",
                case.tool
            );
        }
    }

    /// The anti-injection property: an argument the tool never advertised is
    /// refused, not dropped. Dropping one would let a renamed parameter default
    /// its way into a different account.
    #[test]
    fn an_unadvertised_argument_is_refused() {
        let surface = Surface::shared();
        let err = surface
            .command(
                "kraken_ticker",
                serde_json::json!({"pairs": ["BTCUSD"], "api_key": "leaked"})
                    .as_object()
                    .cloned()
                    .expect("an object"),
            )
            .expect_err("an unadvertised argument must be refused");
        assert!(
            matches!(err, CallError::UnknownArgument { .. }),
            "expected a refusal naming the argument, got {err}"
        );
    }

    /// An unknown tool name never reaches a command.
    #[test]
    fn an_unknown_tool_is_refused() {
        let surface = Surface::shared();
        let err = surface
            .command("kraken_teleport", Map::new())
            .expect_err("an unknown tool must be refused");
        assert!(matches!(err, CallError::UnknownTool(_)), "{err}");
    }

    /// The CLI spelling drops the groups clap flattens, so a tool an operator
    /// reads about is a command they can type.
    #[test]
    fn a_transparent_group_is_absent_from_the_cli_spelling() {
        let market = CommandPath::new(&["market"], "ticker");
        assert_eq!(market.to_string(), "ticker");
        assert_eq!(ToolName::from(market).to_string(), "kraken_ticker");

        let order = CommandPath::new(&["order"], "cancel-batch");
        assert_eq!(order.to_string(), "order cancel-batch");
        assert_eq!(
            ToolName::from(order).to_string(),
            "kraken_order_cancel_batch"
        );
    }

    /// Paper-safe cannot be declared without being dangerous: the variant that
    /// used to allow that combination does not exist.
    #[test]
    fn paper_safe_is_always_dangerous() {
        assert!(Effect::PaperSafe.is_dangerous());
        assert!(Effect::PaperSafe.is_paper_safe());
        assert!(!Effect::Local.is_dangerous());
        assert!(!Effect::LocalOrLive.is_dangerous());
        assert!(!Effect::ReadOnly.is_dangerous());
        assert!(!Effect::Dangerous.is_paper_safe());
    }
}

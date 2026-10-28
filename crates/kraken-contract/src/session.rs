//! Recorded windows over a scope's shared account journal.

use crate::{
    domain_values::{DecisionKind, Zone},
    subscription::{CHANNEL_OPTIONS_NOTE, SubscriptionOptions},
    value_types::SinkTarget,
};
use clap::Subcommand;
use kraken_core::SubscribableChannel;

/// Run lifecycle commands: `start` streams (handled by the interactive
/// dispatcher); everything else is request/response and routes through the
/// shared executor.
// One value exists per process, parsed once at startup, so `Start`'s size gap
// over the request verbs costs nothing worth boxing for — the same reasoning as
// `Command` and `StreamingCommand`.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SessionCommand {
    /// Open a session window on the account journal and record its market tape.
    #[command(after_help = CHANNEL_OPTIONS_NOTE)]
    Start(Start),

    #[command(flatten)]
    Request(SessionRequestCommand),
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Start {
    /// Comma-separated trading pairs to record (e.g. BTC/USD,ETH/USD);
    /// repeatable.
    #[arg(long, value_delimiter = ',')]
    #[serde(default)]
    pub symbols: Vec<String>,
    /// Comma-separated market-data channels to capture (ticker, trade, book,
    /// ohlc); repeatable.
    #[arg(long, value_delimiter = ',', ignore_case = true)]
    #[serde(default)]
    pub channels: Vec<SubscribableChannel>,
    /// Per-channel subscribe options, shared with `kraken record`.
    #[command(flatten)]
    #[serde(flatten)]
    pub options: SubscriptionOptions,
    /// Sink targets: any combination of duckdb, jsonl, stdout
    /// (comma-separated).
    ///
    /// `duckdb`/`jsonl` always write their native on-disk format;
    /// `-o`/`--output` (table|json) controls only the `stdout` echo, never
    /// the durable backends.
    #[arg(
        long,
        value_enum,
        value_delimiter = ',',
        default_value = "stdout,jsonl"
    )]
    #[serde(default = "stdout_and_jsonl")]
    pub to: Vec<SinkTarget>,
    /// Human handle for this session, resolvable anywhere a session ref is
    /// (`--session <label>`). Must be unique within the scope.
    #[arg(long)]
    pub label: Option<String>,
    /// Identifier of the driving template (e.g. a recipe skill name); recorded
    /// in session.json.
    ///
    /// The run hosts no strategy itself — this only labels the window so
    /// replay, explain-pnl, and the lab can attribute it to a hypothesis.
    #[arg(long)]
    pub strategy: Option<String>,
    /// Opaque JSON parameters for the strategy, recorded verbatim in
    /// session.json.
    ///
    /// Requires `--strategy`. A JSON object, e.g.
    /// `{"dollars_per_buy":100,"rounds":10}`.
    // Typed as the object it is, not as text holding one: a tool call then passes
    // the parameters directly instead of a string of escaped JSON, and the schema
    // says `object`. Not a doc comment — the reason is for whoever edits this
    // field, and the published description is for whoever calls it.
    #[arg(long, value_parser = parse_strategy_params)]
    pub strategy_params: Option<serde_json::Map<String, serde_json::Value>>,
    /// Lab experiment this session belongs to; stamped verbatim in session.json
    /// — the stamp is what `lab compare` discovers sessions by.
    #[arg(long)]
    pub experiment: Option<String>,
    /// Replay a finalized recorded tape instead of streaming the live market.
    ///
    /// Takes a ref from `kraken tape list` (`tape:<name>` or `session:s<n>`).
    /// The replayed frames are re-stamped to run wall time and re-recorded
    /// as the session's own tape, so trading, scoring, and explain work
    /// unchanged and stay REST-free.
    #[arg(long, conflicts_with_all = ["symbols", "channels"])]
    pub from: Option<String>,
    /// Playback speed for --from: tape seconds per wall second.
    #[arg(long, default_value_t = 1.0, requires = "from")]
    #[serde(default = "crate::value_types::real_time")]
    pub speed: f64,
    /// Auto-stop after this wall-clock window, then close the session
    /// (e.g. 20s, 5m, 1h, 24h). Without it, the session records until you
    /// `kraken session stop`. `kraken lab next` emits it for a plan's live
    /// entry.
    #[arg(long = "for")]
    pub window: Option<String>,
}

/// Request/response run commands, executable via the shared executor.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SessionRequestCommand {
    /// Stop a session's recorder, reconcile (real account), and close its
    /// window.
    ///
    /// Against the real account it first reconciles the journal from venue
    /// history — fills the mirror missed enter under their venue txids,
    /// idempotently — and adopts the venue balance snapshot. Idempotent on
    /// an already-stopped session.
    Stop(Stop),
    /// Show a session's window, status, and valuation.
    Show(Show),
    /// List the scope's sessions and their status.
    List(List),
    /// Append a decision (the "why") to the active session's decision log.
    Note(Note),
    /// Read a session's decision log verbatim — the round-gate's counter.
    ///
    /// The counter a looping agent gates its rounds on without knowing the
    /// on-disk layout. Offline; an empty log is count 0, never an error.
    Decisions(Decisions),
    /// The agent's durable cursor: read or replace a session's working state.
    /// A resumption aid, not history — deliberately outside the replay/scoring
    /// evidence, so a `set` never shows in `replay`/`explain pnl`; use
    /// `session note` to record a transition worth a post-mortem.
    #[command(subcommand)]
    State(StateCommand),
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Stop {
    /// Which session: `latest`, an ordinal (`s3`), or a label.
    #[arg(long, default_value = "latest")]
    #[serde(default = "crate::value_types::latest_session")]
    pub session: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Show {
    /// Which session: `latest`, an ordinal (`s3`), or a label.
    #[arg(long, default_value = "latest")]
    #[serde(default = "crate::value_types::latest_session")]
    pub session: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct List {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Note {
    /// What kind of decision this records.
    #[arg(long, value_enum)]
    pub kind: DecisionKind,
    /// The trading pair the decision concerns.
    #[arg(long)]
    pub symbol: Option<String>,
    /// Free-text rationale for the decision.
    #[arg(long)]
    pub reason: String,
    /// An order this decision references.
    #[arg(long)]
    pub order_id: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Decisions {
    /// Which session: `latest`, an ordinal (`s3`), or a label.
    #[arg(long, default_value = "latest")]
    #[serde(default = "crate::value_types::latest_session")]
    pub session: String,
}

/// `session state`: the typed cursor a long-running loop grounds on each
/// firing — get reads any session's cell, set replaces the ACTIVE one's.
/// A resumption aid, not history: it is deliberately outside the replay and
/// scoring evidence, so `set`s never appear in `replay`/`explain pnl`. The
/// decision log is the evidence channel — record a transition worth a
/// post-mortem with `session note`.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum StateCommand {
    /// Read a session's state cell (unset reads as an empty cursor).
    ///
    /// The durable cursor a long-running agent loop grounds on at each firing
    /// (round, legs done, zone, last action, note). A resumption aid, not
    /// history: deliberately outside the replay and scoring evidence, so it
    /// never appears in replay or `explain pnl`. An unset cell reads as an
    /// empty cursor with a null `updated_at`, never an error.
    Get(StateGet),
    /// Replace the active session's state cell with the given fields.
    ///
    /// Unset flags clear their cases. Refuses when no session is recording, and
    /// a mistyped field or zone refuses rather than steering the loop
    /// silently. This is a resumption cursor, not history — a transition
    /// worth a post-mortem belongs in `kraken session note`.
    Set(StateSet),
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct StateGet {
    /// Which session: `latest`, an ordinal (`s3`), or a label.
    #[arg(long, default_value = "latest")]
    #[serde(default = "crate::value_types::latest_session")]
    pub session: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct StateSet {
    /// The last COMPLETED round; a firing acts as round+1 and writes it back.
    #[arg(long)]
    pub round: Option<u32>,
    /// Pairs already executed in the round in flight (comma-separated).
    #[arg(long, value_delimiter = ',')]
    pub legs_done: Option<Vec<String>>,
    /// Price zone for crossing-based strategies.
    #[arg(long, value_enum)]
    pub zone: Option<Zone>,
    /// Instant of the loop's last order (RFC 3339).
    #[arg(long)]
    pub last_action_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Free-text context for a human reading the artifact.
    #[arg(long)]
    pub note: Option<String>,
}

impl Start {
    /// Whether this session settles its fills from a recorded tape rather than
    /// the live market. Read by [`kraken-cli's workspace guard`], which
    /// refuses replay against a live workspace's ledger.
    pub fn is_replay(&self) -> bool {
        self.from.is_some()
    }
}

impl Start {
    /// The `kraken playground` demo's session: default channels, the template
    /// as the strategy label, self-stopping after `window`. Sinks echo the
    /// live tape to stdout (the demo must be visibly alive) while JSONL keeps
    /// the durable record — replayable later via `--from session:s<n>`.
    pub fn demo(symbols: Vec<String>, template: String, window: String) -> Self {
        Self {
            symbols,
            channels: vec![
                kraken_core::SubscribableChannel::Ticker,
                kraken_core::SubscribableChannel::Trade,
            ],
            options: Default::default(),
            to: vec![SinkTarget::Stdout, SinkTarget::Jsonl],
            label: None,
            strategy: Some(template),
            strategy_params: None,
            experiment: None,
            from: None,
            speed: 1.0,
            window: Some(window),
        }
    }

    /// A recipe's session: the inverse of `recipe create` — the
    /// artifact supplies feeds, sinks, and the strategy stamp; the run flags
    /// supply the window, the experiment, and any replay source. On a replay,
    /// symbols and channels are declared from the tape, exactly as the clap
    /// path behaves.
    #[allow(clippy::too_many_arguments)] // A flat mirror of the artifact + run flags; a builder would only rename them.
    pub fn from_recipe(
        symbols: Vec<String>,
        channels: Vec<kraken_core::SubscribableChannel>,
        to: Vec<SinkTarget>,
        strategy: String,
        strategy_params: Option<serde_json::Map<String, serde_json::Value>>,
        experiment: Option<String>,
        from: Option<String>,
        speed: f64,
        window: Option<String>,
    ) -> Self {
        Self {
            symbols,
            channels,
            options: Default::default(),
            to,
            label: None,
            strategy: Some(strategy),
            strategy_params,
            experiment,
            from,
            speed,
            window,
        }
    }
}

/// A `--strategy-params` value, as the command line supplies it: the text of a
/// JSON object.
///
/// Object-only because the parameters are a named set — an array or a bare
/// number names nothing a strategy could read — and `serde`'s own message
/// already says which shape arrived.
fn parse_strategy_params(
    raw: &str,
) -> std::result::Result<serde_json::Map<String, serde_json::Value>, String> {
    serde_json::from_str(raw).map_err(|e| format!("must be a JSON object: {e}"))
}

/// Where a session's frames land when the caller names no sink: the terminal,
/// and a file the session can be replayed from.
fn stdout_and_jsonl() -> Vec<SinkTarget> {
    vec![SinkTarget::Stdout, SinkTarget::Jsonl]
}

crate::tools! {
    /// The `kraken session` commands.
    fn session in ["session"] of SessionCommand {
        Start as "start" => Workspace, Local, None, Stream
            example "kraken session start --symbols BTC/USD --channels trades --for 15m";
    }
}

crate::tools! {
    /// The `kraken session request` commands.
    fn session_request in ["session", "request"] of SessionRequestCommand {
        Stop      as "stop"      => Workspace, Local,    None, Tool
            example "kraken session stop -o json";
        Show      as "show"      => Workspace, ReadOnly, None, Tool
            example "kraken session show -o json";
        List      as "list"      => Workspace, ReadOnly, None, Tool
            example "kraken session list -o json";
        Note      as "note"      => Workspace, Local,    None, Tool
            example "kraken session note --kind skip --reason \"spread too wide\" -o json";
        Decisions as "decisions" => Workspace, ReadOnly, None, Tool
            example "kraken session decisions --session latest -o json";
    }
}

crate::tools! {
    /// The `kraken session request state` commands.
    fn session_request_state in ["session", "request", "state"] of StateCommand {
        StateGet as "get" => Workspace, ReadOnly, None, Tool
            example "kraken session state get -o json";
        StateSet as "set" => Workspace, Local,    None, Tool
            example "kraken session state set --round 3 --zone above -o json";
    }
}

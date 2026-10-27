//! Snapshot pins for the crate's one behavioral seam. The real tool surface and
//! a recording [`ToolHost`] drive the whole server — discovery, instructions,
//! gates, and the typed command that crosses the seam — with no binary and no
//! stdio transport. What crosses the seam (a `Command`) and what returns to the
//! client (schemas, results, refusals) are wire contracts; a diff here is a
//! deliberate contract change, reviewed via `cargo insta review`, never an
//! accident.
//!
//! There is no fixture catalog and no fixture clap tree: the surface is derived
//! from the command tree's own types, so a fixture would only be able to drift
//! from it. The discovery snapshots serialize `rmcp::model::Tool` directly, so
//! an rmcp version bump that changes Tool's JSON shape diffs them too — review
//! such a diff as a wire-contract change, never rubber-stamp it.

#![allow(clippy::unwrap_used, clippy::panic)]

use insta::{assert_json_snapshot, assert_snapshot};
use kraken_contract::{
    command::Command,
    tool::{Service, Surface},
};
use kraken_mcp::{
    ActiveWorkspace, Approval, GateMode, KrakenMcpServer, Operator, ServerConfig, ToolHost,
    ToolOutcome, ToolRegistry,
};
use rmcp::{handler::server::ServerHandler, model::CallToolRequestParams};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

/// Three groups, covering the three postures the gate distinguishes: read-only
/// (`market`), the mode-routed paper-safe relaxation (`trade`), and
/// always-dangerous (`funding`).
const SERVICES: &[Service] = &[Service::Market, Service::Trade, Service::Funding];

/// The tools these snapshots pin by name, so the file stays reviewable as the
/// surface grows.
const PINNED: &[&str] = &["kraken_ticker", "kraken_order_buy", "kraken_withdraw"];

fn registry(allow_dangerous: bool, mode: GateMode) -> ToolRegistry {
    ToolRegistry::build(Surface::shared(), SERVICES, allow_dangerous, mode).unwrap()
}

fn pinned_definitions(registry: &ToolRegistry) -> Vec<rmcp::model::Tool> {
    let pinned: Vec<rmcp::model::Tool> = registry
        .tool_definitions()
        .into_iter()
        .filter(|tool| PINNED.contains(&tool.name.as_ref()))
        .collect();
    assert_eq!(
        pinned.len(),
        PINNED.len(),
        "every pinned tool must be registered: {PINNED:?}"
    );
    pinned
}

/// The whole seam, fake: records every command the server hands across and
/// answers with a canned payload.
#[derive(Clone)]
struct RecordingHost {
    mode: GateMode,
    reply: Value,
    calls: Arc<Mutex<Vec<Value>>>,
    /// The group the seam filed each command under, so a test can pin that the
    /// host is told — it decides on this whether to serialize the call.
    groups: Arc<Mutex<Vec<Service>>>,
}

impl RecordingHost {
    fn new(mode: GateMode, reply: Value) -> Self {
        Self {
            mode,
            reply,
            calls: Arc::new(Mutex::new(Vec::new())),
            groups: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The commands that crossed the seam, as their wire form — the shape a
    /// policy engine would judge.
    fn recorded(&self) -> Vec<Value> {
        self.calls.lock().unwrap().clone()
    }

    /// The groups the seam declared alongside them.
    fn groups(&self) -> Vec<Service> {
        self.groups.lock().unwrap().clone()
    }
}

impl ToolHost for RecordingHost {
    async fn execute(&self, command: Command, group: Service) -> ToolOutcome {
        self.calls
            .lock()
            .unwrap()
            .push(serde_json::to_value(&command).unwrap());
        self.groups.lock().unwrap().push(group);
        ToolOutcome::Success(self.reply.clone())
    }

    fn gate_mode(&self) -> GateMode {
        self.mode
    }
}

fn server(
    allow_dangerous: bool,
    workspace: Option<ActiveWorkspace>,
    host: RecordingHost,
) -> KrakenMcpServer<RecordingHost> {
    let registry = registry(allow_dangerous, host.gate_mode());
    KrakenMcpServer::new(
        registry,
        host,
        ServerConfig {
            allow_dangerous,
            active_services: SERVICES.to_vec(),
            workspace,
            server_version: "0.0.0-test".into(),
            agent: "insta".into(),
            instance_id: "fixed-instance".into(),
        },
    )
}

/// The operator, faked. [`Approval::Unreachable`] is the shape of a client that
/// declared no elicitation capability, which is what every snapshot below pins:
/// the gate then falls back to the `acknowledged` argument, and that fallback's
/// refusal wording is the wire contract.
struct Answering(Approval);

impl Operator for Answering {
    async fn approve(&self, _what: &str) -> Approval {
        self.0
    }
}

/// A client that can reach nobody.
const NO_OPERATOR: Answering = Answering(Approval::Unreachable);

fn call(name: &'static str, arguments: Value) -> CallToolRequestParams {
    let mut request = CallToolRequestParams::new(name);
    request.arguments = match arguments {
        Value::Object(map) => Some(map),
        Value::Null => None,
        other => panic!("fixture arguments must be an object or null, got {other}"),
    };
    request
}

#[test]
fn discovery_pins_the_advertised_tool_contract() {
    assert_json_snapshot!(pinned_definitions(&registry(false, GateMode::Master)));
}

#[test]
fn discovery_relaxes_paper_safe_tools_inside_a_paper_workspace() {
    assert_json_snapshot!(pinned_definitions(&registry(false, GateMode::Paper)));
}

#[test]
fn instructions_pin_the_agent_facing_briefing() {
    let host = RecordingHost::new(GateMode::Paper, Value::Null);
    let server = server(
        false,
        Some(ActiveWorkspace::Paper {
            name: "sandbox".into(),
        }),
        host,
    );
    assert_snapshot!(server.get_info().instructions.unwrap());
}

/// The seam carries the same typed command the CLI parses — no argv, no second
/// parse. The snapshot is that command's wire form.
#[tokio::test]
async fn dispatch_hands_a_typed_command_across_the_seam() {
    let host = RecordingHost::new(GateMode::Master, json!({"BTC/USD": {"last": "50000.0"}}));
    let server = server(false, None, host.clone());

    let result = server
        .dispatch(
            call(
                "kraken_ticker",
                json!({"pairs": ["BTC/USD", "ETH/USD"], "asset_class": "forex"}),
            ),
            &NO_OPERATOR,
        )
        .await
        .expect("read-only tool dispatches ungated");

    assert_json_snapshot!(json!({
        "command": host.recorded(),
        "result": result,
    }));
}

#[tokio::test]
async fn dispatch_refuses_a_dangerous_tool_before_the_seam() {
    let host = RecordingHost::new(GateMode::Master, Value::Null);
    let server = server(false, None, host.clone());

    let refusal = server
        .dispatch(
            call(
                "kraken_withdraw",
                json!({"asset": "BTC", "key": "cold-storage", "amount": "0.1"}),
            ),
            &NO_OPERATOR,
        )
        .await
        .expect_err("unacknowledged withdraw is refused");

    assert!(
        host.recorded().is_empty(),
        "a gate refusal must never cross the seam"
    );
    assert_json_snapshot!(refusal);
}

/// The whole reason the operator seam exists: a client that can reach a person
/// asks them, and their answer — not the model's `acknowledged` — decides.
#[tokio::test]
async fn an_approved_dangerous_call_crosses_the_seam() {
    let host = RecordingHost::new(GateMode::Master, json!({"refid": "FT4567"}));
    let server = server(false, None, host.clone());
    let withdraw = || {
        call(
            "kraken_withdraw",
            json!({"asset": "BTC", "key": "cold-storage", "amount": "0.1"}),
        )
    };

    server
        .dispatch(withdraw(), &Answering(Approval::Granted))
        .await
        .expect("an approved withdrawal executes");
    assert_eq!(host.recorded().len(), 1, "the approved call crossed");

    server
        .dispatch(withdraw(), &Answering(Approval::Refused))
        .await
        .expect_err("a refused withdrawal does not");
    assert_eq!(
        host.recorded().len(),
        1,
        "a refusal must never cross the seam"
    );
}

#[tokio::test]
async fn dispatch_refuses_unknown_arguments_before_the_seam() {
    let host = RecordingHost::new(GateMode::Master, Value::Null);
    let server = server(false, None, host.clone());

    let refusal = server
        .dispatch(
            call(
                "kraken_ticker",
                json!({"pairs": ["BTC/USD"], "api_key": "injected"}),
            ),
            &NO_OPERATOR,
        )
        .await
        .expect_err("arguments outside the schema are refused, not dropped");

    assert!(
        host.recorded().is_empty(),
        "an argument refusal must never cross the seam"
    );
    assert_json_snapshot!(refusal);
}

/// A shape the command line refuses is refused here too, before the seam: both
/// `pairs` and `--all` is a contradiction, and answering it with every tradable
/// pair is the wrong answer to the wrong question.
#[tokio::test]
async fn dispatch_refuses_a_contradictory_shape_before_the_seam() {
    let host = RecordingHost::new(GateMode::Master, Value::Null);
    let server = server(false, None, host.clone());

    let refusal = server
        .dispatch(
            call("kraken_ticker", json!({"pairs": ["BTC/USD"], "all": true})),
            &NO_OPERATOR,
        )
        .await
        .expect_err("`pairs` and `all` are mutually exclusive");

    assert!(
        host.recorded().is_empty(),
        "a shape refusal must never cross the seam"
    );
    assert_json_snapshot!(refusal);
}

/// The host is told which group a command belongs to, because that is what lets
/// it serialize only the calls that open local state: a market read must not
/// queue behind a paper fill.
#[tokio::test]
async fn the_seam_declares_the_group_of_each_command() {
    let host = RecordingHost::new(GateMode::Paper, json!({}));
    let server = server(false, None, host.clone());

    server
        .dispatch(
            call("kraken_ticker", json!({"pairs": ["BTC/USD"]})),
            &NO_OPERATOR,
        )
        .await
        .expect("a read-only tool dispatches ungated");
    server
        .dispatch(
            call(
                "kraken_order_buy",
                json!({"pair": "BTC/USD", "volume": "0.01"}),
            ),
            &NO_OPERATOR,
        )
        .await
        .expect("paper-safe inside a paper workspace needs no acknowledgment");

    assert_eq!(host.groups(), vec![Service::Market, Service::Trade]);
}

/// A result too large to hand a model is refused with the arguments that narrow
/// it. One `--all` sweep is 1.6 MB of quotes, which displaces whatever the
/// agent was doing with a payload it did not ask for.
#[tokio::test]
async fn an_oversized_result_is_refused_with_the_way_to_narrow_it() {
    let bulky: Value = (0..40_000)
        .map(|i| (format!("PAIR{i}"), json!({"last": "50000.0"})))
        .collect::<serde_json::Map<_, _>>()
        .into();
    let server = server(false, None, RecordingHost::new(GateMode::Master, bulky));

    let result = server
        .dispatch(call("kraken_ticker", json!({"all": true})), &NO_OPERATOR)
        .await
        .expect("an oversized result is a tool error, not a protocol error");

    assert_eq!(result.is_error, Some(true));
    let envelope = result.structured_content.expect("a structured refusal");
    assert_eq!(envelope["error"], "validation");
    let message = envelope["message"].as_str().expect("a message");
    assert!(
        message.contains("drop `all`"),
        "the advice names what this call asked for: {message}"
    );
}

/// rmcp writes the payload into `content` as well as `structuredContent`.
/// Beyond a few KiB that doubles what crosses the wire and what the model
/// reads, for a copy the spec only recommends.
#[tokio::test]
async fn a_large_result_is_not_mirrored_as_text() {
    let sizeable: Value = (0..2_000)
        .map(|i| (format!("PAIR{i}"), json!({"last": "50000.0"})))
        .collect::<serde_json::Map<_, _>>()
        .into();
    let server = server(false, None, RecordingHost::new(GateMode::Master, sizeable));

    let result = server
        .dispatch(call("kraken_ticker", json!({"all": true})), &NO_OPERATOR)
        .await
        .expect("a large but permitted result is returned");

    assert!(result.structured_content.is_some(), "the payload is served");
    assert!(
        result.content.is_empty(),
        "a large payload is not duplicated as text"
    );
}

/// A small result keeps the text copy: a client that reads no structured
/// content still gets the payload.
#[tokio::test]
async fn a_small_result_keeps_its_text_copy() {
    let server = server(
        false,
        None,
        RecordingHost::new(GateMode::Master, json!({"BTC/USD": {"last": "50000.0"}})),
    );

    let result = server
        .dispatch(
            call("kraken_ticker", json!({"pairs": ["BTC/USD"]})),
            &NO_OPERATOR,
        )
        .await
        .expect("a small result is returned");

    assert_eq!(result.content.len(), 1, "the text fallback is kept");
}

/// MCP types `structuredContent` as an object, and a strict client rejects the
/// whole result otherwise. `deposit-status` answers with an array, which the
/// text copy keeps as `-o json` prints it.
#[tokio::test]
async fn an_array_result_is_wrapped_in_structured_content_but_not_in_text() {
    let statuses = json!([{"method": "Bitcoin", "status": "Success"}]);
    let server = server(
        false,
        None,
        RecordingHost::new(GateMode::Master, statuses.clone()),
    );

    let result = server
        .dispatch(call("kraken_deposit_status", json!({})), &NO_OPERATOR)
        .await
        .expect("an array result is returned");

    assert_eq!(
        result.structured_content,
        Some(json!({"result": statuses.clone()}))
    );
    let text = result.content[0].as_text().expect("a text copy");
    assert_eq!(serde_json::from_str::<Value>(&text.text).unwrap(), statuses);
}

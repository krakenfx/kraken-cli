//! MCP server (rmcp, stdio). A tool call's arguments are the leaf's own fields,
//! so it becomes a typed `Command` here and crosses the [`ToolHost`] seam as
//! one — the same value the CLI's parser produces, with nothing in between.

use crate::{
    error::{McpError, Result},
    registry::{GateMode, ToolRegistry},
};
use kraken_contract::{
    command::Command,
    domain_values::WorkspaceMode,
    tool::{Effect, Service},
};
use rmcp::{
    handler::server::ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData,
        Implementation, InitializeRequestParams, InitializeResult, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities,
    },
    service::{RequestContext, RoleServer},
};
use serde::Serialize;
use serde_json::Value;
use std::{collections::BTreeMap, future::Future, sync::Once, time::Instant};

/// The execution side of the MCP seam: the host executes one typed command;
/// this crate owns discovery, gating, and audit.
///
/// The seam carries a `Command`, not argv, so a malformed call cannot reach the
/// host at all — it fails while its arguments are still being read.
pub trait ToolHost: Send + Sync + 'static {
    /// Execute one command through the host's dispatch path.
    ///
    /// `group` is the service group the contract files the command under. The
    /// host needs it to schedule: the local state a call opens follows its
    /// group, and a host that serializes access to that state has no other
    /// way to tell which calls contend.
    fn execute(&self, command: Command, group: Service)
    -> impl Future<Output = ToolOutcome> + Send;

    /// The gate the next command executes under, resolved now — a workspace
    /// promoted while the server runs must not keep a stale paper relaxation
    /// handed out at discovery.
    fn gate_mode(&self) -> GateMode;
}

/// Who answers for a call that moves money.
///
/// `"acknowledged": true` in the arguments is the model's own word, and a model
/// that reads the refusal learns to send it — so on its own it attests to
/// nothing a person decided. A client that declared MCP elicitation can reach
/// the operator instead, and where it can, that answer is the only one this
/// gate accepts.
///
/// One method, because asking a person is the single thing the two front-ends
/// cannot share: the CLI has a terminal to prompt on, a server has a protocol
/// round-trip. A seam rather than a direct rmcp call so the gate stays testable
/// without a transport.
pub trait Operator: Send + Sync {
    /// Put `what` to the operator and wait for their answer.
    fn approve(&self, what: &str) -> impl Future<Output = Approval> + Send;
}

/// What came back when the operator was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    Granted,
    /// Declined, cancelled, or never answered — all of which are a no.
    Refused,
    /// There is nobody to ask: this client declared no way to reach a person.
    /// The `acknowledged` argument is the only assurance it can offer, so that
    /// is the case the gate falls back to.
    Unreachable,
}

/// What became of one tool invocation on the host side.
#[derive(Debug)]
pub enum ToolOutcome {
    /// The command executed; the structured success payload.
    Success(Value),
    /// The command executed and failed; the host's JSON error envelope,
    /// returned to the client as a structured tool error.
    Failure(Value),
}

/// The workspace the host resolved at startup, for the instructions banner.
/// Discovery-time only: per-call gating re-resolves through
/// [`ToolHost::gate_mode`].
#[derive(Debug, Clone)]
pub enum ActiveWorkspace {
    /// A paper workspace: simulated fills, unprompted order tools.
    Paper { name: String },
    /// Any non-paper mode, routed at the real account. `mode` is the host's
    /// display label (e.g. `live`).
    Live { name: String, mode: String },
}

/// Host-supplied identity and mode for one server process.
#[derive(Debug)]
pub struct ServerConfig {
    pub allow_dangerous: bool,
    pub active_services: Vec<Service>,
    pub workspace: Option<ActiveWorkspace>,
    /// The host binary's version, advertised in the handshake and stamped on
    /// audit lines — this crate's own version is not the product's.
    pub server_version: String,
    /// Agent-client label for the audit trail (stdio has no source address).
    pub agent: String,
    /// Install-instance identifier for the audit trail.
    pub instance_id: String,
}

pub struct KrakenMcpServer<H> {
    registry: ToolRegistry,
    host: H,
    config: ServerConfig,
    instructions: String,
    /// Gates the session-start audit event to exactly one emission, whichever
    /// handler opens the session. rmcp refuses a request before `initialize`,
    /// so in practice `initialize` is always first — the guard costs
    /// nothing and keeps the audit invariant ("tool activity is preceded by
    /// a session_start record") true if that ever stops holding.
    session_audited: Once,
}

impl<H: ToolHost> KrakenMcpServer<H> {
    pub fn new(registry: ToolRegistry, host: H, config: ServerConfig) -> Self {
        let instructions = build_instructions(
            &config.active_services,
            config.allow_dangerous,
            config.workspace.as_ref(),
        );
        Self {
            registry,
            host,
            config,
            instructions,
            session_audited: Once::new(),
        }
    }

    /// Emit the session-start audit event, once per session (stdio: one
    /// process, one session). Called from every handler an agent can open a
    /// session through, so the invariant "tool activity is preceded by a
    /// session_start record" holds whichever handler opens it.
    fn audit_session_start(&self) {
        self.session_audited.call_once(|| {
            audit_log(AuditEvent::SessionStart {
                server_version: &self.config.server_version,
                agent: &self.config.agent,
                instance_id: &self.config.instance_id,
                pid: std::process::id(),
            });
        });
    }

    /// The transport-free tool-call entry: gate, read the arguments into a
    /// command, execute it through the [`ToolHost`] seam.
    /// [`ServerHandler::call_tool`] wraps this with the audit trail; tests
    /// drive it directly to pin the seam contract without a stdio
    /// transport.
    ///
    /// `operator` is who answers for a call that moves money — see
    /// [`Operator`].
    pub async fn dispatch(
        &self,
        request: CallToolRequestParams,
        operator: &impl Operator,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let tool_name = &request.name;
        let entry = self
            .registry
            .get_by_name(tool_name)
            .ok_or_else(|| ErrorData::invalid_params(format!("Unknown tool: {tool_name}"), None))?;

        let mut arguments = request.arguments.unwrap_or_default();

        // Re-derive the gate from the host as it is NOW. A workspace promoted
        // to live after startup must not keep the paper relaxation that tool
        // discovery handed out: `entry.armed` is a cached snapshot, this is
        // the authority.
        let armed = entry.armed_under(self.host.gate_mode());
        if let Some(gated) = (!self.config.allow_dangerous)
            .then(|| gated(armed, entry.effect(), &arguments))
            .flatten()
        {
            let request = approval_request(entry.command(), &arguments, gated);
            enforce_approval(&request, &arguments, operator, gated).await?;
        }

        // The arguments are the leaf's own fields, so reading them yields the
        // same typed command the CLI parses. `acknowledged` is this server's
        // gate, not the command's, and does not cross the seam.
        arguments.remove("acknowledged");
        let asked = arguments.clone();
        let command = entry
            .call(arguments)
            .map_err(|why| ErrorData::invalid_params(why.to_string(), None))?;

        // Return the payload as MCP structured content: clients that support it get
        // typed JSON, and rmcp includes a text fallback for those that don't.
        match self.host.execute(command, entry.group()).await {
            ToolOutcome::Success(data) => Ok(bounded(data, entry, &asked)),
            ToolOutcome::Failure(envelope) => Ok(CallToolResult::structured_error(envelope)),
        }
    }
}

/// The most JSON one tool call may return.
///
/// A tool result is context: `ticker --all` is 1.6 MB of live quotes, which
/// costs an agent several hundred thousand tokens and displaces everything it
/// was doing. A refusal that names the argument to narrow is worth more than
/// the payload.
const MAX_PAYLOAD_BYTES: usize = 256 * 1024;

/// Above this, the `content` text mirror is dropped and only
/// `structuredContent` is returned. rmcp writes the payload into both, so a
/// large result otherwise crosses the wire — and into the model — twice; the
/// spec makes the text copy a SHOULD, and a client reading structured content
/// gains nothing from it.
const MAX_MIRRORED_BYTES: usize = 16 * 1024;

/// One successful payload, held to what the seam will carry.
fn bounded(
    data: Value,
    entry: &crate::ToolEntry,
    asked: &serde_json::Map<String, Value>,
) -> CallToolResult {
    // Rendered once, because the payload's size on the wire is the length of its
    // JSON and a `Value`'s in-memory size says nothing about it. Under the mirror
    // limit that same string *is* the text copy, so the text stays the command's
    // own output, exactly as `-o json` prints it.
    let Ok(rendered) = serde_json::to_string(&data) else {
        // Unserializable is not oversized: let the ordinary path report it.
        return CallToolResult::structured(Value::Object(structured_object(data)));
    };
    let size = rendered.len();
    if size > MAX_PAYLOAD_BYTES {
        tracing::warn!(
            tool = entry.name(),
            bytes = size,
            limit = MAX_PAYLOAD_BYTES,
            "refused an oversized tool result"
        );
        return CallToolResult::structured_error(oversize_envelope(size, entry, asked));
    }

    let mirror = if size > MAX_MIRRORED_BYTES {
        Vec::new()
    } else {
        vec![ContentBlock::text(rendered)]
    };
    let mut result = CallToolResult::success(mirror);
    result.structured_content = Some(Value::Object(structured_object(data)));
    result
}

/// Spec versions before 2026-07-28 type `structuredContent` as an object
/// ([schema](https://modelcontextprotocol.io/specification/2025-06-18/schema#calltoolresult))
/// and strict clients reject anything else, so an array or scalar payload
/// (`deposit-status`) is served under `result` for every version alike.
fn structured_object(data: Value) -> serde_json::Map<String, Value> {
    match data {
        Value::Object(object) => object,
        other => serde_json::Map::from_iter([("result".to_owned(), other)]),
    }
}

/// The refusal for a payload too large to return: the category a client routes
/// on, the numbers, and how to ask the same question smaller.
///
/// The advice is built from what the call actually asked for. A sweep argument
/// the caller set is the thing to drop; the narrowing arguments it left unset
/// are the ones to reach for.
fn oversize_envelope(
    size: usize,
    entry: &crate::ToolEntry,
    asked: &serde_json::Map<String, Value>,
) -> Value {
    let mut advice = Vec::new();
    for sweep in SWEEP_ARGUMENTS {
        if asked.get(*sweep).and_then(Value::as_bool).unwrap_or(false) {
            advice.push(format!("drop `{sweep}` and name only what you need"));
        }
    }
    let unset: Vec<&str> = entry
        .narrowing_arguments()
        .into_iter()
        .filter(|name| !asked.contains_key(*name))
        .collect();
    if !unset.is_empty() {
        advice.push(format!("narrow it with `{}`", unset.join("`, `")));
    }
    if advice.is_empty() {
        advice.push("ask for a narrower slice of the same data".to_owned());
    }

    serde_json::json!({
        "error": "validation",
        "message": format!(
            "This call returned {} KiB, over the {} KiB a tool result may carry — {}.",
            size / 1024,
            MAX_PAYLOAD_BYTES / 1024,
            advice.join("; "),
        ),
        "bytes": size,
        "limit_bytes": MAX_PAYLOAD_BYTES,
    })
}

/// Arguments that widen a call to everything the venue has. Dropping one is the
/// first thing to suggest when a result will not fit.
const SWEEP_ARGUMENTS: &[&str] = &["all"];

fn build_instructions(
    active_services: &[Service],
    allow_dangerous: bool,
    workspace: Option<&ActiveWorkspace>,
) -> String {
    let svc_list = active_services
        .iter()
        .map(|service| service.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let mode = if allow_dangerous {
        "autonomous"
    } else {
        "guarded"
    };

    let mut text = format!("Kraken exchange CLI tools. Active services: {svc_list}. Mode: {mode}.");

    // The resolved execution context, so an agent never has to guess which
    // account its order tools address.
    match workspace {
        Some(ActiveWorkspace::Paper { name }) => {
            text.push_str(&format!(
                " Workspace: {name} (mode: paper) — a simulated account; order tools execute \
                 paper fills and run without acknowledgment."
            ));
        }
        Some(ActiveWorkspace::Live { name, mode }) => {
            text.push_str(&format!(
                " Workspace: {name} (mode: {mode}) — treated as the real account."
            ));
        }
        None => {
            text.push_str(" No workspace: tools address the real Kraken account.");
        }
    }

    let missing: Vec<String> = crate::services::exposable()
        .filter(|service| !active_services.contains(service))
        .map(|service| service.to_string())
        .collect();

    if !missing.is_empty() {
        text.push_str(&format!(
            " Services not loaded: {}. To enable them, the user must update their MCP client \
             config to: {{\"command\": \"kraken-mcp\", \"args\": [\"--services\", \"all\"]}} and \
             restart the MCP connection.",
            missing.join(", "),
        ));
    }

    if !allow_dangerous {
        text.push_str(
            " Dangerous tools (orders, withdrawals, cancellations, and workspace_create with mode \
             \"live\") are put to the user for approval through MCP elicitation. Clients that \
             cannot present one accept \"acknowledged\": true in arguments instead. To run \
             without per-call confirmation, the user must add \"--allow-dangerous\" to args in \
             their MCP client config and restart.",
        );
    }

    text
}

// --- MCP audit logging ---
//
// Emits one JSON object per line (JSONL) to stderr for every tool invocation,
// namespaced under an `mcp_audit` key so consumers can select audit records
// (`jq 'select(.mcp_audit)'`) while the stream stays valid JSONL. MCP uses
// stdio transport, so there is no TCP source address; the trail identifies
// callers by agent client, instance ID, and PID.
//
// Argument *keys* are always recorded. Values are logged only for an allowlist
// of financial parameters (amounts, assets, pairs, destinations, order
// prices/sizes, …). Credentials and other non-financial fields never have
// their values written -- even if somehow present on the call.
//
// Boundary: requests rmcp rejects before dispatch (unsupported per-request
// protocol version, missing 2026-07-28 request metadata) never reach these
// handlers and therefore leave no tool_call record.

const MCP_TRANSPORT: &str = "stdio";

/// Argument ids whose values are safe and useful to retain on `tool_call`
/// audit lines. Keys are compared after normalizing `--flag-name` →
/// `flag_name`.
const FINANCIAL_AUDIT_ARGS: &[&str] = &[
    "address",
    "amount",
    "asset",
    "asset_class",
    "capital",
    "cash_order_qty",
    "close_ordertype",
    "close_price",
    "close_price2",
    "currency",
    "display_qty",
    "displayvol",
    "fee_preference",
    "fee_rate",
    "from",
    "from_account",
    "key", // withdrawal address key name (not an API secret)
    "leverage",
    "limit_price",
    "margin",
    "max_fee",
    "max_leverage",
    "oflags",
    "order_id",
    "order_qty",
    "order_type",
    "pair",
    "pairs",
    "preference",
    "price",
    "price2",
    "rebase_multiplier",
    "reduce_only",
    "refid",
    "side",
    "size",
    "slippage_rate",
    "stop_price",
    "stp_type",
    "stptype",
    "symbol",
    "time_in_force",
    "timeinforce",
    "to",
    "to_account",
    "trailing_stop_deviation_unit",
    "trailing_stop_max_deviation",
    "trigger",
    "trigger_price",
    "trigger_price_type",
    "trigger_reference",
    "trigger_signal",
    "txid",
    "type",
    "unit",
    "volume",
];

/// One line of the MCP audit stream: a single [`AuditRecord`] namespaced under
/// an `mcp_audit` key.
#[derive(Serialize)]
struct AuditLine<'a> {
    mcp_audit: AuditRecord<'a>,
}

/// The envelope every audit record shares — a timestamp and the transport —
/// with the [`AuditEvent`] flattened so its fields sit alongside them.
#[derive(Serialize)]
struct AuditRecord<'a> {
    ts: String,
    transport: &'static str,
    #[serde(flatten)]
    event: AuditEvent<'a>,
}

/// The MCP audit events. `#[serde(tag = "event")]` writes the variant name into
/// an `event` field (`session_start`, `tool_call`, …).
#[derive(Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum AuditEvent<'a> {
    /// An MCP session was opened: client `initialize`, or the first request
    /// of an initialize-less session (2026-07-28 inline lifecycle).
    SessionStart {
        server_version: &'a str,
        agent: &'a str,
        instance_id: &'a str,
        pid: u32,
    },
    /// The server process started and is about to serve on stdio.
    ServerStart {
        server_version: &'a str,
        tool_count: usize,
        mode: &'static str,
        pid: u32,
    },
    /// A tool invocation was received.
    ToolCall {
        tool: &'a str,
        /// Active workspace trading mode at call time: `"paper"` or `"live"`.
        /// Re-resolved per invocation so a mid-session promote is visible.
        trading_mode: &'static str,
        /// True when fills are simulated (paper workspace); false for the real
        /// account.
        paper: bool,
        arg_keys: Vec<String>,
        arg_count: usize,
        /// Allowlisted financial argument values (amounts, assets, pairs, …).
        /// Omitted from JSON when empty so read-only calls stay compact.
        #[serde(skip_serializing_if = "BTreeMap::is_empty")]
        financial: BTreeMap<String, Value>,
        agent: &'a str,
        instance_id: &'a str,
        pid: u32,
    },
    /// A tool invocation completed (executed or rejected).
    ToolResult {
        tool: &'a str,
        /// Same resolution as [`AuditEvent::ToolCall::trading_mode`].
        trading_mode: &'static str,
        paper: bool,
        status: &'static str,
        error_code: Option<String>,
        duration_ms: u64,
    },
}

/// Render a wrapped audit line as a single JSONL string, or `None` if it cannot
/// be serialized. Split out from [`audit_log`] so the JSONL contract can be
/// asserted without capturing stderr.
fn audit_line(line: &AuditLine<'_>) -> Option<String> {
    match serde_json::to_string(line) {
        Ok(rendered) => Some(rendered),
        Err(e) => {
            tracing::warn!(error = %e, "failed to serialize MCP audit line");
            None
        }
    }
}

/// Emit one audit event to stderr as an JSONL line, stamped with the current
/// time and transport. Always-on: written directly rather than through
/// `tracing`, whose default filter would drop it below the warning level.
fn audit_log(event: AuditEvent<'_>) {
    let line = AuditLine {
        mcp_audit: AuditRecord {
            ts: now_iso(),
            transport: MCP_TRANSPORT,
            event,
        },
    };
    if let Some(rendered) = audit_line(&line) {
        eprintln!("{rendered}");
    }
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn argument_keys(args: Option<&serde_json::Map<String, Value>>) -> Vec<String> {
    let Some(args) = args else {
        return Vec::new();
    };
    let mut keys: Vec<String> = args.keys().cloned().collect();
    keys.sort();
    keys
}

/// Normalize an MCP / clap argument id for allowlist checks: strip a leading
/// `--` and map hyphens to underscores (`--max-fee` → `max_fee`).
fn normalize_arg_id(raw: &str) -> String {
    raw.trim()
        .trim_start_matches("--")
        .replace('-', "_")
        .to_ascii_lowercase()
}

/// Whether an already-normalized argument id may have its value audited.
fn is_financial_audit_arg(id: &str) -> bool {
    // Defense in depth: never treat a credential or transport override as
    // financial, even if someone expands the allowlist carelessly. No tool
    // schema names these — they are global CLI flags, not command fields, so a
    // call carrying one is already refused — but the audit trail must not print
    // a secret even if that ever changes.
    if NEVER_AUDITED.contains(&id) {
        return false;
    }
    FINANCIAL_AUDIT_ARGS.binary_search(&id).is_ok()
}

/// Argument names whose values must never reach the audit trail.
const NEVER_AUDITED: &[&str] = &[
    "api_key",
    "api_secret",
    "api_secret_file",
    "api_secret_stdin",
    "api_url",
    "futures_api_key",
    "futures_api_secret",
    "futures_api_secret_file",
    "futures_api_secret_stdin",
    "futures_url",
    "otp",
];

/// Collect allowlisted financial argument values for the MCP audit trail.
/// Non-allowlisted keys (including secrets) contribute nothing; nulls are
/// dropped.
fn financial_arguments(args: Option<&serde_json::Map<String, Value>>) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    let Some(args) = args else {
        return out;
    };
    for (key, value) in args {
        if value.is_null() {
            continue;
        }
        // Normalized once and reused as the key: the allowlist check and the
        // recorded name must be the same string.
        let id = normalize_arg_id(key);
        if is_financial_audit_arg(&id) {
            out.insert(id, value.clone());
        }
    }
    out
}

fn audit_error_code(error: &ErrorData) -> String {
    format!("{:?}", error.code)
}

impl<H: ToolHost> ServerHandler for KrakenMcpServer<H> {
    fn get_info(&self) -> InitializeResult {
        // Single source of the server identity: the SDK serves it from both the
        // `initialize` handshake and the 2026-07-28 `discover` flow.
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("kraken-cli", self.config.server_version.clone())
                    .with_description(
                        "Kraken exchange CLI tools. Use service filtering to control which \
                         command groups are available.",
                    ),
            )
            .with_instructions(&self.instructions)
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<InitializeResult, ErrorData> {
        self.audit_session_start();

        // Overridden only for the audit event above. That replaces the SDK's
        // default handler, whose peer-info capture and version negotiation
        // (`negotiate_protocol_version`, crate-private) are reproduced here:
        // echo a supported client version, otherwise answer with the server
        // default and let the client decide whether to proceed.
        context.peer.set_peer_info(request.clone());
        let mut info = self.get_info();
        if self
            .supported_protocol_versions()
            .contains(&request.protocol_version)
        {
            info.protocol_version = request.protocol_version;
        } else {
            tracing::warn!(
                client_requested = %request.protocol_version,
                server_fallback = %info.protocol_version,
                "client requested unsupported protocol version"
            );
        }
        Ok(info)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, ErrorData> {
        self.audit_session_start();
        // `ttl_ms` and `cache_scope` are optional in rmcp but *required* of a
        // paginated result by spec 2026-07-28 (SEP-2549), which `server/discover`
        // advertises support for. rmcp leaves them unset, so a client that picks
        // that version rejects the whole result and the server ends up connected
        // with zero tools. Declared here until rmcp fills them in itself.
        //
        // Not cacheable: the surface is derived per process, and the armed set
        // depends on the workspace's mode as it is now.
        Ok(ListToolsResult {
            tools: self.registry.tool_definitions(),
            ttl_ms: Some(0),
            cache_scope: Some(rmcp::model::CacheScope::Private),
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, ErrorData> {
        self.audit_session_start();
        let tool_name = request.name.to_string();
        let started = Instant::now();
        let arg_keys = argument_keys(request.arguments.as_ref());
        let arg_count = arg_keys.len();
        let financial = financial_arguments(request.arguments.as_ref());
        let (trading_mode, paper) = audit_trading_mode(self.host.gate_mode());

        audit_log(AuditEvent::ToolCall {
            tool: &tool_name,
            trading_mode,
            paper,
            arg_keys,
            arg_count,
            financial,
            agent: &self.config.agent,
            instance_id: &self.config.instance_id,
            pid: std::process::id(),
        });

        let result = self
            .dispatch(
                request,
                &ClientOperator {
                    peer: &context.peer,
                },
            )
            .await;

        let (status, error_code) = match &result {
            Ok(_) => ("executed", None),
            Err(e) => ("rejected", Some(audit_error_code(e))),
        };
        // Re-resolve after dispatch: a promote/reset during the call must show
        // on the result line, not only the pre-call snapshot.
        let (trading_mode, paper) = audit_trading_mode(self.host.gate_mode());

        audit_log(AuditEvent::ToolResult {
            tool: &tool_name,
            trading_mode,
            paper,
            status,
            error_code,
            duration_ms: started.elapsed().as_millis() as u64,
        });

        // Every dispatch completes synchronously (`CallToolResponse::Complete`).
        // A gated call does request client input, but it awaits the answer here
        // rather than materializing a task — the elicitation has to be issued
        // inside this handler (SEP-2260 associates it with this request, and rmcp
        // carries that association in a task-local that no `spawn` would cross).
        result.map(Into::into)
    }
}

/// How long the operator has to answer before the call is refused.
///
/// A gated call holds no lock while it waits — the host's execution queue is
/// entered after the gate — but it does hold the client's request open, and an
/// unanswered prompt must not hold it forever. Long enough for a person to read
/// an order and decide.
const APPROVAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// The operator as an MCP client can reach them: one elicitation round-trip,
/// where the client declared it can present one.
struct ClientOperator<'a> {
    peer: &'a rmcp::service::Peer<RoleServer>,
}

impl ClientOperator<'_> {
    /// Whether this client can put a question to a person.
    ///
    /// Presence of the capability is the test, not `form`: a client on the
    /// pre-`mode` spec declares a bare `elicitation: {}` and still presents a
    /// form. Only one that declared url mode *alone* has no form to show.
    fn reachable(&self) -> bool {
        self.peer.peer_info().is_some_and(|info| {
            info.capabilities
                .elicitation
                .as_ref()
                .is_some_and(|elicitation| elicitation.form.is_some() || elicitation.url.is_none())
        })
    }
}

impl Operator for ClientOperator<'_> {
    async fn approve(&self, what: &str) -> Approval {
        if !self.reachable() {
            return Approval::Unreachable;
        }
        // An empty schema: this asks for a decision, not for data, and
        // `Accept` is the decision.
        let params = rmcp::model::ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: what.to_owned(),
            requested_schema: rmcp::model::ElicitationSchema::new(BTreeMap::new()),
        };
        match self
            .peer
            .create_elicitation_with_timeout(params, Some(APPROVAL_TIMEOUT))
            .await
        {
            // `ElicitationAction` is `#[non_exhaustive]`: only an explicit
            // `Accept` opens the gate, so an action added to the spec later
            // refuses rather than being read as approval.
            Ok(result) => match result.action {
                rmcp::model::ElicitationAction::Accept => Approval::Granted,
                _ => Approval::Refused,
            },
            // Fail closed. A client that declared it could ask and then did not
            // answer is not the same as one that never could: falling back here
            // would let the model's own acknowledgment stand in for the person
            // this client promised to reach.
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "the client could not put the call to its operator; refusing it"
                );
                Approval::Refused
            }
        }
    }
}

/// Audit labels for the active trading context. `"paper"` / `paper: true` only
/// when the gate is the paper workspace; everything else is `"live"`.
fn audit_trading_mode(mode: GateMode) -> (&'static str, bool) {
    match mode {
        GateMode::Paper => ("paper", true),
        GateMode::Master => ("live", false),
    }
}

const DANGEROUS_GATE_ERROR: &str = "This operation modifies account state. Set \"acknowledged\": \
                                    true to proceed, or start the server with --allow-dangerous.";

const LIVE_ACCOUNT_GATE_ERROR: &str = "This shape allocates or moves real money on the configured \
                                       Kraken account. Set \"acknowledged\": true to proceed, or \
                                       start the server with --allow-dangerous.";

const APPROVAL_REFUSED: &str = "The operator did not approve this call.";

/// Why a call needs a person's approval.
///
/// Each reason carries both of the words it needs — what the operator is asked
/// to approve, and what a client with nobody to ask is told instead — so the
/// two cannot be paired wrongly at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gated {
    /// The command modifies account state, whatever its arguments say.
    AccountState,
    /// A command whose ordinary shape is local, pointed at the real account by
    /// one argument.
    RealAccount,
}

impl Gated {
    /// What the operator is asked to approve.
    const fn prompt(self) -> &'static str {
        match self {
            Self::AccountState => "modifies account state on the configured Kraken account",
            Self::RealAccount => "allocates or moves real money on the configured Kraken account",
        }
    }

    /// What a client that can reach nobody is told.
    const fn refusal(self) -> &'static str {
        match self {
            Self::AccountState => DANGEROUS_GATE_ERROR,
            Self::RealAccount => LIVE_ACCOUNT_GATE_ERROR,
        }
    }
}

/// Why this call is gated, or `None` when it is not.
///
/// [`Effect::LocalOrLive`] is not in the danger set — its ordinary shape writes
/// local state — but one argument points it at the real account, so that shape
/// is judged here, per call, the same way the mode-routed order verbs re-derive
/// their gate.
fn gated(
    armed: bool,
    effect: Effect,
    arguments: &serde_json::Map<String, serde_json::Value>,
) -> Option<Gated> {
    if armed {
        Some(Gated::AccountState)
    } else if effect == Effect::LocalOrLive && names_the_real_account(arguments) {
        Some(Gated::RealAccount)
    } else {
        None
    }
}

/// Put a gated call to the operator, and fall back to the argument only when
/// there is nobody to put it to.
///
/// The fallback is not a second route past the operator: a client that *can*
/// reach one is always asked, so `"acknowledged": true` from the model does not
/// substitute for the answer. It is the only assurance a client without
/// elicitation can offer, and the refusal names it.
async fn enforce_approval(
    request: &str,
    arguments: &serde_json::Map<String, serde_json::Value>,
    operator: &impl Operator,
    gated: Gated,
) -> std::result::Result<(), ErrorData> {
    match operator.approve(request).await {
        Approval::Granted => Ok(()),
        Approval::Refused => Err(ErrorData::invalid_params(APPROVAL_REFUSED, None)),
        Approval::Unreachable if acknowledged(arguments) => Ok(()),
        Approval::Unreachable => Err(ErrorData::invalid_params(gated.refusal(), None)),
    }
}

/// What the operator is shown: the command as they would have typed it, why it
/// is being asked about, and the arguments it carries.
///
/// Every argument is shown but two — the acknowledgment, which is this server's
/// own bookkeeping, and anything [`NEVER_AUDITED`] names. Somebody approving a
/// transfer has to see the amount; nobody should be shown a secret in order to
/// approve one.
fn approval_request(
    command: &str,
    arguments: &serde_json::Map<String, serde_json::Value>,
    gated: Gated,
) -> String {
    let shown: BTreeMap<&String, &Value> = arguments
        .iter()
        .filter(|(name, _)| {
            let id = normalize_arg_id(name);
            id != "acknowledged" && !NEVER_AUDITED.contains(&id.as_str())
        })
        .collect();
    let carrying = if shown.is_empty() {
        String::new()
    } else {
        // Compact, not pretty: a client renders this as one prompt, not a page.
        format!(
            "\n\nArguments: {}",
            serde_json::to_string(&shown).unwrap_or_default()
        )
    };
    format!("`{command}` {}.{carrying}\n\nRun it?", gated.prompt())
}

/// Whether a [`Effect::LocalOrLive`] call's arguments point it at the real
/// account: `--mode live` on a workspace, `--real-account` on a recipe rerun.
///
/// `mode` is read through [`WorkspaceMode`] rather than compared to the literal
/// `"live"`, so the gate and the command agree on what a mode is by
/// construction: renaming the variant moves both at once. A value that is not a
/// mode at all never reaches execution — `Command::try_from` refuses it a few
/// lines later — so failing to recognise one here cannot open the gate.
///
/// One arm per escalating argument the contract declares, pinned by
/// `every_escalating_command_is_armed_by_an_argument_the_gate_reads` — a new
/// `LocalOrLive` leaf spelling its escalation some third way fails that test
/// rather than passing this gate unnoticed.
fn names_the_real_account(arguments: &serde_json::Map<String, serde_json::Value>) -> bool {
    let live_mode = arguments.get("mode").is_some_and(|mode| {
        serde_json::from_value::<WorkspaceMode>(mode.clone())
            .is_ok_and(|mode| mode == WorkspaceMode::Live)
    });
    live_mode
        || arguments
            .get("real_account")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

fn acknowledged(arguments: &serde_json::Map<String, serde_json::Value>) -> bool {
    arguments
        .get("acknowledged")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Serve over stdio until the client disconnects. Emits the server-start
/// audit line before accepting requests.
pub async fn serve_stdio<H: ToolHost>(server: KrakenMcpServer<H>) -> Result<()> {
    let mode = if server.config.allow_dangerous {
        "autonomous"
    } else {
        "guarded"
    };
    audit_log(AuditEvent::ServerStart {
        server_version: &server.config.server_version,
        tool_count: server.registry.tools().len(),
        mode,
        pid: std::process::id(),
    });

    let transport = rmcp::transport::io::stdio();

    use rmcp::service::ServiceExt;
    let service = server
        .serve(transport)
        .await
        .map_err(|e| McpError::Transport(format!("Failed to start MCP server: {e}")))?;

    service
        .waiting()
        .await
        .map_err(|e| McpError::Transport(format!("MCP server error: {e}")))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_with(
        key: &str,
        value: serde_json::Value,
    ) -> serde_json::Map<String, serde_json::Value> {
        let mut args = serde_json::Map::new();
        args.insert(key.into(), value);
        args
    }

    /// An operator who always answers the same way.
    struct Answering(Approval);

    impl Operator for Answering {
        async fn approve(&self, _what: &str) -> Approval {
            self.0
        }
    }

    /// Put a gated call to `operator` with the given arguments.
    async fn ask(
        answer: Approval,
        arguments: &serde_json::Map<String, serde_json::Value>,
    ) -> std::result::Result<(), ErrorData> {
        enforce_approval(
            "run it?",
            arguments,
            &Answering(answer),
            Gated::AccountState,
        )
        .await
    }

    // The plain `Granted` and `Refused` answers are not asserted here:
    // `an_approved_dangerous_call_crosses_the_seam` drives both through
    // `dispatch` end to end, and pins in addition that a refusal never reaches
    // the host. What is left for this module is the cases that stand alone.

    /// The property the acknowledgment argument alone could never give: a
    /// client that can reach its operator is asked, and their `no` stands
    /// even though the model attested to the call itself.
    #[tokio::test]
    async fn the_model_cannot_acknowledge_its_way_past_a_reachable_operator() {
        let acknowledged = args_with("acknowledged", serde_json::json!(true));
        assert!(
            ask(Approval::Refused, &acknowledged).await.is_err(),
            "the operator refused; the model's own acknowledgment is not a second vote"
        );
    }

    /// Only where there is nobody to ask does the argument stand in — and then
    /// a call without it is refused, with the wording pinned in
    /// tests/seam_snapshots.rs.
    #[tokio::test]
    async fn an_unreachable_operator_falls_back_to_the_acknowledgment() {
        let acknowledged = args_with("acknowledged", serde_json::json!(true));
        assert!(ask(Approval::Unreachable, &acknowledged).await.is_ok());

        let err = ask(Approval::Unreachable, &serde_json::Map::new())
            .await
            .unwrap_err();
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }

    /// `"true"` is a string, not an acknowledgment.
    #[tokio::test]
    async fn a_stringified_acknowledgment_does_not_stand_in() {
        let spelled = args_with("acknowledged", serde_json::json!("true"));
        assert!(ask(Approval::Unreachable, &spelled).await.is_err());
    }

    /// The operator must see what they are approving — and must not be shown a
    /// secret in order to approve it.
    #[test]
    fn the_approval_request_shows_the_call_but_never_a_credential() {
        let mut arguments = args_with("amount", serde_json::json!("0.1"));
        arguments.insert("otp".into(), serde_json::json!("123456"));
        arguments.insert("acknowledged".into(), serde_json::json!(true));

        let request = approval_request("kraken withdraw", &arguments, Gated::AccountState);
        assert!(request.contains("kraken withdraw"), "{request}");
        assert!(
            request.contains("0.1"),
            "the amount is the thing to judge: {request}"
        );
        assert!(
            !request.contains("123456"),
            "a credential must not be shown: {request}"
        );
        assert!(
            !request.contains("acknowledged"),
            "this server's own bookkeeping is not part of the decision: {request}"
        );
    }

    /// `workspace create` is outside the danger set, so this argument-shape
    /// judgment is the only thing standing between an agent and an unprompted
    /// live workspace.
    #[test]
    fn a_local_command_is_gated_by_the_mode_argument() {
        let live = args_with("mode", serde_json::json!("live"));
        assert_eq!(
            gated(false, Effect::LocalOrLive, &live),
            Some(Gated::RealAccount),
            "a live workspace allocates real money"
        );
        let paper = args_with("mode", serde_json::json!("paper"));
        assert_eq!(gated(false, Effect::LocalOrLive, &paper), None);
    }

    /// The gate follows the contract's classification, so a command that only
    /// ever writes local state is never prompted — whatever its arguments say.
    #[test]
    fn the_escalation_is_scoped_to_the_effect_that_declares_it() {
        let live = args_with("mode", serde_json::json!("live"));
        assert_eq!(gated(false, Effect::Local, &live), None);
        assert_eq!(gated(false, Effect::ReadOnly, &live), None);
    }

    /// The live shape of the arguments the gate reads, so the invariant below
    /// is stated once per spelling rather than per command.
    fn live_shape(argument: &str) -> Option<serde_json::Value> {
        match argument {
            "mode" => Some(serde_json::json!("live")),
            "real_account" => Some(serde_json::json!(true)),
            _ => None,
        }
    }

    /// Read against the contract rather than a written list: every command that
    /// declares [`Effect::LocalOrLive`] must escalate through an argument this
    /// gate recognizes. A leaf that spells it a third way fails here instead of
    /// reaching the real account unacknowledged.
    #[test]
    fn every_escalating_command_is_armed_by_an_argument_the_gate_reads() {
        for entry in kraken_contract::tool::Surface::shared().iter() {
            if entry.effect != Effect::LocalOrLive {
                continue;
            }
            let escalated = entry
                .arguments()
                .find_map(|argument| live_shape(argument).map(|value| args_with(argument, value)))
                .unwrap_or_else(|| {
                    panic!(
                        "`{}` declares LocalOrLive but no argument the live-account gate reads",
                        entry.command
                    )
                });
            assert_eq!(
                gated(false, entry.effect, &escalated),
                Some(Gated::RealAccount),
                "`{}` must be approved before it touches the real account",
                entry.command
            );
        }
    }

    #[test]
    fn instructions_all_services_autonomous() {
        let active: Vec<Service> = crate::services::parse_services("all").expect("all parses");
        let text = build_instructions(&active, true, None);
        assert!(text.contains("Mode: autonomous"));
        assert!(!text.contains("Services not loaded"));
        assert!(!text.contains("acknowledged"));
    }

    /// The paper-workspace banner is snapshot-pinned verbatim in
    /// tests/seam_snapshots.rs; the live variant is only pinned here.
    #[test]
    fn instructions_name_a_live_workspace_as_the_real_account() {
        let live = build_instructions(
            &[Service::Paper],
            false,
            Some(&ActiveWorkspace::Live {
                name: "prod".into(),
                mode: "live".into(),
            }),
        );
        assert!(live.contains("Workspace: prod (mode: live)"));
        assert!(live.contains("treated as the real account"));
    }

    #[test]
    fn argument_keys_returns_sorted_keys() {
        let mut args = serde_json::Map::new();
        args.insert("pair".into(), serde_json::json!("BTCUSD"));
        args.insert("api_key".into(), serde_json::json!("my-key"));
        args.insert("count".into(), serde_json::json!("10"));
        let keys = argument_keys(Some(&args));
        assert_eq!(keys, vec!["api_key", "count", "pair"]);
    }

    #[test]
    fn argument_keys_empty_on_none() {
        let keys = argument_keys(None);
        assert!(keys.is_empty());
    }

    #[test]
    fn financial_audit_allowlist_is_sorted_for_binary_search() {
        let mut sorted = FINANCIAL_AUDIT_ARGS.to_vec();
        sorted.sort_unstable();
        assert_eq!(
            FINANCIAL_AUDIT_ARGS,
            sorted.as_slice(),
            "FINANCIAL_AUDIT_ARGS must stay sorted for binary_search"
        );
    }

    #[test]
    fn financial_arguments_capture_trade_and_withdraw_values() {
        let mut args = serde_json::Map::new();
        args.insert("pair".into(), serde_json::json!("BTCUSD"));
        args.insert("volume".into(), serde_json::json!("0.001"));
        args.insert("price".into(), serde_json::json!("50000"));
        args.insert("type".into(), serde_json::json!("limit"));
        args.insert("acknowledged".into(), serde_json::json!(true));
        let financial = financial_arguments(Some(&args));
        assert_eq!(
            financial.get("pair").and_then(Value::as_str),
            Some("BTCUSD")
        );
        assert_eq!(
            financial.get("volume").and_then(Value::as_str),
            Some("0.001")
        );
        assert_eq!(
            financial.get("price").and_then(Value::as_str),
            Some("50000")
        );
        assert_eq!(financial.get("type").and_then(Value::as_str), Some("limit"));
        assert!(
            !financial.contains_key("acknowledged"),
            "gate flags are not financial: {financial:?}"
        );

        let mut withdraw = serde_json::Map::new();
        withdraw.insert("asset".into(), serde_json::json!("BTC"));
        withdraw.insert("key".into(), serde_json::json!("cold-storage"));
        withdraw.insert("amount".into(), serde_json::json!("0.1"));
        withdraw.insert("--max-fee".into(), serde_json::json!("0.0001"));
        let financial = financial_arguments(Some(&withdraw));
        assert_eq!(financial.get("asset").and_then(Value::as_str), Some("BTC"));
        assert_eq!(
            financial.get("key").and_then(Value::as_str),
            Some("cold-storage")
        );
        assert_eq!(financial.get("amount").and_then(Value::as_str), Some("0.1"));
        assert_eq!(
            financial.get("max_fee").and_then(Value::as_str),
            Some("0.0001"),
            "hyphenated / dashed flag names normalize into the map"
        );
    }

    #[test]
    fn financial_arguments_never_log_secret_values() {
        let mut args = serde_json::Map::new();
        args.insert("api_secret".into(), serde_json::json!("super-secret-value"));
        args.insert("api_key".into(), serde_json::json!("attacker-key"));
        args.insert("otp".into(), serde_json::json!("123456"));
        args.insert("pair".into(), serde_json::json!("ETHUSD"));
        args.insert("volume".into(), serde_json::json!("1"));
        let financial = financial_arguments(Some(&args));
        let rendered = serde_json::to_string(&financial).unwrap();
        assert!(!rendered.contains("super-secret-value"), "{rendered}");
        assert!(!rendered.contains("attacker-key"), "{rendered}");
        assert!(!rendered.contains("123456"), "{rendered}");
        assert_eq!(
            financial.get("pair").and_then(Value::as_str),
            Some("ETHUSD")
        );
        assert_eq!(financial.get("volume").and_then(Value::as_str), Some("1"));
    }

    #[test]
    fn financial_arguments_empty_when_no_money_fields() {
        let mut args = serde_json::Map::new();
        args.insert("count".into(), serde_json::json!("10"));
        args.insert("acknowledged".into(), serde_json::json!(true));
        assert!(financial_arguments(Some(&args)).is_empty());
        assert!(financial_arguments(None).is_empty());
    }

    #[test]
    fn audit_error_code_is_stable_string() {
        let err = ErrorData::invalid_params("invalid", None);
        let code = audit_error_code(&err);
        assert!(!code.is_empty());
    }

    #[test]
    fn now_iso_is_valid_rfc3339() {
        let ts = now_iso();
        assert!(chrono::DateTime::parse_from_rfc3339(&ts).is_ok());
    }

    #[test]
    fn audit_line_is_jsonl_namespaced_under_mcp_audit() {
        // No text prefix: the line must parse as a JSON object on its own so the
        // stderr stream stays valid JSONL, with the record under `mcp_audit`.
        let line = AuditLine {
            mcp_audit: AuditRecord {
                ts: "2026-07-01T00:00:00.000Z".to_string(),
                transport: MCP_TRANSPORT,
                event: AuditEvent::ToolCall {
                    tool: "ticker",
                    trading_mode: "live",
                    paper: false,
                    arg_keys: vec!["pair".to_string()],
                    arg_count: 1,
                    financial: BTreeMap::new(),
                    agent: "test-agent",
                    instance_id: "abc123",
                    pid: 42,
                },
            },
        };
        let rendered = audit_line(&line).expect("event serializes");
        let parsed: serde_json::Value =
            serde_json::from_str(&rendered).expect("audit line is valid JSON");
        assert!(
            parsed.is_object(),
            "audit line must be a JSON object: {rendered}"
        );

        let record = &parsed["mcp_audit"];
        assert!(
            record.is_object(),
            "record is nested under mcp_audit: {rendered}"
        );
        assert_eq!(record["event"], "tool_call");
        assert_eq!(record["tool"], "ticker");
        assert_eq!(record["trading_mode"], "live");
        assert_eq!(record["paper"], false);
        assert_eq!(record["transport"], "stdio");
        assert_eq!(record["arg_count"], 1);
        assert!(
            record.get("financial").is_none(),
            "empty financial map must be omitted: {rendered}"
        );
    }

    #[test]
    fn audit_line_includes_financial_values_for_withdraw() {
        let mut financial = BTreeMap::new();
        financial.insert("amount".into(), serde_json::json!("0.1"));
        financial.insert("asset".into(), serde_json::json!("BTC"));
        financial.insert("key".into(), serde_json::json!("cold-storage"));

        let line = AuditLine {
            mcp_audit: AuditRecord {
                ts: "2026-07-01T00:00:00.000Z".to_string(),
                transport: MCP_TRANSPORT,
                event: AuditEvent::ToolCall {
                    tool: "kraken_withdraw",
                    trading_mode: "live",
                    paper: false,
                    arg_keys: vec![
                        "acknowledged".into(),
                        "amount".into(),
                        "asset".into(),
                        "key".into(),
                    ],
                    arg_count: 4,
                    financial,
                    agent: "cursor",
                    instance_id: "abc123",
                    pid: 7,
                },
            },
        };
        let rendered = audit_line(&line).expect("event serializes");
        let parsed: serde_json::Value =
            serde_json::from_str(&rendered).expect("audit line is valid JSON");
        let record = &parsed["mcp_audit"];
        assert_eq!(record["tool"], "kraken_withdraw");
        assert_eq!(record["trading_mode"], "live");
        assert_eq!(record["paper"], false);
        assert_eq!(record["financial"]["asset"], "BTC");
        assert_eq!(record["financial"]["amount"], "0.1");
        assert_eq!(record["financial"]["key"], "cold-storage");
        assert!(
            record["arg_keys"]
                .as_array()
                .unwrap()
                .iter()
                .any(|k| k == "acknowledged"),
            "arg_keys still lists non-financial keys: {rendered}"
        );
    }

    #[test]
    fn audit_line_marks_paper_trading_mode() {
        let line = AuditLine {
            mcp_audit: AuditRecord {
                ts: "2026-07-01T00:00:00.000Z".to_string(),
                transport: MCP_TRANSPORT,
                event: AuditEvent::ToolResult {
                    tool: "kraken_paper_buy",
                    trading_mode: "paper",
                    paper: true,
                    status: "executed",
                    error_code: None,
                    duration_ms: 12,
                },
            },
        };
        let rendered = audit_line(&line).expect("event serializes");
        let parsed: serde_json::Value =
            serde_json::from_str(&rendered).expect("audit line is valid JSON");
        let record = &parsed["mcp_audit"];
        assert_eq!(record["event"], "tool_result");
        assert_eq!(record["trading_mode"], "paper");
        assert_eq!(record["paper"], true);
        assert_eq!(record["status"], "executed");
    }
}

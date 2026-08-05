//! MCP server (rmcp, stdio). Tool calls are rewritten to argv and re-enter the
//! CLI's own clap parse + dispatch path, so tool and CLI behaviour cannot drift.

use std::env;
use std::sync::Arc;
use std::time::Instant;

use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ErrorData, Implementation, InitializeRequestParams,
    InitializeResult, ListToolsResult, PaginatedRequestParams, ServerCapabilities, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use serde::Serialize;

use super::registry::{GateMode, ToolRegistry};
use crate::AppContext;
use crate::errors::KrakenError;

pub(crate) struct KrakenMcpServer {
    registry: ToolRegistry,
    ctx: Arc<AppContext>,
    allow_dangerous: bool,
    instructions: String,
    /// Serializes command execution across concurrent tool calls. The account
    /// journal and its tape are single-writer (an exclusive file lock), so two
    /// overlapping paper/session tool calls would otherwise race that lock and
    /// one would fail "in use by another process". A long-lived server handles
    /// requests concurrently, so it queues them here instead — the CLI's own
    /// process-per-command isolation has no equivalent to reproduce.
    exec_lock: tokio::sync::Mutex<()>,
}

impl KrakenMcpServer {
    pub(crate) fn new(
        registry: ToolRegistry,
        ctx: AppContext,
        allow_dangerous: bool,
        active_services: &[String],
        workspace: Option<&kraken_workspace::WorkspaceManifest>,
    ) -> Self {
        let instructions = build_instructions(active_services, allow_dangerous, workspace);
        Self {
            registry,
            ctx: Arc::new(ctx),
            allow_dangerous,
            instructions,
            exec_lock: tokio::sync::Mutex::new(()),
        }
    }

    async fn dispatch_tool(
        &self,
        request: CallToolRequestParams,
    ) -> Result<CallToolResult, ErrorData> {
        let tool_name = &request.name;
        let entry = self
            .registry
            .get_by_name(tool_name)
            .ok_or_else(|| ErrorData::invalid_params(format!("Unknown tool: {tool_name}"), None))?;

        enforce_dangerous_gate(entry.armed, self.allow_dangerous, &request.arguments)?;

        let argv = build_argv(&entry.canonical_key, &request.arguments, &entry.clap_args)
            .map_err(|why| ErrorData::invalid_params(why, None))?;

        let parsed = match crate::Cli::try_parse_from(&argv) {
            Ok(cli) => cli,
            Err(e) => {
                return Err(ErrorData::invalid_params(
                    format!("Argument validation failed: {e}"),
                    None,
                ));
            }
        };

        let command = parsed
            .command
            .ok_or_else(|| ErrorData::invalid_params("No command parsed from arguments", None))?;

        // Serialize the execution itself: concurrent tool calls otherwise race
        // the account's single-writer lock (see `exec_lock`). The async guard is
        // held across the command's `.await` deliberately — that is the window
        // being serialized — and released the moment it returns.
        let _exec = self.exec_lock.lock().await;
        // Return the payload as MCP structured content: clients that support it get
        // typed JSON, and rmcp includes a text fallback for those that don't.
        match crate::commands::execute_command(&self.ctx, command).await {
            Ok(output) => Ok(CallToolResult::structured(output.data)),
            Err(e) => Ok(CallToolResult::structured_error(e.to_json_envelope())),
        }
    }
}

fn build_instructions(
    active_services: &[String],
    allow_dangerous: bool,
    workspace: Option<&kraken_workspace::WorkspaceManifest>,
) -> String {
    let svc_list = active_services.join(", ");
    let mode = if allow_dangerous {
        "autonomous"
    } else {
        "guarded"
    };

    let mut text = format!("Kraken exchange CLI tools. Active services: {svc_list}. Mode: {mode}.");

    // The resolved execution context, so an agent never has to guess which
    // account its order tools address.
    match workspace {
        Some(m) if m.mode == kraken_workspace::WorkspaceMode::Paper => {
            text.push_str(&format!(
                " Workspace: {} (mode: paper) — a simulated account; order tools execute \
                 paper fills and run without acknowledgment.",
                m.name
            ));
        }
        Some(m) => {
            text.push_str(&format!(
                " Workspace: {} (mode: {}) — treated as the real account.",
                m.name, m.mode
            ));
        }
        None => {
            text.push_str(" No workspace: tools address the real Kraken account.");
        }
    }

    let all_services = [
        "market",
        "account",
        "trade",
        "funding",
        "earn",
        "subaccount",
        "futures",
        "paper",
        "workspace",
        "auth",
        "feedback",
    ];
    let missing: Vec<&str> = all_services
        .iter()
        .filter(|s| !active_services.iter().any(|a| a == **s))
        .copied()
        .collect();

    if !missing.is_empty() {
        text.push_str(&format!(
            " Services not loaded: {}. To enable them, \
             the user must update their MCP client config to: \
             {{\"command\": \"kraken\", \"args\": [\"mcp\", \"-s\", \"all\"]}} \
             and restart the MCP connection.",
            missing.join(", "),
        ));
    }

    if !allow_dangerous {
        text.push_str(
            " Dangerous tools (orders, withdrawals, cancellations) require \
             \"acknowledged\": true in arguments. \
             To run without per-call confirmation, the user must add \"--allow-dangerous\" \
             to args in their MCP client config and restart.",
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
// callers by agent client, instance ID, and PID. Only argument *metadata* is
// recorded, never values, so no credentials or account data can leak.

const MCP_TRANSPORT: &str = "stdio";

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
    /// An MCP session was opened (client `initialize`).
    SessionStart {
        server_version: &'static str,
        agent: &'static str,
        instance_id: &'static str,
        pid: u32,
    },
    /// The server process started and is about to serve on stdio.
    ServerStart {
        server_version: &'static str,
        tool_count: usize,
        mode: &'static str,
        pid: u32,
    },
    /// A tool invocation was received.
    ToolCall {
        tool: &'a str,
        arg_keys: Vec<String>,
        arg_count: usize,
        agent: &'static str,
        instance_id: &'static str,
        pid: u32,
    },
    /// A tool invocation completed (executed or rejected).
    ToolResult {
        tool: &'a str,
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

fn argument_keys(args: &Option<serde_json::Map<String, serde_json::Value>>) -> Vec<String> {
    let Some(args) = args else {
        return Vec::new();
    };
    let mut keys: Vec<String> = args.keys().cloned().collect();
    keys.sort();
    keys
}

fn audit_error_code(error: &ErrorData) -> String {
    format!("{:?}", error.code)
}

impl ServerHandler for KrakenMcpServer {
    async fn initialize(
        &self,
        _request: InitializeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        audit_log(AuditEvent::SessionStart {
            server_version: env!("CARGO_PKG_VERSION"),
            agent: crate::telemetry::agent_client(),
            instance_id: crate::telemetry::instance_id(),
            pid: std::process::id(),
        });
        Ok(
            InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
                .with_server_info(
                    Implementation::new("kraken-cli", env!("CARGO_PKG_VERSION")).with_description(
                        "Kraken exchange CLI tools. Use service filtering to control \
                         which command groups are available.",
                    ),
                )
                .with_instructions(&self.instructions),
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: self.registry.tool_definitions(),
            ..Default::default()
        })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.registry.get_by_name(name).map(|e| e.tool.clone())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let tool_name = request.name.to_string();
        let started = Instant::now();
        let arg_keys = argument_keys(&request.arguments);
        let arg_count = arg_keys.len();

        audit_log(AuditEvent::ToolCall {
            tool: &tool_name,
            arg_keys,
            arg_count,
            agent: crate::telemetry::agent_client(),
            instance_id: crate::telemetry::instance_id(),
            pid: std::process::id(),
        });

        let result = self.dispatch_tool(request).await;

        let (status, error_code) = match &result {
            Ok(_) => ("executed", None),
            Err(e) => ("rejected", Some(audit_error_code(e))),
        };

        audit_log(AuditEvent::ToolResult {
            tool: &tool_name,
            status,
            error_code,
            duration_ms: started.elapsed().as_millis() as u64,
        });

        result
    }
}

const DANGEROUS_GATE_ERROR: &str = "This operation modifies account state. Set \"acknowledged\": true to proceed, \
     or start the server with --allow-dangerous.";

fn enforce_dangerous_gate(
    dangerous: bool,
    allow_dangerous: bool,
    arguments: &Option<serde_json::Map<String, serde_json::Value>>,
) -> Result<(), ErrorData> {
    if !dangerous || allow_dangerous {
        return Ok(());
    }

    let confirmed = arguments
        .as_ref()
        .and_then(|a| a.get("acknowledged"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if confirmed {
        Ok(())
    } else {
        Err(ErrorData::invalid_params(DANGEROUS_GATE_ERROR, None))
    }
}

fn build_argv(
    canonical_key: &str,
    arguments: &Option<serde_json::Map<String, serde_json::Value>>,
    arg_meta: &[super::registry::ArgMeta],
) -> Result<Vec<String>, String> {
    let mut argv = vec![
        "kraken".to_string(),
        "-o".to_string(),
        "json".to_string(),
        "--yes".to_string(),
    ];

    let command_parts: Vec<&str> = canonical_key.split_whitespace().collect();
    argv.extend(command_parts.iter().map(|s| s.to_string()));

    let Some(args) = arguments else {
        return Ok(argv);
    };

    let mut positionals: Vec<(usize, Vec<String>)> = Vec::new();
    let mut flags: Vec<String> = Vec::new();

    for (key, value) in args {
        if value.is_null() || key == "acknowledged" {
            continue;
        }

        // Match by id first, then by clap alias — an MCP call built against
        // an older schema (e.g. `balance` for `--capital`) keeps working
        // exactly like the CLI flag it maps to.
        let meta = arg_meta
            .iter()
            .find(|m| m.id == *key)
            .or_else(|| arg_meta.iter().find(|m| m.aliases.iter().any(|a| a == key)));

        match meta {
            Some(&super::registry::ArgMeta {
                positional_index: Some(idx),
                ..
            }) => {
                positionals.push((idx, json_to_strings(value)));
            }
            Some(m) if m.is_bool_flag => {
                let truthy = match value {
                    serde_json::Value::Bool(b) => *b,
                    serde_json::Value::String(s) => s.eq_ignore_ascii_case("true"),
                    _ => false,
                };
                if truthy {
                    let flag = m.long.as_deref().unwrap_or(key);
                    flags.push(format!("--{flag}"));
                }
            }
            Some(m) => {
                let flag = m.long.as_deref().unwrap_or(key);
                emit_flag_values(&mut flags, flag, value);
            }
            None => {
                // Fail-closed AND loud: a key outside the tool's arg_meta is
                // refused, never silently dropped — a renamed parameter must
                // not default its way into a wrong-capital account, and a
                // global-flag injection attempt (--api-key etc., excluded
                // from MCP schemas) gets a refusal instead of a no-op.
                let known: Vec<&str> = arg_meta.iter().map(|m| m.id.as_str()).collect();
                return Err(format!(
                    "unknown argument '{key}'; this tool accepts: {}",
                    known.join(", ")
                ));
            }
        }
    }

    // Flags first, then positionals behind a `--` end-of-options marker.
    // Combined with the `--flag=value` form (see `emit_flag_values`), this keeps
    // any value beginning with `-` (e.g. a negative number) from being misparsed
    // as a flag when the argv is re-parsed by clap (`try_parse_from`).
    argv.extend(flags);
    positionals.sort_by_key(|(idx, _)| *idx);
    if !positionals.is_empty() {
        argv.push("--".to_string());
        for (_, vals) in positionals {
            argv.extend(vals);
        }
    }
    Ok(argv)
}

fn json_to_strings(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::Array(arr) => arr
            .iter()
            .map(|v| match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect(),
        serde_json::Value::String(s) => vec![s.clone()],
        other => vec![other.to_string()],
    }
}

/// Emit a value-taking flag as a single `--flag=value` token.
///
/// The `=` form (rather than two `--flag` `value` tokens) makes a value that
/// begins with `-` unambiguous when the argv is re-parsed by clap.
fn emit_flag_values(flags: &mut Vec<String>, flag: &str, value: &serde_json::Value) {
    match value {
        serde_json::Value::Array(arr) => {
            for item in arr {
                let v = match item {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                flags.push(format!("--{flag}={v}"));
            }
        }
        serde_json::Value::String(s) => flags.push(format!("--{flag}={s}")),
        serde_json::Value::Number(n) => flags.push(format!("--{flag}={n}")),
        serde_json::Value::Bool(b) => flags.push(format!("--{flag}={b}")),
        serde_json::Value::Null => {}
        other => flags.push(format!("--{flag}={other}")),
    }
}

pub(crate) async fn run_server(
    ctx: &AppContext,
    services: &str,
    allow_dangerous: bool,
) -> crate::errors::Result<()> {
    let parsed_services = super::parse_services(services)?;
    let active_services = super::apply_exclusions(&parsed_services);

    if active_services.is_empty() {
        return Err(KrakenError::Validation(
            "No REST-eligible services remain after filtering. \
             Streaming groups (websocket, futures-ws) are excluded in MCP v1."
                .into(),
        ));
    }

    // Resolve once so a long-lived server cannot change its safety gate
    // between tool discovery and execution.
    let workspace = crate::commands::workspace_guard::active_manifest(ctx)?;
    let gate_mode = match &workspace {
        Some(m) if m.mode == kraken_workspace::WorkspaceMode::Paper => GateMode::Paper,
        // A named live workspace gates like the master account: fail closed.
        _ => GateMode::Master,
    };
    let registry = ToolRegistry::build_with_options(&active_services, allow_dangerous, gate_mode)?;

    // Reuse the credentials and URLs already resolved by the CLI entry point
    // (clap flags + env vars + config); only the MCP-specific runtime settings
    // are overridden here.
    let mcp_ctx = AppContext {
        format: crate::output::OutputFormat::Json,
        api_url: ctx.api_url.clone(),
        futures_url: ctx.futures_url.clone(),
        ws_public_url: ctx.ws_public_url.clone(),
        ws_auth_url: ctx.ws_auth_url.clone(),
        ws_l3_url: ctx.ws_l3_url.clone(),
        ws_futures_url: ctx.ws_futures_url.clone(),
        ws_reconnect_base_ms: ctx.ws_reconnect_base_ms,
        spot_credentials: ctx.spot_credentials.clone(),
        futures_credentials: ctx.futures_credentials.clone(),
        api_secret: None,
        otp: None,
        workspace: ctx.workspace.clone(),
        force: true,
        accept_invalid_certs: ctx.accept_invalid_certs,
        allow_any_url_host: ctx.allow_any_url_host,
        mcp_mode: true,
        // The MCP server doesn't drive the streaming `stream::run` path.
        monitor: None,
        // Built once, reused across every tool call for this long-lived server.
        spot_client: std::sync::OnceLock::new(),
        futures_client: std::sync::OnceLock::new(),
    };

    let server = KrakenMcpServer::new(
        registry,
        mcp_ctx,
        allow_dangerous,
        &active_services,
        workspace.as_ref(),
    );

    let mode_label = if allow_dangerous {
        "autonomous"
    } else {
        "guarded"
    };
    audit_log(AuditEvent::ServerStart {
        server_version: env!("CARGO_PKG_VERSION"),
        tool_count: server.registry.tools().len(),
        mode: mode_label,
        pid: std::process::id(),
    });

    let transport = rmcp::transport::io::stdio();

    use rmcp::service::ServiceExt;
    let service = server
        .serve(transport)
        .await
        .map_err(|e| KrakenError::Config(format!("Failed to start MCP server: {e}")))?;

    service
        .waiting()
        .await
        .map_err(|e| KrakenError::Config(format!("MCP server error: {e}")))?;

    Ok(())
}

use clap::Parser;

#[cfg(test)]
mod tests {
    use super::super::registry::ArgMeta;
    use super::*;

    fn pos(id: &str, index: usize) -> ArgMeta {
        ArgMeta {
            id: id.into(),
            long: None,
            aliases: Vec::new(),
            is_bool_flag: false,
            positional_index: Some(index),
        }
    }

    fn flag(id: &str, long: &str) -> ArgMeta {
        ArgMeta {
            id: id.into(),
            long: Some(long.into()),
            aliases: Vec::new(),
            is_bool_flag: false,
            positional_index: None,
        }
    }

    fn bool_flag(id: &str, long: &str) -> ArgMeta {
        ArgMeta {
            id: id.into(),
            long: Some(long.into()),
            aliases: Vec::new(),
            is_bool_flag: true,
            positional_index: None,
        }
    }

    fn args_with(
        key: &str,
        value: serde_json::Value,
    ) -> Option<serde_json::Map<String, serde_json::Value>> {
        let mut args = serde_json::Map::new();
        args.insert(key.into(), value);
        Some(args)
    }

    #[test]
    fn dangerous_gate_allows_non_dangerous() {
        assert!(enforce_dangerous_gate(false, false, &None).is_ok());
    }

    #[test]
    fn dangerous_gate_rejects_guarded_without_ack() {
        let err = enforce_dangerous_gate(true, false, &None).unwrap_err();
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        assert!(err.message.as_ref().contains("acknowledged"));
    }

    #[test]
    fn dangerous_gate_accepts_guarded_with_ack() {
        assert!(
            enforce_dangerous_gate(
                true,
                false,
                &args_with("acknowledged", serde_json::json!(true))
            )
            .is_ok()
        );
    }

    #[test]
    fn dangerous_gate_rejects_string_ack() {
        let err = enforce_dangerous_gate(
            true,
            false,
            &args_with("acknowledged", serde_json::json!("true")),
        )
        .unwrap_err();
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn dangerous_gate_allows_autonomous() {
        assert!(enforce_dangerous_gate(true, true, &None).is_ok());
    }

    #[test]
    fn instructions_default_mode_lists_missing_services() {
        let active = vec!["market".into(), "account".into(), "paper".into()];
        let text = build_instructions(&active, false, None);
        assert!(text.contains("Active services: market, account, paper"));
        assert!(text.contains("Mode: guarded"));
        assert!(text.contains("Services not loaded: trade"));
        assert!(text.contains("\"args\": [\"mcp\", \"-s\", \"all\"]"));
        assert!(text.contains("acknowledged"));
        assert!(text.contains("--allow-dangerous"));
    }

    #[test]
    fn instructions_all_services_autonomous() {
        let active: Vec<String> = [
            "market",
            "account",
            "trade",
            "funding",
            "earn",
            "subaccount",
            "futures",
            "paper",
            "workspace",
            "auth",
            "feedback",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let text = build_instructions(&active, true, None);
        assert!(text.contains("Mode: autonomous"));
        assert!(!text.contains("Services not loaded"));
        assert!(!text.contains("acknowledged"));
    }

    #[test]
    fn build_argv_simple_command() {
        let argv = build_argv("ticker", &None, &[]).unwrap();
        assert_eq!(argv, vec!["kraken", "-o", "json", "--yes", "ticker"]);
    }

    #[test]
    fn build_argv_with_string_flag() {
        let meta = vec![flag("count", "count")];
        let mut args = serde_json::Map::new();
        args.insert("count".into(), serde_json::json!("10"));
        let argv = build_argv("orderbook", &Some(args), &meta).unwrap();
        assert!(argv.contains(&"--count=10".to_string()));
    }

    #[test]
    fn build_argv_with_bool_flag() {
        let meta = vec![bool_flag("trades", "trades")];
        let mut args = serde_json::Map::new();
        args.insert("trades".into(), serde_json::json!(true));
        let argv = build_argv("open-orders", &Some(args), &meta).unwrap();
        assert!(argv.contains(&"--trades".to_string()));
    }

    #[test]
    fn build_argv_bool_false_omitted() {
        let meta = vec![bool_flag("trades", "trades")];
        let mut args = serde_json::Map::new();
        args.insert("trades".into(), serde_json::json!(false));
        let argv = build_argv("open-orders", &Some(args), &meta).unwrap();
        assert!(!argv.contains(&"--trades".to_string()));
    }

    #[test]
    fn build_argv_with_positional_array() {
        let meta = vec![pos("pairs", 0)];
        let mut args = serde_json::Map::new();
        args.insert("pairs".into(), serde_json::json!(["BTCUSD", "ETHUSD"]));
        let argv = build_argv("ticker", &Some(args), &meta).unwrap();
        let sep = argv.iter().position(|a| a == "--").unwrap();
        assert_eq!(argv[sep + 1], "BTCUSD");
        assert_eq!(argv[sep + 2], "ETHUSD");
        assert!(!argv.contains(&"--pairs".to_string()));
    }

    #[test]
    fn build_argv_nested_command() {
        let argv = build_argv("order buy", &None, &[]).unwrap();
        assert_eq!(argv, vec!["kraken", "-o", "json", "--yes", "order", "buy"]);
    }

    #[test]
    fn build_argv_null_skipped() {
        let meta = vec![flag("since", "since")];
        let mut args = serde_json::Map::new();
        args.insert("since".into(), serde_json::Value::Null);
        let argv = build_argv("trades", &Some(args), &meta).unwrap();
        assert!(!argv.contains(&"--since".to_string()));
    }

    #[test]
    fn build_argv_flags_precede_guarded_positionals() {
        let meta = vec![pos("pair", 0), flag("count", "count")];
        let mut args = serde_json::Map::new();
        args.insert("pair".into(), serde_json::json!("BTCUSD"));
        args.insert("count".into(), serde_json::json!("10"));
        let argv = build_argv("orderbook", &Some(args), &meta).unwrap();
        assert_eq!(
            argv,
            vec![
                "kraken",
                "-o",
                "json",
                "--yes",
                "orderbook",
                "--count=10",
                "--",
                "BTCUSD"
            ]
        );
    }

    #[test]
    fn build_argv_flag_value_with_leading_dash_round_trips() {
        // Regression: a value beginning with '-' must survive the clap re-parse
        // instead of being read as an unknown flag.
        let meta = vec![pos("pair", 0), flag("since", "since")];
        let mut args = serde_json::Map::new();
        args.insert("pair".into(), serde_json::json!("BTCUSD"));
        args.insert("since".into(), serde_json::json!("-1"));
        let argv = build_argv("ohlc", &Some(args), &meta).unwrap();
        assert!(argv.contains(&"--since=-1".to_string()));
        let result = crate::Cli::try_parse_from(&argv);
        assert!(result.is_ok(), "clap parse failed: {:?}", result.err());
    }

    #[test]
    fn build_argv_flag_array_repeats_flag() {
        let meta = vec![flag("pair", "pair")];
        let mut args = serde_json::Map::new();
        args.insert("pair".into(), serde_json::json!(["BTCUSD", "ETHUSD"]));
        let argv = build_argv("volume", &Some(args), &meta).unwrap();
        let pairs: Vec<_> = argv
            .iter()
            .filter(|a| a.starts_with("--pair="))
            .cloned()
            .collect();
        assert_eq!(pairs, vec!["--pair=BTCUSD", "--pair=ETHUSD"]);
    }

    #[test]
    fn build_argv_option_bool_emits_value() {
        let meta = vec![flag("verified", "verified")];
        let mut args = serde_json::Map::new();
        args.insert("verified".into(), serde_json::json!(true));
        let argv = build_argv("withdrawal addresses", &Some(args), &meta).unwrap();
        assert!(argv.contains(&"--verified=true".to_string()));
    }

    #[test]
    fn build_argv_multiple_positionals_ordered() {
        let meta = vec![pos("asset", 0), pos("key", 1), pos("amount", 2)];
        let mut args = serde_json::Map::new();
        args.insert("amount".into(), serde_json::json!("100"));
        args.insert("asset".into(), serde_json::json!("XBT"));
        args.insert("key".into(), serde_json::json!("myaddr"));
        let argv = build_argv("withdraw", &Some(args), &meta).unwrap();
        let sep = argv.iter().position(|a| a == "--").unwrap();
        assert_eq!(argv[sep + 1], "XBT");
        assert_eq!(argv[sep + 2], "myaddr");
        assert_eq!(argv[sep + 3], "100");
    }

    #[test]
    fn acknowledged_stripped_from_argv() {
        let meta = vec![pos("pair", 0)];
        let mut args = serde_json::Map::new();
        args.insert("pair".into(), serde_json::json!("BTCUSD"));
        args.insert("acknowledged".into(), serde_json::json!(true));
        let argv = build_argv("ticker", &Some(args), &meta).unwrap();
        assert!(!argv.iter().any(|a| a.contains("acknowledged")));
    }

    /// Unknown keys are refused LOUDLY, never silently dropped: a global
    /// CLI flag injected past the schema (--api-key etc.) gets a refusal
    /// instead of a no-op, and a renamed parameter can never default its
    /// way into a wrong-capital account.
    #[test]
    fn build_argv_unknown_keys_are_refused_not_dropped() {
        let meta = vec![pos("pair", 0)];
        let mut args = serde_json::Map::new();
        args.insert("pair".into(), serde_json::json!("BTCUSD"));
        args.insert("api_key".into(), serde_json::json!("attacker-key"));
        let err = build_argv("ticker", &Some(args), &meta).unwrap_err();
        assert!(err.contains("unknown argument 'api_key'"), "{err}");
        assert!(
            err.contains("pair"),
            "the refusal names what IS accepted: {err}"
        );
        assert!(
            !err.contains("attacker"),
            "the refusal never echoes the injected value: {err}"
        );
    }

    /// A clap alias works through MCP exactly like the CLI flag it maps to
    /// — a public-schema call with `balance` reaches `--capital`.
    #[test]
    fn build_argv_honors_clap_aliases() {
        let meta = vec![ArgMeta {
            id: "capital".into(),
            long: Some("capital".into()),
            aliases: vec!["balance".into()],
            is_bool_flag: false,
            positional_index: None,
        }];
        let mut args = serde_json::Map::new();
        args.insert("balance".into(), serde_json::json!(12345));
        let argv = build_argv("paper init", &Some(args), &meta).unwrap();
        assert!(
            argv.contains(&"--capital=12345".to_string()),
            "alias maps to the canonical flag: {argv:?}"
        );
    }

    #[test]
    fn clap_parses_orderbook_with_positional_pair() {
        let meta = vec![pos("pair", 0), flag("count", "count")];
        let mut args = serde_json::Map::new();
        args.insert("pair".into(), serde_json::json!("BTCUSD"));
        args.insert("count".into(), serde_json::json!("10"));
        let argv = build_argv("orderbook", &Some(args), &meta).unwrap();
        let result = crate::Cli::try_parse_from(&argv);
        assert!(result.is_ok(), "clap parse failed: {:?}", result.err());
    }

    #[test]
    fn clap_parses_volume_with_flag_pair() {
        let meta = vec![flag("pair", "pair")];
        let mut args = serde_json::Map::new();
        args.insert("pair".into(), serde_json::json!(["BTCUSD"]));
        let argv = build_argv("volume", &Some(args), &meta).unwrap();
        let result = crate::Cli::try_parse_from(&argv);
        assert!(result.is_ok(), "clap parse failed: {:?}", result.err());
    }

    #[test]
    fn clap_parses_ticker_with_positional_pairs() {
        let meta = vec![pos("pairs", 0)];
        let mut args = serde_json::Map::new();
        args.insert("pairs".into(), serde_json::json!(["BTCUSD", "ETHUSD"]));
        let argv = build_argv("ticker", &Some(args), &meta).unwrap();
        let result = crate::Cli::try_parse_from(&argv);
        assert!(result.is_ok(), "clap parse failed: {:?}", result.err());
    }

    #[test]
    fn argument_keys_returns_sorted_keys() {
        let mut args = serde_json::Map::new();
        args.insert("pair".into(), serde_json::json!("BTCUSD"));
        args.insert("api_key".into(), serde_json::json!("my-key"));
        args.insert("count".into(), serde_json::json!("10"));
        let keys = argument_keys(&Some(args));
        assert_eq!(keys, vec!["api_key", "count", "pair"]);
    }

    #[test]
    fn argument_keys_empty_on_none() {
        let keys = argument_keys(&None);
        assert!(keys.is_empty());
    }

    #[test]
    fn argument_keys_do_not_include_values() {
        let mut args = serde_json::Map::new();
        args.insert("api_secret".into(), serde_json::json!("super-secret-value"));
        let keys = argument_keys(&Some(args));
        assert_eq!(keys, vec!["api_secret"]);
        assert!(!keys.iter().any(|k| k.contains("super-secret-value")));
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
                ts: "2026-07-01T00:00:00.000Z".parse().unwrap(),
                transport: MCP_TRANSPORT,
                event: AuditEvent::ToolCall {
                    tool: "ticker",
                    arg_keys: vec!["pair".to_string()],
                    arg_count: 1,
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
        assert_eq!(record["transport"], "stdio");
        assert_eq!(record["arg_count"], 1);
    }

    #[test]
    fn resolve_env_picks_up_spot_url() {
        let result =
            crate::client::resolve_url_override(Some("https://api.kraken.com"), false).unwrap();
        assert_eq!(result, Some("https://api.kraken.com".to_string()));
    }

    #[test]
    fn resolve_env_none_when_unset() {
        let result = crate::client::resolve_url_override(None, false).unwrap();
        assert_eq!(result, None);
    }
}

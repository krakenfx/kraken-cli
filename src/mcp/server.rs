/// MCP server implementation using rmcp over stdio transport.
///
/// Implements ServerHandler to handle initialize, tools/list, and tools/call
/// requests. Tool execution reuses existing CLI command handlers through the
/// shared dispatch path.
use std::borrow::Cow;
use std::env;
use std::sync::Arc;
use std::time::Instant;

use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, Implementation, InitializeRequestParams,
    InitializeResult, ListToolsResult, PaginatedRequestParams, ServerCapabilities, Tool,
};
use rmcp::service::{RequestContext, RoleServer};

use super::registry::ToolRegistry;
use crate::errors::KrakenError;
use crate::AppContext;

pub(crate) struct KrakenMcpServer {
    registry: ToolRegistry,
    ctx: Arc<AppContext>,
    allow_dangerous: bool,
    instructions: String,
}

impl KrakenMcpServer {
    pub(crate) fn new(
        registry: ToolRegistry,
        ctx: AppContext,
        allow_dangerous: bool,
        active_services: &[String],
    ) -> Self {
        let instructions = build_instructions(active_services, allow_dangerous);
        Self {
            registry,
            ctx: Arc::new(ctx),
            allow_dangerous,
            instructions,
        }
    }

    async fn dispatch_tool(
        &self,
        request: CallToolRequestParams,
    ) -> Result<CallToolResult, rmcp::model::ErrorData> {
        let tool_name = &request.name;
        let entry = self
            .registry
            .get_by_name(tool_name)
            .ok_or_else(|| rmcp::model::ErrorData {
                code: rmcp::model::ErrorCode::INVALID_PARAMS,
                message: Cow::Owned(format!("Unknown tool: {tool_name}")),
                data: None,
            })?;

        enforce_dangerous_gate(entry.dangerous, self.allow_dangerous, &request.arguments)?;

        let argv = build_argv(&entry.canonical_key, &request.arguments, &entry.clap_args);

        let parsed = match crate::Cli::try_parse_from(&argv) {
            Ok(cli) => cli,
            Err(e) => {
                return Err(rmcp::model::ErrorData {
                    code: rmcp::model::ErrorCode::INVALID_PARAMS,
                    message: Cow::Owned(format!("Argument validation failed: {e}")),
                    data: None,
                });
            }
        };

        let command = parsed.command.ok_or(rmcp::model::ErrorData {
            code: rmcp::model::ErrorCode::INVALID_PARAMS,
            message: Cow::Borrowed("No command parsed from arguments"),
            data: None,
        })?;

        match crate::execute_command(&self.ctx, command).await {
            Ok(output) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&output.data).unwrap_or_default(),
            )])),
            Err(e) => {
                let envelope = e.to_json_envelope();
                Ok(CallToolResult::error(vec![Content::text(
                    serde_json::to_string_pretty(&envelope).unwrap_or_default(),
                )]))
            }
        }
    }
}

fn build_instructions(active_services: &[String], allow_dangerous: bool) -> String {
    let svc_list = active_services.join(", ");
    let mode = if allow_dangerous {
        "autonomous"
    } else {
        "guarded"
    };

    let mut text = format!("Kraken exchange CLI tools. Active services: {svc_list}. Mode: {mode}.");

    let all_services = [
        "market",
        "account",
        "trade",
        "funding",
        "earn",
        "subaccount",
        "futures",
        "paper",
        "auth",
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
// Emits structured JSON events to stderr for every tool invocation.
// MCP uses stdio transport, so there is no TCP source address; the
// audit trail identifies callers by agent client, instance ID, and PID.
// To avoid sensitive data leakage, only argument metadata is logged.

const MCP_TRANSPORT: &str = "stdio";

fn audit_log(event: &serde_json::Value) {
    eprintln!(
        "[mcp audit] {}",
        serde_json::to_string(event).unwrap_or_default()
    );
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

fn audit_error_code(error: &rmcp::model::ErrorData) -> String {
    format!("{:?}", error.code)
}

impl ServerHandler for KrakenMcpServer {
    async fn initialize(
        &self,
        _request: InitializeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, rmcp::model::ErrorData> {
        audit_log(&serde_json::json!({
            "ts": now_iso(),
            "event": "session_start",
            "server_version": env!("CARGO_PKG_VERSION"),
            "agent": crate::telemetry::agent_client(),
            "instance_id": crate::telemetry::instance_id(),
            "pid": std::process::id(),
            "transport": MCP_TRANSPORT,
        }));
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
    ) -> Result<ListToolsResult, rmcp::model::ErrorData> {
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
    ) -> Result<CallToolResult, rmcp::model::ErrorData> {
        let tool_name = request.name.to_string();
        let started = Instant::now();
        let arg_keys = argument_keys(&request.arguments);
        let arg_count = arg_keys.len();

        audit_log(&serde_json::json!({
            "ts": now_iso(),
            "event": "tool_call",
            "tool": &tool_name,
            "arg_keys": arg_keys,
            "arg_count": arg_count,
            "agent": crate::telemetry::agent_client(),
            "instance_id": crate::telemetry::instance_id(),
            "pid": std::process::id(),
            "transport": MCP_TRANSPORT,
        }));

        let result = self.dispatch_tool(request).await;

        let (status, error_code) = match &result {
            Ok(_) => ("executed", None),
            Err(e) => ("rejected", Some(audit_error_code(e))),
        };

        audit_log(&serde_json::json!({
            "ts": now_iso(),
            "event": "tool_result",
            "tool": &tool_name,
            "status": status,
            "error_code": error_code,
            "duration_ms": started.elapsed().as_millis() as u64,
            "transport": MCP_TRANSPORT,
        }));

        result
    }
}

const DANGEROUS_GATE_ERROR: &str =
    "This operation modifies account state. Set \"acknowledged\": true to proceed, \
     or start the server with --allow-dangerous.";

fn enforce_dangerous_gate(
    dangerous: bool,
    allow_dangerous: bool,
    arguments: &Option<serde_json::Map<String, serde_json::Value>>,
) -> Result<(), rmcp::model::ErrorData> {
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
        Err(rmcp::model::ErrorData {
            code: rmcp::model::ErrorCode::INVALID_PARAMS,
            message: Cow::Borrowed(DANGEROUS_GATE_ERROR),
            data: None,
        })
    }
}

fn build_argv(
    canonical_key: &str,
    arguments: &Option<serde_json::Map<String, serde_json::Value>>,
    arg_meta: &[super::registry::ArgMeta],
) -> Vec<String> {
    let mut argv = vec![
        "kraken".to_string(),
        "-o".to_string(),
        "json".to_string(),
        "--yes".to_string(),
    ];

    let command_parts: Vec<&str> = canonical_key.split_whitespace().collect();
    argv.extend(command_parts.iter().map(|s| s.to_string()));

    let Some(args) = arguments else {
        return argv;
    };

    let mut positionals: Vec<(usize, Vec<String>)> = Vec::new();
    let mut flags: Vec<String> = Vec::new();

    for (key, value) in args {
        if value.is_null() || key == "acknowledged" {
            continue;
        }

        let meta = arg_meta.iter().find(|m| m.id == *key);

        match meta {
            Some(m) if m.positional_index.is_some() => {
                let idx = m.positional_index.unwrap();
                let vals = json_to_strings(value);
                positionals.push((idx, vals));
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
                // Fail-closed: drop keys not in the tool's arg_meta.
                // Prevents injection of global CLI flags (--api-key, --api-secret, etc.)
                // that are excluded from MCP schemas but could be sent by malicious clients.
                continue;
            }
        }
    }

    positionals.sort_by_key(|(idx, _)| *idx);
    for (_, vals) in positionals {
        argv.extend(vals);
    }
    argv.extend(flags);
    argv
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

fn emit_flag_values(flags: &mut Vec<String>, flag: &str, value: &serde_json::Value) {
    match value {
        serde_json::Value::Array(arr) => {
            for item in arr {
                flags.push(format!("--{flag}"));
                flags.push(match item {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                });
            }
        }
        serde_json::Value::String(s) => {
            flags.push(format!("--{flag}"));
            flags.push(s.clone());
        }
        serde_json::Value::Number(n) => {
            flags.push(format!("--{flag}"));
            flags.push(n.to_string());
        }
        serde_json::Value::Bool(b) => {
            flags.push(format!("--{flag}"));
            flags.push(b.to_string());
        }
        serde_json::Value::Null => {}
        other => {
            flags.push(format!("--{flag}"));
            flags.push(other.to_string());
        }
    }
}

pub(crate) async fn run_server(services: &str, allow_dangerous: bool) -> crate::errors::Result<()> {
    let parsed_services = super::parse_services(services)?;
    let active_services = super::apply_exclusions(&parsed_services);

    if active_services.is_empty() {
        return Err(KrakenError::Validation(
            "No REST-eligible services remain after filtering. \
             Streaming groups (websocket, futures-ws) are excluded in MCP v1."
                .into(),
        ));
    }

    let registry = ToolRegistry::build_with_options(&active_services, allow_dangerous)?;

    let resolve_env = |env_name: &str| -> crate::errors::Result<Option<String>> {
        let env_val = env::var(env_name).ok();
        crate::client::resolve_url_override(None, env_val.as_deref())
    };

    let api_url = resolve_env("KRAKEN_SPOT_URL")?;
    let futures_url = resolve_env("KRAKEN_FUTURES_URL")?;
    let ws_public_url = resolve_env("KRAKEN_WS_PUBLIC_URL")?;
    let ws_auth_url = resolve_env("KRAKEN_WS_AUTH_URL")?;
    let ws_l3_url = resolve_env("KRAKEN_WS_L3_URL")?;

    let ctx = AppContext {
        format: crate::output::OutputFormat::Json,
        verbose: false,
        api_url,
        futures_url,
        ws_public_url,
        ws_auth_url,
        ws_l3_url,
        api_key: None,
        api_secret: None,
        otp: None,
        force: true,
        secret_from_flag: false,
        mcp_mode: true,
    };

    let server = KrakenMcpServer::new(registry, ctx, allow_dangerous, &active_services);

    let mode_label = if allow_dangerous {
        "autonomous"
    } else {
        "guarded"
    };
    eprintln!(
        "kraken-cli MCP server v{} starting on stdio ({} tools, mode: {})",
        env!("CARGO_PKG_VERSION"),
        server.registry.tools().len(),
        mode_label,
    );

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
            is_bool_flag: false,
            positional_index: Some(index),
        }
    }

    fn flag(id: &str, long: &str) -> ArgMeta {
        ArgMeta {
            id: id.into(),
            long: Some(long.into()),
            is_bool_flag: false,
            positional_index: None,
        }
    }

    fn bool_flag(id: &str, long: &str) -> ArgMeta {
        ArgMeta {
            id: id.into(),
            long: Some(long.into()),
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

    // --- dangerous gate tests ---

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
        assert!(enforce_dangerous_gate(
            true,
            false,
            &args_with("acknowledged", serde_json::json!(true))
        )
        .is_ok());
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

    // --- instructions tests ---

    #[test]
    fn instructions_default_mode_lists_missing_services() {
        let active = vec!["market".into(), "account".into(), "paper".into()];
        let text = build_instructions(&active, false);
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
            "auth",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let text = build_instructions(&active, true);
        assert!(text.contains("Mode: autonomous"));
        assert!(!text.contains("Services not loaded"));
        assert!(!text.contains("acknowledged"));
    }

    // --- build_argv tests ---

    #[test]
    fn build_argv_simple_command() {
        let argv = build_argv("ticker", &None, &[]);
        assert_eq!(argv, vec!["kraken", "-o", "json", "--yes", "ticker"]);
    }

    #[test]
    fn build_argv_with_string_flag() {
        let meta = vec![flag("count", "count")];
        let mut args = serde_json::Map::new();
        args.insert("count".into(), serde_json::json!("10"));
        let argv = build_argv("orderbook", &Some(args), &meta);
        assert!(argv.contains(&"--count".to_string()));
        assert!(argv.contains(&"10".to_string()));
    }

    #[test]
    fn build_argv_with_bool_flag() {
        let meta = vec![bool_flag("trades", "trades")];
        let mut args = serde_json::Map::new();
        args.insert("trades".into(), serde_json::json!(true));
        let argv = build_argv("open-orders", &Some(args), &meta);
        assert!(argv.contains(&"--trades".to_string()));
    }

    #[test]
    fn build_argv_bool_false_omitted() {
        let meta = vec![bool_flag("trades", "trades")];
        let mut args = serde_json::Map::new();
        args.insert("trades".into(), serde_json::json!(false));
        let argv = build_argv("open-orders", &Some(args), &meta);
        assert!(!argv.contains(&"--trades".to_string()));
    }

    #[test]
    fn build_argv_with_positional_array() {
        let meta = vec![pos("pairs", 0)];
        let mut args = serde_json::Map::new();
        args.insert("pairs".into(), serde_json::json!(["BTCUSD", "ETHUSD"]));
        let argv = build_argv("ticker", &Some(args), &meta);
        let cmd_idx = argv.iter().position(|a| a == "ticker").unwrap();
        assert_eq!(argv[cmd_idx + 1], "BTCUSD");
        assert_eq!(argv[cmd_idx + 2], "ETHUSD");
        assert!(!argv.contains(&"--pairs".to_string()));
    }

    #[test]
    fn build_argv_nested_command() {
        let argv = build_argv("order buy", &None, &[]);
        assert_eq!(argv, vec!["kraken", "-o", "json", "--yes", "order", "buy"]);
    }

    #[test]
    fn build_argv_null_skipped() {
        let meta = vec![flag("since", "since")];
        let mut args = serde_json::Map::new();
        args.insert("since".into(), serde_json::Value::Null);
        let argv = build_argv("trades", &Some(args), &meta);
        assert!(!argv.contains(&"--since".to_string()));
    }

    #[test]
    fn build_argv_positional_before_flags() {
        let meta = vec![pos("pair", 0), flag("count", "count")];
        let mut args = serde_json::Map::new();
        args.insert("pair".into(), serde_json::json!("BTCUSD"));
        args.insert("count".into(), serde_json::json!("10"));
        let argv = build_argv("orderbook", &Some(args), &meta);
        assert_eq!(
            argv,
            vec![
                "kraken",
                "-o",
                "json",
                "--yes",
                "orderbook",
                "BTCUSD",
                "--count",
                "10"
            ]
        );
    }

    #[test]
    fn build_argv_flag_array_repeats_flag() {
        let meta = vec![flag("pair", "pair")];
        let mut args = serde_json::Map::new();
        args.insert("pair".into(), serde_json::json!(["BTCUSD", "ETHUSD"]));
        let argv = build_argv("volume", &Some(args), &meta);
        let pair_positions: Vec<_> = argv
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "--pair")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(pair_positions.len(), 2);
        assert_eq!(argv[pair_positions[0] + 1], "BTCUSD");
        assert_eq!(argv[pair_positions[1] + 1], "ETHUSD");
    }

    #[test]
    fn build_argv_option_bool_emits_value() {
        let meta = vec![flag("verified", "verified")];
        let mut args = serde_json::Map::new();
        args.insert("verified".into(), serde_json::json!(true));
        let argv = build_argv("withdrawal addresses", &Some(args), &meta);
        let idx = argv.iter().position(|a| a == "--verified").unwrap();
        assert_eq!(argv[idx + 1], "true");
    }

    #[test]
    fn build_argv_multiple_positionals_ordered() {
        let meta = vec![pos("asset", 0), pos("key", 1), pos("amount", 2)];
        let mut args = serde_json::Map::new();
        args.insert("amount".into(), serde_json::json!("100"));
        args.insert("asset".into(), serde_json::json!("XBT"));
        args.insert("key".into(), serde_json::json!("myaddr"));
        let argv = build_argv("withdraw", &Some(args), &meta);
        let cmd_idx = argv.iter().position(|a| a == "withdraw").unwrap();
        assert_eq!(argv[cmd_idx + 1], "XBT");
        assert_eq!(argv[cmd_idx + 2], "myaddr");
        assert_eq!(argv[cmd_idx + 3], "100");
    }

    #[test]
    fn acknowledged_stripped_from_argv() {
        let meta = vec![pos("pair", 0)];
        let mut args = serde_json::Map::new();
        args.insert("pair".into(), serde_json::json!("BTCUSD"));
        args.insert("acknowledged".into(), serde_json::json!(true));
        let argv = build_argv("ticker", &Some(args), &meta);
        assert!(!argv.iter().any(|a| a.contains("acknowledged")));
    }

    #[test]
    fn build_argv_unknown_keys_dropped() {
        let meta = vec![pos("pair", 0)];
        let mut args = serde_json::Map::new();
        args.insert("pair".into(), serde_json::json!("BTCUSD"));
        args.insert("api_key".into(), serde_json::json!("attacker-key"));
        args.insert("api_secret".into(), serde_json::json!("attacker-secret"));
        args.insert("some_random_flag".into(), serde_json::json!("malicious"));
        args.insert("verbose".into(), serde_json::json!(true));
        let argv = build_argv("ticker", &Some(args), &meta);
        assert!(!argv
            .iter()
            .any(|a| a.contains("api_key") || a.contains("api-key")));
        assert!(!argv
            .iter()
            .any(|a| a.contains("api_secret") || a.contains("api-secret")));
        assert!(!argv.iter().any(|a| a.contains("attacker")));
        assert!(!argv.iter().any(|a| a.contains("malicious")));
        assert!(!argv
            .iter()
            .any(|a| a.contains("some_random") || a.contains("some-random")));
        assert!(!argv.iter().any(|a| a == "--verbose"));
        assert!(argv.contains(&"BTCUSD".to_string()));
    }

    #[test]
    fn clap_parses_orderbook_with_positional_pair() {
        let meta = vec![pos("pair", 0), flag("count", "count")];
        let mut args = serde_json::Map::new();
        args.insert("pair".into(), serde_json::json!("BTCUSD"));
        args.insert("count".into(), serde_json::json!("10"));
        let argv = build_argv("orderbook", &Some(args), &meta);
        let result = crate::Cli::try_parse_from(&argv);
        assert!(result.is_ok(), "clap parse failed: {:?}", result.err());
    }

    #[test]
    fn clap_parses_volume_with_flag_pair() {
        let meta = vec![flag("pair", "pair")];
        let mut args = serde_json::Map::new();
        args.insert("pair".into(), serde_json::json!(["BTCUSD"]));
        let argv = build_argv("volume", &Some(args), &meta);
        let result = crate::Cli::try_parse_from(&argv);
        assert!(result.is_ok(), "clap parse failed: {:?}", result.err());
    }

    #[test]
    fn clap_parses_ticker_with_positional_pairs() {
        let meta = vec![pos("pairs", 0)];
        let mut args = serde_json::Map::new();
        args.insert("pairs".into(), serde_json::json!(["BTCUSD", "ETHUSD"]));
        let argv = build_argv("ticker", &Some(args), &meta);
        let result = crate::Cli::try_parse_from(&argv);
        assert!(result.is_ok(), "clap parse failed: {:?}", result.err());
    }

    // --- audit metadata tests ---

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
        let err = rmcp::model::ErrorData {
            code: rmcp::model::ErrorCode::INVALID_PARAMS,
            message: Cow::Borrowed("invalid"),
            data: None,
        };
        let code = audit_error_code(&err);
        assert!(!code.is_empty());
    }

    #[test]
    fn now_iso_is_valid_rfc3339() {
        let ts = now_iso();
        assert!(chrono::DateTime::parse_from_rfc3339(&ts).is_ok());
    }

    #[test]
    fn resolve_env_picks_up_spot_url() {
        let result =
            crate::client::resolve_url_override(None, Some("https://api.kraken.com")).unwrap();
        assert_eq!(result, Some("https://api.kraken.com".to_string()));
    }

    #[test]
    fn resolve_env_none_when_unset() {
        let result = crate::client::resolve_url_override(None, None).unwrap();
        assert_eq!(result, None);
    }
}

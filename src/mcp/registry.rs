/// MCP tool definitions derived from clap metadata and the public catalog.
use std::collections::HashMap;
use std::sync::Arc;

use rmcp::model::{Tool, ToolAnnotations};
use serde_json::Value;

use super::schema::clap_command_to_schema;
use crate::errors::{KrakenError, Result};

#[derive(Debug, Clone)]
pub(crate) struct ArgMeta {
    pub(crate) id: String,
    /// Long flag name; absent for positional arguments.
    pub(crate) long: Option<String>,
    /// clap aliases (e.g. `balance` for `--capital`) — MCP calls built
    /// against an older schema keep working through the same aliases the
    /// CLI honors.
    pub(crate) aliases: Vec<String>,
    /// Whether argv emits this argument as a presence flag.
    pub(crate) is_bool_flag: bool,
    /// 0-based position index for positional args. None for flag args.
    pub(crate) positional_index: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct ToolEntry {
    pub(crate) tool: Tool,
    pub(crate) canonical_key: String,
    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) group: String,
    /// Destructive under the server's resolved mode — the acknowledgment
    /// gate and the MCP annotations key off this, never off the raw
    /// catalog classification.
    pub(crate) armed: bool,
    pub(crate) clap_args: Vec<ArgMeta>,
}

/// The server's execution context, resolved once at startup from the
/// workspace it was scoped to. Promotion cannot change the mode while the
/// server is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GateMode {
    /// No workspace — the real Kraken account. A named live workspace also
    /// gates like Master: fail-closed.
    Master,
    /// A paper workspace: simulated fills, no real money reachable through
    /// the mode-routed verbs.
    Paper,
}

#[derive(Debug)]
pub(crate) struct ToolRegistry {
    tools: Vec<ToolEntry>,
    by_name: HashMap<String, usize>,
}

impl ToolRegistry {
    #[cfg(test)]
    pub(crate) fn build(active_services: &[String]) -> Result<Self> {
        Self::build_with_options(active_services, false, GateMode::Master)
    }

    pub(crate) fn build_with_options(
        active_services: &[String],
        allow_dangerous: bool,
        mode: GateMode,
    ) -> Result<Self> {
        let catalog = load_catalog()?;
        let clap_root = crate::Cli::command();

        let catalog_index = build_catalog_index(&catalog)?;
        let mut tools = Vec::new();
        let mut by_name = HashMap::new();

        collect_clap_tools(
            &clap_root,
            &catalog_index,
            active_services,
            allow_dangerous,
            mode,
            &mut tools,
        );

        for (i, entry) in tools.iter().enumerate() {
            by_name.insert(entry.tool.name.to_string(), i);
        }

        if tools.is_empty() {
            return Err(KrakenError::Validation(
                "No tools available after service filtering. Ensure at least one \
                 REST-eligible service group is specified."
                    .into(),
            ));
        }

        Ok(Self { tools, by_name })
    }

    pub(crate) fn tools(&self) -> &[ToolEntry] {
        &self.tools
    }

    pub(crate) fn get_by_name(&self, name: &str) -> Option<&ToolEntry> {
        self.by_name.get(name).map(|&i| &self.tools[i])
    }

    pub(crate) fn tool_definitions(&self) -> Vec<Tool> {
        self.tools.iter().map(|e| e.tool.clone()).collect()
    }
}

use clap::CommandFactory;

struct CatalogEntry {
    group: String,
    dangerous: bool,
    /// Additive annotation on the mode-routed trading verbs:
    /// dangerous against the real account, but a simulated fill inside a
    /// paper workspace — the gate relaxes only when both agree.
    paper_safe: bool,
    description: String,
}

impl CatalogEntry {
    /// Whether the tool is destructive under the resolved mode.
    fn armed(&self, mode: GateMode) -> bool {
        self.dangerous && !(self.paper_safe && mode == GateMode::Paper)
    }
}

fn load_catalog() -> Result<Value> {
    let catalog_bytes = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/agents/tool-catalog.json"
    ));
    serde_json::from_str(catalog_bytes).map_err(|e| KrakenError::Parse(e.to_string()))
}

fn build_catalog_index(catalog: &Value) -> Result<HashMap<String, CatalogEntry>> {
    let commands = catalog
        .get("commands")
        .and_then(|c| c.as_array())
        .ok_or_else(|| KrakenError::Parse("Catalog missing 'commands' array".into()))?;

    let mut index = HashMap::new();
    for cmd in commands {
        let raw_command = cmd
            .get("command")
            .and_then(|c| c.as_str())
            .unwrap_or_default();
        let key = canonical_key_from_catalog(raw_command);
        if key.is_empty() {
            continue;
        }
        let group = cmd
            .get("group")
            .and_then(|g| g.as_str())
            .unwrap_or("unknown")
            .to_string();
        let dangerous = cmd
            .get("dangerous")
            .and_then(|d| d.as_bool())
            .unwrap_or(false);
        let paper_safe = cmd
            .get("paper_safe")
            .and_then(|d| d.as_bool())
            .unwrap_or(false);
        let description = cmd
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_string();
        index.insert(
            key,
            CatalogEntry {
                group,
                dangerous,
                paper_safe,
                description,
            },
        );
    }
    Ok(index)
}

fn canonical_key_from_catalog(command_str: &str) -> String {
    command_str
        .split_whitespace()
        .skip(1) // skip "kraken"
        // Drop placeholder positionals in either notation: required `<PAIR>` and
        // optional `[PAIR...]`. Stripping only `<...>` left `[...]` tokens in the
        // key (e.g. `ws instrument [pair...]`), silently failing to match the
        // clap path.
        .filter(|t| {
            !t.starts_with('<') && !t.ends_with('>') && !t.starts_with('[') && !t.ends_with(']')
        })
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .replace('_', "-")
}

fn canonical_key_from_clap(path: &[&str]) -> String {
    path.to_vec().join(" ").to_lowercase().replace('_', "-")
}

fn tool_name_from_key(key: &str) -> String {
    format!("kraken_{}", key.replace([' ', '-'], "_"))
}

/// Visit every leaf command in the clap tree with its subcommand path
/// (e.g. `["order", "buy"]`), skipping clap's implicit `help` subcommand.
fn walk_clap_leaves<'a>(
    cmd: &'a clap::Command,
    path: &mut Vec<&'a str>,
    f: &mut impl FnMut(&clap::Command, &[&str]),
) {
    let subs: Vec<_> = cmd.get_subcommands().collect();
    if subs.is_empty() {
        if !path.is_empty() {
            f(cmd, path);
        }
        return;
    }
    for sub in subs {
        let sub_name = sub.get_name();
        if sub_name == "help" {
            continue;
        }
        path.push(sub_name);
        walk_clap_leaves(sub, path, f);
        path.pop();
    }
}

fn collect_clap_tools(
    root: &clap::Command,
    catalog_index: &HashMap<String, CatalogEntry>,
    active_services: &[String],
    allow_dangerous: bool,
    mode: GateMode,
    out: &mut Vec<ToolEntry>,
) {
    walk_clap_leaves(root, &mut Vec::new(), &mut |cmd, path| {
        let key = canonical_key_from_clap(path);
        let Some(catalog_entry) = catalog_index.get(&key) else {
            return;
        };
        if !active_services.contains(&catalog_entry.group) {
            return;
        }
        if super::schema::is_mcp_excluded_command(&key) {
            return;
        }
        let name = tool_name_from_key(&key);
        let armed = catalog_entry.armed(mode);
        let description = build_description(cmd, catalog_entry, armed);
        let mut input_schema = clap_command_to_schema(cmd);
        if armed && !allow_dangerous {
            super::schema::inject_dangerous_confirmation(&mut input_schema);
        }
        #[expect(
            clippy::expect_used,
            reason = "clap_command_to_schema builds the value as a JSON object unconditionally"
        )]
        let schema_obj: serde_json::Map<String, Value> = serde_json::from_value(input_schema)
            .expect("clap_command_to_schema always produces a JSON object");

        let mut tool = Tool::new(name, description, Arc::new(schema_obj));

        if armed {
            tool = tool.with_annotations(ToolAnnotations::new().destructive(true));
        }

        let clap_args = extract_clap_arg_meta(cmd);

        out.push(ToolEntry {
            tool,
            canonical_key: key,
            group: catalog_entry.group.clone(),
            armed,
            clap_args,
        });
    });
}

fn build_description(cmd: &clap::Command, catalog_entry: &CatalogEntry, armed: bool) -> String {
    let base = cmd
        .get_about()
        .map(|a| a.to_string())
        .or_else(|| {
            if !catalog_entry.description.is_empty() {
                Some(catalog_entry.description.clone())
            } else {
                None
            }
        })
        .unwrap_or_default();

    if armed {
        format!("[DANGEROUS: requires human confirmation] {base}")
    } else {
        base
    }
}

fn extract_clap_arg_meta(cmd: &clap::Command) -> Vec<ArgMeta> {
    let mut meta = Vec::new();
    let mut positional_idx = 0usize;

    for arg in cmd.get_arguments() {
        let id = arg.get_id().as_str();
        if id == "help" || id == "version" {
            continue;
        }
        if super::schema::is_mcp_excluded_arg(id) {
            continue;
        }

        let long = arg.get_long().map(|s| s.to_string());
        let aliases = arg
            .get_all_aliases()
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        let is_bool_flag = !arg.get_action().takes_values();
        let is_positional = long.is_none() && arg.get_short().is_none();

        let positional_index = if is_positional {
            let idx = positional_idx;
            positional_idx += 1;
            Some(idx)
        } else {
            None
        };

        meta.push(ArgMeta {
            id: id.to_string(),
            long,
            aliases,
            is_bool_flag,
            positional_index,
        });
    }

    meta
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_key_from_catalog_strips_kraken_and_placeholders() {
        assert_eq!(
            canonical_key_from_catalog("kraken server-time"),
            "server-time"
        );
        assert_eq!(
            canonical_key_from_catalog("kraken order buy <PAIR> <VOLUME>"),
            "order buy"
        );
        assert_eq!(
            canonical_key_from_catalog("kraken deposit methods <ASSET>"),
            "deposit methods"
        );
        assert_eq!(
            canonical_key_from_catalog("kraken ticker <PAIR...>"),
            "ticker"
        );
        // Optional-positional `[...]` notation must strip too, not just `<...>`.
        assert_eq!(
            canonical_key_from_catalog("kraken ws instrument [PAIR...]"),
            "ws instrument"
        );
    }

    #[test]
    fn canonical_key_from_clap_joins_path() {
        assert_eq!(canonical_key_from_clap(&["server-time"]), "server-time");
        assert_eq!(canonical_key_from_clap(&["order", "buy"]), "order buy");
    }

    #[test]
    fn tool_name_generation() {
        assert_eq!(tool_name_from_key("server-time"), "kraken_server_time");
        assert_eq!(tool_name_from_key("order buy"), "kraken_order_buy");
        assert_eq!(
            tool_name_from_key("deposit methods"),
            "kraken_deposit_methods"
        );
    }

    #[test]
    fn registry_builds_for_market() {
        let registry = ToolRegistry::build(&["market".into()]).unwrap();
        assert!(!registry.tools().is_empty());
        for entry in registry.tools() {
            assert_eq!(entry.group, "market");
        }
    }

    #[test]
    fn registry_rejects_empty_after_filter() {
        let err = ToolRegistry::build(&[]).unwrap_err().to_string();
        assert!(err.contains("No tools available"));
    }

    #[test]
    fn live_mode_tools_are_ungated_for_a_paper_workspace_registry() {
        // Inside a paper workspace the mode-routed order verbs simulate —
        // gating them would block unattended lab runs for no safety gain.
        let registry =
            ToolRegistry::build_with_options(&["trade".into()], false, GateMode::Paper).unwrap();
        let tool = registry.get_by_name("kraken_order_buy").expect("exposed");
        assert!(!tool.armed, "paper-mode order buy is not destructive");
        let schema = serde_json::to_value(tool.tool.input_schema.as_ref()).unwrap();
        assert!(
            schema["properties"].get("acknowledged").is_none(),
            "no acknowledgment field injected for a simulated fill"
        );
        // Always-dangerous tools stay gated regardless of mode.
        let withdraw_registry =
            ToolRegistry::build_with_options(&["funding".into()], false, GateMode::Paper).unwrap();
        let withdraw = withdraw_registry
            .get_by_name("kraken_withdraw")
            .expect("exposed");
        assert!(withdraw.armed, "withdraw is always dangerous");
    }

    #[test]
    fn live_mode_tools_stay_gated_for_master() {
        let registry =
            ToolRegistry::build_with_options(&["trade".into()], false, GateMode::Master).unwrap();
        let tool = registry.get_by_name("kraken_order_buy").expect("exposed");
        assert!(tool.armed, "master-mode order buy moves real money");
        let schema = serde_json::to_value(tool.tool.input_schema.as_ref()).unwrap();
        assert!(
            schema["properties"].get("acknowledged").is_some(),
            "acknowledgment stays required against the real account"
        );
    }

    /// Cross-check with `workspace_guard::no_paper_equivalent`: every surface
    /// the guard refuses inside a paper workspace must be classified
    /// always-dangerous in the catalog — a new money command that reaches
    /// neither list fails `mutating_commands_are_classified_dangerous`,
    /// and one that reaches only the catalog fails here by omission review.
    #[test]
    fn guard_refusal_set_is_always_dangerous_in_the_catalog() {
        const REFUSAL_CATALOG_KEYS: &[&str] = &[
            "order amend",
            "order edit",
            "order batch",
            "order cancel-batch",
            "order cancel-after",
            "withdraw",
            "wallet-transfer",
            "withdrawal cancel",
            "earn allocate",
            "earn deallocate",
            "subaccount transfer",
            "futures order buy",
            "futures order sell",
            "futures edit-order",
            "futures cancel",
            "futures cancel-all",
            "futures cancel-after",
            "futures batch-order",
            "futures transfer",
            "futures wallet-transfer",
            "futures set-subaccount-status",
            "ws add-order",
            "ws amend-order",
            "ws cancel-order",
            "ws cancel-all",
            "ws cancel-after",
            "ws batch-add",
            "ws batch-cancel",
        ];
        let catalog = load_catalog().unwrap();
        let index = build_catalog_index(&catalog).unwrap();
        for key in REFUSAL_CATALOG_KEYS {
            let entry = index
                .get(*key)
                .unwrap_or_else(|| panic!("guard refusal '{key}' has no catalog entry"));
            assert!(
                entry.dangerous && !entry.paper_safe,
                "guard refusal '{key}' must be dangerous in every mode"
            );
        }
    }

    #[test]
    fn dangerous_tools_have_annotation() {
        let registry = ToolRegistry::build(&["trade".into()]).unwrap();
        let dangerous_tools: Vec<_> = registry.tools().iter().filter(|e| e.armed).collect();
        for entry in &dangerous_tools {
            let desc = entry.tool.description.as_deref().unwrap_or("");
            assert!(
                desc.contains("[DANGEROUS"),
                "Tool {} missing danger prefix in description",
                entry.tool.name
            );
            assert!(
                entry
                    .tool
                    .annotations
                    .as_ref()
                    .and_then(|a| a.destructive_hint)
                    .unwrap_or(false),
                "Tool {} missing destructive_hint annotation",
                entry.tool.name
            );
        }
    }

    #[test]
    fn market_tools_not_dangerous() {
        let registry = ToolRegistry::build(&["market".into()]).unwrap();
        for entry in registry.tools() {
            assert!(
                !entry.armed,
                "Market tool {} should not be dangerous",
                entry.tool.name
            );
        }
    }

    #[test]
    fn registry_filters_by_service() {
        let market = ToolRegistry::build(&["market".into()]).unwrap();
        let all = ToolRegistry::build(&[
            "market".into(),
            "account".into(),
            "trade".into(),
            "funding".into(),
            "earn".into(),
            "subaccount".into(),
            "futures".into(),
            "paper".into(),
            "auth".into(),
        ])
        .unwrap();
        assert!(all.tools().len() > market.tools().len());
    }

    #[test]
    fn tool_lookup_by_name() {
        let registry = ToolRegistry::build(&["market".into()]).unwrap();
        let ticker = registry.get_by_name("kraken_ticker");
        assert!(ticker.is_some(), "Should find kraken_ticker tool");
    }

    #[test]
    fn explain_pnl_is_a_paper_tool_and_not_dangerous() {
        // `explain pnl` rides the default `paper` service set (agents are
        // its primary consumer) and is read-only.
        let registry = ToolRegistry::build(&["paper".into()]).unwrap();
        let tool = registry
            .get_by_name("kraken_explain_pnl")
            .expect("explain pnl exposed with the paper group");
        assert!(!tool.armed);
    }

    #[test]
    fn lab_score_is_a_paper_tool_and_not_dangerous() {
        // `lab score` rides the default `paper` service set like explain-pnl
        // (agents are its primary consumer) and is read-only.
        let registry = ToolRegistry::build(&["paper".into()]).unwrap();
        let tool = registry
            .get_by_name("kraken_lab_score")
            .expect("lab score exposed with the paper group");
        assert!(!tool.armed);
    }

    #[test]
    fn lab_compare_is_a_paper_tool_and_not_dangerous() {
        // `lab compare` is a read-side projection over recorded sessions; it
        // rides the same default `paper` service set as `lab score`.
        let registry = ToolRegistry::build(&["paper".into()]).unwrap();
        let tool = registry
            .get_by_name("kraken_lab_compare")
            .expect("lab compare exposed with the paper group");
        assert!(!tool.armed);
    }

    #[test]
    fn lab_next_is_a_paper_tool_and_not_dangerous() {
        // `lab next` is a derived read model over the sealed plan and the
        // session directories; it starts nothing itself.
        let registry = ToolRegistry::build(&["paper".into()]).unwrap();
        let tool = registry
            .get_by_name("kraken_lab_next")
            .expect("lab next exposed with the paper group");
        assert!(!tool.armed);
    }

    #[test]
    fn tape_list_is_a_market_tool_and_not_dangerous() {
        // The tape catalog is read-only local data; it rides the default
        // `market` service set so replay sources are discoverable everywhere.
        let registry = ToolRegistry::build(&["market".into()]).unwrap();
        let tool = registry
            .get_by_name("kraken_tape_list")
            .expect("tape list exposed with the market group");
        assert!(!tool.armed);
    }

    #[test]
    fn auth_excluded_commands() {
        let registry = ToolRegistry::build(&["auth".into()]).unwrap();
        assert!(
            registry.get_by_name("kraken_auth_set").is_none(),
            "auth set should be excluded from MCP registration"
        );
        assert!(
            registry.get_by_name("kraken_auth_reset").is_none(),
            "auth reset should be excluded from MCP registration"
        );
        assert!(
            registry.get_by_name("kraken_auth_show").is_some(),
            "auth show should remain registered"
        );
        assert!(
            registry.get_by_name("kraken_auth_test").is_some(),
            "auth test should remain registered"
        );
    }

    #[test]
    fn dangerous_tools_have_acknowledged_in_schema() {
        let registry = ToolRegistry::build(&["trade".into()]).unwrap();
        let dangerous_tools: Vec<_> = registry.tools().iter().filter(|e| e.armed).collect();
        assert!(
            !dangerous_tools.is_empty(),
            "trade group should have dangerous tools"
        );
        for entry in &dangerous_tools {
            let schema = &entry.tool.input_schema;
            let props = schema
                .get("properties")
                .and_then(|p| p.as_object())
                .expect("schema should have properties");
            assert!(
                props.contains_key("acknowledged"),
                "Dangerous tool {} missing acknowledged in schema",
                entry.tool.name
            );
        }
    }

    #[test]
    fn websocket_tools_excluded() {
        let services = super::super::apply_exclusions(&["websocket".into()]);
        if services.is_empty() {
            return;
        }
        let registry = ToolRegistry::build(&services);
        if let Ok(r) = registry {
            for entry in r.tools() {
                assert_ne!(entry.group, "websocket");
                assert_ne!(entry.group, "futures-ws");
            }
        }
    }

    /// Guardrail against silent MCP tool loss.
    ///
    /// Tools are registered by matching a clap subcommand path against a
    /// `command` string in `agents/tool-catalog.json` (`collect_clap_tools`);
    /// a mismatch on either side drops the tool with no error. This pins the
    /// two command surfaces together so a rename/addition fails loudly here
    /// instead of silently vanishing a tool. `CLAP_ONLY` lists the interactive
    /// commands deliberately absent from the (request/response) tool catalog.
    #[test]
    fn catalog_and_clap_command_surfaces_match() {
        const CLAP_ONLY: &[&str] = &[
            "mcp",
            "record",
            "replay",
            "streamd start",
            // `run start` streams (the dispatcher intercepts it) — a catalog
            // entry would mint an MCP tool that can only refuse; bare
            // `playground` is its one-command demo composition.
            "session start",
            "playground",
            // `ws ping` is a live-connection diagnostic, not a request/response tool.
            "ws ping",
        ];

        let catalog = load_catalog().unwrap();
        let catalog_index = build_catalog_index(&catalog).unwrap();
        let catalog_keys: std::collections::BTreeSet<String> =
            catalog_index.keys().cloned().collect();

        let root = crate::Cli::command();
        let mut clap_keys = std::collections::BTreeSet::new();
        walk_clap_leaves(&root, &mut Vec::new(), &mut |_, path| {
            clap_keys.insert(canonical_key_from_clap(path));
        });

        // The allowlist polices itself: an entry naming no clap path is drift
        // (a removed or renamed command) and would silently widen the filter.
        let stale_allowlist: Vec<_> = CLAP_ONLY
            .iter()
            .filter(|k| !clap_keys.contains(**k))
            .collect();
        assert!(
            stale_allowlist.is_empty(),
            "CLAP_ONLY names commands with no clap path — remove the stale \
             entries: {stale_allowlist:?}"
        );

        let missing_from_catalog: Vec<_> = clap_keys
            .iter()
            .filter(|k| !catalog_keys.contains(*k) && !CLAP_ONLY.contains(&k.as_str()))
            .collect();
        assert!(
            missing_from_catalog.is_empty(),
            "clap commands with no agents/tool-catalog.json entry \
             (add a catalog entry or list in CLAP_ONLY): {missing_from_catalog:?}"
        );

        let missing_from_clap: Vec<_> = catalog_keys
            .iter()
            .filter(|k| !clap_keys.contains(*k))
            .collect();
        assert!(
            missing_from_clap.is_empty(),
            "catalog commands with no matching clap path \
             (a rename in clap silently dropped an MCP tool): {missing_from_clap:?}"
        );
    }

    /// A catalog command string must show exactly the positional arguments the
    /// binary accepts — no more, no fewer. `catalog_and_clap_command_surfaces_match`
    /// strips the `<...>`/`[...]` placeholders before comparing paths, so both
    /// a *phantom* placeholder (one the CLI has no positional for — an agent
    /// that supplies it gets `exit 2`, how `explain pnl [NAME]` shipped) and an
    /// *omission* (a real positional the string hides, e.g. `workspace create`
    /// without its `<NAME>`) slip past it. Pin the count in both directions
    /// against the clap positionals.
    #[test]
    fn catalog_command_placeholders_match_clap_positionals() {
        let root = crate::Cli::command();
        let mut clap_positionals = std::collections::HashMap::new();
        walk_clap_leaves(&root, &mut Vec::new(), &mut |cmd, path| {
            clap_positionals.insert(canonical_key_from_clap(path), cmd.get_positionals().count());
        });

        let catalog = load_catalog().unwrap();
        for cmd in catalog["commands"].as_array().unwrap() {
            let command = cmd["command"].as_str().unwrap();
            let placeholders = command
                .split_whitespace()
                .filter(|t| {
                    (t.starts_with('<') && t.ends_with('>'))
                        || (t.starts_with('[') && t.ends_with(']'))
                })
                .count();
            let key = canonical_key_from_catalog(command);
            let positionals = clap_positionals.get(&key).copied().unwrap_or(0);
            assert_eq!(
                placeholders, positionals,
                "`{command}`: {placeholders} positional placeholder(s) in the \
                 string but the CLI has {positionals} — the string must show \
                 exactly the binary's positionals (a phantom fails a call with \
                 exit 2; an omission hides a real argument)"
            );
        }
    }

    #[test]
    fn catalog_parameter_types_use_canonical_vocabulary() {
        const TYPES: &[&str] = &[
            "boolean",
            "decimal",
            "integer",
            "integer[]",
            "number",
            "string",
            "string[]",
        ];

        let catalog = load_catalog().unwrap();
        let invalid: Vec<_> = catalog["commands"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|command| {
                command["parameters"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|parameter| {
                        let parameter_type = parameter["type"].as_str().unwrap();
                        (!TYPES.contains(&parameter_type)).then(|| {
                            format!(
                                "{} {}: {parameter_type}",
                                command["name"].as_str().unwrap(),
                                parameter["name"].as_str().unwrap()
                            )
                        })
                    })
            })
            .collect();

        assert!(
            invalid.is_empty(),
            "catalog parameters use non-canonical type notation: {invalid:?}"
        );
    }

    /// The catalog is the agent contract, so omitting a real option is as
    /// breaking as removing it from clap: generated callers cannot discover it.
    #[test]
    fn catalog_options_match_clap_options() {
        type OptionNames = std::collections::BTreeSet<String>;
        type CommandOptions = (Vec<OptionNames>, OptionNames);

        fn argument_names(argument: &clap::Arg) -> OptionNames {
            argument
                .get_long()
                .into_iter()
                .chain(argument.get_all_aliases().into_iter().flatten())
                .filter(|name| !matches!(*name, "help" | "version"))
                .map(str::to_owned)
                .collect()
        }

        fn walk_options(
            command: &clap::Command,
            path: &mut Vec<String>,
            inherited_globals: &OptionNames,
            commands: &mut std::collections::HashMap<String, CommandOptions>,
        ) {
            let mut globals = inherited_globals.clone();
            for argument in command
                .get_arguments()
                .filter(|argument| argument.is_global_set())
            {
                globals.extend(argument_names(argument));
            }

            let subcommands: Vec<_> = command.get_subcommands().collect();
            if subcommands.is_empty() {
                let required = command
                    .get_arguments()
                    .filter(|argument| !argument.is_global_set() && !argument.is_hide_set())
                    .map(argument_names)
                    .filter(|names| !names.is_empty())
                    .collect::<Vec<_>>();
                let mut accepted = globals;
                accepted.extend(required.iter().flatten().cloned());
                let path = path.iter().map(String::as_str).collect::<Vec<_>>();
                commands.insert(canonical_key_from_clap(&path), (required, accepted));
                return;
            }

            for subcommand in subcommands {
                path.push(subcommand.get_name().to_owned());
                walk_options(subcommand, path, &globals, commands);
                path.pop();
            }
        }

        let catalog = load_catalog().unwrap();
        let root = crate::Cli::command();
        let mut clap_options = std::collections::HashMap::new();
        walk_options(
            &root,
            &mut Vec::new(),
            &OptionNames::new(),
            &mut clap_options,
        );

        let mut mismatches = Vec::new();
        for command in catalog["commands"].as_array().unwrap() {
            let command_text = command["command"].as_str().unwrap();
            let key = canonical_key_from_catalog(command_text);
            let documented = command["parameters"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|parameter| parameter["name"].as_str())
                .filter_map(|name| name.strip_prefix("--"))
                .map(str::to_owned)
                .collect::<std::collections::BTreeSet<_>>();
            let (required, accepted) = clap_options.get(&key).unwrap();
            let missing = required
                .iter()
                .filter(|aliases| aliases.is_disjoint(&documented))
                .map(|aliases| aliases.iter().cloned().collect::<Vec<_>>())
                .collect::<Vec<_>>();
            let unknown = documented.difference(accepted).cloned().collect::<Vec<_>>();

            if !missing.is_empty() || !unknown.is_empty() {
                mismatches.push(format!(
                    "`{command_text}`: missing={missing:?}, unknown={unknown:?}"
                ));
            }
        }
        assert!(
            mismatches.is_empty(),
            "catalog options differ from clap:\n{}",
            mismatches.join("\n")
        );
    }

    /// Pin the set of commands flagged `dangerous` in `agents/tool-catalog.json`
    /// — the single source of truth consumed by the MCP confirmation gate
    /// (`enforce_dangerous_gate`). Changing the danger classification must be a
    /// conscious edit here, so this fails loudly if a flag is added or removed,
    /// guarding against silently un-gating a real-money operation. Keep in sync
    /// with CLAUDE.md ("41 commands").
    #[test]
    fn dangerous_command_set_is_pinned() {
        const EXPECTED_DANGEROUS: &[&str] = &[
            "auth reset",
            "earn allocate",
            "earn deallocate",
            "export-delete",
            "export-retrieve",
            "futures batch-order",
            "futures cancel",
            "futures cancel-after",
            "futures cancel-all",
            "futures edit-order",
            "futures order buy",
            "futures order sell",
            "futures set-leverage",
            "futures set-pnl-preference",
            "futures set-subaccount-status",
            "futures transfer",
            "futures wallet-transfer",
            "order amend",
            "order batch",
            "order buy",
            "order cancel",
            "order cancel-after",
            "order cancel-all",
            "order cancel-batch",
            "order edit",
            "order sell",
            "paper reset",
            "subaccount create",
            "subaccount transfer",
            "wallet-transfer",
            "withdraw",
            "withdrawal cancel",
            "workspace promote",
            "workspace reset",
            "ws add-order",
            "ws amend-order",
            "ws batch-add",
            "ws batch-cancel",
            "ws cancel-after",
            "ws cancel-all",
            "ws cancel-order",
        ];

        /// The additive paper_safe annotation: dangerous against the real
        /// account, simulated inside a paper workspace.
        const EXPECTED_PAPER_SAFE: &[&str] = &[
            "order buy",
            "order cancel",
            "order cancel-all",
            "order sell",
        ];

        let catalog = load_catalog().unwrap();
        let index = build_catalog_index(&catalog).unwrap();
        let mut actual: Vec<String> = index
            .iter()
            .filter(|(_, e)| e.dangerous)
            .map(|(k, _)| k.clone())
            .collect();
        actual.sort();
        let mut expected: Vec<String> = EXPECTED_DANGEROUS.iter().map(|s| s.to_string()).collect();
        expected.sort();
        assert_eq!(
            actual, expected,
            "tool-catalog.json dangerous set changed; if intentional, update \
             EXPECTED_DANGEROUS here and the count in CLAUDE.md"
        );
        let mut paper_safe: Vec<String> = index
            .iter()
            .filter(|(_, e)| e.paper_safe)
            .map(|(k, _)| k.clone())
            .collect();
        paper_safe.sort();
        assert_eq!(
            paper_safe,
            EXPECTED_PAPER_SAFE
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
            "paper_safe is exactly the mode-routed trading verb set"
        );
        for key in &paper_safe {
            assert!(
                index[key].dangerous,
                "paper_safe only relaxes a dangerous tool; '{key}' is not dangerous"
            );
        }
    }

    /// Forcing function the pinned-set test cannot be: it only detects edits to
    /// already-classified commands, so a NEW mutating command shipped with
    /// `dangerous: false` would sail through. Here every catalog key whose words
    /// match a mutating verb must be flagged dangerous or consciously exempted.
    /// Do not weaken MUTATING_VERBS to make this pass; investigate every hit.
    /// `set`/`create`/`delete` are in the list because a settings mutation is
    /// destructive with no money verb anywhere in its name.
    #[test]
    fn mutating_commands_are_classified_dangerous() {
        const MUTATING_VERBS: &[&str] = &[
            "order",
            "buy",
            "sell",
            "cancel",
            "withdraw",
            "transfer",
            "allocate",
            "deallocate",
            "amend",
            "edit",
            "batch",
            "reset",
            "stake",
            "unstake",
            "set",
            "create",
            "delete",
        ];
        // Adding a key here is the explicit "reviewed, mutates nothing on the
        // real account" decision. Each entry needs a justification.
        const SAFE_EXCEPTIONS: &[&str] = &[
            // Read-only status queries on prior allocations/orders.
            "earn allocate-status",
            "earn deallocate-status",
            "futures order-status",
            // Local credential keystore, and excluded from MCP entirely.
            "auth set",
            // Local manifest; real-account routing is gated at `workspace promote`.
            "workspace create",
            // Local session cursor, unauthenticated.
            "session state set",
        ];

        let catalog = load_catalog().unwrap();
        let index = build_catalog_index(&catalog).unwrap();
        for (key, entry) in &index {
            let words: Vec<&str> = key.split([' ', '-']).collect();
            // Paper engines simulate against live prices; no real money moves.
            if words.contains(&"paper") {
                continue;
            }
            if !words.iter().any(|w| MUTATING_VERBS.contains(w)) {
                continue;
            }
            if SAFE_EXCEPTIONS.contains(&key.as_str()) {
                continue;
            }
            assert!(
                entry.dangerous,
                "`{key}` matches a mutating verb but has dangerous: false in \
                 agents/tool-catalog.json; either flag it dangerous (and update \
                 dangerous_command_set_is_pinned + CLAUDE.md) or add it to \
                 SAFE_EXCEPTIONS with a justification"
            );
        }
    }

    /// Pin the numeric surface claims in the agent-facing docs to the catalogs,
    /// so adding a command (or error category) can't silently strand the
    /// "N commands" prose again. Any number written before "commands" must be
    /// the catalog total or its dangerous count; "N groups" must match the
    /// catalog's group map; "N error categories" must match the error catalog.
    #[test]
    fn doc_surface_count_claims_match_the_catalogs() {
        const DOCS: &[(&str, &str)] = &[
            (
                "README.md",
                include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")),
            ),
            (
                "CLAUDE.md",
                include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/CLAUDE.md")),
            ),
            (
                "CONTEXT.md",
                include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/CONTEXT.md")),
            ),
            (
                "AGENTS.md",
                include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/AGENTS.md")),
            ),
            (
                "agents/README.md",
                include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/agents/README.md")),
            ),
        ];

        /// Numbers written immediately before `unit`, e.g. "173 commands".
        fn count_claims(text: &str, unit: &str) -> Vec<usize> {
            text.match_indices(unit)
                .filter_map(|(idx, _)| {
                    let digits: String = text[..idx]
                        .chars()
                        .rev()
                        .take_while(char::is_ascii_digit)
                        .collect();
                    digits.chars().rev().collect::<String>().parse().ok()
                })
                .collect()
        }

        let catalog = load_catalog().unwrap();
        let commands = catalog["commands"].as_array().unwrap();
        let total = commands.len();
        let dangerous = commands
            .iter()
            .filter(|c| c["dangerous"].as_bool() == Some(true))
            .count();
        let groups = catalog["groups"].as_object().unwrap().len();
        let errors: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/agents/error-catalog.json"
        )))
        .unwrap();
        let categories = errors["categories"].as_array().unwrap().len();

        let mut command_claims = 0;
        for (name, text) in DOCS {
            for n in count_claims(text, " commands") {
                command_claims += 1;
                assert!(
                    n == total || n == dangerous,
                    "{name} claims {n} commands; the catalog has {total} \
                     (dangerous: {dangerous})"
                );
            }
            for n in count_claims(text, " groups") {
                assert_eq!(
                    n, groups,
                    "{name} claims {n} groups; the catalog has {groups}"
                );
            }
            for n in count_claims(text, " error categories") {
                assert_eq!(
                    n, categories,
                    "{name} claims {n} error categories; the error catalog has {categories}"
                );
            }
        }
        assert!(
            command_claims >= 5,
            "only {command_claims} command-count claims found — if the docs \
             dropped their numbers, retire this pin consciously"
        );
    }
}

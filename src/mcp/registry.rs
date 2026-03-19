/// Tool registry: merges clap command metadata with the tool catalog
/// to produce MCP tool definitions.
use std::collections::HashMap;
use std::sync::Arc;

use rmcp::model::{Tool, ToolAnnotations};
use serde_json::Value;

use super::schema::clap_command_to_schema;
use crate::errors::{KrakenError, Result};

#[derive(Debug, Clone)]
pub(crate) struct ArgMeta {
    pub(crate) id: String,
    /// Long flag name (e.g., "count", "asset-class"). None for positional args.
    pub(crate) long: Option<String>,
    /// True for SetTrue/SetFalse/Count actions that emit as presence flags.
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
    pub(crate) dangerous: bool,
    pub(crate) clap_args: Vec<ArgMeta>,
}

#[derive(Debug)]
pub(crate) struct ToolRegistry {
    tools: Vec<ToolEntry>,
    by_name: HashMap<String, usize>,
}

impl ToolRegistry {
    #[cfg(test)]
    pub(crate) fn build(active_services: &[String]) -> Result<Self> {
        Self::build_with_options(active_services, false)
    }

    pub(crate) fn build_with_options(
        active_services: &[String],
        allow_dangerous: bool,
    ) -> Result<Self> {
        let catalog = load_catalog()?;
        let clap_root = crate::Cli::command();

        let catalog_index = build_catalog_index(&catalog)?;
        let mut tools = Vec::new();
        let mut by_name = HashMap::new();

        collect_clap_tools(
            &clap_root,
            &[],
            &catalog_index,
            active_services,
            allow_dangerous,
            &mut tools,
        )?;

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
    description: String,
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
        .filter(|t| !t.starts_with('<') && !t.ends_with('>'))
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

fn collect_clap_tools(
    cmd: &clap::Command,
    parent_path: &[&str],
    catalog_index: &HashMap<String, CatalogEntry>,
    active_services: &[String],
    allow_dangerous: bool,
    out: &mut Vec<ToolEntry>,
) -> Result<()> {
    let subs: Vec<_> = cmd.get_subcommands().collect();

    if subs.is_empty() && !parent_path.is_empty() {
        let key = canonical_key_from_clap(parent_path);
        if let Some(catalog_entry) = catalog_index.get(&key) {
            if !active_services.contains(&catalog_entry.group) {
                return Ok(());
            }
            if super::schema::is_mcp_excluded_command(&key) {
                return Ok(());
            }
            let name = tool_name_from_key(&key);
            let description = build_description(cmd, catalog_entry);
            let mut input_schema = clap_command_to_schema(cmd);
            if catalog_entry.dangerous && !allow_dangerous {
                super::schema::inject_dangerous_confirmation(&mut input_schema);
            }
            let input_schema = input_schema;
            let schema_obj: serde_json::Map<String, Value> =
                serde_json::from_value(input_schema).unwrap_or_default();

            let mut tool = Tool::new(name.clone(), description, Arc::new(schema_obj));

            if catalog_entry.dangerous {
                tool = tool.with_annotations(ToolAnnotations::from_raw(
                    None,
                    None,
                    Some(true),
                    None,
                    None,
                ));
            }

            let clap_args = extract_clap_arg_meta(cmd);

            out.push(ToolEntry {
                tool,
                canonical_key: key,
                group: catalog_entry.group.clone(),
                dangerous: catalog_entry.dangerous,
                clap_args,
            });
        }
        return Ok(());
    }

    for sub in subs {
        let sub_name = sub.get_name();
        if sub_name == "help" {
            continue;
        }
        let mut path = parent_path.to_vec();
        path.push(sub_name);
        collect_clap_tools(
            sub,
            &path,
            catalog_index,
            active_services,
            allow_dangerous,
            out,
        )?;
    }

    Ok(())
}

fn build_description(cmd: &clap::Command, catalog_entry: &CatalogEntry) -> String {
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

    if catalog_entry.dangerous {
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
    fn dangerous_tools_have_annotation() {
        let registry = ToolRegistry::build(&["trade".into()]).unwrap();
        let dangerous_tools: Vec<_> = registry.tools().iter().filter(|e| e.dangerous).collect();
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
                !entry.dangerous,
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
        let dangerous_tools: Vec<_> = registry.tools().iter().filter(|e| e.dangerous).collect();
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
}

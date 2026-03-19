/// Derives JSON Schema from clap command argument metadata.
///
/// Positional args become explicit named properties. Optional flags become
/// optional properties. Bool flags become boolean type. All others default
/// to string type.
use serde_json::{json, Value};

pub(crate) fn clap_command_to_schema(cmd: &clap::Command) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();

    for arg in cmd.get_arguments() {
        let id = arg.get_id().as_str();
        if id == "help" || id == "version" {
            continue;
        }

        // Skip global flags that are handled by the MCP context
        if is_mcp_excluded_arg(id) {
            continue;
        }

        let mut prop = serde_json::Map::new();

        let is_bool = !arg.get_action().takes_values();
        let is_repeatable = matches!(
            arg.get_action(),
            clap::ArgAction::Append | clap::ArgAction::Count
        );
        let num_vals = arg.get_num_args();
        let is_multi = num_vals
            .map(|r| r.max_values() > 1 || r.max_values() == 0)
            .unwrap_or(false);

        if is_bool {
            prop.insert("type".into(), json!("boolean"));
        } else if is_repeatable || is_multi {
            prop.insert("type".into(), json!("array"));
            prop.insert("items".into(), json!({"type": "string"}));
        } else {
            let possible_values: Vec<String> = arg
                .get_possible_values()
                .iter()
                .map(|v| v.get_name().to_string())
                .collect();

            if !possible_values.is_empty() {
                prop.insert("type".into(), json!("string"));
                prop.insert(
                    "enum".into(),
                    Value::Array(possible_values.into_iter().map(Value::String).collect()),
                );
            } else {
                prop.insert("type".into(), json!("string"));
            }
        }

        if let Some(help) = arg.get_help() {
            prop.insert("description".into(), Value::String(help.to_string()));
        }

        if let Some(default) = arg.get_default_values().first() {
            prop.insert(
                "default".into(),
                Value::String(default.to_string_lossy().to_string()),
            );
        }

        properties.insert(id.to_string(), Value::Object(prop));

        if arg.is_required_set() {
            required.push(Value::String(id.to_string()));
        }
    }

    let mut schema = serde_json::Map::new();
    schema.insert("type".into(), json!("object"));
    schema.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".into(), Value::Array(required));
    }
    schema.insert("additionalProperties".into(), json!(false));

    Value::Object(schema)
}

/// Injects a required `acknowledged` boolean into the schema for dangerous tools.
/// The caller must set this to `true` to confirm intent.
pub(crate) fn inject_dangerous_confirmation(schema: &mut Value) {
    if let Some(props) = schema.get_mut("properties").and_then(|p| p.as_object_mut()) {
        let mut prop = serde_json::Map::new();
        prop.insert("type".into(), json!("boolean"));
        prop.insert(
            "description".into(),
            json!("Set to true to confirm you intend to execute this operation."),
        );
        props.insert("acknowledged".into(), Value::Object(prop));
    }
    if let Some(req) = schema.get_mut("required").and_then(|r| r.as_array_mut()) {
        req.push(json!("acknowledged"));
    } else {
        schema
            .as_object_mut()
            .map(|s| s.insert("required".into(), json!(["acknowledged"])));
    }
}

/// Returns true for args that must not appear in MCP tool schemas.
///
/// Covers global CLI flags (handled by the MCP execution context) and
/// stdin-consuming flags that would corrupt the JSON-RPC transport.
pub(crate) fn is_mcp_excluded_arg(id: &str) -> bool {
    matches!(
        id,
        "output"
            | "verbose"
            | "api_url"
            | "futures_url"
            | "api_key"
            | "api_secret"
            | "api_secret_stdin"
            | "api_secret_file"
            | "otp"
            | "yes"
            | "futures_api_key"
            | "futures_api_secret"
            | "futures_api_secret_stdin"
            | "futures_api_secret_file"
    )
}

/// Commands excluded from MCP registration.
///
/// `auth set`: requires secret input incompatible with MCP's stateless
/// tool-call model. Its local arg ids collide with global exclusion ids,
/// stripping required args from schemas.
///
/// `auth reset`: deletes stored credentials, a local-only destructive
/// operation with no meaningful use from an MCP client.
pub(crate) fn is_mcp_excluded_command(canonical_key: &str) -> bool {
    matches!(canonical_key, "auth set" | "auth reset")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Arg, Command};

    fn make_cmd() -> Command {
        Command::new("test")
            .arg(Arg::new("pair").required(true).help("Trading pair"))
            .arg(
                Arg::new("count")
                    .long("count")
                    .default_value("25")
                    .help("Number of levels"),
            )
            .arg(
                Arg::new("trades")
                    .long("trades")
                    .action(clap::ArgAction::SetTrue),
            )
    }

    #[test]
    fn schema_has_object_type() {
        let schema = clap_command_to_schema(&make_cmd());
        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn required_positional_in_required_array() {
        let schema = clap_command_to_schema(&make_cmd());
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("pair")));
    }

    #[test]
    fn optional_flag_not_in_required() {
        let schema = clap_command_to_schema(&make_cmd());
        let required = schema["required"].as_array().cloned().unwrap_or_default();
        assert!(!required.contains(&json!("count")));
    }

    #[test]
    fn bool_flag_has_boolean_type() {
        let schema = clap_command_to_schema(&make_cmd());
        assert_eq!(schema["properties"]["trades"]["type"], "boolean");
    }

    #[test]
    fn string_arg_has_string_type() {
        let schema = clap_command_to_schema(&make_cmd());
        assert_eq!(schema["properties"]["pair"]["type"], "string");
    }

    #[test]
    fn default_value_present() {
        let schema = clap_command_to_schema(&make_cmd());
        assert_eq!(schema["properties"]["count"]["default"], "25");
    }

    #[test]
    fn additional_properties_false() {
        let schema = clap_command_to_schema(&make_cmd());
        assert_eq!(schema["additionalProperties"], false);
    }

    #[test]
    fn enum_values_from_possible_values() {
        let cmd =
            Command::new("test").arg(Arg::new("mode").long("mode").value_parser(["fast", "slow"]));
        let schema = clap_command_to_schema(&cmd);
        let enum_vals = schema["properties"]["mode"]["enum"].as_array().unwrap();
        assert!(enum_vals.contains(&json!("fast")));
        assert!(enum_vals.contains(&json!("slow")));
    }

    #[test]
    fn multi_value_arg_becomes_array() {
        let cmd = Command::new("test").arg(Arg::new("pairs").num_args(1..).help("Trading pairs"));
        let schema = clap_command_to_schema(&cmd);
        assert_eq!(schema["properties"]["pairs"]["type"], "array");
    }

    #[test]
    fn global_flags_excluded() {
        let cmd = Command::new("test")
            .arg(Arg::new("pair").required(true))
            .arg(Arg::new("output").long("output"))
            .arg(
                Arg::new("verbose")
                    .long("verbose")
                    .action(clap::ArgAction::SetTrue),
            )
            .arg(Arg::new("api_key").long("api-key"));
        let schema = clap_command_to_schema(&cmd);
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("pair"));
        assert!(!props.contains_key("output"));
        assert!(!props.contains_key("api_key"));
    }

    #[test]
    fn excluded_commands() {
        assert!(is_mcp_excluded_command("auth set"));
        assert!(is_mcp_excluded_command("auth reset"));
        assert!(!is_mcp_excluded_command("auth show"));
        assert!(!is_mcp_excluded_command("auth test"));
        assert!(!is_mcp_excluded_command("ticker"));
    }

    #[test]
    fn inject_dangerous_adds_acknowledged_field() {
        let mut schema = clap_command_to_schema(&make_cmd());
        inject_dangerous_confirmation(&mut schema);
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("acknowledged"));
        assert_eq!(props["acknowledged"]["type"], "boolean");
        let req = schema["required"].as_array().unwrap();
        assert!(req.contains(&json!("acknowledged")));
    }

    #[test]
    fn futures_api_secret_stdin_excluded() {
        let cmd = Command::new("test")
            .arg(Arg::new("api_key").long("api-key").required(true))
            .arg(
                Arg::new("futures_api_secret_stdin")
                    .long("futures-api-secret-stdin")
                    .action(clap::ArgAction::SetTrue),
            );
        let schema = clap_command_to_schema(&cmd);
        let props = schema["properties"].as_object().unwrap();
        assert!(!props.contains_key("futures_api_secret_stdin"));
    }

    #[test]
    fn futures_credential_args_excluded() {
        let cmd = Command::new("test")
            .arg(Arg::new("pair").required(true))
            .arg(Arg::new("futures_api_key").long("futures-api-key"))
            .arg(Arg::new("futures_api_secret").long("futures-api-secret"))
            .arg(Arg::new("futures_api_secret_file").long("futures-api-secret-file"));
        let schema = clap_command_to_schema(&cmd);
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("pair"));
        assert!(!props.contains_key("futures_api_key"));
        assert!(!props.contains_key("futures_api_secret"));
        assert!(!props.contains_key("futures_api_secret_file"));
    }
}

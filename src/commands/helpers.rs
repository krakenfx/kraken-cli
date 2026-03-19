/// Shared helper functions used across multiple command modules.
use serde_json::Value;

use crate::errors::{KrakenError, Result};
use crate::output::CommandOutput;

/// Extract a string value from a JSON object by key, returning "-" on miss.
pub(crate) fn jstr(val: &Value, key: &str) -> String {
    val.get(key)
        .map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .unwrap_or_else(|| "-".to_string())
}

/// Parse a generic JSON value into key-value pairs for rendering.
pub(crate) fn parse_generic(data: &Value) -> CommandOutput {
    let pairs: Vec<(String, String)> = if let Some(obj) = data.as_object() {
        obj.iter()
            .map(|(k, v)| {
                let val = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                (k.clone(), val)
            })
            .collect()
    } else if let Some(arr) = data.as_array() {
        arr.iter()
            .enumerate()
            .map(|(i, v)| (format!("[{i}]"), v.to_string()))
            .collect()
    } else {
        vec![("Result".into(), data.to_string())]
    };
    CommandOutput::key_value(pairs, data.clone())
}

/// Prompt user to confirm a destructive operation; abort on decline.
pub(crate) fn confirm_destructive(prompt: &str) -> Result<()> {
    let confirmed = dialoguer::Confirm::new()
        .with_prompt(prompt)
        .default(false)
        .interact()
        .map_err(|e| KrakenError::Config(format!("Prompt error: {e}")))?;

    if !confirmed {
        return Err(KrakenError::Validation(
            "Operation cancelled by user".into(),
        ));
    }
    Ok(())
}

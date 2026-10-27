//! Reading a client's arguments the way the command line would.
//!
//! The command line hands clap strings and clap parses them into the leaf's
//! types, so `--interval 60` and `--all` are the same call whether a human
//! typed them or a model emitted `"60"` and `"true"`. A tool call deserializes
//! straight into the leaf, which is stricter than that: an LLM client routinely
//! stringifies a number, and `invalid type: string "60", expected u32` costs a
//! whole round-trip to learn what the schema already said.
//!
//! So the boundary coerces where the schema is unambiguous — string→number,
//! string→boolean, scalar→one-element list — and nowhere else. It never guesses
//! a type the schema does not name, so a coercion cannot change which command
//! runs; the worst case is the same refusal one step later.

use serde_json::{Map, Value};

/// Rewrites `arguments` in place so the values a client sent match the types
/// the schema names. `properties` is the advertised schema's `properties` map.
///
/// Each coercion is logged at debug: a client that always stringifies its
/// numbers is worth knowing about, and silent repair is worth being able to
/// see.
pub(super) fn to_schema(arguments: &mut Map<String, Value>, properties: &Map<String, Value>) {
    for (name, value) in arguments.iter_mut() {
        let Some(schema) = properties.get(name) else {
            continue;
        };
        if let Some(coerced) = coerced(value, schema) {
            tracing::debug!(
                argument = %name,
                from = %value,
                to = %coerced,
                "coerced a tool argument to the type its schema names"
            );
            *value = coerced;
        }
    }
}

/// The value as the schema's type, or `None` when it already matches, when the
/// schema is ambiguous, or when the text is not that type after all.
fn coerced(value: &Value, schema: &Value) -> Option<Value> {
    let types = super::schema::declared_types(schema);
    // A union of two value types is the schema of a `Decimal`, which accepts
    // both spellings: `{"type": ["string", "number"]}`. Coercing either way
    // would be a choice the schema does not license.
    if types.iter().filter(|kind| **kind != "null").count() != 1 {
        return None;
    }
    match types.iter().find(|kind| **kind != "null")? {
        &"integer" | &"number" => number(value),
        &"boolean" => boolean(value),
        &"array" => list(value, schema),
        _ => None,
    }
}

/// `"60"` → `60`. Parsed through JSON rather than `f64` so a large integer
/// keeps its precision, and rejected — left alone — if the text is not a
/// number.
fn number(value: &Value) -> Option<Value> {
    let text = value.as_str()?;
    match serde_json::from_str::<Value>(text.trim()) {
        Ok(parsed) if parsed.is_number() => Some(parsed),
        _ => None,
    }
}

/// `"true"` → `true`. Only the two spellings JSON itself uses: `"yes"` and
/// `"1"` are not booleans on the command line either.
fn boolean(value: &Value) -> Option<Value> {
    match value.as_str()?.trim() {
        "true" => Some(Value::Bool(true)),
        "false" => Some(Value::Bool(false)),
        _ => None,
    }
}

/// A scalar where a list is wanted: `"XBTUSD"` → `["XBTUSD"]`. A
/// comma-separated string splits, because that is how every list argument's own
/// help text describes itself (`Transaction IDs (comma-delimited, up to 50)`)
/// and how the command line accepts several.
fn list(value: &Value, schema: &Value) -> Option<Value> {
    if value.is_array() || value.is_null() {
        return None;
    }
    let items_are_strings = schema
        .get("items")
        .and_then(|items| items.get("type"))
        .and_then(Value::as_str)
        == Some("string");
    let Some(text) = value.as_str() else {
        // A lone number or boolean for a list of them: wrap it, whatever the
        // item type is, and let the item's own schema judge the value.
        return Some(Value::Array(vec![value.clone()]));
    };
    if !items_are_strings {
        return Some(Value::Array(vec![value.clone()]));
    }
    let values = text
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| Value::String(part.to_owned()))
        .collect();
    Some(Value::Array(values))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coerce(arguments: Value, properties: Value) -> Value {
        let mut arguments = arguments.as_object().cloned().expect("an object");
        to_schema(&mut arguments, properties.as_object().expect("an object"));
        Value::Object(arguments)
    }

    /// The regression an agent hits constantly: a stringified number for an
    /// argument the schema types as an integer.
    #[test]
    fn a_stringified_number_becomes_a_number() {
        let out = coerce(
            serde_json::json!({"interval": "60"}),
            serde_json::json!({"interval": {"type": "integer"}}),
        );
        assert_eq!(out["interval"], serde_json::json!(60));
    }

    #[test]
    fn a_stringified_flag_becomes_a_boolean() {
        let out = coerce(
            serde_json::json!({"all": "false", "validate": "true"}),
            serde_json::json!({"all": {"type": "boolean"}, "validate": {"type": "boolean"}}),
        );
        assert_eq!(out["all"], serde_json::json!(false));
        assert_eq!(out["validate"], serde_json::json!(true));
    }

    #[test]
    fn a_scalar_becomes_a_one_element_list() {
        let out = coerce(
            serde_json::json!({"pairs": "XBTUSD"}),
            serde_json::json!({"pairs": {"type": "array", "items": {"type": "string"}}}),
        );
        assert_eq!(out["pairs"], serde_json::json!(["XBTUSD"]));
    }

    /// The list arguments describe themselves as comma-delimited, and the
    /// command line takes several values, so a model that follows the help text
    /// must not end up with one pair named `BTCUSD,ETHUSD`.
    #[test]
    fn a_comma_separated_string_splits_into_a_list() {
        let out = coerce(
            serde_json::json!({"txids": "OQ-1, OQ-2"}),
            serde_json::json!({"txids": {"type": "array", "items": {"type": "string"}}}),
        );
        assert_eq!(out["txids"], serde_json::json!(["OQ-1", "OQ-2"]));
    }

    /// `Decimal` accepts both spellings, so neither is the one the schema
    /// means: a price must arrive exactly as it was sent.
    #[test]
    fn a_decimal_union_is_left_alone() {
        let out = coerce(
            serde_json::json!({"capital": "10000.5"}),
            serde_json::json!({"capital": {"type": ["string", "number"]}}),
        );
        assert_eq!(out["capital"], serde_json::json!("10000.5"));
    }

    /// Text that is not a number is left for the deserializer to refuse, so the
    /// error names the field rather than the coercion.
    #[test]
    fn unparsable_text_is_left_for_the_deserializer() {
        let out = coerce(
            serde_json::json!({"interval": "hourly"}),
            serde_json::json!({"interval": {"type": "integer"}}),
        );
        assert_eq!(out["interval"], serde_json::json!("hourly"));
    }

    /// A nullable integer is still an integer; the `null` in its type union is
    /// absence, not a second value type.
    #[test]
    fn a_nullable_number_still_coerces() {
        let out = coerce(
            serde_json::json!({"userref": "42"}),
            serde_json::json!({"userref": {"type": ["integer", "null"]}}),
        );
        assert_eq!(out["userref"], serde_json::json!(42));
    }

    /// An argument the schema does not name is never touched: `Entry::command`
    /// refuses it, and rewriting it first would only obscure that.
    #[test]
    fn an_unadvertised_argument_is_untouched() {
        let out = coerce(
            serde_json::json!({"bogus": "60"}),
            serde_json::json!({"interval": {"type": "integer"}}),
        );
        assert_eq!(out["bogus"], serde_json::json!("60"));
    }
}

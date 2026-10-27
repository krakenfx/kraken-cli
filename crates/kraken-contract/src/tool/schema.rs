//! The generated schema, reshaped into the subset every client understands.
//!
//! `schemars` describes Rust faithfully: an `Option<u32>` is
//! `{"type": ["integer", "null"]}`, a documented value enum is a `oneOf` of
//! `const`s, and every named type carries its Rust name as a `title`. All three
//! are valid JSON Schema and none of them are portable — a tool schema is read
//! by whatever model the client runs, and the strict function-calling modes of
//! other vendors reject type unions and `oneOf` outright. The Rust type name is
//! worse than useless: `"title": "Buy"` invites a model to call something that
//! is not a tool name.
//!
//! So the deserialize contract is stated the plainest way that still means the
//! same thing: one type, one `enum`, no `title`, and `additionalProperties:
//! false` — which the seam has always enforced but never advertised.

use schemars::{
    Schema,
    transform::{Transform, transform_subschemas},
};
use serde_json::{Map, Value};
use std::borrow::Cow;

/// Reshapes a generated schema and every subschema in place.
///
/// A `Transform` rather than a hand-rolled walk: `transform_subschemas` already
/// knows every keyword a subschema can hide under, so a schemars upgrade that
/// grows one does not quietly stop being reshaped.
pub(super) fn portable(schema: &mut Schema) {
    Portable.transform(schema);
}

struct Portable;

impl Transform for Portable {
    fn transform(&mut self, schema: &mut Schema) {
        unwrap_description(schema);
        drop_rust_type_name(schema);
        collapse_nullable_type(schema);
        drop_null_from_enum(schema);
        collapse_const_variants(schema);
        transform_subschemas(self, schema);
        // After the subschemas: a `oneOf` collapsed above turns its parent into
        // a plain object, and only then is it right to close it.
        close_object(schema);
    }
}

/// A doc comment's hard wraps, joined back into prose.
///
/// `schemars` publishes a doc comment verbatim, so wherever the source happens
/// to wrap is where a client renders a line break — mid-sentence, at whatever
/// width the formatter chose. A single newline is that wrap. A blank line is
/// the author's paragraph and stays.
pub(super) fn unwrap_prose(text: &str) -> Cow<'_, str> {
    if !text.contains('\n') {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.split("\n\n")
            .map(|paragraph| {
                paragraph
                    .split('\n')
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
    )
}

fn unwrap_description(schema: &mut Schema) {
    let Some(text) = schema.get("description").and_then(Value::as_str) else {
        return;
    };
    if let Cow::Owned(joined) = unwrap_prose(text) {
        schema.insert("description".to_owned(), Value::String(joined));
    }
}

/// `"title": "Buy"` is the Rust struct's name, which is neither the tool's name
/// nor anything a caller can use. Descriptions carry the meaning.
fn drop_rust_type_name(schema: &mut Schema) {
    schema.remove("title");
}

/// `["integer", "null"]` → `"integer"`, and `["string", "number", "null"]` →
/// `["string", "number"]`.
///
/// The argument is optional either way — it is absent from `required` — and an
/// explicit `null` still deserializes, because the field is an `Option`. What
/// changes is only what a strict client is willing to accept as a schema. A
/// union that remains after `null` goes is a `Decimal`, which genuinely takes a
/// string or a number, so it stays a union.
fn collapse_nullable_type(schema: &mut Schema) {
    let Some(types) = schema.get("type").and_then(Value::as_array) else {
        return;
    };
    let named: Vec<&str> = types
        .iter()
        .filter_map(Value::as_str)
        .filter(|kind| *kind != "null")
        .collect();
    let collapsed = match named.as_slice() {
        [] => return,
        [one] => Value::String((*one).to_owned()),
        many => Value::Array(
            many.iter()
                .map(|kind| Value::String((*kind).to_owned()))
                .collect(),
        ),
    };
    schema.insert("type".to_owned(), collapsed);
}

/// An `Option<AssetClass>` puts `null` among the permitted values. Absence
/// already covers it, and a `null` inside `enum` is what OpenAI's strict mode
/// and Gemini refuse.
fn drop_null_from_enum(schema: &mut Schema) {
    let Some(values) = schema.get_mut("enum").and_then(Value::as_array_mut) else {
        return;
    };
    if values.iter().any(Value::is_null) {
        values.retain(|value| !value.is_null());
    }
}

/// A documented value enum generates one `const` branch per variant so each
/// keeps its doc comment. Restated as an `enum` with the variants' prose folded
/// into one description, which every client reads.
fn collapse_const_variants(schema: &mut Schema) {
    let Some(branches) = schema.get("oneOf").and_then(Value::as_array) else {
        return;
    };
    let mut values = Vec::with_capacity(branches.len());
    let mut described = Vec::new();
    for branch in branches {
        // Every branch must be a plain constant (or a bare enum of them), or
        // this is a real union and flattening it would lose a shape.
        match (branch.get("const"), branch.get("enum")) {
            (Some(one), None) => values.push(one.clone()),
            (None, Some(Value::Array(many))) => values.extend(many.iter().cloned()),
            _ => return,
        }
        if let (Some(Value::String(value)), Some(Value::String(prose))) =
            (branch.get("const"), branch.get("description"))
        {
            described.push(format!("{value}: {}", unwrap_prose(prose)));
        }
    }
    if values.is_empty() {
        return;
    }

    let kinds: Vec<&str> = values
        .iter()
        .map(|value| match value {
            Value::String(_) => "string",
            Value::Number(_) => "number",
            Value::Bool(_) => "boolean",
            _ => "null",
        })
        .collect();
    let one_kind = kinds
        .first()
        .copied()
        .filter(|kind| kinds.iter().all(|other| other == kind) && *kind != "null");

    schema.remove("oneOf");
    if let Some(kind) = one_kind {
        schema.insert("type".to_owned(), Value::String(kind.to_owned()));
    }
    schema.insert("enum".to_owned(), Value::Array(values));
    if !described.is_empty() {
        let mut prose = match schema.get("description").and_then(Value::as_str) {
            Some(existing) if !existing.is_empty() => format!("{existing} "),
            _ => String::new(),
        };
        prose.push_str(&described.join("; "));
        schema.insert("description".to_owned(), Value::String(prose));
    }
}

/// The seam refuses an argument the schema does not name, so the schema should
/// say so: `additionalProperties: false` is what a strict client needs to catch
/// the same mistake a round-trip earlier.
fn close_object(schema: &mut Schema) {
    let is_object = schema.get("properties").is_some()
        || schema.get("type").and_then(Value::as_str) == Some("object");
    if is_object && schema.get("additionalProperties").is_none() {
        schema.insert("additionalProperties".to_owned(), Value::Bool(false));
    }
}

/// Adds `fields` to the schema's `required` list.
///
/// A list argument clap marks `required` arrives through `serde` as an empty
/// `Vec`, so nothing in the generated schema says it must carry a value. The
/// [`Rule::NonEmpty`](super::Rule::NonEmpty) that refuses the empty call is
/// where that fact is declared; this publishes it.
pub(super) fn require(schema: &mut Schema, fields: &[&str]) {
    if fields.is_empty() {
        return;
    }
    let object = schema.ensure_object();
    let required = object
        .entry("required")
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(names) = required.as_array_mut() else {
        return;
    };
    for field in fields {
        let name = Value::String((*field).to_owned());
        if !names.contains(&name) {
            names.push(name);
        }
    }
}

/// Whether the schema advertises `field` at all — the check that keeps a
/// declared rule from naming an argument no client can send.
#[cfg(test)]
pub(super) fn advertises(schema: &Schema, field: &str) -> bool {
    properties(schema).is_some_and(|properties| properties.contains_key(field))
}

/// The advertised arguments. One resolution of where argument names live,
/// shared by every consumer that reads them.
pub(super) fn properties(schema: &Schema) -> Option<&Map<String, Value>> {
    schema.get("properties").and_then(Value::as_object)
}

/// The `type` keyword as a list, however it is spelled: `"integer"` or
/// `["integer", "null"]`. Empty when the keyword is absent or unreadable, which
/// every caller must read as "no constraint" rather than as a mismatch.
pub(super) fn declared_types(schema: &Value) -> Vec<&str> {
    match schema.get("type") {
        Some(Value::String(one)) => vec![one.as_str()],
        Some(Value::Array(many)) => many.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

/// The arguments whose value is not the type their schema names.
///
/// Used only to name the argument a failed deserialize could not: `serde` loses
/// the path through a `#[serde(flatten)]` group, which is exactly the shape the
/// argument-heavy trading leaves have, and "expected i32" without a field name
/// is not something an agent can act on. A heuristic by construction — the
/// authoritative refusal is still `serde`'s.
pub(super) fn type_mismatches(schema: &Schema, arguments: &Map<String, Value>) -> Vec<String> {
    let Some(properties) = properties(schema) else {
        return Vec::new();
    };
    arguments
        .iter()
        .filter(|(name, value)| {
            properties
                .get(*name)
                .is_some_and(|declared| !matches_type(declared, value))
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// Whether a value could be what the schema's `type` asks for. Absent or
/// unreadable types answer yes: this must never invent a mismatch.
fn matches_type(declared: &Value, value: &Value) -> bool {
    let named = declared_types(declared);
    if named.is_empty() {
        return true;
    }
    named.iter().any(|kind| match *kind {
        "string" => value.is_string(),
        // A whole number satisfies both, the way JSON itself treats them.
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn portable_json(schema: Value) -> Value {
        let mut schema = Schema::try_from(schema).expect("a schema");
        portable(&mut schema);
        schema.to_value()
    }

    #[test]
    fn a_nullable_type_collapses_to_the_type_itself() {
        let out = portable_json(serde_json::json!({
            "type": "object",
            "properties": {"price": {"type": ["string", "null"]}}
        }));
        assert_eq!(out["properties"]["price"]["type"], "string");
    }

    /// A `Decimal` accepts a string or a number; collapsing that union would
    /// forbid one of the two spellings the API itself uses. An optional one
    /// adds `null`, which absence already covers.
    #[test]
    fn a_two_value_type_union_is_kept_without_its_null() {
        let out = portable_json(serde_json::json!({
            "type": "object",
            "properties": {
                "capital": {"type": ["string", "number"]},
                "fee_rate": {"type": ["string", "number", "null"]}
            }
        }));
        for field in ["capital", "fee_rate"] {
            assert_eq!(
                out["properties"][field]["type"],
                serde_json::json!(["string", "number"]),
                "{field}"
            );
        }
    }

    #[test]
    fn null_is_dropped_from_an_enum() {
        let out = portable_json(serde_json::json!({
            "type": "object",
            "properties": {"asset_class": {"type": ["string", "null"], "enum": ["currency", null]}}
        }));
        assert_eq!(
            out["properties"]["asset_class"]["enum"],
            serde_json::json!(["currency"])
        );
    }

    /// The Rust type name leaks through as a title; a model reading it as a
    /// tool name is the failure mode.
    #[test]
    fn the_rust_type_name_is_dropped() {
        let out = portable_json(serde_json::json!({
            "title": "Buy", "type": "object", "properties": {}
        }));
        assert!(out.get("title").is_none(), "{out}");
    }

    #[test]
    fn documented_const_branches_become_one_enum() {
        let out = portable_json(serde_json::json!({
            "type": "object",
            "properties": {"mode": {"oneOf": [
                {"type": "string", "const": "paper", "description": "Simulated fills."},
                {"type": "string", "const": "live", "description": "Real orders."}
            ]}}
        }));
        let mode = &out["properties"]["mode"];
        assert_eq!(mode["enum"], serde_json::json!(["paper", "live"]));
        assert_eq!(mode["type"], "string");
        assert!(mode.get("oneOf").is_none(), "{mode}");
        let prose = mode["description"].as_str().expect("prose");
        assert!(
            prose.contains("paper: Simulated fills.") && prose.contains("live: Real orders."),
            "{prose}"
        );
    }

    /// A genuine union of object shapes is not an enum of constants, and must
    /// survive: flattening it would advertise arguments that do not exist.
    #[test]
    fn a_union_of_shapes_is_left_alone() {
        let out = portable_json(serde_json::json!({
            "type": "object",
            "properties": {"target": {"oneOf": [
                {"type": "object", "properties": {"tape": {"type": "string"}}},
                {"type": "object", "properties": {"session": {"type": "string"}}}
            ]}}
        }));
        assert!(out["properties"]["target"]["oneOf"].is_array());
    }

    #[test]
    fn an_object_is_closed_to_unknown_arguments() {
        let out = portable_json(serde_json::json!({
            "type": "object", "properties": {"pair": {"type": "string"}}
        }));
        assert_eq!(out["additionalProperties"], serde_json::json!(false));
    }

    #[test]
    fn require_adds_a_name_once() {
        let mut schema = Schema::try_from(serde_json::json!({
            "type": "object",
            "properties": {"order_ids": {"type": "array"}},
            "required": ["order_ids"]
        }))
        .expect("a schema");
        require(&mut schema, &["order_ids"]);
        assert_eq!(
            schema.to_value()["required"],
            serde_json::json!(["order_ids"])
        );
    }

    #[test]
    fn unwrap_prose_joins_a_wrapped_sentence() {
        assert_eq!(
            unwrap_prose("Filter by lock type. Can specify\nmultiple."),
            "Filter by lock type. Can specify multiple."
        );
    }

    #[test]
    fn unwrap_prose_keeps_a_paragraph_break() {
        assert_eq!(
            unwrap_prose("Save credentials.\n\nThe secret is\nresolved by flag."),
            "Save credentials.\n\nThe secret is resolved by flag.",
            "a blank line is the author's paragraph, not the formatter's wrap"
        );
    }

    #[test]
    fn unwrap_prose_does_not_allocate_for_unwrapped_prose() {
        assert!(
            matches!(unwrap_prose("One line."), Cow::Borrowed(_)),
            "most descriptions have no newline and must not be copied"
        );
    }

    #[test]
    fn unwrap_description_rewrites_the_schema_in_place() {
        let mut schema = Schema::try_from(serde_json::json!({
            "type": "string",
            "description": "Read the secret from stdin (exclusive with\n--secret-file)."
        }))
        .expect("a schema");
        unwrap_description(&mut schema);
        assert_eq!(
            schema.to_value()["description"],
            serde_json::json!("Read the secret from stdin (exclusive with --secret-file).")
        );
    }
}

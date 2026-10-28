//! Cross-field argument rules: the validity a JSON Schema cannot state and a
//! `serde` deserialize never sees.
//!
//! clap enforces these on the command line through `conflicts_with`,
//! `required_unless_present` and `requires`. A tool call does not reach clap —
//! its arguments deserialize straight into the leaf — so `kraken ticker` with
//! both `pairs` and `all` would return every tradable pair instead of refusing,
//! and with neither would return nothing at all. Each such leaf declares the
//! same rule here, on its [`tools!`](crate::tools) line, and
//! [`Entry::command`](super::Entry::command) enforces it before the call
//! becomes a command.
//!
//! The declaration is checked against clap's own tree by two tests, so a rule
//! dropped from either side fails rather than reopening the hole:
//! `a_shape_the_command_line_refuses_is_refused_of_a_tool_call` catches a
//! requirement (an argument the command line will not do without), and
//! `every_conflicting_argument_pair_is_declared` catches an exclusion, through
//! the one constraint clap exposes.
//!
//! The gap that leaves: a leaf that gains a `required_unless_present` pair
//! *and* has an unconditionally required argument is refused with no arguments
//! either way, so the first test cannot see the pair. clap keeps
//! `required_unless_present` private, so nothing but its `Debug` output could —
//! which is not a thing to build a test on.

use serde_json::{Map, Value};

/// A relationship between arguments that decides whether a call is well-shaped.
///
/// Tuple variants, not struct variants, because the `tools!` table spells them
/// positionally: `where ExactlyOne(&["pairs", "all"])`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// Exactly one of these must be given — clap's `required_unless_present`
    /// pair plus the `conflicts_with` that keeps them apart.
    ExactlyOne(&'static [&'static str]),
    /// At least one of these must be given — clap's `required_unless_present`
    /// pair without a conflict, where giving both is a legitimate call
    /// (`futures cancel` takes either ID, and both together name the same
    /// order twice).
    AtLeastOne(&'static [&'static str]),
    /// A list argument that must carry at least one value — clap's `required`
    /// on a `Vec`, which `#[serde(default)]` would otherwise let arrive empty.
    NonEmpty(&'static str),
}

impl Rule {
    /// The arguments this rule speaks about, so a test can prove each one is a
    /// field the tool actually advertises.
    pub fn mentions(&self) -> Vec<&'static str> {
        match self {
            Self::ExactlyOne(fields) | Self::AtLeastOne(fields) => fields.to_vec(),
            Self::NonEmpty(field) => vec![field],
        }
    }

    /// The rule as a client should read it: one clause, appended to the tool's
    /// description so a caller can satisfy it without a failed call first.
    pub fn requirement(&self) -> String {
        match self {
            Self::ExactlyOne(fields) => format!("give exactly one of {}", list(fields)),
            Self::AtLeastOne(fields) => format!("give at least one of {}", list(fields)),
            Self::NonEmpty(field) => format!("`{field}` must list at least one value"),
        }
    }

    /// Why `arguments` breaks this rule, or `None` when they satisfy it. The
    /// message states the fix, not just the fault: it is what a model reads.
    pub fn violation(&self, arguments: &Map<String, Value>) -> Option<String> {
        match self {
            Self::ExactlyOne(fields) => match given(fields, arguments).as_slice() {
                [] => Some(format!("give one of {}", list(fields))),
                [_] => None,
                many => Some(format!(
                    "{} are mutually exclusive; drop all but one",
                    list(many)
                )),
            },
            Self::AtLeastOne(fields) => given(fields, arguments)
                .is_empty()
                .then(|| format!("give one of {}", list(fields))),
            Self::NonEmpty(field) => {
                (!present(arguments, field)).then(|| format!("`{field}` needs at least one value"))
            }
        }
    }
}

/// Which of `fields` the call actually gives.
fn given(fields: &[&'static str], arguments: &Map<String, Value>) -> Vec<&'static str> {
    fields
        .iter()
        .copied()
        .filter(|field| present(arguments, field))
        .collect()
}

/// Whether an argument counts as given, in clap's terms rather than JSON's: a
/// flag left `false` was never passed, an empty list carries no values, and an
/// explicit `null` is the absence a nullable schema invites a client to send.
pub(super) fn present(arguments: &Map<String, Value>, field: &str) -> bool {
    match arguments.get(field) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(set)) => *set,
        Some(Value::Array(values)) => !values.is_empty(),
        Some(Value::String(text)) => !text.is_empty(),
        Some(_) => true,
    }
}

/// `` `a`, `b` `` — the argument names as prose, backticked so a model can see
/// where each one ends.
fn list(fields: &[&str]) -> String {
    fields
        .iter()
        .map(|field| format!("`{field}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(json: Value) -> Map<String, Value> {
        json.as_object().cloned().expect("an object")
    }

    /// The shape that dumped 1812 pairs instead of refusing: both sides given,
    /// where clap says they conflict.
    #[test]
    fn exactly_one_refuses_both() {
        let rule = Rule::ExactlyOne(&["pairs", "all"]);
        let why = rule
            .violation(&arguments(
                serde_json::json!({"pairs": ["XBTUSD"], "all": true}),
            ))
            .expect("both given is a violation");
        assert!(why.contains("mutually exclusive"), "{why}");
    }

    /// The shape that returned an empty body instead of refusing: neither side.
    #[test]
    fn exactly_one_refuses_neither() {
        let rule = Rule::ExactlyOne(&["pairs", "all"]);
        assert!(rule.violation(&arguments(serde_json::json!({}))).is_some());
        assert!(
            rule.violation(&arguments(serde_json::json!({"pairs": []})))
                .is_some(),
            "an empty list carries no values"
        );
    }

    /// A boolean flag left `false` is not a given argument — that is what
    /// absence means on the command line, and a client that spells the default
    /// out must not trip the rule.
    #[test]
    fn a_false_flag_is_not_given() {
        let rule = Rule::ExactlyOne(&["pairs", "all"]);
        assert!(
            rule.violation(&arguments(
                serde_json::json!({"pairs": ["XBTUSD"], "all": false})
            ))
            .is_none()
        );
    }

    /// An explicit `null` is absence: a nullable schema invites a client to
    /// send one for an argument it does not want.
    #[test]
    fn an_explicit_null_is_not_given() {
        let rule = Rule::ExactlyOne(&["order_id", "cli_ord_id"]);
        let why = rule.violation(&arguments(
            serde_json::json!({"order_id": null, "cli_ord_id": null}),
        ));
        assert!(why.is_some(), "two nulls give neither side");
    }

    /// `futures cancel` takes either ID and clap declares no conflict, so both
    /// together is a call it accepts — only neither is a refusal.
    #[test]
    fn at_least_one_accepts_both_and_refuses_neither() {
        let rule = Rule::AtLeastOne(&["order_id", "cli_ord_id"]);
        assert!(
            rule.violation(&arguments(
                serde_json::json!({"order_id": "1", "cli_ord_id": "abc"})
            ))
            .is_none()
        );
        assert!(rule.violation(&arguments(serde_json::json!({}))).is_some());
    }

    #[test]
    fn non_empty_refuses_an_absent_or_empty_list() {
        let rule = Rule::NonEmpty("order_ids");
        assert!(rule.violation(&arguments(serde_json::json!({}))).is_some());
        assert!(
            rule.violation(&arguments(serde_json::json!({"order_ids": []})))
                .is_some()
        );
        assert!(
            rule.violation(&arguments(serde_json::json!({"order_ids": ["1"]})))
                .is_none()
        );
    }
}

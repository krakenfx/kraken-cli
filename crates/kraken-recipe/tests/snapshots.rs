//! Pinned artifact shapes: `recipe.json` is the agent-parsed contract and
//! the draft `SKILL.md` is the generated package, so both are snapshots —
//! a shape or wording change must be a reviewed diff, never an accident.
//! Regenerate intentionally with
//! `INSTA_UPDATE=always cargo test -p kraken-recipe --test snapshots`
//! (or `cargo insta review`) and commit.

// Test fixtures on known-good literals: unwrap is the assertion idiom here.
#![allow(clippy::unwrap_used)]

use insta::{assert_json_snapshot, assert_snapshot};
use kraken_recipe::{CreateSpec, digest, draft_skill, from_session};
use kraken_session::{
    decision::{Decision, DecisionKind},
    manifest::{SessionSource, StrategyRef},
    timeline::fixtures,
};

fn decision(at: &str, kind: DecisionKind) -> Decision {
    Decision {
        timestamp: at.parse().unwrap(),
        kind,
        symbol: Some("BTCUSD".to_string()),
        reason: "level crossed".to_string(),
        order_id: None,
    }
}

fn worked_example() -> kraken_recipe::Recipe {
    let mut session = fixtures::manifest(vec![fixtures::jsonl_recording()]);
    session.window.ended_at = Some("2026-01-01T00:30:00Z".parse().unwrap());
    session.label = Some("walk-1".to_string());
    session.experiment = Some("walk".to_string());
    session.strategy = Some(StrategyRef {
        name: "recipe-playground-price-alert".to_string(),
        params: Some(serde_json::json!({"upper": 70000, "lower": 60000, "interval_s": 300})),
    });
    session.source = Some(SessionSource {
        tape: "tape:btc-jan".to_string(),
        speed: 10.0,
        anchor: "2026-01-01T00:00:00Z".parse().unwrap(),
        started_at: "2026-01-01T00:00:00Z".parse().unwrap(),
        content_hash: Some("sha256:abc123".to_string()),
    });
    let spec = CreateSpec {
        name: "btc-levels".to_string(),
        description: Some("Watch BTC levels on replayed January data.".to_string()),
        driver: None,
        overwrite: false,
    };
    let mut recipe =
        from_session(&session, &spec, "2026-02-01T12:00:00Z".parse().unwrap()).unwrap();
    // The version stamp tracks the workspace release; pin the snapshot to a
    // stable value instead.
    recipe.cli_version = "test".to_string();
    recipe
}

#[test]
fn recipe_wire_shape_is_pinned() {
    assert_json_snapshot!(worked_example());
}

#[test]
fn draft_skill_of_the_worked_example_is_pinned() {
    let decisions = [
        decision("2026-01-01T00:05:00Z", DecisionKind::Alert),
        decision("2026-01-01T00:10:00Z", DecisionKind::Alert),
        decision("2026-01-01T00:20:00Z", DecisionKind::Buy),
    ];
    let draft = draft_skill(&worked_example(), &digest(&decisions));
    assert!(
        draft.contains("UNRESOLVED RULES"),
        "the draft must never read as authored logic"
    );
    assert_snapshot!(draft);
}

#[test]
fn draft_skill_without_params_or_provenance_extras_is_pinned() {
    let mut session = fixtures::manifest(vec![fixtures::jsonl_recording()]);
    session.window.ended_at = Some("2026-01-01T00:30:00Z".parse().unwrap());
    let spec = CreateSpec {
        name: "bare".to_string(),
        description: None,
        driver: Some("my-skill".to_string()),
        overwrite: false,
    };
    let mut recipe =
        from_session(&session, &spec, "2026-02-01T12:00:00Z".parse().unwrap()).unwrap();
    recipe.cli_version = "test".to_string();
    assert_snapshot!(draft_skill(&recipe, &digest(&[])));
}

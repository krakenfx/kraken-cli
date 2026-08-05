//! The experiment lifecycle through the public facade only: freeze → show →
//! evaluate on a real scorecard → on-disk tamper reads as damage. All state is
//! files, so this is exactly what a fresh agent context replays.

use kraken_lab::{Criteria, Lab, LabError, experiments_root, score};
use kraken_paper::OrderSide;
use kraken_paper::account::Origin;
use kraken_session::manifest::SessionOutcome;
use kraken_session::timeline::fixtures::{self, SessionBuilder};
use rust_decimal_macros::dec;

// Fixture misuse is a test bug; the parse target is a literal.
#[allow(clippy::unwrap_used)]
fn stopped_session() -> kraken_session::timeline::SessionTimeline {
    let mut builder = SessionBuilder::with_rates(dec!(10_000), dec!(0.0026), dec!(0.0));
    builder.ticker(
        "2026-01-01T00:00:01Z",
        "BTC/USD",
        dec!(50_000),
        dec!(50_000),
    );
    builder.market_fill(
        "2026-01-01T00:00:02Z",
        Origin::Cli,
        OrderSide::Buy,
        "BTCUSD",
        dec!(0.1),
        (dec!(50_000), dec!(50_000)),
    );
    let mut manifest = fixtures::manifest(vec![]);
    manifest.summary = Some(SessionOutcome {
        ended_at: "2026-01-01T00:00:04Z".parse().unwrap(),
        final_value: dec!(10_187),
        pnl: dec!(187),
    });
    builder.session(None, manifest)
}

#[test]
fn freeze_show_evaluate_reaches_a_mechanical_verdict() {
    let base = tempfile::tempdir().unwrap();
    let lab = Lab::new(base.path());
    lab.freeze(
        "momentum-1",
        "buying strength beats sitting in cash",
        None,
        Criteria {
            min_return_pct: Some(dec!(1.0)),
            max_drawdown_pct: None,
            min_fills: Some(1),
        },
        &[],
    )
    .unwrap();

    // A fresh load — as a cold context would do — then judge a real card.
    let experiment = lab.show("momentum-1").unwrap();
    let card = score(&stopped_session()).unwrap();
    let verdict = experiment.criteria.evaluate(&card);

    assert!(verdict.pass, "1.87% return ≥ 1.0%, 1 fill ≥ 1");
    assert_eq!(verdict.checks.len(), 2);
    assert!(verdict.checks.iter().all(|check| check.pass));
}

#[test]
fn failing_criteria_produce_a_failing_verdict_with_named_checks() {
    let base = tempfile::tempdir().unwrap();
    let lab = Lab::new(base.path());
    lab.freeze(
        "moonshot",
        "this strategy doubles the account",
        None,
        Criteria {
            min_return_pct: Some(dec!(100.0)),
            max_drawdown_pct: None,
            min_fills: Some(1),
        },
        &[],
    )
    .unwrap();

    let experiment = lab.show("moonshot").unwrap();
    let verdict = experiment
        .criteria
        .evaluate(&score(&stopped_session()).unwrap());

    assert!(!verdict.pass);
    let failed: Vec<&str> = verdict
        .checks
        .iter()
        .filter(|check| !check.pass)
        .map(|check| check.criterion)
        .collect();
    assert_eq!(failed, ["min_return_pct"]);
}

#[test]
fn on_disk_tamper_is_damage_at_show() {
    let base = tempfile::tempdir().unwrap();
    let lab = Lab::new(base.path());
    lab.freeze(
        "sealed",
        "the spec is immutable",
        None,
        Criteria {
            min_return_pct: Some(dec!(2.0)),
            ..Criteria::default()
        },
        &[],
    )
    .unwrap();

    let path = experiments_root(base.path()).join("sealed.json");
    let mut doctored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    doctored["criteria"]["min_return_pct"] = serde_json::json!(0.5);
    std::fs::write(&path, serde_json::to_string(&doctored).unwrap()).unwrap();

    let err = lab.show("sealed").unwrap_err();
    assert!(matches!(err, LabError::Damaged(_)), "got: {err}");
    assert!(err.to_string().contains("modified after"), "got: {err}");
}

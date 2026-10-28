//! The full journal lifecycle driven through the public API only — the
//! boundary proof: if this flow cannot be written against `pub` items, the
//! next crate up cannot write it either.

use kraken_paper::{
    AccountEvent, CommandEntry, OrderSide, PaperConfig, PaperError,
    account::{Origin, PaperAccount},
};
use rust_decimal_macros::dec;

fn config(balance: rust_decimal::Decimal) -> PaperConfig {
    PaperConfig {
        balance,
        currency: "USD".into(),
        fee_rate: dec!(0.0026),
        slippage_rate: dec!(0.0),
    }
}

fn entry(name: &str) -> CommandEntry {
    CommandEntry {
        name: name.into(),
        ..CommandEntry::default()
    }
}

/// Journal-is-the-source-of-truth: a lived sequence of init, market fill,
/// limit submit, and cancel folds back — on a fresh open — into exactly the
/// live state the commands built.
#[test]
fn replayed_journal_equals_the_lived_account() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");

    let mut account = PaperAccount::open_at(path.clone(), Origin::Cli).unwrap();
    assert!(!account.is_initialized());
    account
        .commit_if_uninitialized(
            vec![AccountEvent::Initialized(config(dec!(10_000)))],
            entry("init"),
        )
        .unwrap();

    let trade = account
        .state()
        .unwrap()
        .decide_market_order(
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.1),
            dec!(50_000),
            dec!(49_900),
        )
        .unwrap();
    account
        .commit(vec![AccountEvent::OrderFilled { trade }], entry("buy"))
        .unwrap();

    let order = account
        .state()
        .unwrap()
        .decide_limit_order(OrderSide::Sell, "BTCUSD", dec!(0.05), dec!(60_000))
        .unwrap();
    let order_id = order.id.clone();
    account
        .commit(vec![AccountEvent::OrderSubmitted { order }], entry("sell"))
        .unwrap();

    let cancelled = account.state().unwrap().decide_cancel(&order_id).unwrap();
    account
        .commit(
            vec![AccountEvent::OrderCancelled { order: cancelled }],
            entry("cancel"),
        )
        .unwrap();

    let live = serde_json::to_value(account.state().unwrap()).unwrap();
    drop(account); // release the writer lock for the fresh open

    let reopened = PaperAccount::open_at(path, Origin::Cli).unwrap();
    let folded = reopened.state().unwrap();
    assert_eq!(serde_json::to_value(folded).unwrap(), live);
    assert_eq!(folded.filled_trades.len(), 1);
    assert_eq!(folded.cancelled_orders.len(), 1);
    assert!(folded.open_orders.is_empty());
}

/// A journal written by a newer build refuses the whole handle at open, as
/// `Incompatible` — visible to an external consumer as its own variant.
#[test]
fn newer_version_record_is_refused_as_incompatible() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    std::fs::write(
        &path,
        concat!(
            r#"{"v":99,"ts":"2026-01-01T00:00:00Z","origin":"cli","event":"initialized","balance":"500.0","currency":"USD","fee_rate":"0.0","slippage_rate":"0.0"}"#,
            "\n"
        ),
    )
    .unwrap();
    let err = match PaperAccount::open_at(path, Origin::Cli) {
        Ok(_) => panic!("newer journal must be refused at open"),
        Err(e) => e,
    };
    assert!(matches!(err, PaperError::Incompatible(_)), "got: {err}");
}

/// Observational audit entries land on the record but never change the fold,
/// across a reload.
#[test]
fn audit_entries_round_trip_without_changing_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");

    let mut account = PaperAccount::open_at(path.clone(), Origin::Cli).unwrap();
    account
        .commit_if_uninitialized(
            vec![AccountEvent::Initialized(config(dec!(500)))],
            entry("init"),
        )
        .unwrap();
    let before = serde_json::to_value(account.state().unwrap()).unwrap();

    account.audit(&account.stamp(vec![AccountEvent::Command(entry("status"))]));
    drop(account);

    let reopened = PaperAccount::open_at(path, Origin::Cli).unwrap();
    assert_eq!(
        serde_json::to_value(reopened.state().unwrap()).unwrap(),
        before
    );
}

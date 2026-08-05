//! The windowed reader's contract: the synthesized epoch equals the prefix
//! fold, the slice partitions the journal, and the anchor is frozen in
//! `session.json` — pinned here because every score and P&L report downstream
//! anchors on exactly this arithmetic.

#![allow(clippy::unwrap_used)]

use std::path::Path;

use chrono::{DateTime, TimeZone, Utc};
use kraken_paper::account::{Origin, PaperAccount};
use kraken_paper::{AccountEvent, CommandEntry, OrderSide, PaperConfig, PaperState};
use kraken_session::manifest::{PaperRef, SessionManifest, SessionWindow};
use kraken_session::session::{
    START_MARKER, STOP_MARKER, SessionTracks, parse_ordinal, read_account_window, read_window,
    session_dir,
};
use kraken_session::timeline::event::{EventPayload, final_epoch_start};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

fn t(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 1, 0, minute, 0).unwrap()
}

fn config(balance: Decimal) -> PaperConfig {
    PaperConfig {
        balance,
        currency: "USD".to_string(),
        fee_rate: dec!(0.0026),
        slippage_rate: Decimal::ZERO,
    }
}

fn marker(name: &str, run: &str) -> CommandEntry {
    CommandEntry {
        name: name.to_string(),
        session: Some(run.to_string()),
        ..CommandEntry::default()
    }
}

/// A market fill decided against fixed quotes, committed like the real path.
fn fill(account: &mut PaperAccount, at: DateTime<Utc>, volume: Decimal) {
    let trade = account
        .state()
        .unwrap()
        .clone()
        .decide_market_order(OrderSide::Buy, "BTCUSD", volume, dec!(100.0), dec!(99.0))
        .unwrap();
    let records = account.stamp_at(vec![AccountEvent::OrderFilled { trade }], at);
    account.append(&records).unwrap();
}

fn run_manifest(scope: &Path, run: &str, started_at: DateTime<Utc>) -> SessionManifest {
    let _ = scope;
    SessionManifest::new(
        run.parse().unwrap(),
        "0.0.0-test".to_string(),
        4242,
        Vec::new(),
        PaperRef {
            starting_balance: dec!(10_000),
            currency: "USD".to_string(),
        },
        SessionWindow {
            session: run.to_string(),
            started_at,
            opening_equity: dec!(9_950),
            opening_complete: true,
            ended_at: None,
        },
    )
    .with_label(Some("momentum-s1".to_string()))
}

/// Journal: epoch → pre-run fill → start marker → 2 window fills → stop
/// marker → post-run fill. The window must see exactly the 2 fills plus the
/// stop marker, opening from the prefix-folded balances.
fn seeded_scope() -> (tempfile::TempDir, PaperState) {
    let scope = tempfile::tempdir().unwrap();
    let journal = scope.path().join("journal.jsonl");
    let mut account = PaperAccount::open_at(journal, Origin::Cli).unwrap();

    let records = account.stamp_at(vec![AccountEvent::Initialized(config(dec!(10_000)))], t(0));
    account.append(&records).unwrap();
    fill(&mut account, t(1), dec!(0.5));

    // What the prefix fold must reproduce: the state at the start marker.
    let records = account.stamp_at(
        vec![AccountEvent::Command(marker(START_MARKER, "s1"))],
        t(2),
    );
    account.append(&records).unwrap();
    let opening = account.state().unwrap().clone();

    fill(&mut account, t(3), dec!(0.1));
    fill(&mut account, t(4), dec!(0.2));
    let records = account.stamp_at(vec![AccountEvent::Command(marker(STOP_MARKER, "s1"))], t(5));
    account.append(&records).unwrap();
    fill(&mut account, t(6), dec!(1.0));

    (scope, opening)
}

fn tracks(scope: &Path, run: &str, started_at: DateTime<Utc>) -> SessionTracks {
    SessionTracks {
        journal: scope.join("journal.jsonl"),
        session_dir: session_dir(scope, parse_ordinal(run).unwrap()),
        manifest: run_manifest(scope, run, started_at),
    }
}

fn account_events(timeline: &kraken_session::timeline::SessionTimeline) -> Vec<&AccountEvent> {
    timeline
        .events
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::Account(record) => Some(&record.event),
            _ => None,
        })
        .collect()
}

#[test]
fn windowed_fold_equals_prefix_fold() {
    let (scope, opening) = seeded_scope();
    let timeline = read_window(tracks(scope.path(), "s1", t(2))).unwrap();

    let events = account_events(&timeline);
    let AccountEvent::Attached(snapshot) = events[0] else {
        panic!(
            "window opens with the synthesized epoch, got {:?}",
            events[0]
        );
    };
    for (asset, amount) in &opening.balances {
        assert_eq!(
            snapshot.balances.get(asset),
            Some(amount),
            "synthesized {asset} equals the prefix fold"
        );
    }
    let anchor = snapshot
        .anchor
        .as_ref()
        .expect("the window opener carries an anchor");
    assert_eq!(anchor.equity, dec!(9_950), "anchor from session.json");
    assert_eq!(anchor.currency, "USD");
}

#[test]
fn read_account_window_folds_the_window_without_a_tape() {
    // A recording session holds its tape locked, so `session show` folds the
    // account window tape-free. The seeded scope writes a journal but no tape;
    // the fold must still yield exactly the window's two fills — the count
    // `session show` reports for an open session, window-scoped like a stopped
    // one, never the account lifetime.
    let (scope, _) = seeded_scope();
    let journal = scope.path().join("journal.jsonl");
    let manifest = run_manifest(scope.path(), "s1", t(2));

    let events = read_account_window(&journal, &manifest).unwrap();
    let mut state = PaperState::default();
    for event in &events {
        if let EventPayload::Account(record) = &event.payload {
            state.apply(record.ts, &record.event);
        }
    }
    assert_eq!(
        state.filled_trades.len(),
        2,
        "the account window folds its own two fills, not the account's four"
    );
}

#[test]
fn window_slice_partitions_the_journal() {
    let (scope, _) = seeded_scope();
    let timeline = read_window(tracks(scope.path(), "s1", t(2))).unwrap();

    let fills: Vec<Decimal> = timeline
        .events
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::Account(record) => match &record.event {
                AccountEvent::OrderFilled { trade } => Some(trade.volume),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        fills,
        vec![dec!(0.1), dec!(0.2)],
        "pre-session and post-session fills are outside the window"
    );
}

#[test]
fn synthesized_epoch_is_the_final_epoch_and_anchors_the_fold() {
    let (scope, _) = seeded_scope();
    let timeline = read_window(tracks(scope.path(), "s1", t(2))).unwrap();

    let epoch = final_epoch_start(&timeline.events);
    let start = epoch.start.expect("the synthesized Attached is an epoch");

    // Fold from the epoch exactly as explain-pnl does: default state forward.
    let mut state = PaperState::default();
    for event in &timeline.events[start..] {
        if let EventPayload::Account(record) = &event.payload {
            state.apply(record.ts, &record.event);
        }
    }
    assert_eq!(
        state.starting_balance,
        dec!(9_950),
        "the windowed fold anchors on the frozen opening equity"
    );
    assert_eq!(state.filled_trades.len(), 2);
}

#[test]
fn open_run_reads_to_the_journal_end() {
    let scope = tempfile::tempdir().unwrap();
    let journal = scope.path().join("journal.jsonl");
    let mut account = PaperAccount::open_at(journal, Origin::Cli).unwrap();
    let records = account.stamp_at(vec![AccountEvent::Initialized(config(dec!(10_000)))], t(0));
    account.append(&records).unwrap();
    let records = account.stamp_at(
        vec![AccountEvent::Command(marker(START_MARKER, "s1"))],
        t(1),
    );
    account.append(&records).unwrap();
    fill(&mut account, t(2), dec!(0.3));
    drop(account);

    let timeline = read_window(tracks(scope.path(), "s1", t(1))).unwrap();
    let events = account_events(&timeline);
    assert!(
        matches!(events.last().unwrap(), AccountEvent::OrderFilled { trade } if trade.volume == dec!(0.3)),
        "an open session's window extends to the journal end"
    );
}

#[test]
fn missing_start_marker_is_rejected() {
    let scope = tempfile::tempdir().unwrap();
    let journal = scope.path().join("journal.jsonl");
    let mut account = PaperAccount::open_at(journal, Origin::Cli).unwrap();
    let records = account.stamp_at(vec![AccountEvent::Initialized(config(dec!(10_000)))], t(0));
    account.append(&records).unwrap();
    drop(account);

    let err = read_window(tracks(scope.path(), "s9", t(1))).unwrap_err();
    assert!(
        err.to_string().contains("no start marker"),
        "unexpected: {err}"
    );
}

#[test]
fn absent_journal_reads_as_the_epoch_alone() {
    let scope = tempfile::tempdir().unwrap();
    let timeline = read_window(tracks(scope.path(), "s1", t(1))).unwrap();
    let events = account_events(&timeline);
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], AccountEvent::Attached(_)));
}

#[test]
fn run_manifest_round_trips_label_and_window() {
    let scope = tempfile::tempdir().unwrap();
    let path = scope.path().join("session.json");
    let manifest = run_manifest(scope.path(), "s1", t(2));
    manifest.save(&path).unwrap();
    let back = SessionManifest::load(&path).unwrap();
    assert_eq!(back.label.as_deref(), Some("momentum-s1"));
    assert_eq!(back.window.session, "s1");
    assert_eq!(back.window.opening_equity, dec!(9_950));
    assert_eq!(back.id.to_string(), "s1");
}

/// The run keys ride BESIDE the pinned session keys — an accidental move of
/// `label`/`window` into `SessionManifest` (whose key set is pinned) or a
/// rename here must fail loud.
#[test]
fn run_json_carries_the_session_keys_plus_label_and_window() {
    let value = serde_json::to_value(run_manifest(Path::new("/tmp"), "s1", t(2))).unwrap();
    let object = value.as_object().unwrap();
    for key in [
        "manifest_version",
        "cli_version",
        "id",
        "pid",
        "paper",
        "status",
        "created_at",
    ] {
        assert!(object.contains_key(key), "session key {key} missing");
    }
    assert!(object.contains_key("label"));
    let window = object["window"].as_object().unwrap();
    let keys: Vec<&str> = window.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "ended_at",
            "opening_complete",
            "opening_equity",
            "session",
            "started_at"
        ],
        "the window's serialized key set is pinned"
    );
    assert_eq!(
        window["opening_equity"], "9950",
        "money is a string on the wire"
    );
}

#[test]
fn ordinals_parse_strictly() {
    assert_eq!(parse_ordinal("s1"), Some(1));
    assert_eq!(parse_ordinal("s42"), Some(42));
    for bad in ["s0", "s01", "S3", "s", "x1", "s-1", "s1x", "r3", "1"] {
        assert_eq!(parse_ordinal(bad), None, "{bad} must not parse");
    }
}

mod proptests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        /// Any prefix of fills folds to the same balances the synthesized
        /// epoch carries — the windowed read can never disagree with the
        /// journal's own arithmetic.
        #[test]
        fn windowed_opening_always_equals_the_prefix_fold(
            volumes in proptest::collection::vec(1u32..=50, 0..6),
        ) {
            let scope = tempfile::tempdir().unwrap();
            let journal = scope.path().join("journal.jsonl");
            let mut account = PaperAccount::open_at(journal, Origin::Cli).unwrap();
            let records = account.stamp_at(
                vec![AccountEvent::Initialized(config(dec!(10_000)))],
                t(0),
            );
            account.append(&records).unwrap();
            for (i, centi) in volumes.iter().enumerate() {
                let volume = Decimal::from(*centi) / Decimal::from(100);
                fill(&mut account, t(1 + i as u32), volume);
            }
            let records = account.stamp_at(
                vec![AccountEvent::Command(marker(START_MARKER, "s1"))],
                t(20),
            );
            account.append(&records).unwrap();
            let expected = account.state().unwrap().balances.clone();
            drop(account);

            let timeline = read_window(tracks(scope.path(), "s1", t(20))).unwrap();
            let events = account_events(&timeline);
            let AccountEvent::Attached(snapshot) = events[0] else {
                panic!("window must open with the synthesized epoch");
            };
            for (asset, amount) in &expected {
                prop_assert_eq!(snapshot.balances.get(asset), Some(amount));
            }
        }
    }
}

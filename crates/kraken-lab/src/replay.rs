//! Deterministic scorecard inputs derived from a session timeline.
//!
//! Values use recorded mid marks, never live prices or the stop anchor. The
//! final curve may therefore differ from bid-marked stop totals.

use crate::{LabError, Result, mark};
use chrono::{DateTime, Utc};
use kraken_paper::{PaperState, PaperTrade};
use kraken_recording::{CaptureState, RecordingManifest};
use kraken_session::timeline::{
    SessionTimeline,
    event::{EventPayload, TimelineEvent, final_epoch_start},
};
use rust_decimal::Decimal;
use serde::Serialize;
use std::collections::BTreeSet;

/// Replay the journal's final epoch against the recorded marks.
///
/// Market frames feed the mark table from the timeline's start — epochs
/// partition the account journal, not the market tape, so the first
/// post-epoch valuation gets warmed-up marks. `validation` when the journal
/// has no epoch: folding from a default account would fabricate an equity
/// line for money that never existed.
pub fn replay(session: &SessionTimeline) -> Result<Replay> {
    let epoch = final_epoch_start(&session.events);
    let Some(start) = epoch.start else {
        return Err(LabError::NoEpoch {
            session: session.manifest.id.clone(),
        });
    };

    let mut table = mark::Table::default();
    // Placeholder until the epoch-start record replaces it wholesale; no
    // point is emitted before then.
    let mut state = PaperState::default();
    let mut points: Vec<Point> = Vec::new();
    let mut unmarked = BTreeSet::new();
    for (index, event) in session.events.iter().enumerate() {
        match &event.payload {
            EventPayload::Market(frame) => {
                let moved = table.observe(frame);
                if moved && index > start {
                    let point = sample(&state, &table, event, &mut unmarked);
                    let repeats =
                        |last: &Point| (last.value, last.complete) == (point.value, point.complete);
                    // A moved mark re-values holdings; unheld symbols and
                    // median-absorbed outliers land here as no-ops.
                    if !points.last().is_some_and(repeats) {
                        points.push(point);
                    }
                }
            }
            // One point per journal line in the epoch, even for
            // observational records — the predictable contract. Earlier
            // epochs are skipped wholesale: the epoch-start record rebuilds
            // state from its own config.
            EventPayload::Account(record) if index >= start => {
                state.apply(record.ts, &record.event);
                points.push(sample(&state, &table, event, &mut unmarked));
            }
            EventPayload::Account(_) | EventPayload::Decision(_) => {}
        }
    }

    let caveats = caveats(epoch.skipped, &points, &unmarked, session.capture.as_ref());
    Ok(Replay {
        curve: Curve {
            currency: state.starting_currency.clone(),
            points,
            unmarked,
        },
        fills: state.filled_trades,
        epochs_skipped: epoch.skipped,
        caveats,
    })
}

/// A session's final epoch, replayed and marked against its own recording.
#[derive(Debug, Clone, Serialize)]
pub struct Replay {
    pub curve: Curve,
    /// The fill ledger: the epoch's fills verbatim, in journal order —
    /// price, volume, fee, and `reference_quote` (`None` on legacy fills).
    pub fills: Vec<PaperTrade>,
    pub epochs_skipped: usize,
    pub caveats: Vec<String>,
}

/// The marked equity curve: portfolio value at every valuation-changing
/// instant of the final epoch.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Curve {
    /// Valuation currency: the epoch's starting currency.
    pub currency: String,
    pub points: Vec<Point>,
    /// Assets ever held with no usable recorded mark, named by the same
    /// code path that skipped them in valuation (sorted, so serialized
    /// output is deterministic).
    pub unmarked: BTreeSet<String>,
}

/// One curve sample: emitted after every account record in the epoch and
/// after every recorded frame that moved the valuation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Point {
    pub at: DateTime<Utc>,
    /// The triggering event's within-track `seq`, tracing the point back to
    /// its timeline event.
    pub seq: i64,
    pub value: Decimal,
    /// `false` when a held asset had no usable recorded mark at this
    /// instant: the value counts cash and marked assets only — flagged,
    /// never a silent zero.
    pub complete: bool,
}

fn sample(
    state: &PaperState,
    table: &mark::Table,
    event: &TimelineEvent,
    ever_unmarked: &mut BTreeSet<String>,
) -> Point {
    let valuation = state.valuation(&table.to_prices(&state.starting_currency));
    let complete = valuation.is_complete();
    ever_unmarked.extend(valuation.unmarked);
    Point {
        at: event.at,
        seq: event.seq,
        value: valuation.value,
        complete,
    }
}

fn caveats(
    epochs_skipped: usize,
    points: &[Point],
    unmarked: &BTreeSet<String>,
    capture: Option<&RecordingManifest>,
) -> Vec<String> {
    let mut caveats = Vec::new();
    if epochs_skipped > 0 {
        caveats.push(format!(
            "{epochs_skipped} earlier account epoch(s) on the journal were skipped; the curve and \
             ledger describe the final epoch only"
        ));
    }
    let incomplete = points.iter().filter(|point| !point.complete).count();
    if incomplete > 0 {
        let assets: Vec<&str> = unmarked.iter().map(String::as_str).collect();
        caveats.push(format!(
            "{incomplete} of {} curve point(s) could not mark every holding from the recording \
             (unmarked: {}); their values count cash and marked assets only",
            points.len(),
            assets.join(", ")
        ));
    }
    match CaptureState::of(capture) {
        // Absent needs no health caveat: valuation impact already surfaces
        // through the incomplete-point disclosure above.
        CaptureState::Absent => {}
        CaptureState::Unfinalized(_) => caveats.push(
            "the market recording never finalized (recorder still running or crashed); the marks' \
             completeness is unknown"
                .to_string(),
        ),
        CaptureState::Finalized(_, summary) => {
            // Reconnects are deliberately not caveated: each replays a
            // snapshot of absolute values, which the mark table absorbs.
            if summary.events_dropped > 0 || summary.frames_unparsed > 0 {
                caveats.push(format!(
                    "the recording reports {} dropped and {} unparsed frame(s); marks may have \
                     holes",
                    summary.events_dropped, summary.frames_unparsed
                ));
            }
        }
    }
    caveats
}

#[cfg(test)]
mod tests {
    use super::*;
    use kraken_core::OrderSide;
    use kraken_paper::{AccountEvent, CommandEntry, PaperConfig, account::Origin};
    use kraken_recording::{RecordingDeclaration, RecordingIntegrity};
    use kraken_session::timeline::{
        fixtures::{self, SessionBuilder},
        read,
    };
    use rust_decimal_macros::dec;

    fn assert_close(actual: Decimal, expected: Decimal, what: &str) {
        assert!(
            (actual - expected).abs() < dec!(0.000000001),
            "{what}: {actual} != {expected}"
        );
    }

    fn shape(curve: &Curve) -> Vec<(DateTime<Utc>, Decimal, bool)> {
        // `seq` is deliberately excluded: twin sessions differing by one
        // extra (absorbed) frame allocate different seqs downstream.
        curve
            .points
            .iter()
            .map(|point| (point.at, point.value, point.complete))
            .collect()
    }

    fn unfinalized_capture() -> RecordingManifest {
        RecordingManifest {
            schema_version: "3.0".to_string(),
            meta: RecordingDeclaration {
                source: "test".to_string(),
                symbols: vec!["BTC/USD".to_string()],
                channels: vec!["ticker".to_string()],
                window_start: fixtures::T0.parse().unwrap(),
                cli_version: "test".to_string(),
            },
            summary: None,
        }
    }

    fn finalized_capture(summary: RecordingIntegrity) -> RecordingManifest {
        RecordingManifest {
            summary: Some(summary),
            ..unfinalized_capture()
        }
    }

    // -- acceptance criteria -------------------------------------------------

    #[test]
    fn same_fixture_session_folds_to_byte_identical_output_twice() {
        let base = tempfile::tempdir().unwrap();
        let name = fixtures::provision(base.path(), vec![fixtures::jsonl_recording()]);
        fixtures::write_market(
            base.path(),
            &name,
            &["2026-01-01T00:00:01.000000Z", "2026-01-01T00:00:03.000000Z"],
        );
        fixtures::write_account(
            base.path(),
            &name,
            &[
                fixtures::account_record("2026-01-01T00:00:00+00:00"),
                fixtures::fill_record("2026-01-01T00:00:02+00:00"),
            ],
        );

        let first = replay(&read(base.path(), &name).unwrap()).unwrap();
        let second = replay(&read(base.path(), &name).unwrap()).unwrap();

        assert_eq!(
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&second).unwrap()
        );
        assert!(!first.curve.points.is_empty());
    }

    #[test]
    fn multi_asset_valuation_is_deterministic_across_replays() {
        // Decimal sums at these scales are exact in any order, but the
        // byte-identity contract must not rest on that: `valuation` iterates
        // balances sorted so even a pathological 28-digit rounding case
        // lands identically every replay. This pins the serialized bytes of
        // a three-asset fold across repeated replays (fresh HashMaps each).
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        builder.market_fill(
            "2026-01-01T00:01:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.01),
            (dec!(50_000.0), dec!(49_900.0)),
        );
        builder.market_fill(
            "2026-01-01T00:02:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "ETHUSD",
            dec!(0.5),
            (dec!(2_950.0), dec!(2_940.0)),
        );
        builder.ticker(
            "2026-01-01T00:03:00Z",
            "BTC/USD",
            dec!(50_000.0),
            dec!(50_100.0),
        );
        builder.ticker(
            "2026-01-01T00:04:00Z",
            "ETH/USD",
            dec!(3_330.0),
            dec!(3_339.3),
        );
        let session = builder.session(None, fixtures::manifest(vec![]));

        let reference = serde_json::to_string(&replay(&session).unwrap()).unwrap();
        for _ in 0..8 {
            assert_eq!(
                serde_json::to_string(&replay(&session).unwrap()).unwrap(),
                reference
            );
        }
    }

    #[test]
    fn single_outlier_tick_between_good_ticks_leaves_the_curve_identical() {
        let build = |with_outlier: bool| {
            let mut builder = SessionBuilder::with_rates(dec!(100_000.0), dec!(0.0026), dec!(0.0));
            builder.market_fill(
                "2026-01-01T00:01:00Z",
                Origin::Cli,
                OrderSide::Buy,
                "BTCUSD",
                dec!(0.1),
                (dec!(60_000.0), dec!(59_900.0)),
            );
            builder.ticker(
                "2026-01-01T00:02:00Z",
                "BTC/USD",
                dec!(60_000.0),
                dec!(60_100.0),
            );
            builder.ticker(
                "2026-01-01T00:03:00Z",
                "BTC/USD",
                dec!(60_000.0),
                dec!(60_100.0),
            );
            builder.ticker(
                "2026-01-01T00:04:00Z",
                "BTC/USD",
                dec!(60_000.0),
                dec!(60_100.0),
            );
            if with_outlier {
                builder.ticker("2026-01-01T00:05:00Z", "BTC/USD", dec!(6.0), dec!(6.2));
            }
            builder.ticker(
                "2026-01-01T00:06:00Z",
                "BTC/USD",
                dec!(60_000.0),
                dec!(60_100.0),
            );
            replay(&builder.session(None, fixtures::manifest(vec![]))).unwrap()
        };

        let clean = build(false);
        let poisoned = build(true);
        assert_eq!(shape(&poisoned.curve), shape(&clean.curve));
    }

    #[test]
    fn unmarkable_held_asset_flags_every_point_and_names_it() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        builder.market_fill(
            "2026-01-01T00:01:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "ETHUSD",
            dec!(1.0),
            (dec!(3_000.0), dec!(2_990.0)),
        );
        // Only BTC is recorded; the moved BTC mark values no holding, so it
        // must not add a point either.
        builder.ticker(
            "2026-01-01T00:02:00Z",
            "BTC/USD",
            dec!(60_000.0),
            dec!(60_100.0),
        );
        let replay = replay(&builder.session(None, fixtures::manifest(vec![]))).unwrap();

        assert_eq!(replay.curve.points.len(), 2, "init + fill only");
        let fill_point = &replay.curve.points[1];
        assert!(!fill_point.complete);
        assert_close(
            fill_point.value,
            dec!(10_000.0) - dec!(3_000.0) - dec!(7.8),
            "cash remainder",
        );
        assert_eq!(
            replay.curve.unmarked,
            BTreeSet::from(["ETH".to_string()]),
            "the culprit is named"
        );
        assert!(
            replay.caveats.iter().any(|c| c.contains("ETH")),
            "caveats: {:?}",
            replay.caveats
        );
    }

    #[test]
    fn session_with_no_market_recording_yields_flagged_cash_only_curve() {
        let base = tempfile::tempdir().unwrap();
        let name = fixtures::provision(base.path(), vec![]);
        fixtures::write_account(
            base.path(),
            &name,
            &[
                fixtures::account_record("2026-01-01T00:00:00+00:00"),
                fixtures::fill_record("2026-01-01T00:00:02+00:00"),
            ],
        );

        let session = read(base.path(), &name).unwrap();
        assert!(session.capture.is_none());
        let replay = replay(&session).unwrap();

        assert_eq!(replay.curve.points.len(), 2);
        assert!(replay.curve.points[0].complete, "all-cash start values 1:1");
        assert!(!replay.curve.points[1].complete, "held BTC has no mark");
        assert_eq!(replay.curve.unmarked, BTreeSet::from(["BTC".to_string()]));
    }

    // -- emission rule -------------------------------------------------------

    #[test]
    fn mark_moves_after_a_fill_extend_the_curve() {
        let mut builder = SessionBuilder::with_rates(dec!(100_000.0), dec!(0.0026), dec!(0.0));
        builder.market_fill(
            "2026-01-01T00:01:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.1),
            (dec!(50_000.0), dec!(49_900.0)),
        );
        builder.ticker(
            "2026-01-01T00:02:00Z",
            "BTC/USD",
            dec!(50_000.0),
            dec!(50_100.0),
        );
        builder.ticker(
            "2026-01-01T00:03:00Z",
            "BTC/USD",
            dec!(51_000.0),
            dec!(51_100.0),
        );
        let replay = replay(&builder.session(None, fixtures::manifest(vec![]))).unwrap();

        // init, fill (unmarked), first mark, warm-up mean move.
        assert_eq!(replay.curve.points.len(), 4);
        let cash = dec!(100_000.0)
            - dec!(0.1) * dec!(50_000.0)
            - dec!(0.1) * dec!(50_000.0) * dec!(0.0026);
        assert_close(
            replay.curve.points[2].value,
            cash + dec!(0.1) * dec!(50_050.0),
            "first mark",
        );
        assert_close(
            replay.curve.points[3].value,
            cash + dec!(0.1) * (dec!(50_050.0) + dec!(51_050.0)) / dec!(2.0),
            "two-observation warm-up mean",
        );
        assert!(replay.curve.points[2].complete);
        let seqs: Vec<i64> = replay.curve.points.iter().map(|p| p.seq).collect();
        assert_eq!(
            seqs,
            [1, 2, 3, 4],
            "each point carries its triggering event's seq"
        );
    }

    #[test]
    fn decision_events_emit_no_curve_points() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        builder.decision("2026-01-01T00:01:00Z", "PAPER-00001", "dip");
        let replay = replay(&builder.session(None, fixtures::manifest(vec![]))).unwrap();
        assert_eq!(replay.curve.points.len(), 1, "the init point only");
    }

    #[test]
    fn every_account_record_in_the_epoch_emits_a_point() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        builder.account(
            "2026-01-01T00:01:00Z",
            Origin::Cli,
            AccountEvent::Command(CommandEntry::default()),
        );
        let replay = replay(&builder.session(None, fixtures::manifest(vec![]))).unwrap();

        // The observational no-op changes nothing, yet still lands a point:
        // one point per journal line is the predictable contract.
        assert_eq!(replay.curve.points.len(), 2);
        assert_eq!(replay.curve.points[0].value, replay.curve.points[1].value);
    }

    #[test]
    fn first_mark_for_a_held_asset_emits_a_point_and_clears_the_flag() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        builder.market_fill(
            "2026-01-01T00:01:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "ETHUSD",
            dec!(1.0),
            (dec!(3_000.0), dec!(2_990.0)),
        );
        builder.ticker(
            "2026-01-01T00:02:00Z",
            "ETH/USD",
            dec!(3_000.0),
            dec!(3_010.0),
        );
        let replay = replay(&builder.session(None, fixtures::manifest(vec![]))).unwrap();

        assert_eq!(replay.curve.points.len(), 3);
        assert!(!replay.curve.points[1].complete);
        let marked = &replay.curve.points[2];
        assert!(marked.complete);
        assert_close(
            marked.value,
            dec!(10_000.0) - dec!(3_007.8) + dec!(3_005.0),
            "ETH at mid",
        );
        // Ever-unmarked is a history, not the final state.
        assert_eq!(replay.curve.unmarked, BTreeSet::from(["ETH".to_string()]));
    }

    #[test]
    fn same_instant_frame_marks_before_the_fill_is_valued() {
        let ts = "2026-01-01T00:01:00Z";
        let mut builder = SessionBuilder::with_rates(dec!(100_000.0), dec!(0.0026), dec!(0.0));
        builder.market_fill(
            ts,
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.1),
            (dec!(50_000.0), dec!(49_900.0)),
        );
        // Pushed after the fill but tied on the instant: the merge order
        // (Market < Account) must mark the book before the fill is valued.
        builder.ticker(ts, "BTC/USD", dec!(50_000.0), dec!(50_100.0));
        let replay = replay(&builder.session(None, fixtures::manifest(vec![]))).unwrap();

        assert_eq!(
            replay.curve.points.len(),
            2,
            "init + fill; no separate mark point"
        );
        assert!(replay.curve.points[1].complete);
        assert!(replay.curve.unmarked.is_empty());
    }

    // -- epochs ---------------------------------------------------------------

    #[test]
    fn reset_mid_session_folds_only_the_final_epoch() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        builder.market_fill(
            "2026-01-01T00:01:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.1),
            (dec!(50_000.0), dec!(49_900.0)),
        );
        builder.account(
            "2026-01-01T00:02:00Z",
            Origin::Cli,
            AccountEvent::Reset(PaperConfig {
                balance: dec!(5_000.0),
                currency: "USD".to_string(),
                fee_rate: dec!(0.0026),
                slippage_rate: dec!(0.0),
            }),
        );
        let epoch_two_fill = builder.market_fill(
            "2026-01-01T00:03:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "ETHUSD",
            dec!(1.0),
            (dec!(3_000.0), dec!(2_990.0)),
        );
        let replay = replay(&builder.session(None, fixtures::manifest(vec![]))).unwrap();

        assert_eq!(replay.epochs_skipped, 1);
        assert!(replay.caveats.iter().any(|c| c.contains("skipped")));
        let first = &replay.curve.points[0];
        assert_close(
            first.value,
            dec!(5_000.0),
            "curve starts at the reset balance",
        );
        assert!(first.complete);
        // Order ids restart per epoch (both epochs mint PAPER-00001), so the
        // pair is what proves this is the epoch-2 fill.
        assert_eq!(replay.fills.len(), 1, "only the final epoch's fills");
        assert_eq!(replay.fills[0].id, epoch_two_fill.id);
        assert_eq!(replay.fills[0].pair, "ETHUSD");
    }

    #[test]
    fn pre_epoch_frames_warm_marks_for_the_first_post_reset_point() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        // Three pre-epoch observations: the post-reset fill is valued with a
        // fully warmed median, so the market tape is proven epoch-unscoped.
        builder.ticker(
            "2026-01-01T00:01:00Z",
            "BTC/USD",
            dec!(60_000.0),
            dec!(60_100.0),
        );
        builder.ticker(
            "2026-01-01T00:01:10Z",
            "BTC/USD",
            dec!(60_000.0),
            dec!(60_100.0),
        );
        builder.ticker(
            "2026-01-01T00:01:20Z",
            "BTC/USD",
            dec!(60_000.0),
            dec!(60_100.0),
        );
        builder.account(
            "2026-01-01T00:02:00Z",
            Origin::Cli,
            AccountEvent::Reset(PaperConfig {
                balance: dec!(100_000.0),
                currency: "USD".to_string(),
                fee_rate: dec!(0.0),
                slippage_rate: dec!(0.0),
            }),
        );
        builder.market_fill(
            "2026-01-01T00:03:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(1.0),
            (dec!(60_050.0), dec!(60_000.0)),
        );
        let replay = replay(&builder.session(None, fixtures::manifest(vec![]))).unwrap();

        // Exactly the reset point and the fill point: the pre-epoch frames
        // must feed the table silently, never emit points of their own.
        assert_eq!(replay.curve.points.len(), 2);
        let reset_point = &replay.curve.points[0];
        assert_close(
            reset_point.value,
            dec!(100_000.0),
            "curve starts at the reset",
        );
        let fill_point = &replay.curve.points[1];
        assert!(fill_point.complete, "pre-epoch marks value the fill");
        assert_close(
            fill_point.value,
            dec!(100_000.0) - dec!(60_050.0) + dec!(60_050.0),
            "cash after fill + 1 BTC at the warmed mid",
        );
    }

    #[test]
    fn market_only_session_is_a_validation_error_not_a_fabricated_curve() {
        let base = tempfile::tempdir().unwrap();
        let name = fixtures::provision(base.path(), vec![fixtures::jsonl_recording()]);
        fixtures::write_market(base.path(), &name, &["2026-01-01T00:00:01.000000Z"]);

        let err = replay(&read(base.path(), &name).unwrap()).unwrap_err();
        assert!(matches!(err, LabError::NoEpoch { .. }), "got: {err}");
        assert!(
            err.to_string().contains("no paper account epoch"),
            "got: {err}"
        );
    }

    // -- engine subtleties ----------------------------------------------------

    #[test]
    fn short_position_values_at_face_without_marks_or_flags() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        // Handcrafted oversell: `apply` trusts the journal, exactly like a
        // replayed real journal would (decide_* would refuse this).
        builder.account(
            "2026-01-01T00:01:00Z",
            Origin::Cli,
            AccountEvent::OrderFilled {
                trade: PaperTrade {
                    id: "PAPER-00001".to_string(),
                    order_id: "PAPER-00001".to_string(),
                    pair: "BTCUSD".to_string(),
                    base: "BTC".to_string(),
                    quote: "USD".to_string(),
                    side: OrderSide::Sell,
                    volume: dec!(2.0),
                    price: dec!(50_000.0),
                    fee: dec!(0.0),
                    cost: dec!(100_000.0),
                    filled_at: "2026-01-01T00:01:00Z".parse().unwrap(),
                    reference_quote: None,
                },
            },
        );
        builder.ticker(
            "2026-01-01T00:02:00Z",
            "BTC/USD",
            dec!(60_000.0),
            dec!(60_100.0),
        );
        let replay = replay(&builder.session(None, fixtures::manifest(vec![]))).unwrap();

        // Pins compute_portfolio_value's pre-existing rule: a negative
        // balance is added at face value (units, not marked) and never
        // flagged — so the BTC mark arriving must not move the curve.
        assert_eq!(replay.curve.points.len(), 2);
        let short_point = &replay.curve.points[1];
        assert!(short_point.complete);
        assert_close(
            short_point.value,
            dec!(10_000.0) + dec!(100_000.0) - dec!(2.0),
            "face value",
        );
        assert!(replay.curve.unmarked.is_empty());
    }

    #[test]
    fn dust_balance_neither_values_nor_flags() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        builder.account(
            "2026-01-01T00:01:00Z",
            Origin::Cli,
            AccountEvent::OrderFilled {
                trade: PaperTrade {
                    id: "PAPER-00001".to_string(),
                    order_id: "PAPER-00001".to_string(),
                    pair: "BTCUSD".to_string(),
                    base: "BTC".to_string(),
                    quote: "USD".to_string(),
                    side: OrderSide::Buy,
                    volume: dec!(0.0000000000001),
                    price: dec!(50_000.0),
                    fee: dec!(0.0),
                    cost: dec!(0.000000005),
                    filled_at: "2026-01-01T00:01:00Z".parse().unwrap(),
                    reference_quote: None,
                },
            },
        );
        let replay = replay(&builder.session(None, fixtures::manifest(vec![]))).unwrap();

        let point = replay.curve.points.last().unwrap();
        assert!(
            point.complete,
            "dust is not a holding, so nothing is unmarked"
        );
        assert!(replay.curve.unmarked.is_empty());
    }

    // -- ledger ---------------------------------------------------------------

    #[test]
    fn ledger_rows_are_the_epoch_fills_verbatim_in_journal_order() {
        let mut builder = SessionBuilder::with_rates(dec!(100_000.0), dec!(0.0026), dec!(0.001));
        let first = builder.market_fill(
            "2026-01-01T00:01:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.1),
            (dec!(50_000.0), dec!(49_900.0)),
        );
        let second = builder.market_fill(
            "2026-01-01T00:02:00Z",
            Origin::Mcp,
            OrderSide::Sell,
            "BTCUSD",
            dec!(0.05),
            (dec!(51_000.0), dec!(50_900.0)),
        );
        let replay = replay(&builder.session(None, fixtures::manifest(vec![]))).unwrap();

        assert_eq!(replay.fills.len(), 2);
        assert_eq!(replay.fills[0].id, first.id);
        assert_eq!(replay.fills[1].id, second.id);
        assert_close(replay.fills[0].price, first.price, "price verbatim");
        assert_close(replay.fills[0].fee, first.fee, "fee verbatim");
        assert!(
            replay.fills[0].reference_quote.is_some(),
            "engine fills carry the quote"
        );
        let quote = replay.fills[0].reference_quote.unwrap();
        assert_close(quote.ask, dec!(50_000.0), "raw pre-slippage top of book");
    }

    #[test]
    fn legacy_fill_without_reference_quote_serializes_without_the_field() {
        let base = tempfile::tempdir().unwrap();
        let name = fixtures::provision(base.path(), vec![]);
        fixtures::write_account(
            base.path(),
            &name,
            &[
                fixtures::account_record("2026-01-01T00:00:00+00:00"),
                fixtures::fill_record("2026-01-01T00:00:02+00:00"),
            ],
        );

        let replay = replay(&read(base.path(), &name).unwrap()).unwrap();
        assert!(replay.fills[0].reference_quote.is_none());
        let row = serde_json::to_value(&replay.fills[0]).unwrap();
        assert!(
            row.get("reference_quote").is_none(),
            "absence is the signal"
        );
    }

    // -- capture health ---------------------------------------------------------

    #[test]
    fn unfinalized_capture_is_disclosed_as_a_caveat() {
        let builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        let session = builder.session(Some(unfinalized_capture()), fixtures::manifest(vec![]));
        let replay = replay(&session).unwrap();
        assert!(
            replay.caveats.iter().any(|c| c.contains("never finalized")),
            "caveats: {:?}",
            replay.caveats
        );
    }

    #[test]
    fn dropped_frames_are_disclosed_as_a_caveat() {
        let builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        let session = builder.session(
            Some(finalized_capture(RecordingIntegrity {
                window_end: "2026-01-01T01:00:00Z".parse().unwrap(),
                events_dropped: 3,
                ..RecordingIntegrity::now()
            })),
            fixtures::manifest(vec![]),
        );
        let replay = replay(&session).unwrap();
        assert!(
            replay.caveats.iter().any(|c| c.contains("3 dropped")),
            "caveats: {:?}",
            replay.caveats
        );
    }

    #[test]
    fn clean_finalized_capture_adds_no_caveat() {
        let builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        let session = builder.session(
            Some(finalized_capture(RecordingIntegrity {
                window_end: "2026-01-01T01:00:00Z".parse().unwrap(),
                ..RecordingIntegrity::now()
            })),
            fixtures::manifest(vec![]),
        );
        assert!(replay(&session).unwrap().caveats.is_empty());
    }

    // -- properties -------------------------------------------------------------

    mod properties {
        use super::*;
        use proptest::prelude::*;
        use rust_decimal::prelude::FromPrimitive;

        #[derive(Debug, Clone)]
        enum Op {
            Ticker(Decimal),
            Fill(Decimal),
            Decision,
        }

        /// Fuzzed f64 ranges, snapped to `Decimal` at the boundary so the
        /// engine math stays float-free.
        fn op() -> impl Strategy<Value = Op> {
            prop_oneof![
                (1_000.0..100_000.0_f64)
                    .prop_map(|price| Op::Ticker(Decimal::from_f64(price).unwrap())),
                (0.001..0.1_f64).prop_map(|volume| Op::Fill(Decimal::from_f64(volume).unwrap())),
                Just(Op::Decision),
            ]
        }

        /// Drive the real engine through a random single-epoch op sequence;
        /// infeasible fills are skipped, exactly as `decide_*` rejects live.
        fn random_events(ops: &[Op]) -> Vec<TimelineEvent> {
            let mut builder =
                SessionBuilder::with_rates(dec!(1_000_000.0), dec!(0.0026), dec!(0.0));
            for (index, op) in ops.iter().enumerate() {
                let ts = format!("2026-01-01T00:{:02}:{:02}Z", 10 + index / 60, index % 60);
                match op {
                    Op::Ticker(price) => {
                        builder.ticker(&ts, "BTC/USD", *price, *price * dec!(1.001));
                    }
                    Op::Fill(volume) => {
                        if builder
                            .state
                            .decide_market_order(
                                OrderSide::Buy,
                                "BTCUSD",
                                *volume,
                                dec!(50_000.0),
                                dec!(49_900.0),
                            )
                            .is_ok()
                        {
                            builder.market_fill(
                                &ts,
                                Origin::Cli,
                                OrderSide::Buy,
                                "BTCUSD",
                                *volume,
                                (dec!(50_000.0), dec!(49_900.0)),
                            );
                        }
                    }
                    Op::Decision => builder.decision(&ts, "PAPER-00001", "why"),
                }
            }
            builder.events
        }

        fn session_of(events: Vec<TimelineEvent>) -> SessionTimeline {
            SessionTimeline {
                events,
                capture: None,
                manifest: fixtures::manifest(vec![]),
                newer_records_skipped: 0,
            }
        }

        proptest! {
            /// The fold is a pure left fold with no lookahead: replaying a
            /// timeline prefix yields exactly a prefix of the full curve.
            /// A lookahead-based outlier filter would fail this.
            #[test]
            fn curve_of_a_prefix_is_a_prefix_of_the_curve(
                ops in prop::collection::vec(op(), 0..40),
                cut in 1usize..41,
            ) {
                let events = random_events(&ops);
                let cut = cut.min(events.len());
                let prefix = replay(&session_of(events[..cut].to_vec())).unwrap();
                let full = replay(&session_of(events)).unwrap();

                let taken = prefix.curve.points.len();
                prop_assert!(taken <= full.curve.points.len());
                prop_assert_eq!(&full.curve.points[..taken], &prefix.curve.points[..]);
            }

            /// After a symbol's warm-up, one arbitrary tick never pushes the
            /// curve outside the clean observations' hull.
            #[test]
            fn one_tick_after_warmup_stays_within_the_good_range(
                outlier in 0.0001..1_000_000.0_f64,
            ) {
                let outlier = Decimal::from_f64(outlier).unwrap();
                let mut builder = SessionBuilder::with_rates(dec!(100_000.0), dec!(0.0026), dec!(0.0));
                builder.market_fill(
                    "2026-01-01T00:01:00Z",
                    Origin::Cli,
                    OrderSide::Buy,
                    "BTCUSD",
                    dec!(1.0),
                    (dec!(60_000.0), dec!(59_900.0)),
                );
                builder.ticker("2026-01-01T00:02:00Z", "BTC/USD", dec!(59_990.0), dec!(60_010.0));
                builder.ticker("2026-01-01T00:03:00Z", "BTC/USD", dec!(59_990.0), dec!(60_010.0));
                builder.ticker("2026-01-01T00:04:00Z", "BTC/USD", dec!(59_990.0), dec!(60_010.0));
                builder.ticker("2026-01-01T00:05:00Z", "BTC/USD", outlier, outlier * dec!(1.001));
                let replayed = replay(&builder.session(None, fixtures::manifest(vec![]))).unwrap();

                let cash = dec!(100_000.0) - dec!(60_000.0) - dec!(60_000.0) * dec!(0.0026);
                let good_mark = dec!(60_000.0);
                // Points 0 and 1 (init, pre-mark fill) predate any mark.
                for point in replayed.curve.points.iter().skip(2) {
                    prop_assert!(
                        (point.value - (cash + good_mark)).abs() <= dec!(1.0),
                        "point {} strayed from the good hull",
                        point.value
                    );
                }
            }
        }
    }
}

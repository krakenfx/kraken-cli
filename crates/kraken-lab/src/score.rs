//! Mechanical metrics for a stopped session.
//!
//! Shared P&L values are carried from the explanation report; added metrics
//! remain derivable from the recorded curve and ledger.

use std::collections::BTreeSet;

use kraken_paper::{OrderSide, PaperTrade};
use kraken_replay::{Anchor, ComponentKind, ComponentLine, PnlReport, explain_pnl};
use kraken_session::timeline::SessionTimeline;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Serialize;
use serde_with::skip_serializing_none;

use crate::trades::RoundTrip;
use crate::{Curve, Result, replay};

/// Scores a stopped session, refusing incomplete anchors or account epochs.
pub fn score(session: &SessionTimeline) -> Result<Scorecard> {
    let report = explain_pnl(session)?;
    let folded = replay(session)?;

    let source = source_of(&session.manifest);
    let metrics = metrics(&report, &folded.curve, &folded.fills);
    let trades = trade_stats(&report, &folded.fills);
    let curve = curve_summary(&folded.curve);
    let divergence = divergence_caveat(&report.anchor, &curve);
    let caveats = merged_caveats(
        report.caveats,
        folded
            .caveats
            .into_iter()
            .chain(divergence)
            .chain(replay_caveats(&session.manifest)),
    );

    let PnlReport {
        session,
        strategy,
        anchor,
        components,
        epochs_skipped,
        ..
    } = report;
    Ok(Scorecard {
        group: "lab",
        kind: "scorecard",
        session,
        strategy,
        anchor,
        components,
        metrics,
        trades,
        curve,
        epochs_skipped,
        source,
        caveats,
    })
}

/// A stopped session's honest scorecard. The serialized shape is a public
/// contract (the future kraken-app command surface); fields are
/// additive-only.
#[skip_serializing_none]
#[derive(Debug, Serialize)]
pub struct Scorecard {
    /// Output-envelope discriminators: `group` names the command surface
    /// (distinct from an account's execution `mode` of paper/live), `type` the
    /// result kind.
    pub group: &'static str,
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// The session's id, verbatim from the explain-pnl report.
    pub session: String,
    pub strategy: Option<String>,
    /// The stop totals, verbatim from the explain-pnl report.
    pub anchor: Anchor,
    /// The additive waterfall, verbatim from the explain-pnl report — the
    /// can't-disagree contract.
    pub components: Vec<ComponentLine>,
    pub metrics: Metrics,
    pub trades: TradeStats,
    pub curve: CurveSummary,
    pub epochs_skipped: usize,
    /// What the session traded against — derived from the session manifest, never
    /// asserted. First-class so a reader (and the promotion gate) need not
    /// parse provenance out of the caveat strings.
    pub source: ScoreSource,
    pub caveats: Vec<String>,
}

/// A scored session's market-data provenance.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScoreSource {
    /// A live-market run over these symbols.
    Live {
        symbols: Vec<String>,
        /// Actual live exposure (`ended_at - started_at`), whole seconds;
        /// `None` for a still-open or corrupt window. Surfaced because a
        /// sealed `live:<window>` is not enforced at plan-match time — the
        /// evidence reader must see how long the run really was.
        #[serde(skip_serializing_if = "Option::is_none")]
        window_secs: Option<u64>,
    },
    /// A replay of a recorded tape at this playback speed.
    Replay { tape: String, speed: f64 },
}

/// The manifest's source block is the single authority: present ⇒ replay,
/// absent ⇒ live over whatever symbols the session recorded.
fn source_of(manifest: &kraken_session::manifest::SessionManifest) -> ScoreSource {
    match &manifest.source {
        Some(source) => ScoreSource::Replay {
            tape: source.tape.clone(),
            speed: source.speed,
        },
        None => ScoreSource::Live {
            symbols: manifest
                .recordings
                .iter()
                .flat_map(|recording| recording.symbols.iter().cloned())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            window_secs: manifest.window.ended_at.and_then(|ended| {
                u64::try_from((ended - manifest.window.started_at).num_seconds()).ok()
            }),
        },
    }
}

/// The derived numbers. Every `None` is an honest "not derivable for this
/// session", never a fake zero.
#[skip_serializing_none]
#[derive(Debug, Serialize)]
pub struct Metrics {
    /// `total_pnl / starting_balance`, in percent. `None` only for a
    /// zero-balance epoch (a hand-edited journal).
    pub return_pct: Option<Decimal>,
    /// Largest peak-to-trough drop on the marked curve (≥ 0); `None` when
    /// the curve has no points.
    pub max_drawdown: Option<Decimal>,
    /// The worst drop relative to its peak at any point, in percent — the
    /// conventional maximum drawdown. Zero when the curve never dropped;
    /// `None` when a drop exists but no positive peak anchors a percent.
    pub max_drawdown_pct: Option<Decimal>,
    /// The waterfall's fees line, carried verbatim (typically negative).
    pub fees: Option<Decimal>,
    /// Spread + slippage from the waterfall — the friction the fills paid
    /// beyond fees. `None` when either line is underivable.
    pub friction: Option<Decimal>,
    /// Total converted fill notional over the starting balance, in percent.
    /// `None` when any fill lacks a conversion — a partial sum would
    /// understate turnover, never report a lowball.
    pub turnover_pct: Option<Decimal>,
    /// Closed FIFO round trips won (P&L net of pro-rata fees > 0) over all
    /// closed, 0..=1. Per-lot and unweighted by design: one sell closing N
    /// lots contributes N same-sign observations. `None` when no round trip
    /// closed — open positions are never graded (a buys-only DCA run must
    /// not read as a 0% hit rate).
    pub hit_rate: Option<Decimal>,
    /// Gross profit over gross loss across closed round trips. `None` when
    /// there is no loss to divide by, or when closed trips span multiple
    /// quote currencies (their P&L must not be summed as one number).
    pub profit_factor: Option<Decimal>,
    /// d(P&L)/d(slippage) per basis point of extra slippage:
    /// −(total converted notional) / 10 000, in the valuation currency.
    /// Analytic and linear by assumption — orders never move the tape, so a
    /// refold could not honestly do better. `None` whenever turnover is.
    pub slippage_sensitivity: Option<Decimal>,
}

/// Counts over the fill ledger and the report's joins — no notionals here:
/// fills can be quoted in different currencies, and the converted sums
/// already live in the report's attribution.
#[derive(Debug, Serialize)]
pub struct TradeStats {
    pub fills: usize,
    pub buys: usize,
    pub sells: usize,
    /// Fills whose order id joins a logged decision (`--reason`).
    pub decided: usize,
    /// Resting orders the counterfactual scan flagged (never in any sum).
    pub missed_fills: usize,
}

/// The marked curve, summarized — the full point series stays behind
/// [`fn@crate::replay`] for consumers that want to draw it.
#[skip_serializing_none]
#[derive(Debug, Serialize)]
pub struct CurveSummary {
    pub currency: String,
    pub points: usize,
    /// Whether every point marked every holding.
    pub complete: bool,
    pub first_value: Option<Decimal>,
    pub last_value: Option<Decimal>,
    pub peak: Option<Decimal>,
    pub trough: Option<Decimal>,
    /// Assets ever held with no usable recorded mark.
    pub unmarked: BTreeSet<String>,
}

fn metrics(report: &PnlReport, curve: &Curve, fills: &[PaperTrade]) -> Metrics {
    let component = |kind: ComponentKind| {
        report
            .components
            .iter()
            .find(|line| line.kind == kind)
            .and_then(|line| line.amount)
    };
    let spread = component(ComponentKind::Spread);
    let slippage = component(ComponentKind::Slippage);
    let (max_drawdown, max_drawdown_pct) = drawdown(curve);
    // One missing conversion poisons the whole sum: turnover and the
    // sensitivity derived from it go None together, never a partial figure.
    let notional_total = report
        .trades
        .iter()
        .try_fold(Decimal::ZERO, |sum, line| line.notional.map(|n| sum + n));
    let trips = crate::trades::round_trips(fills);
    let turnover_pct = notional_total
        .and_then(|total| total.checked_div(report.anchor.starting_balance))
        .map(|ratio| ratio * dec!(100));
    Metrics {
        return_pct: report
            .anchor
            .total_pnl
            .checked_div(report.anchor.starting_balance)
            .map(|ratio| ratio * dec!(100)),
        max_drawdown,
        max_drawdown_pct,
        fees: component(ComponentKind::Fees),
        friction: spread.zip(slippage).map(|(s, l)| s + l),
        turnover_pct,
        hit_rate: hit_rate(&trips),
        profit_factor: profit_factor(&trips),
        // Tied to turnover by construction, so "None whenever turnover is"
        // holds even on a zero-balance epoch where only the division fails.
        slippage_sensitivity: notional_total
            .filter(|_| turnover_pct.is_some())
            .map(|total| -total / dec!(10_000)),
    }
}

fn hit_rate(trips: &[RoundTrip]) -> Option<Decimal> {
    if trips.is_empty() {
        return None;
    }
    let wins = trips
        .iter()
        .filter(|trip| trip.net_pnl > Decimal::ZERO)
        .count();
    Some(Decimal::from(wins) / Decimal::from(trips.len()))
}

fn profit_factor(trips: &[RoundTrip]) -> Option<Decimal> {
    let (first, rest) = trips.split_first()?;
    // Round-trip P&L is quoted per pair; a cross-currency sum would be a
    // number with no unit. One quote currency or no figure.
    if rest.iter().any(|trip| trip.quote != first.quote) {
        return None;
    }
    let gross = |sign: Decimal| -> Decimal {
        trips
            .iter()
            .map(|trip| (trip.net_pnl * sign).max(Decimal::ZERO))
            .sum()
    };
    let (profit, loss) = (gross(Decimal::ONE), gross(Decimal::NEGATIVE_ONE));
    (loss > Decimal::ZERO).then(|| profit / loss)
}

/// The seam between deterministic tape marks and the stop snapshot's live
/// REST mark, surfaced when it is material relative to the account size.
const DIVERGENCE_CAVEAT_THRESHOLD_PCT: Decimal = dec!(1);

fn divergence_caveat(anchor: &Anchor, curve: &CurveSummary) -> Option<String> {
    let last = curve.last_value?;
    let divergence_pct = (last - anchor.final_value)
        .abs()
        .checked_div(anchor.starting_balance)?
        * dec!(100);
    (divergence_pct > DIVERGENCE_CAVEAT_THRESHOLD_PCT).then(|| {
        format!(
            "tape-marked end value {last} diverges from the stop anchor {} by \
             {divergence_pct:.2}% of the starting balance — recorded marks and the stop \
             snapshot's live mark disagree",
            anchor.final_value
        )
    })
}

/// Largest peak-to-trough drop, and the worst drop *relative to its peak*
/// at any point in percent (conventional maximum drawdown), one exact pass.
/// The two maxima can anchor on different peaks: a later, taller peak can
/// host the largest absolute drop while an earlier, smaller one hosted the
/// deepest relative one. Percent candidates need a positive peak — a drop
/// in negative equity gets no percent, never a fake (or negative) one — but
/// a curve with no drop at all is honestly 0%, no division needed.
fn drawdown(curve: &Curve) -> (Option<Decimal>, Option<Decimal>) {
    let mut points = curve.points.iter();
    let Some(first) = points.next() else {
        return (None, None);
    };
    let mut peak = first.value;
    let mut worst = Decimal::ZERO;
    let mut worst_pct: Option<Decimal> = None;
    for point in points {
        if point.value > peak {
            peak = point.value;
        }
        let drop = peak - point.value;
        if drop > worst {
            worst = drop;
        }
        // Checked: a hand-edited journal can pair a dust peak with a drop
        // whose ratio overflows Decimal — skip the candidate, never panic.
        if peak > Decimal::ZERO
            && let Some(pct) = drop
                .checked_div(peak)
                .and_then(|ratio| ratio.checked_mul(dec!(100)))
            && worst_pct.is_none_or(|current| pct > current)
        {
            worst_pct = Some(pct);
        }
    }
    if worst == Decimal::ZERO {
        worst_pct = Some(Decimal::ZERO);
    }
    (Some(worst), worst_pct)
}

fn trade_stats(report: &PnlReport, fills: &[PaperTrade]) -> TradeStats {
    let buys = fills
        .iter()
        .filter(|trade| trade.side == OrderSide::Buy)
        .count();
    TradeStats {
        fills: fills.len(),
        buys,
        sells: fills.len() - buys,
        decided: report.attribution.iter().map(|slice| slice.decided).sum(),
        missed_fills: report.missed_fills.orders.len(),
    }
}

fn curve_summary(curve: &Curve) -> CurveSummary {
    let values = || curve.points.iter().map(|point| point.value);
    CurveSummary {
        currency: curve.currency.clone(),
        points: curve.points.len(),
        complete: curve.points.iter().all(|point| point.complete),
        first_value: curve.points.first().map(|point| point.value),
        last_value: curve.points.last().map(|point| point.value),
        peak: values().max(),
        trough: values().min(),
        unmarked: curve.unmarked.clone(),
    }
}

/// Required disclosures that keep replay evidence below live promotion evidence.
fn replay_caveats(manifest: &kraken_session::manifest::SessionManifest) -> Vec<String> {
    let Some(source) = &manifest.source else {
        return Vec::new();
    };
    let mut caveats = vec![
        format!(
            "replayed from {} at {}x speed — a recorded-data session, not a live market",
            source.tape, source.speed
        ),
        "lookahead not enforced: the agent can read the source tape; replay is screening \
         evidence, never promotion-grade alone"
            .to_string(),
        "fills simulate against top-of-book marks (no depth, queue, or partial fills) and \
         orders never move the tape"
            .to_string(),
    ];
    // At or below 1x the agent has live-or-better reaction time; the
    // compression disclosure would be false there.
    if source.speed > 1.0 {
        caveats.push(format!(
            "reaction-time compression: at {}x the agent has 1/{}th of live reaction time",
            source.speed, source.speed
        ));
    }
    caveats
}

/// Report caveats first (the waterfall is the primary artifact), then the
/// fold's; exact duplicates collapse while first-seen order is kept.
fn merged_caveats(report: Vec<String>, fold: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    report
        .into_iter()
        .chain(fold)
        .filter(|caveat| seen.insert(caveat.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use kraken_paper::account::Origin;
    use kraken_session::manifest::SessionOutcome;
    use kraken_session::timeline::fixtures::{self, SessionBuilder};

    use super::*;
    use crate::LabError;

    /// A stopped one-fill session: buy 0.1 BTC at 50k mid (no spread, no
    /// slippage, 0.26% fee), mark moves to 52k.
    fn stopped_session() -> SessionTimeline {
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
        builder.ticker(
            "2026-01-01T00:00:03Z",
            "BTC/USD",
            dec!(52_000),
            dec!(52_000),
        );

        let mut manifest = fixtures::manifest(vec![]);
        // 10_000 − 5_000 cost − 13 fee + 0.1 × 52_000 end mark = 10_187.
        manifest.summary = Some(SessionOutcome {
            ended_at: "2026-01-01T00:00:04Z".parse().unwrap(),
            final_value: dec!(10_187),
            pnl: dec!(187),
        });
        builder.session(None, manifest)
    }

    #[test]
    fn worked_example_scores_the_stop_anchor_exactly() {
        let card = score(&stopped_session()).unwrap();
        assert_eq!(card.anchor.total_pnl, dec!(187));
        assert_eq!(card.metrics.return_pct, Some(dec!(1.87)));
        assert_eq!(card.metrics.fees, Some(-dec!(13)), "the waterfall's line");
        assert_eq!(card.trades.fills, 1);
        assert_eq!(card.trades.buys, 1);
        assert_eq!(card.trades.sells, 0);
        assert_eq!(card.curve.currency, "USD");
        assert!(card.curve.complete);
        // 5_000 notional over 10_000 starting balance; the open lot is
        // never graded.
        assert_eq!(card.metrics.turnover_pct, Some(dec!(50)));
        assert_eq!(card.metrics.slippage_sensitivity, Some(dec!(-0.5)));
        assert_eq!(card.metrics.hit_rate, None, "no closed round trip");
        assert_eq!(card.metrics.profit_factor, None);
    }

    /// A closed winning round trip: buy 0.1 @ 50k, sell 0.1 @ 52k, zero
    /// rates. Hit rate grades the trip; profit factor stays honest — with
    /// no loss there is nothing to divide by.
    #[test]
    fn closed_winning_round_trip_grades_hit_rate_but_not_profit_factor() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000), dec!(0.0), dec!(0.0));
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
        builder.ticker(
            "2026-01-01T00:00:03Z",
            "BTC/USD",
            dec!(52_000),
            dec!(52_000),
        );
        builder.market_fill(
            "2026-01-01T00:00:04Z",
            Origin::Cli,
            OrderSide::Sell,
            "BTCUSD",
            dec!(0.1),
            (dec!(52_000), dec!(52_000)),
        );
        let mut manifest = fixtures::manifest(vec![]);
        manifest.summary = Some(SessionOutcome {
            ended_at: "2026-01-01T00:00:05Z".parse().unwrap(),
            final_value: dec!(10_200),
            pnl: dec!(200),
        });
        let card = score(&builder.session(None, manifest)).unwrap();

        assert_eq!(card.metrics.hit_rate, Some(dec!(1)));
        assert_eq!(card.metrics.profit_factor, None, "gross loss is zero");
        // 5_000 + 5_200 notional over 10_000.
        assert_eq!(card.metrics.turnover_pct, Some(dec!(102)));
        assert_eq!(card.metrics.slippage_sensitivity, Some(dec!(-1.02)));
    }

    /// An all-losing session pins the zero boundaries: hit rate 0, profit
    /// factor 0 — real zeros from graded trips, not absent values.
    #[test]
    fn all_losing_round_trips_score_zero_hit_rate_and_zero_profit_factor() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000), dec!(0.0), dec!(0.0));
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
        builder.ticker(
            "2026-01-01T00:00:03Z",
            "BTC/USD",
            dec!(49_000),
            dec!(49_000),
        );
        builder.market_fill(
            "2026-01-01T00:00:04Z",
            Origin::Cli,
            OrderSide::Sell,
            "BTCUSD",
            dec!(0.1),
            (dec!(49_000), dec!(49_000)),
        );
        let mut manifest = fixtures::manifest(vec![]);
        manifest.summary = Some(SessionOutcome {
            ended_at: "2026-01-01T00:00:05Z".parse().unwrap(),
            final_value: dec!(9_900),
            pnl: dec!(-100),
        });
        let card = score(&builder.session(None, manifest)).unwrap();

        assert_eq!(card.metrics.hit_rate, Some(dec!(0)));
        assert_eq!(card.metrics.profit_factor, Some(dec!(0)));
    }

    /// No fills at all: turnover is a real zero (derivable, and zero), while
    /// the lot metrics are honestly absent.
    #[test]
    fn fill_less_session_has_zero_turnover_and_absent_lot_metrics() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000), dec!(0.0026), dec!(0.0));
        builder.ticker(
            "2026-01-01T00:00:01Z",
            "BTC/USD",
            dec!(50_000),
            dec!(50_000),
        );
        let mut manifest = fixtures::manifest(vec![]);
        manifest.summary = Some(SessionOutcome {
            ended_at: "2026-01-01T00:00:02Z".parse().unwrap(),
            final_value: dec!(10_000),
            pnl: dec!(0),
        });
        let card = score(&builder.session(None, manifest)).unwrap();

        assert_eq!(card.metrics.turnover_pct, Some(dec!(0)));
        assert_eq!(card.metrics.slippage_sensitivity, Some(dec!(0)));
        assert_eq!(card.metrics.hit_rate, None);
        assert_eq!(card.metrics.profit_factor, None);
    }

    /// The can't-disagree pin: the scorecard's components serialize
    /// byte-identically to the explain-pnl report's.
    #[test]
    fn components_are_byte_equal_to_the_pnl_report() {
        let session = stopped_session();
        let card = score(&session).unwrap();
        let report = explain_pnl(&session).unwrap();
        assert_eq!(
            serde_json::to_vec(&card.components).unwrap(),
            serde_json::to_vec(&report.components).unwrap()
        );
        assert_eq!(
            serde_json::to_vec(&card.anchor).unwrap(),
            serde_json::to_vec(&report.anchor).unwrap()
        );
    }

    #[test]
    fn drawdown_is_the_largest_peak_to_trough_drop() {
        // Curve 10_000 → 10_400 → 10_100 → 10_500 → 10_200: the worst drop
        // is 300 from either peak; the pct is the deeper of the two relative
        // drops (300 of 10_400).
        let mut builder = SessionBuilder::with_rates(dec!(10_000), dec!(0.0), dec!(0.0));
        builder.ticker("2026-01-01T00:00:01Z", "BTC/USD", dec!(100), dec!(100));
        builder.market_fill(
            "2026-01-01T00:00:02Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(100),
            (dec!(100), dec!(100)),
        );
        for (ts, mark) in [
            ("2026-01-01T00:00:03Z", dec!(104)),
            ("2026-01-01T00:00:04Z", dec!(101)),
            ("2026-01-01T00:00:05Z", dec!(105)),
            ("2026-01-01T00:00:06Z", dec!(102)),
        ] {
            // Three repeats warm the median window past the outlier guard.
            builder.ticker(ts, "BTC/USD", mark, mark);
            builder.ticker(ts, "BTC/USD", mark, mark);
            builder.ticker(ts, "BTC/USD", mark, mark);
        }
        let mut manifest = fixtures::manifest(vec![]);
        manifest.summary = Some(SessionOutcome {
            ended_at: "2026-01-01T00:00:07Z".parse().unwrap(),
            final_value: dec!(10_200),
            pnl: dec!(200),
        });
        let session = builder.session(None, manifest);

        let card = score(&session).unwrap();
        assert_eq!(card.metrics.max_drawdown, Some(dec!(300)));
        let pct = card.metrics.max_drawdown_pct.expect("peak is nonzero");
        assert!(
            pct > dec!(2.85) && pct < dec!(2.89),
            "300 of ~10_400/10_500: {pct}"
        );
    }

    #[test]
    fn drawdown_pct_is_the_relative_worst_not_the_absolute_worst() {
        // Curve 1_000 → 500 → 10_000 → 8_900: the account halves early
        // (drop 500 of peak 1_000 = 50%), then a larger absolute drop
        // (1_100 of peak 10_000 = 11%) follows. A frozen risk criterion
        // must see the 50%, not the lenient 11% of the absolute worst.
        let mut builder = SessionBuilder::with_rates(dec!(1_000), dec!(0.0), dec!(0.0));
        builder.ticker("2026-01-01T00:00:01Z", "BTC/USD", dec!(100), dec!(100));
        builder.market_fill(
            "2026-01-01T00:00:02Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(10),
            (dec!(100), dec!(100)),
        );
        for (ts, mark) in [
            ("2026-01-01T00:00:03Z", dec!(50)),
            ("2026-01-01T00:00:04Z", dec!(1_000)),
            ("2026-01-01T00:00:05Z", dec!(890)),
        ] {
            // Three repeats warm the median window past the outlier guard.
            builder.ticker(ts, "BTC/USD", mark, mark);
            builder.ticker(ts, "BTC/USD", mark, mark);
            builder.ticker(ts, "BTC/USD", mark, mark);
        }
        let mut manifest = fixtures::manifest(vec![]);
        manifest.summary = Some(SessionOutcome {
            ended_at: "2026-01-01T00:00:06Z".parse().unwrap(),
            final_value: dec!(8_900),
            pnl: dec!(7_900),
        });
        let session = builder.session(None, manifest);

        let card = score(&session).unwrap();
        assert_eq!(card.metrics.max_drawdown, Some(dec!(1_100)));
        assert_eq!(card.metrics.max_drawdown_pct, Some(dec!(50)));
    }

    fn curve_of(values: &[Decimal]) -> crate::Curve {
        crate::Curve {
            currency: "USD".into(),
            points: values
                .iter()
                .enumerate()
                .map(|(i, &value)| crate::Point {
                    at: "2026-01-01T00:00:00Z".parse().unwrap(),
                    seq: i as i64,
                    value,
                    complete: true,
                })
                .collect(),
            unmarked: BTreeSet::new(),
        }
    }

    /// The no-drop boundary: max_drawdown and its pct agree on "nothing
    /// dropped" (0, 0%) whatever the curve length or sign.
    #[test]
    fn dropless_curve_reports_zero_drawdown_and_zero_pct() {
        for dropless in [&[dec!(10_000)][..], &[dec!(-500), dec!(-500)][..]] {
            assert_eq!(
                drawdown(&curve_of(dropless)),
                (Some(Decimal::ZERO), Some(Decimal::ZERO))
            );
        }
    }

    /// Negative equity dropping further: the drop is real, but the percent
    /// has no positive peak to anchor on — None, never a negative percent.
    #[test]
    fn negative_equity_drop_has_no_pct() {
        assert_eq!(
            drawdown(&curve_of(&[dec!(-1_000), dec!(-1_500)])),
            (Some(dec!(500)), None)
        );
    }

    /// The replay honesty envelope rides any session whose manifest carries
    /// a source block, and the compression line names the speed.
    #[test]
    fn replay_provenance_gains_the_honesty_envelope() {
        let mut session = stopped_session();
        session.manifest.source = Some(kraken_session::manifest::SessionSource {
            tape: "tape:jun-crash".to_string(),
            speed: 5.0,
            anchor: "2026-06-01T00:00:00Z".parse().unwrap(),
            started_at: "2026-07-01T00:00:00Z".parse().unwrap(),
            content_hash: None,
        });
        let card = score(&session).unwrap();
        for needle in [
            "replayed from tape:jun-crash at 5x speed",
            "lookahead not enforced",
            "top-of-book marks",
            "reaction-time compression: at 5x",
        ] {
            assert!(
                card.caveats.iter().any(|caveat| caveat.contains(needle)),
                "missing {needle:?} in {:?}",
                card.caveats
            );
        }
    }

    /// At real-time speed the compression disclosure would be false — the
    /// other three replay caveats still apply.
    #[test]
    fn real_time_replay_omits_the_reaction_time_caveat() {
        let mut session = stopped_session();
        session.manifest.source = Some(kraken_session::manifest::SessionSource {
            tape: "tape:jun-crash".to_string(),
            speed: 1.0,
            anchor: "2026-06-01T00:00:00Z".parse().unwrap(),
            started_at: "2026-07-01T00:00:00Z".parse().unwrap(),
            content_hash: None,
        });
        let card = score(&session).unwrap();
        assert!(
            !card
                .caveats
                .iter()
                .any(|caveat| caveat.contains("reaction-time compression")),
            "caveats: {:?}",
            card.caveats
        );
        assert!(
            card.caveats
                .iter()
                .any(|caveat| caveat.contains("replayed from")),
        );
    }

    /// Pins that score inherits explain-pnl's partial-journal refusal: a
    /// timeline that skipped newer-version records must become a named
    /// error (an error column in compare), never a confident scorecard.
    #[test]
    fn partial_journal_is_refused_not_scored() {
        let mut session = stopped_session();
        session.newer_records_skipped = 1;
        let err = score(&session).unwrap_err();
        assert!(
            matches!(
                err,
                LabError::Replay(kraken_replay::ReplayError::NewerJournal { .. })
            ),
            "got: {err}"
        );
    }

    #[test]
    fn unstopped_session_is_refused_through_the_replay_gate() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000), dec!(0.0026), dec!(0.0));
        builder.ticker(
            "2026-01-01T00:00:01Z",
            "BTC/USD",
            dec!(50_000),
            dec!(50_000),
        );
        let session = builder.session(None, fixtures::manifest(vec![]));

        let err = score(&session).unwrap_err();
        assert!(
            matches!(
                err,
                LabError::Replay(kraken_replay::ReplayError::NotStopped { .. })
            ),
            "got: {err}"
        );
    }

    /// A stop anchor 1.13% of the starting balance away from the tape-marked
    /// end value must be called out. The fixture's curve ends at 10_087 —
    /// the two-observation mark warm-up means (50k+52k)/2, not the last
    /// frame's 52k.
    #[test]
    fn material_tape_vs_anchor_divergence_gains_a_caveat() {
        let mut session = stopped_session();
        session.manifest.summary = Some(SessionOutcome {
            ended_at: "2026-01-01T00:00:04Z".parse().unwrap(),
            final_value: dec!(10_200),
            pnl: dec!(200),
        });
        let card = score(&session).unwrap();
        assert!(
            card.caveats
                .iter()
                .any(|caveat| caveat.contains("diverges from the stop anchor")),
            "caveats: {:?}",
            card.caveats
        );
    }

    /// The threshold is strictly greater-than: a divergence of exactly 1%
    /// of the starting balance (curve end 10_087 vs anchor 10_187) stays
    /// silent.
    #[test]
    fn divergence_at_exactly_the_threshold_stays_silent() {
        let mut session = stopped_session();
        session.manifest.summary = Some(SessionOutcome {
            ended_at: "2026-01-01T00:00:04Z".parse().unwrap(),
            final_value: dec!(10_187),
            pnl: dec!(187),
        });
        let card = score(&session).unwrap();
        assert!(
            !card
                .caveats
                .iter()
                .any(|caveat| caveat.contains("diverges from the stop anchor")),
            "caveats: {:?}",
            card.caveats
        );
    }

    /// Round-trip P&L is quoted per pair; summing across quote currencies
    /// would be a number with no unit — the factor honestly refuses.
    #[test]
    fn profit_factor_refuses_mixed_quote_currencies() {
        let trips = [
            RoundTrip {
                quote: "USD".to_string(),
                net_pnl: dec!(10),
            },
            RoundTrip {
                quote: "EUR".to_string(),
                net_pnl: dec!(-5),
            },
        ];
        assert_eq!(profit_factor(&trips), None);
    }

    #[test]
    fn caveat_merge_deduplicates_but_keeps_distinct_texts() {
        let merged = merged_caveats(
            vec!["a".into(), "b".into()],
            vec!["b".into(), "c".into(), "a".into()],
        );
        assert_eq!(merged, ["a", "b", "c"]);
    }

    #[test]
    fn scorecard_serializes_the_pinned_top_level_keys() {
        let card = score(&stopped_session()).unwrap();
        let json = serde_json::to_value(&card).unwrap();
        assert_eq!(json["group"], "lab");
        assert_eq!(json["type"], "scorecard");
        assert_eq!(json["session"], "btc-dip");
        assert_eq!(json["anchor"]["total_pnl"], 187.0);
        assert_eq!(json["metrics"]["return_pct"], 1.87);
        assert_eq!(json["trades"]["fills"], 1);
        assert_eq!(json["curve"]["currency"], "USD");
        // No source block on the fixture manifest ⇒ live, with the recorded
        // symbols (none here). First-class provenance, not a caveat string.
        assert_eq!(json["source"]["kind"], "live");
        assert!(json.get("strategy").is_none(), "absent optionals drop out");
    }

    #[test]
    fn source_is_replay_when_the_manifest_carries_a_source_block() {
        let mut session = stopped_session();
        session.manifest.source = Some(kraken_session::manifest::SessionSource {
            tape: "tape:jun-crash".to_string(),
            speed: 10.0,
            anchor: "2026-06-01T00:00:00Z".parse().unwrap(),
            started_at: "2026-07-01T00:00:00Z".parse().unwrap(),
            content_hash: None,
        });
        let card = score(&session).unwrap();
        match card.source {
            ScoreSource::Replay { tape, speed } => {
                assert_eq!(tape, "tape:jun-crash");
                assert_eq!(speed, 10.0);
            }
            other => panic!("expected replay source, got {other:?}"),
        }
    }

    #[test]
    fn live_source_reports_the_actual_window_seconds() {
        let mut session = stopped_session();
        session.manifest.window.ended_at = Some("2026-01-01T00:00:21Z".parse().unwrap());
        let card = score(&session).unwrap();
        match card.source {
            ScoreSource::Live { window_secs, .. } => assert_eq!(window_secs, Some(21)),
            other => panic!("expected live source, got {other:?}"),
        }
    }

    /// A window that ends before it starts is corrupt; the exposure is
    /// honestly `None`, never a wrapped or clamped number.
    #[test]
    fn live_source_refuses_a_window_that_ends_before_it_starts() {
        let mut session = stopped_session();
        session.manifest.window.ended_at = Some("2025-12-31T23:59:00Z".parse().unwrap());
        let card = score(&session).unwrap();
        match card.source {
            ScoreSource::Live { window_secs, .. } => assert_eq!(window_secs, None),
            other => panic!("expected live source, got {other:?}"),
        }
    }
}

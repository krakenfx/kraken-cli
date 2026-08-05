//! Pure P&L decomposition from a session timeline and manifest.
//!
//! Stop totals remain authoritative. Quantized components plus a remainder
//! preserve the emitted total; the remainder also exposes valuation gaps and
//! disclosed data loss.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use kraken_core::{ChannelData, OrderSide};
use kraken_paper::account::Origin;
use kraken_paper::{AccountEvent, PaperOrder, PaperOrderType, PaperState, PaperTrade};
use kraken_paper::{canonical_key, valid_quote};
use kraken_recording::{CaptureState, RecordingManifest};
use kraken_session::manifest::SessionManifest;
use kraken_session::timeline::SessionTimeline;
use kraken_session::timeline::event::{EventPayload, TimelineEvent, final_epoch_start};
use rust_decimal::{Decimal, RoundingStrategy};
use rust_decimal_macros::dec;

use self::report::{
    Anchor, ComponentKind, ComponentLine, Coverage, EndMarkSource, MissedFill, MissedFillSection,
    MissedOutcome, OriginSlice, PnlReport, SymbolWindow, TradeLine, WindowSummary,
};
use crate::{ReplayError, Result};

pub(crate) mod report;

/// The honesty preamble on every missed-fill section: how fills actually
/// happen, and why this scan is a proxy.
const MISSED_FILL_ASSUMPTIONS: &str = "the paper engine reconciles resting orders only when a command runs, against live REST \
     quotes; this scan replays the recorded stream instead — a proxy that can disagree with \
     what actually happened in either direction — and a cancelled order may have been \
     cancelled before any cross";

/// Keeps typical component values within lossless f64 JSON precision.
const COMPONENT_SCALE: u32 = 8;

/// Decomposes a stopped session, failing closed on incomplete account data.
pub fn explain_pnl(session: &SessionTimeline) -> Result<PnlReport> {
    let manifest: &SessionManifest = &session.manifest;
    let Some(outcome) = &manifest.summary else {
        return Err(ReplayError::NotStopped {
            session: manifest.id.clone(),
        });
    };
    if session.newer_records_skipped > 0 {
        return Err(ReplayError::NewerJournal {
            skipped: session.newer_records_skipped,
        });
    }

    let Some(epoch) = final_epoch(&session.events) else {
        return Err(ReplayError::NoEpoch {
            session: manifest.id.clone(),
        });
    };
    let index = MarketIndex::build(&session.events);
    let sc = epoch.state.starting_currency.clone();

    let mut caveats = Vec::new();
    if epoch.epochs_skipped > 0 {
        caveats.push(format!(
            "{} earlier account epoch(s) on the journal were skipped; the anchor and every \
             component describe the final epoch only",
            epoch.epochs_skipped
        ));
    }

    let decomposition = components(&epoch, &index, &sc, outcome.pnl, &mut caveats);
    let missed = missed_fills(&epoch, &index, session.capture.as_ref(), &sc);
    let attribution = attribution(&decomposition.trades);
    let window = window_summary(&index, session.capture.as_ref());

    Ok(PnlReport {
        group: "session",
        kind: "pnl_explanation",
        session: manifest.id.clone(),
        strategy: manifest.strategy.as_ref().map(|s| s.name.clone()),
        anchor: Anchor {
            starting_balance: epoch.state.starting_balance,
            final_value: outcome.final_value,
            total_pnl: outcome.pnl,
            ended_at: outcome.ended_at,
            currency: sc,
        },
        components: decomposition.components,
        trades: decomposition.trades,
        missed_fills: MissedFillSection {
            assumptions: MISSED_FILL_ASSUMPTIONS,
            orders: missed,
        },
        attribution,
        window,
        epochs_skipped: epoch.epochs_skipped,
        caveats,
    })
}

/// The journal's final epoch: everything at/after the last
/// `Initialized`/`Reset`, folded through [`PaperState::apply`] for exact
/// parity with what `run stop` anchored on, plus the side maps the
/// decomposition joins against.
struct FinalEpoch {
    state: PaperState,
    fills: Vec<Fill>,
    /// Submitted (resting) orders by id — the recorded `order_type` and the
    /// submitting actor. Market fills never journal a submission.
    submissions: HashMap<String, Submission>,
    /// Cancellation instants by order id (the record's `ts` is authoritative
    /// for cancellations).
    cancelled_at: HashMap<String, DateTime<Utc>>,
    /// `--reason` rationale by order id, final-epoch decisions only (order
    /// ids restart per epoch, so an older epoch's decision must not join).
    reasons: HashMap<String, String>,
    epochs_skipped: usize,
}

struct Fill {
    trade: PaperTrade,
    /// The *fill record's* origin: whoever ran the command that reconciled
    /// it — for limit fills the submission's origin names the decider.
    origin: Origin,
}

struct Submission {
    order_type: PaperOrderType,
    origin: Origin,
}

/// `None` when no `Initialized`/`Reset` is on the record: folding from
/// [`PaperState::default`] would conjure an account no writer created (the
/// journal's own projection in `kraken_paper::account` refuses the same).
fn final_epoch(events: &[TimelineEvent]) -> Option<FinalEpoch> {
    let epoch_start = final_epoch_start(events);
    let start = epoch_start.start?;
    let mut epoch = FinalEpoch {
        state: PaperState::default(),
        fills: Vec::new(),
        submissions: HashMap::new(),
        cancelled_at: HashMap::new(),
        reasons: HashMap::new(),
        epochs_skipped: epoch_start.skipped,
    };
    for event in &events[start..] {
        match &event.payload {
            EventPayload::Account(record) => {
                epoch.state.apply(record.ts, &record.event);
                match &record.event {
                    AccountEvent::OrderSubmitted { order } => {
                        epoch.submissions.insert(
                            order.id.clone(),
                            Submission {
                                order_type: order.order_type,
                                origin: record.origin,
                            },
                        );
                    }
                    AccountEvent::OrderFilled { trade } => {
                        epoch.fills.push(Fill {
                            trade: trade.clone(),
                            origin: record.origin,
                        });
                    }
                    AccountEvent::OrderCancelled { order } => {
                        epoch.cancelled_at.insert(order.id.clone(), record.ts);
                    }
                    AccountEvent::Initialized(_)
                    | AccountEvent::Reset(_)
                    | AccountEvent::Command(_)
                    | AccountEvent::OrderRejected(_)
                    | AccountEvent::Unknown => {}
                    // `AccountEvent` is `#[non_exhaustive]`: whatever a newer
                    // build adds is a no-op here, exactly like `Unknown`.
                    _ => {}
                }
            }
            EventPayload::Decision(decision) => {
                if let Some(order_id) = &decision.order_id {
                    epoch
                        .reasons
                        .insert(order_id.clone(), decision.reason.clone());
                }
            }
            EventPayload::Market(_) => {}
        }
    }
    Some(epoch)
}

/// A recorded ticker top-of-book at one timeline instant.
struct TickerPoint {
    at: DateTime<Utc>,
    bid: Decimal,
    ask: Decimal,
}

impl TickerPoint {
    fn mid(&self) -> Decimal {
        (self.ask + self.bid) / dec!(2)
    }
}

/// One instrument's recorded market series.
#[derive(Default)]
struct SymbolSeries {
    ticker: Vec<TickerPoint>,
    last_trade_price: Option<Decimal>,
    last_ohlc_close: Option<Decimal>,
    frames: usize,
}

/// The recorded market, indexed by canonical `(base, quote)` — never by the
/// pair string: `parse_pair` canonicalizes XBT→BTC only in its base/quote
/// outputs, so string keys would not collide `XBTUSD` with `BTC/USD`.
struct MarketIndex {
    series: HashMap<(String, String), SymbolSeries>,
}

impl MarketIndex {
    fn build(events: &[TimelineEvent]) -> Self {
        let mut series: HashMap<(String, String), SymbolSeries> = HashMap::new();
        let mut unparseable: HashSet<String> = HashSet::new();
        for event in events {
            let EventPayload::Market(frame) = &event.payload else {
                continue;
            };
            match &frame.body {
                ChannelData::Ticker(entries) => {
                    for ticker in entries {
                        let Some(entry) = entry_for(&mut series, &mut unparseable, &ticker.symbol)
                        else {
                            continue;
                        };
                        entry.frames += 1;
                        if valid_quote(ticker.ask, ticker.bid) {
                            entry.ticker.push(TickerPoint {
                                at: event.at,
                                bid: ticker.bid,
                                ask: ticker.ask,
                            });
                        }
                    }
                }
                ChannelData::Trade(entries) => {
                    for trade in entries {
                        let Some(entry) = entry_for(&mut series, &mut unparseable, &trade.symbol)
                        else {
                            continue;
                        };
                        entry.frames += 1;
                        if trade.price > Decimal::ZERO {
                            entry.last_trade_price = Some(trade.price);
                        }
                    }
                }
                ChannelData::Ohlc(entries) => {
                    for candle in entries {
                        let Some(entry) = entry_for(&mut series, &mut unparseable, &candle.symbol)
                        else {
                            continue;
                        };
                        entry.frames += 1;
                        if candle.close > Decimal::ZERO {
                            entry.last_ohlc_close = Some(candle.close);
                        }
                    }
                }
                ChannelData::Book(entries) => {
                    for book in entries {
                        if let Some(entry) = entry_for(&mut series, &mut unparseable, &book.symbol)
                        {
                            entry.frames += 1;
                        }
                    }
                }
                // Non-market feeds (account, system) carry no per-symbol
                // price series to index.
                ChannelData::Instrument(_)
                | ChannelData::Executions(_)
                | ChannelData::Balances(_)
                | ChannelData::Level3(_)
                | ChannelData::Status(_) => {}
            }
        }
        Self { series }
    }

    fn series(&self, base: &str, quote: &str) -> Option<&SymbolSeries> {
        self.series.get(&(base.to_string(), quote.to_string()))
    }

    /// The recorded end mark for `base` priced in `quote`: last ticker bid
    /// (mirroring `compute_portfolio_value`'s bid marking, so the residual
    /// stays "live vs recorded"), falling back to last trade price, then
    /// last OHLC close.
    fn end_mark(&self, base: &str, quote: &str) -> Option<(Decimal, EndMarkSource)> {
        let series = self.series(base, quote)?;
        if let Some(point) = series.ticker.last() {
            return Some((point.bid, EndMarkSource::TickerBid));
        }
        if let Some(price) = series.last_trade_price {
            return Some((price, EndMarkSource::TradePrice));
        }
        series
            .last_ohlc_close
            .map(|close| (close, EndMarkSource::OhlcClose))
    }

    /// Value of one unit of `asset` in the starting currency at the recorded
    /// end: `1` for the starting currency itself.
    fn rate(&self, asset: &str, starting_currency: &str) -> Option<Decimal> {
        if asset == starting_currency {
            return Some(Decimal::ONE);
        }
        self.end_mark(asset, starting_currency)
            .map(|(mark, _)| mark)
    }

    fn is_empty(&self) -> bool {
        self.series.is_empty()
    }
}

/// The market track tolerates unknown vocabulary like every other reader:
/// a symbol `parse_pair` cannot split is warned once and skipped, never a
/// silent hole in the index.
fn entry_for<'series>(
    series: &'series mut HashMap<(String, String), SymbolSeries>,
    unparseable: &mut HashSet<String>,
    symbol: &str,
) -> Option<&'series mut SymbolSeries> {
    match canonical_key(symbol) {
        Some(key) => Some(series.entry(key).or_default()),
        None => {
            if unparseable.insert(symbol.to_string()) {
                tracing::warn!(symbol, "skipping market frames for unparseable symbol");
            }
            None
        }
    }
}

fn signed(side: OrderSide, volume: Decimal) -> Decimal {
    match side {
        OrderSide::Buy => volume,
        OrderSide::Sell => -volume,
    }
}

struct Decomposition {
    components: Vec<ComponentLine>,
    /// One line per final-epoch fill, in journal order (attribution zips
    /// these against the epoch's fills).
    trades: Vec<TradeLine>,
}

/// The additive split. Per fill *i* (signed volume `s`, fill price `p`, mid
/// `m` of the recorded reference quote, `q` = end-mark rate of the quote
/// asset, `B` = end-mark rate of the base asset):
///
/// - price movement: `s·(B − m·q)` — the frictionless twin, executed at mid
///   and marked at the recorded end
/// - fees: `−fee·q`
/// - spread: market fills pay the half-spread (`ask−m` buy / `m−bid` sell,
///   × volume); limit fills carry `−s·(p−m)` — with detection-time quotes
///   this is a *cost* (the market has already moved through the resting
///   price when the fill is detected), never "price improvement"
/// - slippage: market buy `−(p−ask)·v`, market sell `−(bid−p)·v`, limit `0`
///
/// The split reconstructs `s·(p−m)` exactly in every case, so the recorded-
/// mark identity is exact; whatever cannot be attributed (no reference
/// quote → `m := p`; no end mark → contribution dropped) degrades into
/// price movement or the residual, each with a caveat.
fn components(
    epoch: &FinalEpoch,
    index: &MarketIndex,
    sc: &str,
    total_pnl: Decimal,
    caveats: &mut Vec<String>,
) -> Decomposition {
    let mut totals = FillTotals::default();
    let trades = epoch
        .fills
        .iter()
        .map(|fill| fill_line(fill, epoch, index, sc, &mut totals))
        .collect();
    caveats.extend(degradation_caveats(&totals));

    // Unknown, never a fake zero: with fills on the record but not one
    // derivable movement (unmarked base or unconverted quote on every fill),
    // the component has no number and the residual carries it.
    let underivable = !epoch.fills.is_empty() && totals.movement_derived == 0;
    if underivable {
        caveats.push(
            "not one fill's price movement was derivable from the recorded market; \
             the residual carries it"
                .to_string(),
        );
    }
    let price_movement = (!underivable).then_some(totals.movement);

    Decomposition {
        components: component_lines(epoch, &totals, price_movement, total_pnl),
        trades,
    }
}

/// Running totals over the fill loop — the aggregation half [`components`]
/// reports while [`fill_line`] does the per-fill math.
#[derive(Default)]
struct FillTotals {
    movement: Decimal,
    /// Fills whose movement was derivable (base and quote both marked).
    movement_derived: usize,
    fees: Decimal,
    market_spread: Decimal,
    limit_spread: Decimal,
    slippage: Decimal,
    market_fills: usize,
    limit_fills: usize,
    /// Fills whose quote asset converts to the anchor currency — the set the
    /// fee/spread/slippage sums actually describe.
    converted_fills: usize,
    fills_without_quote: usize,
    unconverted_quotes: BTreeSet<String>,
    unmarked_bases: BTreeSet<String>,
}

/// One fill's decomposition into its [`TradeLine`], folding the
/// contributions into `totals` (see [`components`] for the formulas).
fn fill_line(
    fill: &Fill,
    epoch: &FinalEpoch,
    index: &MarketIndex,
    sc: &str,
    totals: &mut FillTotals,
) -> TradeLine {
    let trade = &fill.trade;
    let submission = epoch.submissions.get(&trade.order_id);
    let order_type = submission.map_or(PaperOrderType::Market, |sub| sub.order_type);
    let s = signed(trade.side, trade.volume);
    let mid = trade.reference_quote.map(|quote| quote.mid());
    if mid.is_none() {
        totals.fills_without_quote += 1;
    }

    let mut line = TradeLine {
        trade_id: trade.id.clone(),
        order_id: trade.order_id.clone(),
        pair: trade.pair.clone(),
        side: trade.side,
        order_type,
        volume: trade.volume,
        price: trade.price,
        notional: None,
        mid,
        price_movement: None,
        fees: Decimal::ZERO,
        spread: Decimal::ZERO,
        slippage: Decimal::ZERO,
        origin: submission.map_or(fill.origin, |sub| sub.origin),
        reason: epoch.reasons.get(&trade.order_id).cloned(),
    };

    let Some(q) = index.rate(&trade.quote, sc) else {
        // Nothing about this fill converts to the anchor currency; the
        // residual absorbs its whole effect.
        totals.unconverted_quotes.insert(trade.quote.clone());
        return line;
    };
    totals.converted_fills += 1;
    line.notional = Some(trade.cost * q);

    // `m := p` when no reference quote survived: friction is forced to
    // zero and rides inside price movement instead.
    let m = mid.unwrap_or(trade.price);
    let (spread_quote, slippage_quote) = match (order_type, trade.reference_quote) {
        (_, None) => (Decimal::ZERO, Decimal::ZERO),
        (PaperOrderType::Limit, Some(_)) => (-s * (trade.price - m), Decimal::ZERO),
        (PaperOrderType::Market, Some(quote)) => match trade.side {
            OrderSide::Buy => (
                -(quote.ask - m) * trade.volume,
                -(trade.price - quote.ask) * trade.volume,
            ),
            OrderSide::Sell => (
                -(m - quote.bid) * trade.volume,
                -(quote.bid - trade.price) * trade.volume,
            ),
        },
    };

    line.fees = -trade.fee * q;
    line.spread = spread_quote * q;
    line.slippage = slippage_quote * q;
    totals.fees += line.fees;
    totals.slippage += line.slippage;
    match order_type {
        PaperOrderType::Market => {
            totals.market_fills += 1;
            totals.market_spread += line.spread;
        }
        PaperOrderType::Limit => {
            totals.limit_fills += 1;
            totals.limit_spread += line.spread;
        }
    }

    match index.rate(&trade.base, sc) {
        Some(base_rate) => {
            let movement = s * (base_rate - m * q);
            line.price_movement = Some(movement);
            totals.movement += movement;
            totals.movement_derived += 1;
        }
        None => {
            totals.unmarked_bases.insert(trade.base.clone());
        }
    }
    line
}

fn degradation_caveats(totals: &FillTotals) -> Vec<String> {
    let mut caveats = Vec::new();
    if totals.fills_without_quote > 0 {
        caveats.push(format!(
            "{} fill(s) carried no valid recorded reference quote; their friction is folded \
             into price movement",
            totals.fills_without_quote
        ));
    }
    for quote in &totals.unconverted_quotes {
        caveats.push(format!(
            "trades quoted in {quote} have no recorded end mark against the account currency; \
             their components are folded into the residual"
        ));
    }
    for base in &totals.unmarked_bases {
        caveats.push(format!(
            "{base} has no recorded end mark; its price movement is folded into the residual"
        ));
    }
    caveats
}

fn component_lines(
    epoch: &FinalEpoch,
    totals: &FillTotals,
    price_movement: Option<Decimal>,
    total_pnl: Decimal,
) -> Vec<ComponentLine> {
    // Money rounding matches the binary's output convention (midpoint away
    // from zero), applied here so the additive identity is defined over the
    // very numbers the report emits.
    let quantize = |amount: Decimal| {
        amount.round_dp_with_strategy(COMPONENT_SCALE, RoundingStrategy::MidpointAwayFromZero)
    };
    let price_movement = price_movement.map(quantize);
    let fees = quantize(totals.fees);
    let spread = quantize(totals.market_spread + totals.limit_spread);
    let slippage = quantize(totals.slippage);
    let residual = total_pnl - (price_movement.unwrap_or(Decimal::ZERO) + fees + spread + slippage);
    let fee_rate_pct = epoch.state.fee_rate * dec!(100);
    let slippage_rate_pct = epoch.state.slippage_rate * dec!(100);
    vec![
        ComponentLine {
            kind: ComponentKind::PriceMovement,
            amount: price_movement,
            explanation: if price_movement.is_none() {
                "the recorded market cannot mark your position changes (see caveats), so their \
                 frictionless value is unknown; the residual carries it"
                    .to_string()
            } else {
                "your position changes valued at mid-market when filled and marked at the \
                 recorded end, before any costs"
                    .to_string()
            },
        },
        ComponentLine {
            kind: ComponentKind::Fees,
            amount: Some(fees),
            explanation: format!(
                "paid across {} fill(s) at the account's {fee_rate_pct:.2}% fee rate",
                totals.converted_fills
            ),
        },
        ComponentLine {
            kind: ComponentKind::Spread,
            amount: Some(spread),
            explanation: format!(
                "{} market fill(s) paid the half-spread ({:.2}); {} limit fill(s) executed at \
                 their resting price vs mid ({:.2}) — a cost of late fill detection",
                totals.market_fills, totals.market_spread, totals.limit_fills, totals.limit_spread
            ),
        },
        ComponentLine {
            kind: ComponentKind::Slippage,
            amount: Some(slippage),
            explanation: format!(
                "simulated slippage on market fills (rate {slippage_rate_pct:.2}%)"
            ),
        },
        ComponentLine {
            kind: ComponentKind::Residual,
            amount: Some(residual),
            explanation: "the gap between the live prices 'session stop' marked at and this \
                          session's recorded prices (plus anything the caveats folded in); \
                          components + residual sum to the total, exactly in the engine and to \
                          double precision on this JSON wire"
                .to_string(),
        },
    ]
}

/// The counterfactual: every final-epoch resting limit order that ended
/// cancelled or still open, scanned against the *recorded* ticker stream
/// under the engine's own crossing rule ([`PaperOrder::crosses`]). Never
/// part of the component sum.
fn missed_fills(
    epoch: &FinalEpoch,
    index: &MarketIndex,
    capture: Option<&RecordingManifest>,
    sc: &str,
) -> Vec<MissedFill> {
    let candidates = epoch
        .state
        .cancelled_orders
        .iter()
        .map(|order| (order, MissedOutcome::Cancelled))
        .chain(
            epoch
                .state
                .open_orders
                .iter()
                .map(|order| (order, MissedOutcome::StillOpen)),
        )
        .filter(|(order, _)| order.order_type == PaperOrderType::Limit);

    candidates
        .map(|(order, outcome)| {
            let resting_until = match outcome {
                MissedOutcome::Cancelled => epoch.cancelled_at.get(&order.id).copied(),
                MissedOutcome::StillOpen => match CaptureState::of(capture) {
                    CaptureState::Finalized(_, summary) => Some(summary.window_end),
                    _ => None,
                },
            };
            let mut missed = MissedFill {
                order_id: order.id.clone(),
                pair: order.pair.clone(),
                side: order.side,
                volume: order.volume,
                limit_price: order.price,
                resting_from: order.created_at,
                resting_until,
                outcome,
                coverage: Coverage::Uncovered,
                uncovered_reason: None,
                crossed: None,
                first_crossed_at: None,
                hypothetical_pnl: None,
            };
            match coverage_window(order, capture, resting_until) {
                Err(reason) => missed.uncovered_reason = Some(reason),
                Ok((from, until)) => {
                    missed.coverage = Coverage::Covered;
                    scan_crossing(&mut missed, epoch, index, order, sc, from, until);
                }
            }
            missed
        })
        .collect()
}

/// All-or-nothing gate: the recording can answer "would this have filled?"
/// only when it is finalized, gap-free, covers the order's pair on the
/// ticker channel, and spans the whole resting window.
fn coverage_window(
    order: &PaperOrder,
    capture: Option<&RecordingManifest>,
    resting_until: Option<DateTime<Utc>>,
) -> std::result::Result<(DateTime<Utc>, DateTime<Utc>), String> {
    let (report, summary) = match CaptureState::of(capture) {
        CaptureState::Absent => return Err("session has no market recording".to_string()),
        CaptureState::Unfinalized(_) => {
            return Err("the market capture never finalized; completeness is unknown".to_string());
        }
        CaptureState::Finalized(report, summary) => (report, summary),
    };
    if summary.events_dropped > 0 {
        return Err(format!(
            "{} recorded frame(s) were dropped mid-capture; gaps would be invisible",
            summary.events_dropped
        ));
    }
    // An unparsed frame never reached the tape: if it was a ticker for this
    // pair, the crossing scan has a hole it cannot see.
    if summary.frames_unparsed > 0 {
        return Err(format!(
            "{} frame(s) failed to parse mid-capture; ticker holes would be invisible",
            summary.frames_unparsed
        ));
    }
    // A reconnect is an invisible gap too: the socket sends a fresh snapshot
    // on resume, never a replay of the outage, so a cross that happened (and
    // reverted) during the disconnect is simply not on the record.
    if summary.reconnect_count > 0 {
        return Err(format!(
            "{} mid-capture reconnect(s); frames during the outage are unrecorded",
            summary.reconnect_count
        ));
    }
    let Some(order_key) = canonical_key(&order.pair) else {
        return Err(format!("the order's pair {} is unparseable", order.pair));
    };
    let recorded_pair = report
        .meta
        .symbols
        .iter()
        .any(|symbol| canonical_key(symbol).as_ref() == Some(&order_key));
    if !recorded_pair {
        return Err(format!("pair {} was not recorded", order.pair));
    }
    if !report
        .meta
        .channels
        .iter()
        .any(|channel| channel == "ticker")
    {
        return Err("the ticker channel was not recorded".to_string());
    }
    let window_start = report.meta.window_start;
    let window_end = summary.window_end;
    let from = order.created_at;
    let until = resting_until.unwrap_or(window_end);
    // `from > until` is a still-open order created *after* the capture
    // finalized (a recorder stopped early while the session kept trading):
    // its resting window inverts, and an empty scan must read as "outside
    // the window", never as "covered, no cross".
    if from < window_start || until > window_end || from > until {
        return Err("the order rested outside the recorded window".to_string());
    }
    Ok((from, until))
}

fn scan_crossing(
    missed: &mut MissedFill,
    epoch: &FinalEpoch,
    index: &MarketIndex,
    order: &PaperOrder,
    sc: &str,
    from: DateTime<Utc>,
    until: DateTime<Utc>,
) {
    let first_cross = index
        .series(&order.base, &order.quote)
        .into_iter()
        .flat_map(|series| &series.ticker)
        .filter(|point| point.at >= from && point.at <= until)
        .find(|point| order.crosses(point.ask, point.bid));
    missed.crossed = Some(first_cross.is_some());
    let Some(cross) = first_cross else {
        return;
    };
    missed.first_crossed_at = Some(cross.at);
    // The hypothetical fill, at the limit price with the engine's fee rule,
    // marked at the recorded end — only when both conversion marks exist.
    if let (Some(base_rate), Some(q)) = (index.rate(&order.base, sc), index.rate(&order.quote, sc))
    {
        let s = signed(order.side, order.volume);
        let fee = order.price * order.volume * epoch.state.fee_rate;
        missed.hypothetical_pnl = Some(s * (base_rate - order.price * q) - fee * q);
    }
}

/// Per-origin slices over the *actual* components: a limit fill belongs to
/// its submitter (the deciding actor), a market fill to its own record's
/// origin — `TradeLine.origin` already encodes that rule, and every summed
/// figure (including notional) is already in the anchor currency.
fn attribution(trades: &[TradeLine]) -> Vec<OriginSlice> {
    let mut cli = empty_slice(Origin::Cli);
    let mut mcp = empty_slice(Origin::Mcp);
    for line in trades {
        let slice = match line.origin {
            Origin::Cli => &mut cli,
            Origin::Mcp => &mut mcp,
        };
        slice.trades += 1;
        slice.gross_notional += line.notional.unwrap_or(Decimal::ZERO);
        slice.fees += line.fees;
        slice.spread += line.spread;
        slice.slippage += line.slippage;
        if line.reason.is_some() {
            slice.decided += 1;
        }
    }
    [cli, mcp]
        .into_iter()
        .filter(|slice| slice.trades > 0)
        .collect()
}

fn empty_slice(origin: Origin) -> OriginSlice {
    OriginSlice {
        origin,
        trades: 0,
        gross_notional: Decimal::ZERO,
        fees: Decimal::ZERO,
        spread: Decimal::ZERO,
        slippage: Decimal::ZERO,
        decided: 0,
    }
}

/// What the recorded market did — the whole story for a zero-trade session.
fn window_summary(
    index: &MarketIndex,
    capture: Option<&RecordingManifest>,
) -> Option<WindowSummary> {
    if index.is_empty() && capture.is_none() {
        return None;
    }
    let mut symbols: Vec<SymbolWindow> = index
        .series
        .iter()
        .map(|((base, quote), series)| {
            let first_mid = series.ticker.first().map(TickerPoint::mid);
            let last_mid = series.ticker.last().map(TickerPoint::mid);
            let move_pct = match (first_mid, last_mid) {
                (Some(first), Some(last)) if first > Decimal::ZERO => {
                    Some((last - first) / first * dec!(100))
                }
                _ => None,
            };
            let avg_spread = (!series.ticker.is_empty()).then(|| {
                series.ticker.iter().map(|p| p.ask - p.bid).sum::<Decimal>()
                    / Decimal::from(series.ticker.len())
            });
            SymbolWindow {
                symbol: format!("{base}{quote}"),
                first_mid,
                last_mid,
                move_pct,
                avg_spread,
                frames: series.frames,
                end_mark_source: index.end_mark(base, quote).map(|(_, source)| source),
            }
        })
        .collect();
    symbols.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    let (start, end) = match CaptureState::of(capture) {
        CaptureState::Absent => (None, None),
        CaptureState::Unfinalized(report) => (Some(report.meta.window_start), None),
        CaptureState::Finalized(report, summary) => {
            (Some(report.meta.window_start), Some(summary.window_end))
        }
    };
    Some(WindowSummary {
        start,
        end,
        symbols,
    })
}

#[cfg(test)]
mod tests {
    use kraken_paper::PaperConfig;
    use kraken_recording::RecordingIntegrity;
    use kraken_session::manifest::SessionOutcome;
    use kraken_session::timeline::fixtures::{self, SessionBuilder, T0};

    use super::*;

    fn stopped_manifest(final_value: Decimal, starting_balance: Decimal) -> SessionManifest {
        let mut manifest = fixtures::manifest(vec![]);
        manifest.summary = Some(SessionOutcome {
            ended_at: "2026-01-01T01:00:00Z".parse().unwrap(),
            final_value,
            pnl: final_value - starting_balance,
        });
        manifest
    }

    fn finalized_capture() -> RecordingManifest {
        RecordingManifest {
            schema_version: "3.0".to_string(),
            meta: kraken_recording::RecordingDeclaration {
                source: "test".to_string(),
                symbols: vec!["BTC/USD".to_string()],
                channels: vec!["ticker".to_string()],
                window_start: T0.parse().unwrap(),
                cli_version: "test".to_string(),
            },
            summary: Some(RecordingIntegrity {
                window_end: "2026-01-01T01:00:00Z".parse().unwrap(),
                ..RecordingIntegrity::now()
            }),
        }
    }

    fn amount(report: &PnlReport, kind: ComponentKind) -> Option<Decimal> {
        report
            .components
            .iter()
            .find(|line| line.kind == kind)
            .and_then(|line| line.amount)
    }

    fn assert_close(actual: Decimal, expected: Decimal, what: &str) {
        assert!(
            (actual - expected).abs() < dec!(0.000000001),
            "{what}: {actual} != {expected}"
        );
    }

    /// A consumer's exact read: the digits the number wire emitted.
    fn wire(value: &serde_json::Value) -> Decimal {
        value.as_number().unwrap().to_string().parse().unwrap()
    }

    fn wire_component_sum(json: &serde_json::Value) -> Decimal {
        json["components"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| wire(&line["amount"]))
            .sum()
    }

    #[test]
    fn components_and_residual_sum_to_the_anchor_total() {
        // The worked example from the design review: buy 0.1 BTC at
        // ask 50_000 / bid 49_900, slippage 0.001, fee 0.0026, last recorded
        // bid 51_000. Recorded-mark P&L = +81.987, so a recorded-consistent
        // anchor leaves residual == 0.
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.001));
        builder.market_fill(
            "2026-01-01T00:10:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.1),
            (dec!(50_000.0), dec!(49_900.0)),
        );
        builder.ticker(
            "2026-01-01T00:50:00Z",
            "BTC/USD",
            dec!(51_000.0),
            dec!(51_100.0),
        );
        let session = builder.session(None, stopped_manifest(dec!(10_081.987), dec!(10_000.0)));

        let report = explain_pnl(&session).unwrap();

        assert_close(
            amount(&report, ComponentKind::PriceMovement).unwrap(),
            dec!(105.0),
            "price movement",
        );
        assert_close(
            amount(&report, ComponentKind::Fees).unwrap(),
            -dec!(13.013),
            "fees",
        );
        assert_close(
            amount(&report, ComponentKind::Spread).unwrap(),
            -dec!(5.0),
            "spread",
        );
        assert_close(
            amount(&report, ComponentKind::Slippage).unwrap(),
            -dec!(5.0),
            "slippage",
        );
        assert_close(
            amount(&report, ComponentKind::Residual).unwrap(),
            dec!(0.0),
            "residual",
        );
        let sum: Decimal = report
            .components
            .iter()
            .filter_map(|line| line.amount)
            .sum();
        assert_close(sum, report.anchor.total_pnl, "sum equals the anchor");
    }

    /// The identity as a consumer sees it: components whose exact Decimals
    /// exceed what a JSON double carries must still re-sum to the anchor
    /// total digit-for-digit on the emitted wire (regression for the
    /// unquantized f64 wire, where each line rounded independently). This
    /// pins the digit-exact regime — an anchor total within
    /// [`COMPONENT_SCALE`] decimals; the sibling test pins the bound beyond.
    #[test]
    fn emitted_component_lines_re_sum_to_the_emitted_total_digit_for_digit() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.001));
        // 0.12345678 × 50_000.12 × 1.001 × 0.0026 fee rate → fees
        // 16.065469338433875…: 17+ significant digits before quantization.
        builder.market_fill(
            "2026-01-01T00:10:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.12345678),
            (dec!(50_000.12), dec!(49_900.99)),
        );
        builder.ticker(
            "2026-01-01T00:50:00Z",
            "BTC/USD",
            dec!(51_000.37),
            dec!(51_100.11),
        );
        let session = builder.session(None, stopped_manifest(dec!(10_123.45), dec!(10_000.0)));

        let report = explain_pnl(&session).unwrap();
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(
            wire_component_sum(&json),
            wire(&json["anchor"]["total_pnl"])
        );
    }

    /// The other regime: an anchor total shaped like the manifest's f64
    /// wire (scale beyond [`COMPONENT_SCALE`]) makes the residual inherit
    /// that scale, so digit-exactness can fall to double precision. One f64
    /// ulp at this magnitude is ~1e-13; this pins the identity inside the
    /// module's 1e-9 closeness envelope, a loose ceiling over that bound
    /// that still fails any quantization regression (those surface at
    /// ≥ 1e-8, the component scale).
    #[test]
    fn wire_scale_anchor_re_sums_within_double_precision() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.001));
        builder.market_fill(
            "2026-01-01T00:10:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.12345678),
            (dec!(50_000.12), dec!(49_900.99)),
        );
        builder.ticker(
            "2026-01-01T00:50:00Z",
            "BTC/USD",
            dec!(51_000.37),
            dec!(51_100.11),
        );
        // pnl = 1_716.9715859185314: 13 decimals, a shortest-f64 shape.
        let session = builder.session(
            None,
            stopped_manifest(dec!(11_716.9715859185314), dec!(10_000.0)),
        );

        let report = explain_pnl(&session).unwrap();
        let json = serde_json::to_value(&report).unwrap();
        assert_close(
            wire_component_sum(&json),
            wire(&json["anchor"]["total_pnl"]),
            "wire re-sum vs emitted total",
        );
    }

    #[test]
    fn sell_market_fill_splits_friction_symmetrically() {
        let mut builder = SessionBuilder::with_rates(dec!(100_000.0), dec!(0.0026), dec!(0.001));
        builder.market_fill(
            "2026-01-01T00:05:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.2),
            (dec!(50_000.0), dec!(49_900.0)),
        );
        builder.market_fill(
            "2026-01-01T00:10:00Z",
            Origin::Cli,
            OrderSide::Sell,
            "BTCUSD",
            dec!(0.1),
            (dec!(50_000.0), dec!(49_900.0)),
        );
        builder.ticker(
            "2026-01-01T00:50:00Z",
            "BTC/USD",
            dec!(51_000.0),
            dec!(51_100.0),
        );
        let session = builder.session(None, stopped_manifest(dec!(10_000.0), dec!(100_000.0)));
        let report = explain_pnl(&session).unwrap();

        let sell = &report.trades[1];
        // half-spread (m − bid) = 50 → −5.0; slippage (bid − p) = 49.9 → −4.99
        assert_close(sell.spread, -dec!(5.0), "sell half-spread");
        assert_close(sell.slippage, -dec!(4.99), "sell slippage");
    }

    #[test]
    fn limit_fill_execution_vs_mid_is_a_cost_not_price_improvement() {
        // Engine semantics: the resting buy fills at its own price only once
        // ask ≤ limit, with the reference quote taken at detection — so the
        // fill is always at-or-above mid: a cost, never an improvement.
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        builder.submit_limit(
            "2026-01-01T00:05:00Z",
            Origin::Mcp,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.1),
            dec!(45_000.0),
        );
        builder.fill_pending(
            "2026-01-01T00:20:00Z",
            Origin::Cli,
            "BTCUSD",
            dec!(44_900.0),
            dec!(44_800.0),
        );
        builder.ticker(
            "2026-01-01T00:50:00Z",
            "BTC/USD",
            dec!(45_500.0),
            dec!(45_600.0),
        );
        let session = builder.session(None, stopped_manifest(dec!(10_050.0), dec!(10_000.0)));
        let report = explain_pnl(&session).unwrap();

        let fill = &report.trades[0];
        assert_eq!(fill.order_type, PaperOrderType::Limit);
        // mid at detection = 44_850; fill at 45_000 → −(45_000 − 44_850)·0.1
        assert_close(fill.spread, -dec!(15.0), "limit execution vs mid is a cost");
        assert_close(fill.slippage, dec!(0.0), "limit fills carry no slippage");
        assert_eq!(
            fill.origin,
            Origin::Mcp,
            "the submitter decided, not the reconciler"
        );
    }

    #[test]
    fn fill_without_reference_quote_contributes_zero_friction_and_is_caveated() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.001));
        let mut trade = builder
            .state
            .decide_market_order(
                OrderSide::Buy,
                "BTCUSD",
                dec!(0.1),
                dec!(50_000.0),
                dec!(49_900.0),
            )
            .unwrap();
        trade.filled_at = "2026-01-01T00:10:00Z".parse().unwrap();
        trade.reference_quote = None; // a legacy journal line
        builder.account(
            "2026-01-01T00:10:00Z",
            Origin::Cli,
            AccountEvent::OrderFilled { trade },
        );
        builder.ticker(
            "2026-01-01T00:50:00Z",
            "BTC/USD",
            dec!(51_000.0),
            dec!(51_100.0),
        );
        let session = builder.session(None, stopped_manifest(dec!(10_081.987), dec!(10_000.0)));
        let report = explain_pnl(&session).unwrap();

        let fill = &report.trades[0];
        assert_close(fill.spread, dec!(0.0), "no quote, no spread attribution");
        assert_close(
            fill.slippage,
            dec!(0.0),
            "no quote, no slippage attribution",
        );
        // m := p folds the friction into price movement: 0.1·(51_000 − 50_050)
        assert_close(
            fill.price_movement.unwrap(),
            dec!(95.0),
            "friction rides in movement",
        );
        assert!(
            report.caveats.iter().any(|c| c.contains("reference quote")),
            "caveats: {:?}",
            report.caveats
        );
    }

    #[test]
    fn zero_trade_session_reports_window_summary_not_error() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        builder.ticker(
            "2026-01-01T00:05:00Z",
            "BTC/USD",
            dec!(49_000.0),
            dec!(49_100.0),
        );
        builder.ticker(
            "2026-01-01T00:50:00Z",
            "BTC/USD",
            dec!(50_000.0),
            dec!(50_100.0),
        );
        let session = builder.session(
            Some(finalized_capture()),
            stopped_manifest(dec!(10_000.0), dec!(10_000.0)),
        );
        let report = explain_pnl(&session).unwrap();

        assert!(report.trades.is_empty());
        for kind in [
            ComponentKind::PriceMovement,
            ComponentKind::Fees,
            ComponentKind::Spread,
            ComponentKind::Slippage,
            ComponentKind::Residual,
        ] {
            assert_close(
                amount(&report, kind).unwrap(),
                dec!(0.0),
                "all-zero components",
            );
        }
        let window = report.window.expect("window story");
        assert_eq!(window.symbols.len(), 1);
        let symbol = &window.symbols[0];
        assert_eq!(symbol.symbol, "BTCUSD");
        assert_close(symbol.avg_spread.unwrap(), dec!(100.0), "avg spread");
        assert!(symbol.move_pct.unwrap() > dec!(2.0), "mid moved ~+2%");
        assert_eq!(symbol.end_mark_source, Some(EndMarkSource::TickerBid));
    }

    #[test]
    fn unstopped_session_is_a_validation_error() {
        let builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        let session = builder.session(None, fixtures::manifest(vec![])); // summary: None

        let err = explain_pnl(&session).unwrap_err();
        assert!(matches!(err, ReplayError::NotStopped { .. }));
        assert!(err.to_string().contains("session stop"), "got: {err}");
    }

    #[test]
    fn journal_records_from_a_newer_build_fail_the_explanation() {
        // The timeline drops what it cannot read; a fold over the partial
        // account track would report confident, wrong numbers.
        let builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        let mut session = builder.session(None, stopped_manifest(dec!(10_000.0), dec!(10_000.0)));
        session.newer_records_skipped = 1;

        let err = explain_pnl(&session).unwrap_err();
        assert!(matches!(err, ReplayError::NewerJournal { skipped: 1 }));
        assert!(err.to_string().contains("upgrade kraken"), "got: {err}");
    }

    #[test]
    fn journal_without_an_initialization_cannot_anchor_an_epoch() {
        let session = SessionTimeline {
            events: Vec::new(),
            capture: None,
            manifest: stopped_manifest(dec!(10_000.0), dec!(10_000.0)),
            newer_records_skipped: 0,
        };

        let err = explain_pnl(&session).unwrap_err();
        assert!(matches!(err, ReplayError::NoEpoch { .. }));
        assert!(err.to_string().contains("initialization"), "got: {err}");
    }

    #[test]
    fn missing_market_coverage_marks_missed_fills_uncovered_with_reason() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        let order = builder.submit_limit(
            "2026-01-01T00:05:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.1),
            dec!(44_000.0),
        );
        builder.cancel("2026-01-01T00:30:00Z", Origin::Cli, &order.id);
        let session = builder.session(None, stopped_manifest(dec!(10_000.0), dec!(10_000.0)));
        let report = explain_pnl(&session).unwrap();

        let missed = &report.missed_fills.orders[0];
        assert_eq!(missed.coverage, Coverage::Uncovered);
        assert_eq!(missed.outcome, MissedOutcome::Cancelled);
        assert!(
            missed
                .uncovered_reason
                .as_deref()
                .unwrap()
                .contains("no market recording")
        );
        assert_eq!(missed.crossed, None);
    }

    #[test]
    fn missed_fill_uses_the_engine_crossing_predicate() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        let order = builder.submit_limit(
            "2026-01-01T00:05:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.1),
            dec!(44_000.0),
        );
        builder.ticker(
            "2026-01-01T00:10:00Z",
            "BTC/USD",
            dec!(44_400.0),
            dec!(44_500.0),
        ); // no cross
        builder.ticker(
            "2026-01-01T00:20:00Z",
            "BTC/USD",
            dec!(43_800.0),
            dec!(43_900.0),
        ); // ask ≤ 44_000
        builder.cancel("2026-01-01T00:30:00Z", Origin::Cli, &order.id);
        builder.ticker(
            "2026-01-01T00:40:00Z",
            "BTC/USD",
            dec!(45_000.0),
            dec!(45_100.0),
        ); // end mark
        let session = builder.session(
            Some(finalized_capture()),
            stopped_manifest(dec!(10_000.0), dec!(10_000.0)),
        );
        let report = explain_pnl(&session).unwrap();

        let missed = &report.missed_fills.orders[0];
        assert_eq!(missed.coverage, Coverage::Covered);
        assert_eq!(missed.crossed, Some(true));
        assert_eq!(
            missed.first_crossed_at,
            Some("2026-01-01T00:20:00Z".parse().unwrap())
        );
        // 0.1·(45_000 − 44_000) − 44_000·0.1·0.0026 = 100 − 11.44
        assert_close(
            missed.hypothetical_pnl.unwrap(),
            dec!(88.56),
            "hypothetical",
        );
    }

    #[test]
    fn cancelled_before_cross_reports_not_crossed() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        let order = builder.submit_limit(
            "2026-01-01T00:05:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.1),
            dec!(44_000.0),
        );
        builder.ticker(
            "2026-01-01T00:10:00Z",
            "BTC/USD",
            dec!(44_400.0),
            dec!(44_500.0),
        ); // no cross
        builder.cancel("2026-01-01T00:15:00Z", Origin::Cli, &order.id);
        builder.ticker(
            "2026-01-01T00:20:00Z",
            "BTC/USD",
            dec!(43_800.0),
            dec!(43_900.0),
        ); // after cancel
        let session = builder.session(
            Some(finalized_capture()),
            stopped_manifest(dec!(10_000.0), dec!(10_000.0)),
        );
        let report = explain_pnl(&session).unwrap();

        let missed = &report.missed_fills.orders[0];
        assert_eq!(missed.coverage, Coverage::Covered);
        assert_eq!(
            missed.crossed,
            Some(false),
            "the cross came only after cancellation"
        );
        assert!(missed.hypothetical_pnl.is_none());
    }

    #[test]
    fn still_open_order_created_after_the_recorded_window_is_uncovered() {
        // The recorder can stop (capture finalized, window_end stamped)
        // while the session keeps trading: a limit order submitted after
        // that rests entirely outside the recorded window, and its inverted
        // scan range must refuse coverage — never report "covered, no
        // cross" over a tape that ended before the order existed.
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        builder.submit_limit(
            "2026-01-01T01:30:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.1),
            dec!(44_000.0),
        );
        let session = builder.session(
            Some(finalized_capture()),
            stopped_manifest(dec!(10_000.0), dec!(10_000.0)),
        );
        let report = explain_pnl(&session).unwrap();

        let missed = &report.missed_fills.orders[0];
        assert_eq!(missed.outcome, MissedOutcome::StillOpen);
        assert_eq!(missed.coverage, Coverage::Uncovered);
        assert!(
            missed
                .uncovered_reason
                .as_deref()
                .unwrap()
                .contains("outside the recorded window")
        );
        assert_eq!(missed.crossed, None, "an unseen window must not answer");
    }

    #[test]
    fn reconnected_capture_marks_missed_fills_uncovered() {
        // A reconnect is an invisible gap (fresh snapshot, no replay of the
        // outage): the recording must refuse to answer, even though a
        // recorded frame does cross the limit.
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        let order = builder.submit_limit(
            "2026-01-01T00:05:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.1),
            dec!(44_000.0),
        );
        builder.ticker(
            "2026-01-01T00:20:00Z",
            "BTC/USD",
            dec!(43_800.0),
            dec!(43_900.0),
        );
        builder.cancel("2026-01-01T00:30:00Z", Origin::Cli, &order.id);
        let mut capture = finalized_capture();
        if let Some(summary) = &mut capture.summary {
            summary.reconnect_count = 1;
        }
        let session = builder.session(
            Some(capture),
            stopped_manifest(dec!(10_000.0), dec!(10_000.0)),
        );
        let report = explain_pnl(&session).unwrap();

        let missed = &report.missed_fills.orders[0];
        assert_eq!(missed.coverage, Coverage::Uncovered);
        assert!(
            missed
                .uncovered_reason
                .as_deref()
                .unwrap()
                .contains("reconnect")
        );
        assert_eq!(missed.crossed, None, "a gapped recording must not answer");
    }

    #[test]
    fn unparsed_frames_mark_missed_fills_uncovered() {
        // A frame the recorder could not parse never reached the tape: the
        // recording must refuse to answer, even though a recorded frame does
        // cross the limit.
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        let order = builder.submit_limit(
            "2026-01-01T00:05:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.1),
            dec!(44_000.0),
        );
        builder.ticker(
            "2026-01-01T00:20:00Z",
            "BTC/USD",
            dec!(43_800.0),
            dec!(43_900.0),
        );
        builder.cancel("2026-01-01T00:30:00Z", Origin::Cli, &order.id);
        let mut capture = finalized_capture();
        if let Some(summary) = &mut capture.summary {
            summary.frames_unparsed = 1;
        }
        let session = builder.session(
            Some(capture),
            stopped_manifest(dec!(10_000.0), dec!(10_000.0)),
        );
        let report = explain_pnl(&session).unwrap();

        let missed = &report.missed_fills.orders[0];
        assert_eq!(missed.coverage, Coverage::Uncovered);
        assert!(
            missed
                .uncovered_reason
                .as_deref()
                .unwrap()
                .contains("failed to parse")
        );
        assert_eq!(missed.crossed, None, "a holey recording must not answer");
    }

    #[test]
    fn multiple_epochs_decompose_only_the_last_and_disclose_skipped() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        builder.market_fill(
            "2026-01-01T00:05:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.1),
            (dec!(50_000.0), dec!(49_900.0)),
        );
        builder.account(
            "2026-01-01T00:10:00Z",
            Origin::Cli,
            AccountEvent::Reset(PaperConfig {
                balance: dec!(5_000.0),
                currency: "USD".to_string(),
                fee_rate: dec!(0.0026),
                slippage_rate: dec!(0.0),
            }),
        );
        builder.market_fill(
            "2026-01-01T00:20:00Z",
            Origin::Mcp,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.05),
            (dec!(50_000.0), dec!(49_900.0)),
        );
        builder.ticker(
            "2026-01-01T00:50:00Z",
            "BTC/USD",
            dec!(51_000.0),
            dec!(51_100.0),
        );
        let session = builder.session(None, stopped_manifest(dec!(5_040.0), dec!(5_000.0)));
        let report = explain_pnl(&session).unwrap();

        assert_eq!(report.epochs_skipped, 1);
        assert_eq!(report.trades.len(), 1, "only the final epoch's fill");
        assert_close(
            report.anchor.starting_balance,
            dec!(5_000.0),
            "anchor is the final epoch's balance, not the manifest's",
        );
        assert!(report.caveats.iter().any(|c| c.contains("skipped")));
    }

    #[test]
    fn attribution_splits_fees_by_submission_origin_for_limit_fills() {
        let mut builder = SessionBuilder::with_rates(dec!(100_000.0), dec!(0.0026), dec!(0.0));
        builder.submit_limit(
            "2026-01-01T00:05:00Z",
            Origin::Mcp,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.1),
            dec!(45_000.0),
        );
        // A CLI command reconciles the fill — the MCP submitter still owns it.
        builder.fill_pending(
            "2026-01-01T00:20:00Z",
            Origin::Cli,
            "BTCUSD",
            dec!(44_900.0),
            dec!(44_800.0),
        );
        builder.market_fill(
            "2026-01-01T00:25:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.1),
            (dec!(45_000.0), dec!(44_900.0)),
        );
        builder.ticker(
            "2026-01-01T00:50:00Z",
            "BTC/USD",
            dec!(45_500.0),
            dec!(45_600.0),
        );
        let session = builder.session(None, stopped_manifest(dec!(100_100.0), dec!(100_000.0)));
        let report = explain_pnl(&session).unwrap();

        assert_eq!(report.attribution.len(), 2);
        let by_origin = |origin: Origin| {
            report
                .attribution
                .iter()
                .find(|slice| slice.origin == origin)
                .unwrap()
        };
        assert_eq!(by_origin(Origin::Mcp).trades, 1, "the limit fill is MCP's");
        assert_eq!(by_origin(Origin::Cli).trades, 1, "the market fill is CLI's");
        assert!(by_origin(Origin::Mcp).fees < dec!(0.0));
    }

    #[test]
    fn decision_reasons_join_trades_by_order_id() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        let trade = builder.market_fill(
            "2026-01-01T00:05:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.1),
            (dec!(50_000.0), dec!(49_900.0)),
        );
        builder.decision("2026-01-01T00:05:00Z", &trade.order_id, "dip below 20d MA");
        builder.ticker(
            "2026-01-01T00:50:00Z",
            "BTC/USD",
            dec!(51_000.0),
            dec!(51_100.0),
        );
        let session = builder.session(None, stopped_manifest(dec!(10_100.0), dec!(10_000.0)));
        let report = explain_pnl(&session).unwrap();

        assert_eq!(report.trades[0].reason.as_deref(), Some("dip below 20d MA"));
        assert_eq!(report.attribution[0].decided, 1);
    }

    #[test]
    fn unmarked_base_asset_folds_into_residual_with_caveat() {
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        builder.market_fill(
            "2026-01-01T00:05:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "ETHUSD",
            dec!(1.0),
            (dec!(2_000.0), dec!(1_999.0)),
        );
        // Only BTC was recorded: ETH has no end mark.
        builder.ticker(
            "2026-01-01T00:50:00Z",
            "BTC/USD",
            dec!(51_000.0),
            dec!(51_100.0),
        );
        let session = builder.session(None, stopped_manifest(dec!(10_050.0), dec!(10_000.0)));
        let report = explain_pnl(&session).unwrap();

        assert!(report.trades[0].price_movement.is_none());
        assert!(
            amount(&report, ComponentKind::PriceMovement).is_none(),
            "not one fill's movement was derivable: unknown, never a fake zero"
        );
        assert!(
            report.caveats.iter().any(|c| c.contains("ETH")),
            "caveats: {:?}",
            report.caveats
        );
        // The identity still holds: residual absorbs the unmarked movement.
        let sum: Decimal = report
            .components
            .iter()
            .filter_map(|line| line.amount)
            .sum();
        assert_close(sum, report.anchor.total_pnl, "sum equals the anchor");
    }

    #[test]
    fn fees_line_counts_only_the_converted_fills() {
        // A fill whose quote asset has no end mark is excluded from the fee
        // sum; the sentence must count the same set the number describes.
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        builder.market_fill(
            "2026-01-01T00:05:00Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.01),
            (dec!(50_000.0), dec!(49_900.0)),
        );
        // Hand-built EUR-quoted fill: EUR has no recorded end mark below.
        let state = PaperState::new(dec!(10_000.0), "EUR");
        let mut trade = state
            .decide_market_order(
                OrderSide::Buy,
                "BTCEUR",
                dec!(0.01),
                dec!(46_000.0),
                dec!(45_900.0),
            )
            .unwrap();
        trade.filled_at = "2026-01-01T00:06:00Z".parse().unwrap();
        builder.account(
            "2026-01-01T00:06:00Z",
            Origin::Cli,
            AccountEvent::OrderFilled { trade },
        );
        builder.ticker(
            "2026-01-01T00:50:00Z",
            "BTC/USD",
            dec!(51_000.0),
            dec!(51_100.0),
        );
        let session = builder.session(None, stopped_manifest(dec!(10_050.0), dec!(10_000.0)));
        let report = explain_pnl(&session).unwrap();

        let fees = report
            .components
            .iter()
            .find(|line| line.kind == ComponentKind::Fees)
            .unwrap();
        assert!(
            fees.explanation.contains("across 1 fill"),
            "2 fills on record, 1 converted: {}",
            fees.explanation
        );
        assert!(report.trades[1].notional.is_none(), "EUR fill unconverted");
    }

    #[test]
    fn attribution_notional_is_converted_to_the_anchor_currency() {
        // A BTC/EUR fill on a USD account: cost is EUR, and both the trade
        // line and the origin slice must carry it multiplied by the EUR/USD
        // end mark, never the raw quote-currency figure.
        let mut builder = SessionBuilder::with_rates(dec!(10_000.0), dec!(0.0026), dec!(0.0));
        let state = PaperState::new(dec!(100_000.0), "EUR");
        let mut trade = state
            .decide_market_order(
                OrderSide::Buy,
                "BTCEUR",
                dec!(1.0),
                dec!(45_000.0),
                dec!(44_900.0),
            )
            .unwrap();
        trade.filled_at = "2026-01-01T00:05:00Z".parse().unwrap();
        builder.account(
            "2026-01-01T00:05:00Z",
            Origin::Mcp,
            AccountEvent::OrderFilled { trade },
        );
        builder.ticker("2026-01-01T00:50:00Z", "EUR/USD", dec!(1.10), dec!(1.11));
        builder.ticker(
            "2026-01-01T00:51:00Z",
            "BTC/USD",
            dec!(51_000.0),
            dec!(51_100.0),
        );
        let session = builder.session(None, stopped_manifest(dec!(10_050.0), dec!(10_000.0)));
        let report = explain_pnl(&session).unwrap();

        assert_close(
            report.trades[0].notional.unwrap(),
            dec!(45_000.0) * dec!(1.10),
            "trade notional in USD",
        );
        assert_close(
            report.attribution[0].gross_notional,
            dec!(45_000.0) * dec!(1.10),
            "slice notional in USD",
        );
    }

    mod properties {
        use proptest::prelude::*;
        use rust_decimal::prelude::FromPrimitive;

        use super::*;

        #[derive(Debug, Clone)]
        struct Op {
            buy: bool,
            limit: bool,
            volume: Decimal,
            price: Decimal,
            spread_fraction: Decimal,
        }

        /// Fuzzed f64 ranges, snapped to `Decimal` at the boundary — the ops
        /// themselves are exact so the engine math stays float-free.
        fn op() -> impl Strategy<Value = Op> {
            (
                any::<bool>(),
                any::<bool>(),
                0.001..0.5_f64,
                1_000.0..100_000.0_f64,
                0.0001..0.01_f64,
            )
                .prop_map(|(buy, limit, volume, price, spread_fraction)| Op {
                    buy,
                    limit,
                    volume: Decimal::from_f64(volume).unwrap(),
                    price: Decimal::from_f64(price).unwrap(),
                    spread_fraction: Decimal::from_f64(spread_fraction).unwrap(),
                })
        }

        /// Drive the real engine through a random op sequence; infeasible
        /// ops (insufficient balance) are skipped, exactly as `decide_*`
        /// rejects them live.
        fn random_session(
            ops: &[Op],
            slippage_rate: Decimal,
        ) -> (SessionBuilder, Decimal, (Decimal, Decimal)) {
            let mut builder =
                SessionBuilder::with_rates(dec!(1_000_000.0), dec!(0.0026), slippage_rate);
            let mut gross = dec!(0.0);
            let mut last_quote = (dec!(50_000.0), dec!(49_900.0));
            for (index, op) in ops.iter().enumerate() {
                let ts = format!("2026-01-01T00:{:02}:{:02}Z", 10 + index / 60, index % 60);
                let side = if op.buy {
                    OrderSide::Buy
                } else {
                    OrderSide::Sell
                };
                let bid = op.price;
                let ask = bid * (dec!(1.0) + op.spread_fraction);
                last_quote = (ask, bid);
                if op.limit {
                    // A marketable limit: rests, then fills at its own price
                    // on the next reconcile pass against the same quote.
                    let limit_price = if op.buy { ask } else { bid };
                    if builder
                        .state
                        .decide_limit_order(side, "BTCUSD", op.volume, limit_price)
                        .is_ok()
                    {
                        builder.submit_limit(
                            &ts,
                            Origin::Cli,
                            side,
                            "BTCUSD",
                            op.volume,
                            limit_price,
                        );
                        builder.fill_pending(&ts, Origin::Mcp, "BTCUSD", ask, bid);
                        gross += op.volume * limit_price;
                    }
                } else if builder
                    .state
                    .decide_market_order(side, "BTCUSD", op.volume, ask, bid)
                    .is_ok()
                {
                    let trade = builder.market_fill(
                        &ts,
                        Origin::Cli,
                        side,
                        "BTCUSD",
                        op.volume,
                        (ask, bid),
                    );
                    gross += trade.cost;
                }
            }
            builder.ticker(
                "2026-01-01T00:59:59Z",
                "BTC/USD",
                last_quote.1,
                last_quote.0,
            );
            (builder, gross, last_quote)
        }

        proptest! {
            /// The additive identity holds for any anchor. The sum matching
            /// is definitional (the residual is the remainder), so this pins
            /// only that every component exists and sums cleanly over the
            /// fuzzed op space; a *wrong* component is caught by the sibling
            /// recorded-marks test, where the residual must collapse to dust.
            #[test]
            fn identity_holds_for_random_fill_sequences(
                ops in prop::collection::vec(op(), 0..30),
                slippage_rate in 0.0..0.005_f64,
                final_value in 500_000.0..1_500_000.0_f64,
            ) {
                let slippage_rate = Decimal::from_f64(slippage_rate).unwrap();
                let final_value = Decimal::from_f64(final_value).unwrap();
                let (builder, gross, _) = random_session(&ops, slippage_rate);
                let session =
                    builder.session(None, stopped_manifest(final_value, dec!(1_000_000.0)));
                let report = explain_pnl(&session).unwrap();

                let mut sum = dec!(0.0);
                for line in &report.components {
                    let amount = line.amount.expect("fully marked session");
                    sum += amount;
                }
                let tolerance = dec!(0.000001) * gross.max(dec!(1));
                prop_assert!(
                    (sum - report.anchor.total_pnl).abs() <= tolerance,
                    "sum {sum} vs total {}",
                    report.anchor.total_pnl
                );
            }

            /// The ticket's bounded-residual acceptance: when the anchor's
            /// final value is computed from the same recorded end marks the
            /// decomposition uses, the residual collapses to at most
            /// 28-digit rounding dust — any large real-world residual is the
            /// live-vs-recorded gap, never the math.
            #[test]
            fn residual_is_bounded_when_final_value_comes_from_recorded_marks(
                ops in prop::collection::vec(op(), 1..30),
                slippage_rate in 0.0..0.005_f64,
            ) {
                let slippage_rate = Decimal::from_f64(slippage_rate).unwrap();
                let (builder, gross, last_quote) = random_session(&ops, slippage_rate);
                let prices = HashMap::from([("BTCUSD".to_string(), last_quote)]);
                let (final_value, complete) = builder.state.compute_portfolio_value(&prices);
                prop_assume!(complete);
                let session =
                    builder.session(None, stopped_manifest(final_value, dec!(1_000_000.0)));
                let report = explain_pnl(&session).unwrap();

                let residual = report
                    .components
                    .iter()
                    .find(|line| line.kind == ComponentKind::Residual)
                    .and_then(|line| line.amount)
                    .expect("residual present");
                let tolerance = dec!(0.000001) * gross.max(dec!(1));
                prop_assert!(
                    residual.abs() <= tolerance,
                    "residual {residual} exceeds {tolerance} (gross {gross})"
                );
            }
        }
    }
}

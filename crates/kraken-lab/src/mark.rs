//! Recorded-market marks for deterministic replay.
//!
//! Tickers use mid, trades use print price, and OHLC uses close. Book deltas
//! cannot yield top-of-book without reconstruction, so book-only assets remain
//! unmarked. Mid avoids charging spread again after fills already paid it.

use kraken_core::{ChannelData, ChannelMessage};
use kraken_paper::{canonical_key, valid_quote};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::collections::{HashMap, HashSet, VecDeque, hash_map::Entry};

/// The smallest odd window that prevents one observation from becoming a
/// majority.
const MEDIAN_WINDOW: usize = 3;

/// Rolling median marks per canonical `(base, quote)`.
///
/// The left fold remains prefix-stable. Before the third observation, one
/// sample is used directly and two are averaged.
#[derive(Default)]
pub struct Table {
    marks: HashMap<(String, String), SymbolMark>,
    /// Warn-once memory for symbols `parse_pair` cannot split.
    unparseable: HashSet<String>,
}

impl Table {
    /// Absorb one recorded frame, entry by entry in wire order; `true` when
    /// any mark moved (the replay's re-value signal).
    pub fn observe(&mut self, frame: &ChannelMessage) -> bool {
        let mut changed = false;
        match &frame.body {
            ChannelData::Ticker(entries) => {
                for ticker in entries {
                    if valid_quote(ticker.ask, ticker.bid) {
                        let mid = (ticker.ask + ticker.bid) / dec!(2);
                        changed |= self.accept(&ticker.symbol, Channel::Ticker, mid);
                    }
                }
            }
            ChannelData::Trade(entries) => {
                for trade in entries {
                    if trade.price > Decimal::ZERO {
                        changed |= self.accept(&trade.symbol, Channel::Trade, trade.price);
                    }
                }
            }
            ChannelData::Ohlc(entries) => {
                for candle in entries {
                    if candle.close > Decimal::ZERO {
                        changed |= self.accept(&candle.symbol, Channel::Ohlc, candle.close);
                    }
                }
            }
            // Incremental book deltas cannot produce a trustworthy top-of-book mark.
            ChannelData::Book(_)
            | ChannelData::Instrument(_)
            | ChannelData::Executions(_)
            | ChannelData::Balances(_)
            | ChannelData::Level3(_)
            | ChannelData::Status(_) => {}
        }
        changed
    }

    /// Adapts marks to [`kraken_paper::PaperState::valuation`] without cross
    /// rates.
    pub fn to_prices(&self, currency: &str) -> HashMap<String, (Decimal, Decimal)> {
        self.marks
            .iter()
            .filter(|((_, quote), _)| quote == currency)
            .map(|((base, _), symbol)| {
                let mark = symbol.mark();
                (format!("{base}{currency}"), (mark, mark))
            })
            .collect()
    }

    fn accept(&mut self, symbol: &str, channel: Channel, observation: Decimal) -> bool {
        let Some(key) = self.canonical(symbol) else {
            return false;
        };
        match self.marks.entry(key) {
            Entry::Occupied(mut entry) => entry.get_mut().observe(channel, observation),
            Entry::Vacant(entry) => {
                entry.insert(SymbolMark::first(channel, observation));
                true
            }
        }
    }

    fn canonical(&mut self, symbol: &str) -> Option<(String, String)> {
        match canonical_key(symbol) {
            Some(key) => Some(key),
            None => {
                if self.unparseable.insert(symbol.to_string()) {
                    tracing::warn!(symbol, "skipping market frames for unparseable symbol");
                }
                None
            }
        }
    }
}

/// Mark-bearing channels ordered by trust.
///
/// Declaration order is load-bearing: mixing lower-priority prints with
/// higher-priority mids would corrupt the rolling distribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Channel {
    Ohlc,
    Trade,
    Ticker,
}

/// One symbol's elected channel and median window. Holds at least one
/// observation from construction on.
struct SymbolMark {
    channel: Channel,
    window: VecDeque<Decimal>,
}

impl SymbolMark {
    fn first(channel: Channel, observation: Decimal) -> Self {
        let mut window = VecDeque::with_capacity(MEDIAN_WINDOW);
        window.push_back(observation);
        Self { channel, window }
    }

    /// Preserves the window across channel upgrades to avoid restarting
    /// warm-up.
    fn observe(&mut self, channel: Channel, observation: Decimal) -> bool {
        if channel < self.channel {
            return false;
        }
        self.channel = channel;
        let before = self.mark();
        if self.window.len() == MEDIAN_WINDOW {
            self.window.pop_front();
        }
        self.window.push_back(observation);
        self.mark() != before
    }

    /// The window's median; the two-observation warm-up averages.
    fn mark(&self) -> Decimal {
        let mut sorted: Vec<Decimal> = self.window.iter().copied().collect();
        sorted.sort_unstable();
        let mid = sorted.len() / 2;
        if sorted.len().is_multiple_of(2) {
            (sorted[mid - 1] + sorted[mid]) / dec!(2)
        } else {
            sorted[mid]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticker(symbol: &str, bid: Decimal, ask: Decimal) -> ChannelMessage {
        ChannelMessage::parse(&format!(
            r#"{{"channel":"ticker","type":"update","data":[{{"symbol":"{symbol}",
               "bid":{bid},"bid_qty":1.0,"ask":{ask},"ask_qty":1.0,"last":{bid},
               "volume":10.0,"vwap":{bid},"low":{bid},"high":{ask},"change":0.0,
               "change_pct":0.0,"timestamp":"2026-01-01T00:00:00Z"}}]}}"#
        ))
        .unwrap()
    }

    fn trades(symbol: &str, prices: &[Decimal]) -> ChannelMessage {
        let entries: Vec<String> = prices
            .iter()
            .enumerate()
            .map(|(i, price)| {
                format!(
                    r#"{{"symbol":"{symbol}","side":"buy","price":{price},"qty":1.0,
                       "ord_type":"market","trade_id":{i},"timestamp":"2026-01-01T00:00:00Z"}}"#
                )
            })
            .collect();
        ChannelMessage::parse(&format!(
            r#"{{"channel":"trade","type":"update","data":[{}]}}"#,
            entries.join(",")
        ))
        .unwrap()
    }

    fn ohlc(symbol: &str, close: Decimal) -> ChannelMessage {
        ChannelMessage::parse(&format!(
            r#"{{"channel":"ohlc","type":"update","data":[{{"symbol":"{symbol}","open":{close},
               "high":{close},"low":{close},"close":{close},"vwap":{close},"trades":1,
               "volume":1.0,"interval_begin":"2026-01-01T00:00:00Z","interval":1}}]}}"#
        ))
        .unwrap()
    }

    fn book(symbol: &str) -> ChannelMessage {
        ChannelMessage::parse(&format!(
            r#"{{"channel":"book","type":"snapshot","data":[{{"symbol":"{symbol}",
               "bids":[{{"price":50000.0,"qty":1.0}}],"asks":[{{"price":50010.0,"qty":1.0}}],
               "checksum":1,"timestamp":"2026-01-01T00:00:00Z"}}]}}"#
        ))
        .unwrap()
    }

    fn mark_of(table: &Table, base: &str, currency: &str) -> Option<Decimal> {
        table
            .to_prices(currency)
            .get(&format!("{base}{currency}"))
            .map(|&(_, bid)| bid)
    }

    #[test]
    fn ticker_marks_at_mid_not_bid() {
        let mut table = Table::default();
        table.observe(&ticker("BTC/USD", dec!(50_000.0), dec!(50_100.0)));
        assert_eq!(mark_of(&table, "BTC", "USD"), Some(dec!(50_050)));
    }

    #[test]
    fn one_outlier_after_warmup_never_becomes_the_mark() {
        let mut table = Table::default();
        table.observe(&trades(
            "BTC/USD",
            &[dec!(60_000.0), dec!(60_000.0), dec!(60_000.0)],
        ));
        assert!(
            !table.observe(&trades("BTC/USD", &[dec!(6.0)])),
            "the $6 tick must not move the median"
        );
        assert_eq!(mark_of(&table, "BTC", "USD"), Some(dec!(60_000)));
    }

    #[test]
    fn bad_first_frame_self_heals_by_the_third_observation() {
        // Warm-up contamination is documented, not hidden: the bad first
        // observation *is* the mark, the second averages it, the third
        // restores the median guarantee.
        let mut table = Table::default();
        table.observe(&trades("BTC/USD", &[dec!(6.0)]));
        assert_eq!(mark_of(&table, "BTC", "USD"), Some(dec!(6)));
        table.observe(&trades("BTC/USD", &[dec!(60_000.0)]));
        assert_eq!(mark_of(&table, "BTC", "USD"), Some(dec!(30_003)));
        table.observe(&trades("BTC/USD", &[dec!(60_000.0)]));
        assert_eq!(mark_of(&table, "BTC", "USD"), Some(dec!(60_000)));
    }

    #[test]
    fn genuine_jump_is_adopted_after_one_confirming_observation() {
        let mut table = Table::default();
        table.observe(&trades(
            "BTC/USD",
            &[dec!(50_000.0), dec!(50_000.0), dec!(50_000.0)],
        ));
        table.observe(&trades("BTC/USD", &[dec!(55_000.0)]));
        assert_eq!(mark_of(&table, "BTC", "USD"), Some(dec!(50_000)));
        table.observe(&trades("BTC/USD", &[dec!(55_000.0)]));
        assert_eq!(mark_of(&table, "BTC", "USD"), Some(dec!(55_000)));
    }

    #[test]
    fn trade_prints_stop_marking_once_ticker_arrives() {
        let mut table = Table::default();
        table.observe(&trades("BTC/USD", &[dec!(50_000.0)]));
        table.observe(&ticker("BTC/USD", dec!(51_000.0), dec!(51_100.0)));
        assert!(
            !table.observe(&trades(
                "BTC/USD",
                &[dec!(1_000.0), dec!(1_000.0), dec!(1_000.0)]
            )),
            "trade prints are below the ticker latch"
        );
        // Window kept across the upgrade: trade 50_000 + ticker mid 51_050.
        assert_eq!(mark_of(&table, "BTC", "USD"), Some(dec!(50_525)));
    }

    #[test]
    fn ohlc_close_marks_a_symbol_with_no_ticker_or_trade() {
        let mut table = Table::default();
        table.observe(&ohlc("SOL/USD", dec!(150.0)));
        assert_eq!(mark_of(&table, "SOL", "USD"), Some(dec!(150)));
    }

    #[test]
    fn book_frames_derive_no_marks() {
        let mut table = Table::default();
        assert!(!table.observe(&book("BTC/USD")));
        assert!(table.to_prices("USD").is_empty());
    }

    #[test]
    fn multi_entry_frame_feeds_the_window_per_entry_in_order() {
        // Three goods then one outlier inside a single frame: the window has
        // warmed up within the same vec, so the outlier is already absorbed.
        let mut table = Table::default();
        table.observe(&trades(
            "BTC/USD",
            &[dec!(60_000.0), dec!(60_000.0), dec!(60_000.0), dec!(6.0)],
        ));
        assert_eq!(mark_of(&table, "BTC", "USD"), Some(dec!(60_000)));
    }

    #[test]
    fn nonpositive_and_invalid_observations_are_rejected() {
        let mut table = Table::default();
        assert!(!table.observe(&trades("BTC/USD", &[dec!(0.0)])));
        assert!(!table.observe(&trades("BTC/USD", &[-dec!(1.0)])));
        assert!(!table.observe(&ticker("BTC/USD", dec!(0.0), dec!(50_000.0))));
        assert!(table.to_prices("USD").is_empty());
    }

    #[test]
    fn to_prices_filters_to_the_requested_quote_currency() {
        let mut table = Table::default();
        table.observe(&trades("ETH/BTC", &[dec!(0.05)]));
        table.observe(&trades("BTC/USD", &[dec!(60_000.0)]));
        let usd = table.to_prices("USD");
        assert_eq!(usd.len(), 1, "the ETH/BTC cross must not leak into USD");
        assert!(usd.contains_key("BTCUSD"));
    }

    #[test]
    fn xbt_and_btc_symbols_collide_on_one_canonical_mark() {
        let mut table = Table::default();
        table.observe(&trades("XBT/USD", &[dec!(60_000.0)]));
        table.observe(&trades("BTC/USD", &[dec!(61_000.0)]));
        assert_eq!(table.to_prices("USD").len(), 1);
        // The warm-up mean proves both observations share one window — a
        // silently dropped XBT frame would mark 61_000 instead.
        assert_eq!(mark_of(&table, "BTC", "USD"), Some(dec!(60_500)));
    }

    #[test]
    fn trade_print_outranks_a_prior_ohlc_close() {
        let mut table = Table::default();
        table.observe(&ohlc("SOL/USD", dec!(150.0)));
        assert!(
            table.observe(&trades("SOL/USD", &[dec!(155.0)])),
            "a trade print upgrades the latch"
        );
        assert!(
            !table.observe(&ohlc("SOL/USD", dec!(140.0))),
            "ohlc closes are below the trade latch"
        );
        // Window kept across the upgrade: ohlc 150 + trade 155.
        assert_eq!(mark_of(&table, "SOL", "USD"), Some(dec!(152.5)));
    }
}

//! `-o table` lines for the streaming channel payloads.

use super::{Summarize, SummaryLine};
use kraken_core::subscribe::message::{
    BalanceData, BookData, ChannelData, ExecutionData, InstrumentData, Level3Data, OhlcData,
    StatusData, TickerData, TradeData,
};

impl Summarize for ChannelData {
    /// The table view's `data` column: each entry's `key:value` line, joined by
    /// `, `.
    fn summary(&self) -> String {
        match self {
            Self::Ticker(data) => data.summary(),
            Self::Trade(data) => data.summary(),
            Self::Book(data) => data.summary(),
            Self::Ohlc(data) => data.summary(),
            Self::Instrument(data) => data.summary(),
            Self::Executions(data) => data.summary(),
            Self::Balances(data) => data.summary(),
            Self::Level3(data) => data.summary(),
            Self::Status(data) => data.summary(),
        }
    }
}

impl Summarize for TickerData {
    fn summary(&self) -> String {
        SummaryLine::default()
            .field("symbol", &self.symbol)
            .field("last", self.last)
            .field("bid", self.bid)
            .field("ask", self.ask)
            .field("change_pct", self.change_pct)
            .build()
    }
}

impl Summarize for TradeData {
    fn summary(&self) -> String {
        SummaryLine::default()
            .field("symbol", &self.symbol)
            .field("side", self.side)
            .field("qty", self.qty)
            .field("price", self.price)
            .field("ord_type", self.ord_type)
            .build()
    }
}

impl Summarize for BookData {
    fn summary(&self) -> String {
        // Top-of-book price each side (a side can be empty in an update) plus the level
        // count carried in this frame.
        SummaryLine::default()
            .field("symbol", &self.symbol)
            .opt("bid", self.bids.first().map(|l| l.price))
            .opt("ask", self.asks.first().map(|l| l.price))
            .field("bids", self.bids.len())
            .field("asks", self.asks.len())
            .build()
    }
}

impl Summarize for OhlcData {
    fn summary(&self) -> String {
        SummaryLine::default()
            .field("symbol", &self.symbol)
            .field("interval", self.interval)
            .field("open", self.open)
            .field("high", self.high)
            .field("low", self.low)
            .field("close", self.close)
            .field("volume", self.volume)
            .build()
    }
}

impl Summarize for InstrumentData {
    fn summary(&self) -> String {
        SummaryLine::default()
            .field("assets", self.assets.len())
            .field("pairs", self.pairs.len())
            .build()
    }
}

impl Summarize for ExecutionData {
    fn summary(&self) -> String {
        SummaryLine::default()
            .field("exec_type", self.exec_type)
            .field("order_id", &self.order_id)
            .opt("symbol", self.symbol.as_ref())
            .opt("side", self.side)
            .opt("order_status", self.order_status)
            .opt("last_qty", self.last_qty)
            .opt("last_price", self.last_price)
            .build()
    }
}

impl Summarize for BalanceData {
    fn summary(&self) -> String {
        SummaryLine::default()
            .field("asset", &self.asset)
            .field("balance", self.balance)
            .opt("amount", self.amount)
            .opt("ledger_type", self.event_type.as_ref())
            .build()
    }
}

impl Summarize for Level3Data {
    fn summary(&self) -> String {
        SummaryLine::default()
            .field("symbol", &self.symbol)
            .field("checksum", self.checksum)
            .field("bids", self.bids.len())
            .field("asks", self.asks.len())
            .build()
    }
}

impl Summarize for StatusData {
    fn summary(&self) -> String {
        SummaryLine::default()
            .field("system", self.system)
            .field("api_version", &self.api_version)
            .field("version", &self.version)
            .build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kraken_core::subscribe::message::PriceLevel;
    use rust_decimal::Decimal;

    #[test]
    fn summary_omits_the_price_of_an_empty_side() {
        let book = BookData {
            symbol: "BTC/USD".into(),
            bids: vec![PriceLevel {
                price: Decimal::from(50_000),
                qty: Decimal::ONE,
            }],
            asks: vec![],
            checksum: 0,
            timestamp: "TS".into(),
        };
        assert_eq!(book.summary(), "symbol:BTC/USD bid:50000 bids:1 asks:0");
    }
}

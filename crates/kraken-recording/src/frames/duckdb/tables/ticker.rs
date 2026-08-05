//! The `ticker` table: one row per ticker entry.

use kraken_core::ChannelData;
use kraken_core::subscribe::message::{MessageType, TickerData};

use crate::error::Result;
use crate::frames::duckdb::table::{
    Cell, ColType, Column, DriftPolicy, Recordable, Row, Stored, TableSpec, read_decimal, read_rows,
};
use crate::frames::duckdb::tables::{RowEntry, group_frames, message_type};

static TICKER: TableSpec = TableSpec::new(
    "ticker",
    &[
        Column::req("symbol", ColType::Text),
        Column::req("bid", ColType::Decimal),
        Column::req("bid_qty", ColType::Decimal),
        Column::req("ask", ColType::Decimal),
        Column::req("ask_qty", ColType::Decimal),
        Column::req("last", ColType::Decimal),
        Column::req("volume", ColType::Decimal),
        Column::req("vwap", ColType::Decimal),
        Column::req("low", ColType::Decimal),
        Column::req("high", ColType::Decimal),
        Column::req("change", ColType::Decimal),
        Column::req("change_pct", ColType::Decimal),
        Column::req("event_type", ColType::Text),
        Column::req("event_ts", ColType::Text),
        Column::req("recv_ts", ColType::Text),
        Column::req("seq", ColType::Big),
    ],
);

impl Recordable for TickerData {
    fn table() -> &'static TableSpec {
        &TICKER
    }

    fn rows(entries: &[Self], message_type: MessageType, seq: i64, recv_ts: &str) -> Vec<Row> {
        let event_type = message_type.to_string();
        entries
            .iter()
            .map(|t| {
                vec![
                    Cell::Text(t.symbol.clone()),
                    Cell::Decimal(t.bid),
                    Cell::Decimal(t.bid_qty),
                    Cell::Decimal(t.ask),
                    Cell::Decimal(t.ask_qty),
                    Cell::Decimal(t.last),
                    Cell::Decimal(t.volume),
                    Cell::Decimal(t.vwap),
                    Cell::Decimal(t.low),
                    Cell::Decimal(t.high),
                    Cell::Decimal(t.change),
                    Cell::Decimal(t.change_pct),
                    Cell::Text(event_type.clone()),
                    Cell::Text(t.timestamp.clone()),
                    Cell::Text(recv_ts.to_string()),
                    Cell::Big(seq),
                ]
            })
            .collect()
    }

    fn frames(conn: &duckdb::Connection, drift: DriftPolicy) -> Result<Vec<Stored>> {
        let rows = read_rows(conn, &TICKER, drift, |row| {
            Ok(RowEntry {
                entry: TickerData {
                    symbol: row.get("symbol")?,
                    bid: read_decimal(row, "bid")?,
                    bid_qty: read_decimal(row, "bid_qty")?,
                    ask: read_decimal(row, "ask")?,
                    ask_qty: read_decimal(row, "ask_qty")?,
                    last: read_decimal(row, "last")?,
                    volume: read_decimal(row, "volume")?,
                    vwap: read_decimal(row, "vwap")?,
                    low: read_decimal(row, "low")?,
                    high: read_decimal(row, "high")?,
                    change: read_decimal(row, "change")?,
                    change_pct: read_decimal(row, "change_pct")?,
                    timestamp: row.get("event_ts")?,
                },
                message_type: message_type(row)?,
                seq: row.get("seq")?,
            })
        })?;
        Ok(group_frames(ChannelData::Ticker, rows))
    }
}

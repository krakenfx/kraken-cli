//! The `ohlc` table: one row per candle.

use kraken_core::ChannelData;
use kraken_core::subscribe::message::{MessageType, OhlcData};

use crate::error::Result;
use crate::frames::duckdb::table::{
    Cell, ColType, Column, DriftPolicy, Recordable, Row, Stored, TableSpec, read_decimal, read_rows,
};
use crate::frames::duckdb::tables::{RowEntry, group_frames, message_type};

static OHLC: TableSpec = TableSpec::new(
    "ohlc",
    &[
        Column::req("symbol", ColType::Text),
        Column::req("open", ColType::Decimal),
        Column::req("high", ColType::Decimal),
        Column::req("low", ColType::Decimal),
        Column::req("close", ColType::Decimal),
        Column::req("vwap", ColType::Decimal),
        Column::req("volume", ColType::Decimal),
        Column::req("trades", ColType::UBig),
        Column::req("interval", ColType::UInt),
        Column::req("interval_begin", ColType::Text),
        // The deprecated wire `timestamp`, verbatim: reconstructing it from
        // `event_ts` would erase whether the wire sent it, breaking the
        // byte-identical round-trip when it equals `interval_begin`.
        Column::opt("timestamp", ColType::Text),
        Column::req("event_type", ColType::Text),
        Column::req("event_ts", ColType::Text),
        Column::req("recv_ts", ColType::Text),
        Column::req("seq", ColType::Big),
    ],
);

impl Recordable for OhlcData {
    fn table() -> &'static TableSpec {
        &OHLC
    }

    fn rows(entries: &[Self], message_type: MessageType, seq: i64, recv_ts: &str) -> Vec<Row> {
        let event_type = message_type.to_string();
        entries
            .iter()
            .map(|c| {
                vec![
                    Cell::Text(c.symbol.clone()),
                    Cell::Decimal(c.open),
                    Cell::Decimal(c.high),
                    Cell::Decimal(c.low),
                    Cell::Decimal(c.close),
                    Cell::Decimal(c.vwap),
                    Cell::Decimal(c.volume),
                    Cell::UBig(c.trades),
                    Cell::UInt(c.interval),
                    Cell::Text(c.interval_begin.clone()),
                    c.timestamp.clone().map_or(Cell::Null, Cell::Text),
                    Cell::Text(event_type.clone()),
                    Cell::Text(c.event_ts().to_string()),
                    Cell::Text(recv_ts.to_string()),
                    Cell::Big(seq),
                ]
            })
            .collect()
    }

    fn frames(conn: &duckdb::Connection, drift: DriftPolicy) -> Result<Vec<Stored>> {
        let rows = read_rows(conn, &OHLC, drift, |row| {
            Ok(RowEntry {
                entry: OhlcData {
                    symbol: row.get("symbol")?,
                    open: read_decimal(row, "open")?,
                    high: read_decimal(row, "high")?,
                    low: read_decimal(row, "low")?,
                    close: read_decimal(row, "close")?,
                    vwap: read_decimal(row, "vwap")?,
                    trades: row.get("trades")?,
                    volume: read_decimal(row, "volume")?,
                    timestamp: row.get("timestamp")?,
                    interval_begin: row.get("interval_begin")?,
                    interval: row.get("interval")?,
                },
                message_type: message_type(row)?,
                seq: row.get("seq")?,
            })
        })?;
        Ok(group_frames(ChannelData::Ohlc, rows))
    }
}

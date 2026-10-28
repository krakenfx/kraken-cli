//! The `trades` table: one row per execution.

use crate::{
    error::Result,
    frames::duckdb::{
        table::{
            Cell, ColType, Column, DriftPolicy, Recordable, Row, RowError, Stored, TableSpec,
            read_decimal, read_rows,
        },
        tables::{RowEntry, group_frames, message_type},
    },
};
use kraken_core::{
    ChannelData,
    subscribe::message::{MessageType, TradeData},
};

static TRADES: TableSpec = TableSpec::new(
    "trades",
    &[
        Column::req("symbol", ColType::Text),
        Column::req("side", ColType::Text),
        Column::req("price", ColType::Decimal),
        Column::req("qty", ColType::Decimal),
        Column::req("ord_type", ColType::Text),
        Column::req("trade_id", ColType::UBig),
        Column::req("event_type", ColType::Text),
        Column::req("event_ts", ColType::Text),
        Column::req("recv_ts", ColType::Text),
        Column::req("seq", ColType::Big),
    ],
);

impl Recordable for TradeData {
    fn table() -> &'static TableSpec {
        &TRADES
    }

    fn rows(entries: &[Self], message_type: MessageType, seq: i64, recv_ts: &str) -> Vec<Row> {
        let event_type = message_type.to_string();
        entries
            .iter()
            .map(|t| {
                vec![
                    Cell::Text(t.symbol.clone()),
                    Cell::Text(t.side.to_string()),
                    Cell::Decimal(t.price),
                    Cell::Decimal(t.qty),
                    Cell::Text(t.ord_type.to_string()),
                    Cell::UBig(t.trade_id),
                    Cell::Text(event_type.clone()),
                    Cell::Text(t.timestamp.clone()),
                    Cell::Text(recv_ts.to_string()),
                    Cell::Big(seq),
                ]
            })
            .collect()
    }

    /// `side`/`ord_type` parse back through the same `strum` vocabulary that
    /// rendered them, so write and read can't disagree.
    fn frames(conn: &duckdb::Connection, drift: DriftPolicy) -> Result<Vec<Stored>> {
        let rows = read_rows(conn, &TRADES, drift, |row| {
            let side: String = row.get("side")?;
            let ord_type: String = row.get("ord_type")?;
            Ok(RowEntry {
                entry: TradeData {
                    symbol: row.get("symbol")?,
                    side: side.parse().map_err(|_| {
                        RowError::Vocabulary(format!("unrecognized trade side '{side}'"))
                    })?,
                    price: read_decimal(row, "price")?,
                    qty: read_decimal(row, "qty")?,
                    ord_type: ord_type.parse().map_err(|_| {
                        RowError::Vocabulary(format!("unrecognized ord_type '{ord_type}'"))
                    })?,
                    trade_id: row.get("trade_id")?,
                    timestamp: row.get("event_ts")?,
                },
                message_type: message_type(row)?,
                seq: row.get("seq")?,
            })
        })?;
        Ok(group_frames(ChannelData::Trade, rows))
    }
}

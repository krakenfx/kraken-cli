//! The `book` table: one row per price level, regrouped into frames on read.

use kraken_core::ChannelData;
use kraken_core::subscribe::message::{BookData, ChannelMessage, MessageType, PriceLevel};
use strum::{Display, EnumString};

use crate::error::Result;
use crate::frames::duckdb::table::{
    Cell, ColType, Column, DriftPolicy, Recordable, Row, RowError, Stored, TableSpec, read_decimal,
    read_rows,
};
use crate::frames::duckdb::tables::message_type;

/// Which side of the book a level row belongs to. `Display`/`FromStr` share one
/// lowercase vocabulary, so the `side` column parses back exactly as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Display, EnumString)]
#[strum(serialize_all = "lowercase")]
enum LevelSide {
    Bid,
    Ask,
}

static BOOK: TableSpec = TableSpec::new(
    "book",
    &[
        Column::req("symbol", ColType::Text),
        Column::req("seq", ColType::Big),
        Column::req("side", ColType::Text),
        Column::req("price", ColType::Decimal),
        Column::req("qty", ColType::Decimal),
        Column::req("event_type", ColType::Text),
        Column::req("checksum", ColType::UInt),
        Column::req("event_ts", ColType::Text),
        Column::req("recv_ts", ColType::Text),
    ],
);

impl Recordable for BookData {
    fn table() -> &'static TableSpec {
        &BOOK
    }

    fn rows(entries: &[Self], message_type: MessageType, seq: i64, recv_ts: &str) -> Vec<Row> {
        let event_type = message_type.to_string();
        let mut rows = Vec::new();
        for b in entries {
            for (side, levels) in [(LevelSide::Bid, &b.bids), (LevelSide::Ask, &b.asks)] {
                for level in levels {
                    rows.push(vec![
                        Cell::Text(b.symbol.clone()),
                        Cell::Big(seq),
                        Cell::Text(side.to_string()),
                        Cell::Decimal(level.price),
                        Cell::Decimal(level.qty),
                        Cell::Text(event_type.clone()),
                        Cell::UInt(b.checksum),
                        Cell::Text(b.timestamp.clone()),
                        Cell::Text(recv_ts.to_string()),
                    ]);
                }
            }
        }
        rows
    }

    /// Level rows regroup by shared `seq` (row order is write order, so
    /// sides and level order survive), then one [`BookData`] per session of
    /// equal symbols. An all-zero-level frame is never persisted
    /// (`is_recordable` drops it for both sinks); a persisted entry with
    /// no levels, and adjacent same-symbol entries — neither seen on the
    /// v2 feed — don't round-trip.
    fn frames(conn: &duckdb::Connection, drift: DriftPolicy) -> Result<Vec<Stored>> {
        struct LevelRow {
            seq: i64,
            message_type: MessageType,
            symbol: String,
            side: LevelSide,
            level: PriceLevel,
            checksum: u32,
            timestamp: String,
        }
        let rows = read_rows(conn, &BOOK, drift, |row| {
            let side: String = row.get("side")?;
            Ok(LevelRow {
                seq: row.get("seq")?,
                message_type: message_type(row)?,
                symbol: row.get("symbol")?,
                side: side.parse().map_err(|_| {
                    RowError::Vocabulary(format!("unrecognized book side '{side}'"))
                })?,
                level: PriceLevel {
                    price: read_decimal(row, "price")?,
                    qty: read_decimal(row, "qty")?,
                },
                checksum: row.get("checksum")?,
                timestamp: row.get("event_ts")?,
            })
        })?;

        struct Open {
            seq: i64,
            message_type: MessageType,
            data: Vec<BookData>,
        }

        fn open_frame(row: &LevelRow) -> Open {
            Open {
                seq: row.seq,
                message_type: row.message_type,
                data: Vec::new(),
            }
        }

        fn close(open: Open) -> Stored {
            Stored {
                seq: open.seq,
                frame: ChannelMessage {
                    body: ChannelData::Book(open.data),
                    message_type: open.message_type,
                    sequence: None,
                },
            }
        }

        fn push_level(data: &mut Vec<BookData>, row: LevelRow) {
            if data.last().is_none_or(|b| b.symbol != row.symbol) {
                data.push(BookData {
                    symbol: row.symbol,
                    bids: Vec::new(),
                    asks: Vec::new(),
                    checksum: row.checksum,
                    timestamp: row.timestamp,
                });
            }
            if let Some(entry) = data.last_mut() {
                match row.side {
                    LevelSide::Bid => entry.bids.push(row.level),
                    LevelSide::Ask => entry.asks.push(row.level),
                }
            }
        }

        let mut frames: Vec<Stored> = Vec::new();
        let mut current: Option<Open> = None;
        for row in rows {
            let mut frame = match current.take() {
                Some(open) if open.seq == row.seq => open,
                Some(done) => {
                    frames.push(close(done));
                    open_frame(&row)
                }
                None => open_frame(&row),
            };
            push_level(&mut frame.data, row);
            current = Some(frame);
        }
        frames.extend(current.map(close));
        Ok(frames)
    }
}

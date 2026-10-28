//! Channel-table dispatch for the recording schema.
//!
//! Each recordable channel declares its [`TableSpec`], row mapping, and
//! read-back by implementing [`Recordable`] on its payload entry type in its
//! own submodule here (one per channel), so a table's spec, encode, and decode
//! live in one file where they can't drift apart — and the wire types stay free
//! of persistence code: `record` depends on `kraken-core`, never the reverse.
//! This module gathers, dispatches, and hosts the read half the entry-per-row
//! tables share:
//!
//! [`schema_tables_for`] collects every table's spec for the up-front `CREATE
//! TABLE`; [`record_frame`] routes a parsed frame to its channel's rows and
//! writes them through one cached prepared statement per table;
//! [`group_frames`] regroups read rows into the frames the writer expanded.
//! Only the subscribeable market-data channels are recorded — account feeds
//! (executions, balances) are out of scope and skipped without error.

mod book;
mod ohlc;
mod ticker;
mod trade;

use crate::{
    error::Result,
    frames::{
        capture,
        duckdb::table::{Recordable, Row, RowError, Stored, TableSpec},
    },
};
use duckdb::Connection;
use kraken_core::{
    ChannelData, ChannelMessage, SubscribableChannel,
    subscribe::message::{BookData, MessageType, OhlcData, TickerData, TradeData},
};

/// The table a subscribeable channel records into, or `None` for the channels
/// that aren't persisted (`instrument`, `executions`, `balances`, `level3` —
/// the same set [`record_frame`] skips). Single source of truth for the channel
/// → table mapping.
pub(crate) fn table_for(channel: SubscribableChannel) -> Option<&'static TableSpec> {
    use SubscribableChannel as Sub;
    Some(match channel {
        Sub::Ticker => TickerData::table(),
        Sub::Trade => TradeData::table(),
        Sub::Book => BookData::table(),
        Sub::Ohlc => OhlcData::table(),
        Sub::Instrument | Sub::Executions | Sub::Balances | Sub::Level3 => return None,
    })
}

/// The tables to create for a session recording `channels` (the declared set,
/// [`crate::frames::capture::declared_channels`]): the spec for each recordable
/// channel, de-duplicated and order-stable. A channel that isn't persisted
/// contributes no table, so a tape carries only the tables it actually
/// uses rather than every channel's empty table.
pub(crate) fn schema_tables_for(channels: &[SubscribableChannel]) -> Vec<&'static TableSpec> {
    let mut specs: Vec<&'static TableSpec> = Vec::new();
    for &channel in channels {
        if let Some(spec) = table_for(channel)
            && !specs.iter().any(|s| s.name == spec.name)
        {
            specs.push(spec);
        }
    }
    specs
}

/// Persist one parsed frame into its channel table. Returns the number of rows
/// written — `0` for a frame the schema does not record (skipped, not an
/// error), so a stray `status` or account frame that reaches the sink never
/// aborts a capture.
pub(crate) fn record_frame(
    conn: &Connection,
    frame: &ChannelMessage,
    seq: i64,
    recv_ts: &str,
) -> Result<usize> {
    // `capture::is_recordable` is the single source of truth for which
    // frames persist; the match below is only how each is written.
    if !capture::is_recordable(frame) {
        return Ok(0);
    }
    let message_type = frame.message_type;
    match &frame.body {
        ChannelData::Ticker(entries) => insert(
            conn,
            TickerData::table(),
            TickerData::rows(entries, message_type, seq, recv_ts),
        ),
        ChannelData::Trade(entries) => insert(
            conn,
            TradeData::table(),
            TradeData::rows(entries, message_type, seq, recv_ts),
        ),
        ChannelData::Book(entries) => insert(
            conn,
            BookData::table(),
            BookData::rows(entries, message_type, seq, recv_ts),
        ),
        ChannelData::Ohlc(entries) => insert(
            conn,
            OhlcData::table(),
            OhlcData::rows(entries, message_type, seq, recv_ts),
        ),
        _ => Ok(0),
    }
}

/// Decode a row's `event_type` column back into the envelope's type tag. An
/// unrecognized tag is newer vocabulary — a same-MAJOR writer may add one.
pub(crate) fn message_type(row: &duckdb::Row<'_>) -> std::result::Result<MessageType, RowError> {
    let raw: String = row.get("event_type")?;
    raw.parse()
        .map_err(|_| RowError::Vocabulary(format!("unrecognized event_type '{raw}'")))
}

/// One decoded row of a multi-entry channel table: the frame's key columns
/// plus the single data entry the row stored.
pub(crate) struct RowEntry<T> {
    pub seq: i64,
    pub message_type: MessageType,
    pub entry: T,
}

/// Regroup `(seq, rowid)`-ordered rows into the frames [`Recordable::rows`]
/// expanded: same-`seq` runs are one frame, rebuilt through `make` (the
/// channel's [`ChannelData`] variant constructor). The wire `sequence` was
/// never stored; it reads back `None`.
pub(crate) fn group_frames<T>(
    make: fn(Vec<T>) -> ChannelData,
    rows: Vec<RowEntry<T>>,
) -> Vec<Stored> {
    struct Open<T> {
        seq: i64,
        message_type: MessageType,
        data: Vec<T>,
    }
    fn close<T>(make: fn(Vec<T>) -> ChannelData, open: Open<T>) -> Stored {
        Stored {
            seq: open.seq,
            frame: ChannelMessage {
                body: make(open.data),
                message_type: open.message_type,
                sequence: None,
            },
        }
    }
    let mut frames: Vec<Stored> = Vec::new();
    let mut current: Option<Open<T>> = None;
    for row in rows {
        match &mut current {
            Some(open) if open.seq == row.seq => open.data.push(row.entry),
            _ => {
                if let Some(done) = current.take() {
                    frames.push(close(make, done));
                }
                current = Some(Open {
                    seq: row.seq,
                    message_type: row.message_type,
                    data: vec![row.entry],
                });
            }
        }
    }
    frames.extend(current.map(|open| close(make, open)));
    frames
}

/// Bind and execute the generated `INSERT` for each row, reusing one cached
/// prepared statement per table across the whole session.
fn insert(conn: &Connection, spec: &TableSpec, rows: Vec<Row>) -> Result<usize> {
    if rows.is_empty() {
        return Ok(0);
    }
    let mut stmt = conn.prepare_cached(spec.insert_sql())?;
    for row in &rows {
        debug_assert_eq!(
            row.len(),
            spec.columns.len(),
            "row arity must match the column count for table '{}'",
            spec.name
        );
        stmt.execute(duckdb::params_from_iter(row.iter()))?;
    }
    Ok(rows.len())
}

//! Event recording storage: durable persistence of frames, actions, and simulations.
//!
//! Layered bottom-up: a domain-free core — [`Sink`]/[`Source`] plus the
//! generic JSONL pair ([`JsonlSink`]/[`JsonlSource`]) — that any
//! `Serialize`/`Deserialize` type records through, and domain modules built
//! on top of it:
//! - [`frames`] — market data (ticker, trade, book, ohlc): the capture
//!   contracts ([`CaptureSink`]/[`CaptureSource`]) over two backends,
//!   JSONL ([`TapeSink`]/[`TapeSource`]) and feature-gated DuckDB
//! - [`paper`] — the paper-trading account journal
//! - [`actions`] — order placements and fills (planned)
//!
//! Every domain pairs write with read under one vocabulary — schema
//! versioning, metadata stamps, crash recovery — and the symmetric
//! round-trip contract: what was written is exactly what reads back.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

mod error;
mod fsync;
mod jsonl;
mod lock;
mod sink;
mod source;
mod time;

pub mod actions;
pub mod frames;
pub mod paper;
pub mod schema;
pub mod tape;

pub use error::{Error, Result};
pub use frames::capture::{
    CaptureState, RecordingDeclaration, RecordingIntegrity, RecordingManifest, ensure_recordable,
    resolve, sanitize_id, tapes_root, write_json_atomic,
};
pub use frames::{CaptureSink, CaptureSource, MarketEvent, TapeSink, TapeSource};
#[cfg(feature = "duckdb")]
pub use frames::{DuckdbSink, DuckdbSource};
pub use jsonl::{damaged, line_seq};
pub use lock::{FileLock, is_lock_held, lock_path};
pub use sink::{JsonlSink, Sink};
pub use source::{JsonlSource, Source};
pub use tape::{
    TapeBackend, TapeEntry, TapeRef, TapeState, describe_tape, frame_count, list_recordings,
};
pub use time::{format_instant, parse_instant};

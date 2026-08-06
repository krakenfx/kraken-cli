//! The capture contract: what a recording says about itself.
//!
//! One vocabulary shared by the write and read sides — identity
//! ([`RecordingDeclaration`]), completeness ([`RecordingIntegrity`]), the combined
//! self-description ([`RecordingManifest`], the `.meta.json` sidecar mirrored in
//! DuckDB `_meta`), which frames a capture persists (`is_recordable`), and
//! the on-disk path rules. A sink stamps these; a source hands the exact same
//! types back.

use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use kraken_core::{Channel, ChannelData, ChannelMessage, SubscribableChannel};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fsync::fsync_parent;

/// Summary of a finished capture, stamped into the tape metadata at finalize
/// so a tape self-describes when it was captured and whether it is complete.
/// Serde: the sidecar's [`RecordingManifest`] embeds it verbatim and readers decode
/// the same type back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingIntegrity {
    /// Instant the capture ended.
    pub window_end: DateTime<Utc>,
    /// Frames the delivery channel dropped before this recorder read them.
    /// Persisted because a gap is otherwise invisible — `seq`/`last_seq` stay
    /// contiguous across drops. 0 = clean.
    pub events_dropped: u64,
    /// Inbound frames that failed the pinned schema and were dropped at the
    /// connection, before reaching any sink. Persisted for the same reason as
    /// `events_dropped`: without the count, schema drift reads as a clean
    /// capture with silent holes. 0 = clean.
    pub frames_unparsed: u64,
    /// Socket reconnects mid-capture. Each replays a forced snapshot, so a
    /// reader treats a mid-stream snapshot as a state reset, not duplicates.
    pub reconnect_count: u64,
}

/// Metadata describing a recording session, stamped into the tape on open and
/// completed on close. Built by the command from the validated args.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingDeclaration {
    /// Capture source (e.g. [`schema::SOURCE`](crate::schema::SOURCE)).
    pub source: String,
    /// The recorded trading pairs (the human symbols, e.g. `["BTC/USD","ETH/USD"]`)
    /// — one tape can hold several, distinguished by each row's `symbol`.
    pub symbols: Vec<String>,
    /// Channels captured into this tape (e.g. `["trades","book","ohlc"]`).
    pub channels: Vec<String>,
    /// Instant the session started.
    pub window_start: DateTime<Utc>,
    /// CLI version that wrote the tape.
    pub cli_version: String,
}

/// A capture's self-description (the `.meta.json` sidecar; mirrored in DuckDB
/// `_meta`): stamped without a summary at every (re)open and completed at
/// finalize, so a crashed run still has a checkable identity and schema version.
/// On a reopened tape the summary covers the last run only, not all sessions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingManifest {
    /// [`schema::SCHEMA_VERSION`](crate::schema::SCHEMA_VERSION) of the first
    /// writer — a same-MAJOR reopen appends without downgrading the stamp.
    pub schema_version: String,
    #[serde(flatten)]
    pub meta: RecordingDeclaration,
    /// `None` until finalize: the session is live or crashed and its completeness
    /// unknown — consumers must surface that, not replay holes silently.
    #[serde(flatten)]
    pub summary: Option<RecordingIntegrity>,
}

/// The three answers a session's market capture can give about completeness.
/// Consumers must say which — never pretend a hole-free record. One shared
/// view, so no caller re-derives the tri-state and gets a branch wrong.
pub enum CaptureState<'report> {
    /// The session has no market recording at all.
    Absent,
    /// The capture is running or crashed mid-capture; completeness is unknown.
    Unfinalized(&'report RecordingManifest),
    /// Finalized, with its gap counters.
    Finalized(&'report RecordingManifest, &'report RecordingIntegrity),
}

impl<'report> CaptureState<'report> {
    pub fn of(capture: Option<&'report RecordingManifest>) -> Self {
        match capture {
            None => Self::Absent,
            Some(report) => match &report.summary {
                None => Self::Unfinalized(report),
                Some(summary) => Self::Finalized(report, summary),
            },
        }
    }
}

impl RecordingIntegrity {
    /// A clean summary stamped at the current instant: no drops, no
    /// reconnects. For streams that end before any socket work, and fixtures.
    pub fn now() -> Self {
        Self {
            window_end: Utc::now(),
            events_dropped: 0,
            frames_unparsed: 0,
            reconnect_count: 0,
        }
    }
}

impl RecordingDeclaration {
    /// Fold a reopened run into the stored identity: `symbols`/`channels` union
    /// in first-seen order, every other field keeps its first-open value.
    pub(crate) fn absorb(&mut self, run: &RecordingDeclaration) {
        merge_unique(&mut self.symbols, &run.symbols);
        merge_unique(&mut self.channels, &run.channels);
    }
}

/// The channels a recording persists — the one canonical listing;
/// [`ensure_recordable`] derives its gate and message from it, and
/// `is_recordable`'s per-payload match must mirror it (pinned by test).
pub(crate) const RECORDABLE_CHANNELS: [SubscribableChannel; 4] = [
    SubscribableChannel::Ticker,
    SubscribableChannel::Trade,
    SubscribableChannel::Book,
    SubscribableChannel::Ohlc,
];

/// Whether the recording schema persists this frame. One predicate for both sinks, so an
/// unsolicited `status` or a data-less frame (zero DuckDB rows) is skipped by jsonl too.
/// Book frames expand per price level, so one whose entries all carry zero levels is
/// data-less in the recording sense even with a non-empty `data` array.
pub(crate) fn is_recordable(frame: &ChannelMessage) -> bool {
    match &frame.body {
        ChannelData::Ticker(data) => !data.is_empty(),
        ChannelData::Trade(data) => !data.is_empty(),
        ChannelData::Book(data) => data
            .iter()
            .any(|b| !b.bids.is_empty() || !b.asks.is_empty()),
        ChannelData::Ohlc(data) => !data.is_empty(),
        _ => false,
    }
}

/// This session's declared channels, parsed once from the tape meta. An
/// unparseable name contributes nothing — a sink records what it knows —
/// but is warned, so a typo'd declaration doesn't silently record nothing.
pub(crate) fn declared_channels(meta: &RecordingDeclaration) -> Vec<SubscribableChannel> {
    meta.channels
        .iter()
        .filter_map(|name| match name.parse() {
            Ok(channel) => Some(channel),
            Err(_) => {
                tracing::warn!(channel = %name, "ignoring an unrecognized declared channel");
                None
            }
        })
        .collect()
}

/// The per-frame capture gate, one owned by each sink: a frame is admitted
/// iff it is recordable and among this session's declared channels. Both backends
/// build their gate from the same meta, so they skip identical frame sets;
/// an undeclared channel is warned once per gate, not once per frame.
#[derive(Debug)]
pub(crate) struct RecordingFilter {
    declared: Vec<SubscribableChannel>,
    warned: Vec<SubscribableChannel>,
}

impl RecordingFilter {
    pub(crate) fn new(declared: Vec<SubscribableChannel>) -> Self {
        Self {
            declared,
            warned: Vec::new(),
        }
    }

    pub(crate) fn admits(&mut self, frame: &ChannelMessage) -> bool {
        if !is_recordable(frame) {
            return false;
        }
        // `is_recordable` admits market-data payloads only, and every one of
        // those channels is subscribable.
        let Channel::Subscribable(channel) = frame.channel() else {
            return false;
        };
        if self.declared.contains(&channel) {
            return true;
        }
        if !self.warned.contains(&channel) {
            self.warned.push(channel);
            tracing::warn!(
                %channel,
                "skipping frames for a channel this tape does not record"
            );
        }
        false
    }
}

/// Reject any requested channel a recording can't persist, naming them.
/// Empty input passes: "no channels at all" belongs to the caller's own
/// planning, so two rejections never fire on one invocation.
pub fn ensure_recordable(channels: &[SubscribableChannel]) -> Result<()> {
    let rejected: Vec<String> = channels
        .iter()
        .filter(|c| !RECORDABLE_CHANNELS.contains(c))
        .map(SubscribableChannel::to_string)
        .collect();
    if rejected.is_empty() {
        return Ok(());
    }
    Err(Error::Rejected(format!(
        "record captures market-data channels only ({}); cannot record: {}",
        RECORDABLE_CHANNELS.map(|c| c.to_string()).join(", "),
        rejected.join(", ")
    )))
}

/// Atomically write `value` as pretty JSON to `path`: parent dirs are created,
/// the bytes land in an fsync'd temp file *beside the target* (rename is only
/// atomic within one filesystem), then a rename swaps it into place, so a torn
/// file can never be observed.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(value)
        .map_err(|e| Error::Damaged(format!("unencodable json document: {e}")))?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    // The rename is only durable once the directory entry is: without this,
    // a power loss can resurrect the previous stamp — e.g. an old session's
    // *finalized* summary under a log that already holds this session's frames.
    fsync_parent(path)?;
    Ok(())
}

fn merge_unique(into: &mut Vec<String>, from: &[String]) {
    for item in from {
        if !into.contains(item) {
            into.push(item.clone());
        }
    }
}

/// The sidecar's read shape: every completeness field individually optional.
/// The derived flatten-`Option` on [`RecordingManifest`] cannot distinguish
/// a *partial* stamp from an absent one — any missing count would read as
/// `summary: None` — so reads go through this wire type and the all-or-none
/// rule below, matching the DuckDB `_meta` reader's strictness.
#[derive(Deserialize)]
struct SidecarWire {
    schema_version: String,
    #[serde(flatten)]
    meta: RecordingDeclaration,
    window_end: Option<DateTime<Utc>>,
    events_dropped: Option<u64>,
    frames_unparsed: Option<u64>,
    reconnect_count: Option<u64>,
}

/// All-or-none: a partial completeness stamp is a torn or hand-edited
/// sidecar. Reading it as merely unfinalized would silently discard the
/// loss counters, so it is damage.
impl TryFrom<SidecarWire> for RecordingManifest {
    type Error = &'static str;

    fn try_from(wire: SidecarWire) -> std::result::Result<Self, Self::Error> {
        let summary = match (
            wire.window_end,
            wire.events_dropped,
            wire.frames_unparsed,
            wire.reconnect_count,
        ) {
            (None, None, None, None) => None,
            (
                Some(window_end),
                Some(events_dropped),
                Some(frames_unparsed),
                Some(reconnect_count),
            ) => Some(RecordingIntegrity {
                window_end,
                events_dropped,
                frames_unparsed,
                reconnect_count,
            }),
            _ => return Err("partial completeness stamp (torn or hand-edited)"),
        };
        Ok(RecordingManifest {
            schema_version: wire.schema_version,
            meta: wire.meta,
            summary,
        })
    }
}

/// Load and version-gate an existing tape's sidecar — the one JSONL stamp
/// check, shared by the tape sink and source. `action` names the refused
/// operation in the rejection ("append" for the sink, "read" for the
/// source). An unparseable or partially-stamped sidecar is damage: the
/// atomic sidecar write makes a torn file unobservable, so neither can come
/// from a healthy writer.
pub(crate) fn load_sidecar(tape: &Path, action: &str) -> Result<RecordingManifest> {
    let meta_path = sidecar_path(tape);
    let data = match std::fs::read_to_string(&meta_path) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::Rejected(format!(
                "tape '{}' has no metadata sidecar, so its schema version \
                 cannot be verified (unstamped or foreign); refusing to {action} — \
                 re-record it or use a new file",
                tape.display()
            )));
        }
        Err(e) => return Err(e.into()),
    };
    let unreadable = |what: &str| {
        Error::Damaged(format!(
            "unreadable tape sidecar '{}': {what}",
            meta_path.display()
        ))
    };
    let wire =
        serde_json::from_str::<SidecarWire>(&data).map_err(|e| unreadable(&e.to_string()))?;
    let report = RecordingManifest::try_from(wire).map_err(unreadable)?;
    crate::schema::ensure_compatible(&report.schema_version, action)?;
    Ok(report)
}

/// Sidecar path: `BTC-USD.jsonl` -> `BTC-USD.jsonl.meta.json`. Appended rather
/// than `with_extension`, which would collapse sibling tapes onto one sidecar.
pub(crate) fn sidecar_path(tape: &Path) -> PathBuf {
    let mut raw = tape.as_os_str().to_owned();
    raw.push(".meta.json");
    PathBuf::from(raw)
}

/// Turn a symbol into a filesystem-safe tape id: `BTC/USD` -> `BTC-USD`.
/// Any character outside `[A-Za-z0-9._-]` becomes `-` so the id is a single safe
/// path segment.
pub fn sanitize_id(symbol: &str) -> String {
    symbol
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => c,
            _ => '-',
        })
        .collect()
}

/// The global market-memory library every standalone tape lives in:
/// `<base>/tapes/`.
pub fn tapes_root(base: &Path) -> PathBuf {
    base.join("tapes")
}

/// Resolve the tape file path: `<base>/tapes/<id>.<ext>`. `base` is the
/// config dir in production and a temp dir in tests, so the store is test-isolable
/// without touching the real config directory.
pub fn resolve(base: &Path, id: &str, ext: &str) -> PathBuf {
    tapes_root(base).join(format!("{id}.{ext}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_report_matches_the_pinned_sidecar_shape() {
        let report = RecordingManifest {
            schema_version: "1.0".to_string(),
            meta: RecordingDeclaration {
                source: "kraken-spot-ws-v2".to_string(),
                symbols: vec!["BTC/USD".to_string()],
                channels: vec!["trade".to_string(), "book".to_string()],
                window_start: "2026-01-01T00:00:00Z".parse().unwrap(),
                cli_version: "0.3.2".to_string(),
            },
            summary: Some(RecordingIntegrity {
                window_end: "2026-01-01T01:00:00Z".parse().unwrap(),
                events_dropped: 7,
                frames_unparsed: 3,
                reconnect_count: 2,
            }),
        };
        let pinned = serde_json::json!({
            "schema_version": "1.0",
            "source": "kraken-spot-ws-v2",
            "symbols": ["BTC/USD"],
            "channels": ["trade", "book"],
            "window_start": "2026-01-01T00:00:00Z",
            "cli_version": "0.3.2",
            "window_end": "2026-01-01T01:00:00Z",
            "events_dropped": 7,
            "frames_unparsed": 3,
            "reconnect_count": 2
        });
        assert_eq!(serde_json::to_value(&report).unwrap(), pinned);
        assert_eq!(
            serde_json::from_value::<RecordingManifest>(pinned).unwrap(),
            report
        );
    }

    #[test]
    fn unfinalized_report_pins_to_the_summaryless_shape() {
        let report = RecordingManifest {
            schema_version: "1.0".to_string(),
            meta: RecordingDeclaration {
                source: "kraken-spot-ws-v2".to_string(),
                symbols: vec!["BTC/USD".to_string()],
                channels: vec!["trade".to_string()],
                window_start: "2026-01-01T00:00:00Z".parse().unwrap(),
                cli_version: "0.3.2".to_string(),
            },
            summary: None,
        };
        let pinned = serde_json::json!({
            "schema_version": "1.0",
            "source": "kraken-spot-ws-v2",
            "symbols": ["BTC/USD"],
            "channels": ["trade"],
            "window_start": "2026-01-01T00:00:00Z",
            "cli_version": "0.3.2"
        });
        assert_eq!(serde_json::to_value(&report).unwrap(), pinned);
        assert_eq!(
            serde_json::from_value::<RecordingManifest>(pinned).unwrap(),
            report
        );
    }

    #[test]
    fn absorb_unions_symbols_and_channels_keeping_first_run_identity() {
        let mut stored = RecordingDeclaration {
            source: "kraken-spot-ws-v2".to_string(),
            symbols: vec!["BTC/USD".to_string()],
            channels: vec!["ticker".to_string()],
            window_start: "2026-01-01T00:00:00Z".parse().unwrap(),
            cli_version: "0.3.2".to_string(),
        };
        stored.absorb(&RecordingDeclaration {
            source: "other-source".to_string(),
            symbols: vec!["ETH/USD".to_string(), "BTC/USD".to_string()],
            channels: vec!["ticker".to_string(), "trade".to_string()],
            window_start: "2027-01-01T00:00:00Z".parse().unwrap(),
            cli_version: "9.9.9".to_string(),
        });
        assert_eq!(stored.symbols, ["BTC/USD", "ETH/USD"]);
        assert_eq!(stored.channels, ["ticker", "trade"]);
        assert_eq!(stored.source, "kraken-spot-ws-v2");
        assert_eq!(
            stored.window_start,
            "2026-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
        assert_eq!(stored.cli_version, "0.3.2");
    }

    #[test]
    fn data_less_frames_are_not_recordable() {
        let empty = ChannelMessage::parse(r#"{"channel":"trade","type":"update","data":[]}"#)
            .expect("empty-data frame parses");
        assert!(!is_recordable(&empty));
        let full = ChannelMessage::parse(
            r#"{"channel":"trade","type":"update","data":[{"symbol":"BTC/USD","side":"buy",
               "price":1.0,"qty":1.0,"ord_type":"market","trade_id":1,"timestamp":"TS"}]}"#,
        )
        .expect("trade frame parses");
        assert!(is_recordable(&full));
    }

    #[test]
    fn zero_level_book_frames_are_not_recordable() {
        // A book frame's row count is entries × levels, not `data.len()`: all-empty
        // levels would write a JSONL line but zero DuckDB rows, splitting the two
        // backends for the same capture.
        let zero_level = ChannelMessage::parse(
            r#"{"channel":"book","type":"update","data":[{"symbol":"BTC/USD",
               "bids":[],"asks":[],"checksum":1,"timestamp":"TS"}]}"#,
        )
        .expect("zero-level book frame parses");
        assert!(!is_recordable(&zero_level));
        let one_sided = ChannelMessage::parse(
            r#"{"channel":"book","type":"update","data":[{"symbol":"BTC/USD",
               "bids":[{"price":1.0,"qty":2.0}],"asks":[],"checksum":1,"timestamp":"TS"}]}"#,
        )
        .expect("one-sided book frame parses");
        assert!(
            is_recordable(&one_sided),
            "ordinary one-sided updates persist"
        );
    }

    #[test]
    fn recordable_channels_are_exactly_the_market_data_set() {
        for c in [
            SubscribableChannel::Ticker,
            SubscribableChannel::Trade,
            SubscribableChannel::Book,
            SubscribableChannel::Ohlc,
        ] {
            assert!(ensure_recordable(&[c]).is_ok(), "{c} should be recordable");
        }
        for c in [
            SubscribableChannel::Instrument,
            SubscribableChannel::Level3,
            SubscribableChannel::Executions,
            SubscribableChannel::Balances,
        ] {
            assert!(
                ensure_recordable(&[c]).is_err(),
                "{c} should not be recordable"
            );
        }
    }

    #[test]
    fn every_recordable_channel_has_recordable_frames() {
        // Ties is_recordable's per-payload match to RECORDABLE_CHANNELS: a
        // channel added to the const without a match arm panics here (no
        // fixture) instead of silently dropping every frame in both sinks.
        for channel in RECORDABLE_CHANNELS {
            let raw = match channel {
                SubscribableChannel::Ticker => {
                    r#"{"channel":"ticker","type":"update","data":[{"symbol":"BTC/USD",
                       "bid":1.0,"bid_qty":2.0,"ask":3.0,"ask_qty":4.0,"last":5.0,
                       "volume":5.0,"vwap":6.0,"low":0.5,"high":9.0,"change":-1.0,
                       "change_pct":-2.5,"timestamp":"TS"}]}"#
                }
                SubscribableChannel::Trade => {
                    r#"{"channel":"trade","type":"update","data":[{"symbol":"BTC/USD","side":"buy",
                       "price":1.0,"qty":1.0,"ord_type":"market","trade_id":1,"timestamp":"TS"}]}"#
                }
                SubscribableChannel::Book => {
                    r#"{"channel":"book","type":"update","data":[{"symbol":"BTC/USD",
                       "bids":[{"price":1.0,"qty":2.0}],"asks":[],"checksum":1,"timestamp":"TS"}]}"#
                }
                SubscribableChannel::Ohlc => {
                    r#"{"channel":"ohlc","type":"update","data":[{"symbol":"BTC/USD",
                       "open":1.0,"high":2.0,"low":0.5,"close":1.5,"vwap":1.2,"trades":7,
                       "volume":1.0,"interval_begin":"TS","interval":1}]}"#
                }
                other => panic!("no recordable fixture for {other}: add one with its match arm"),
            };
            let frame = ChannelMessage::parse(raw).expect("fixture parses");
            assert!(is_recordable(&frame), "{channel} frames must be recordable");
        }
    }

    #[test]
    fn gate_keeps_skipping_undeclared_frames_after_the_first_warn() {
        let trade = ChannelMessage::parse(
            r#"{"channel":"trade","type":"update","data":[{"symbol":"BTC/USD","side":"buy",
               "price":1.0,"qty":1.0,"ord_type":"market","trade_id":1,"timestamp":"TS"}]}"#,
        )
        .expect("trade frame parses");
        let ticker = ChannelMessage::parse(
            r#"{"channel":"ticker","type":"update","data":[{"symbol":"BTC/USD",
               "bid":1.0,"bid_qty":2.0,"ask":3.0,"ask_qty":4.0,"last":5.0,
               "volume":5.0,"vwap":6.0,"low":0.5,"high":9.0,"change":-1.0,
               "change_pct":-2.5,"timestamp":"TS"}]}"#,
        )
        .expect("ticker frame parses");

        let mut gate = RecordingFilter::new(vec![SubscribableChannel::Trade]);
        assert!(gate.admits(&trade));
        // The warn-once bookkeeping must not change the verdict: every
        // undeclared frame stays skipped, every declared one admitted.
        assert!(!gate.admits(&ticker));
        assert!(!gate.admits(&ticker));
        assert!(gate.admits(&trade));
    }

    #[test]
    fn ensure_recordable_rejects_account_feeds_and_names_them() {
        let Error::Rejected(msg) =
            ensure_recordable(&[SubscribableChannel::Trade, SubscribableChannel::Executions])
                .unwrap_err()
        else {
            panic!("a non-recordable channel must be rejected");
        };
        assert!(
            msg.contains("executions"),
            "error names the bad channel: {msg}"
        );
    }

    #[test]
    fn ensure_recordable_accepts_recordable_channels_and_empty_input() {
        // Empty is deferred to the caller's own planning, so it passes this guard.
        assert!(ensure_recordable(&[]).is_ok());
        assert!(
            ensure_recordable(&[SubscribableChannel::Trade, SubscribableChannel::Book]).is_ok()
        );
    }

    #[test]
    fn write_json_atomic_replaces_content_and_leaves_no_temp_behind() {
        // The mechanism finalize's correctness rests on: the swap is total
        // (old content fully replaced) and the same-directory temp — the
        // rename-atomicity precondition — never outlives the write.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.meta.json");
        write_json_atomic(&path, &serde_json::json!({"session": 1})).unwrap();
        write_json_atomic(&path, &serde_json::json!({"session": 2})).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"session\": 2"));
        assert!(!content.contains("\"session\": 1"));
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n != "report.meta.json")
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }

    fn wire_manifest(json: serde_json::Value) -> std::result::Result<RecordingManifest, String> {
        let wire = serde_json::from_value::<SidecarWire>(json).map_err(|e| e.to_string())?;
        RecordingManifest::try_from(wire).map_err(str::to_string)
    }

    #[test]
    fn partial_completeness_stamp_is_damage_in_both_directions() {
        // window_end without its counts, and counts without a window_end:
        // both are torn stamps, neither may read as merely unfinalized.
        let base = serde_json::json!({
            "schema_version": "1.0",
            "source": "kraken-spot-ws-v2",
            "symbols": ["BTC/USD"],
            "channels": ["trade"],
            "window_start": "2026-01-01T00:00:00Z",
            "cli_version": "0.3.2"
        });
        let mut with_end = base.clone();
        with_end["window_end"] = "2026-01-01T01:00:00Z".into();
        assert!(
            wire_manifest(with_end).is_err(),
            "window_end without counts"
        );

        let mut with_counts = base.clone();
        with_counts["events_dropped"] = 1.into();
        with_counts["frames_unparsed"] = 0.into();
        with_counts["reconnect_count"] = 0.into();
        assert!(
            wire_manifest(with_counts).is_err(),
            "counts without window_end"
        );

        assert_eq!(
            wire_manifest(base).unwrap().summary,
            None,
            "an all-absent stamp is a legitimate unfinalized capture"
        );
    }

    #[test]
    fn sanitizes_symbol_into_safe_id() {
        assert_eq!(sanitize_id("BTC/USD"), "BTC-USD");
        assert_eq!(sanitize_id("AAPLx/USD"), "AAPLx-USD");
        assert_eq!(sanitize_id("ETH_USD.1"), "ETH_USD.1");
    }

    #[test]
    fn resolves_under_recordings_dir() {
        let p = resolve(Path::new("/cfg"), "BTC-USD", "duckdb");
        assert!(p.ends_with("tapes/BTC-USD.duckdb"));
    }
}

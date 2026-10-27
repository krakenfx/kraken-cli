use crate::{
    errors::Result,
    output::{self, OutputFormat, summary::Summarize},
};
use kraken_core::{ChannelMessage, endpoint::Endpoint, subscribe::ChannelData};
use kraken_recording::{CaptureSink, Error as RecordingError, RecordingIntegrity, Sink};
use serde::Serialize;
use std::io::{self, Write};

/// The stdout view — JSONL (`-o json`) or the human single-line table (`-o
/// table`). A [`Sink`] like the durable backends, but it renders frames to the
/// terminal instead of persisting them; the capture summary goes to stderr as
/// one structured log line. A write error — most often a broken pipe once the
/// reader closed, as in `kraken ws … | head` — is surfaced as an `io` error so
/// the stream stops promptly rather than silently dropping frames into a dead
/// pipe.
pub(crate) struct StdoutSink {
    format: OutputFormat,
    out: io::Stdout,
    /// Reused render buffer: each frame is rendered straight into it and the
    /// batch written in a single `write_all`, so a coalesced batch is one
    /// syscall rather than one per frame. Held across calls so a sustained
    /// stream doesn't reallocate it every batch.
    buf: Vec<u8>,
}

impl StdoutSink {
    pub(crate) fn new(format: OutputFormat) -> Self {
        Self {
            format,
            out: io::stdout(),
            buf: Vec::new(),
        }
    }
}

impl Sink for StdoutSink {
    type Item = ChannelMessage;

    fn record(&mut self, batch: &[ChannelMessage]) -> kraken_recording::Result<()> {
        // Render the whole batch into one buffer first. `io::Stdout` is line-buffered
        // (it wraps a `LineWriter`), so writing frames one at a time flushes
        // the fd on every newline; one `write_all` of the joined batch
        // collapses that into a single write.
        self.buf.clear();
        for data in batch {
            render_data(&mut self.buf, self.format, data)?;
            self.buf.push(b'\n');
        }
        if self.buf.is_empty() {
            return Ok(());
        }
        // A write failure (broken pipe, full disk) ends the stream: surface it so the
        // session loop finalizes and the standard `io` error envelope is
        // reported, rather than streaming on into a sink nobody is reading.
        let mut out = self.out.lock();
        out.write_all(&self.buf)
            .and_then(|()| out.flush())
            .map_err(RecordingError::Io)
    }
}

impl CaptureSink for StdoutSink {
    /// Unlike the rest of the lifecycle (stderr diagnostics), the reconnect
    /// marker is data for the stream consumer — the one line telling it the
    /// venue re-sends full snapshots next, so accumulated state is stale — and
    /// so goes to stdout, between frames. A write failure ends the stream,
    /// same policy as [`record`](Sink::record).
    fn record_reconnect(&mut self, endpoint: Endpoint) -> kraken_recording::Result<()> {
        let line = reconnect_line(self.format, endpoint);
        let mut out = self.out.lock();
        out.write_all(line.as_bytes())
            .and_then(|()| out.write_all(b"\n"))
            .and_then(|()| out.flush())
            .map_err(RecordingError::Io)
    }

    /// stdout persists no metadata, so the `summary` goes to stderr as one
    /// structured log line — without it a lossy session (`events_dropped >
    /// 0`) would report its holes nowhere. Must not rewrite `buf`: every
    /// batch was already written and flushed by [`record`](Sink::record),
    /// so a write here would duplicate the final batch on stream end.
    fn finalize(self, summary: RecordingIntegrity) -> kraken_recording::Result<()> {
        // Dropped or unparseable frames are holes in the capture — warn-level, or the
        // default stderr filter (warnings only) would hide exactly the sessions this
        // line exists for.
        if summary.events_dropped > 0 || summary.frames_unparsed > 0 {
            tracing::warn!(
                window_end = %summary.window_end,
                events_dropped = summary.events_dropped,
                frames_unparsed = summary.frames_unparsed,
                reconnect_count = summary.reconnect_count,
                "stream session ended with dropped frames"
            );
        } else {
            tracing::info!(
                window_end = %summary.window_end,
                reconnect_count = summary.reconnect_count,
                "stream session ended"
            );
        }
        self.out.lock().flush().map_err(RecordingError::Io)
    }
}

/// Render one data frame for `format` into `buf`. JSON serializes it back to
/// its wire shape — straight into the reused buffer, so a frame costs no
/// intermediate `String`; table renders a single line. A serialization failure
/// propagates like a write failure — silently emitting fewer lines than frames
/// received would corrupt the NDJSON contract. (The caller never writes a batch
/// whose render failed, so a partial frame in `buf` cannot reach stdout.)
fn render_data(
    buf: &mut Vec<u8>,
    format: OutputFormat,
    data: &ChannelMessage,
) -> kraken_recording::Result<()> {
    match format {
        // Balances frames rename their `type` keys (see `publish_balances`);
        // every other channel serializes straight from the typed frame.
        OutputFormat::Json if matches!(data.body, ChannelData::Balances(_)) => {
            let mut frame = serde_json::to_value(data)
                .map_err(|e| RecordingError::Damaged(format!("unrenderable frame: {e}")))?;
            publish_balances(&mut frame);
            serde_json::to_writer(&mut *buf, &frame)
                .map_err(|e| RecordingError::Damaged(format!("unrenderable frame: {e}")))
        }
        OutputFormat::Json => serde_json::to_writer(&mut *buf, data)
            .map_err(|e| RecordingError::Damaged(format!("unrenderable frame: {e}"))),
        OutputFormat::Table => {
            let channel = data.channel().to_string();
            let line = output::table::stream_line(&[
                ("channel", &channel),
                ("data", &data.body.summary()),
            ]);
            buf.extend_from_slice(line.as_bytes());
            Ok(())
        }
    }
}

/// The single-line reconnect marker, in the session-lifecycle envelope style
/// (`type` discriminator, flat fields) so consumers key on it like
/// `session_started`. `snapshot_follows` carries the actionable bit: the next
/// frames on this endpoint are full snapshots, not deltas on the old state.
fn reconnect_line(format: OutputFormat, endpoint: Endpoint) -> String {
    let endpoint = endpoint.to_string();
    match format {
        // Insertion order matches the alphabetical order the pinned contract
        // line documents, so the bytes are identical whether serde_json is
        // built with `preserve_order` or without it.
        OutputFormat::Json => serde_json::json!({
            "endpoint": endpoint,
            "snapshot_follows": true,
            "type": "reconnected",
        })
        .to_string(),
        OutputFormat::Table => output::table::stream_line(&[
            ("event", "reconnected"),
            ("endpoint", &endpoint),
            ("data", "fresh snapshot follows"),
        ]),
    }
}

/// Render one one-shot method result (orders, ping) to stdout. A broken pipe is
/// a clean end (the consumer left), not a failure; any other write or
/// serialization failure propagates — an order result that never reached stdout
/// must not exit 0. JSON serializes the typed result; table shows the method
/// and its one-line summary.
pub(crate) fn print_result<R: Serialize + Summarize>(
    format: OutputFormat,
    method: &str,
    result: &R,
) -> Result<()> {
    let line = match format {
        OutputFormat::Json => serde_json::to_string(result)?,
        OutputFormat::Table => {
            output::table::stream_line(&[("method", method), ("result", &result.summary())])
        }
    };
    let mut out = io::stdout().lock();
    let written = out
        .write_all(line.as_bytes())
        .and_then(|()| out.write_all(b"\n"))
        .and_then(|()| out.flush());
    match written {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(crate::errors::KrakenError::Io(e)),
    }
}

/// The `balances` channel is the one WS channel whose published keys differ
/// from the wire: an entry's ledger `type` serializes as `ledger_type`, a
/// wallet's flavour as `wallet_type`. Applied to the serialized frame at the
/// stdout boundary only — recorded tapes and the in-process typed model keep
/// the wire shape (`balances` is not recordable anyway).
fn publish_balances(frame: &mut serde_json::Value) {
    let Some(entries) = frame
        .get_mut("data")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    for entry in entries {
        rename_key(entry, "type", "ledger_type");
        if let Some(wallets) = entry
            .get_mut("wallets")
            .and_then(serde_json::Value::as_array_mut)
        {
            for wallet in wallets {
                rename_key(wallet, "type", "wallet_type");
            }
        }
    }
}

fn rename_key(value: &mut serde_json::Value, wire: &str, published: &str) {
    if let Some(obj) = value.as_object_mut()
        && let Some(v) = obj.shift_remove(wire)
    {
        obj.insert(published.to_string(), v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn balances_rename_entry_and_wallet_type_keys() {
        let mut frame = json!({
            "channel": "balances", "type": "snapshot",
            "data": [
                { "asset": "BTC", "balance": 1.5,
                  "wallets": [{ "type": "earn", "id": "flexible", "balance": 1.5 }] },
                { "asset": "USD", "balance": 100.0, "type": "trade", "category": "trade" }
            ],
            "sequence": 9
        });
        publish_balances(&mut frame);
        assert_eq!(frame["data"][0]["wallets"][0]["wallet_type"], "earn");
        assert!(frame["data"][0]["wallets"][0].get("type").is_none());
        assert_eq!(frame["data"][1]["ledger_type"], "trade");
        assert!(frame["data"][1].get("type").is_none());
        assert_eq!(
            frame["type"], "snapshot",
            "the frame-envelope type (snapshot/update) is untouched"
        );
    }

    #[test]
    fn the_json_reconnect_marker_is_the_pinned_contract_line() {
        // A public stream contract: consumers key on this exact shape, so any
        // drift is a breaking change and must fail here first.
        assert_eq!(
            reconnect_line(OutputFormat::Json, Endpoint::Public),
            r#"{"endpoint":"public","snapshot_follows":true,"type":"reconnected"}"#
        );
    }

    #[test]
    fn the_table_reconnect_marker_is_one_stream_line() {
        assert_eq!(
            reconnect_line(OutputFormat::Table, Endpoint::Auth),
            "event: reconnected  |  endpoint: auth  |  data: fresh snapshot follows"
        );
    }
}

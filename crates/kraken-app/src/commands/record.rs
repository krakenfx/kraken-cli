use crate::{
    cli::AppContext,
    commands::subscription::plan_subscriptions,
    config,
    errors::Result,
    record::{RecordFormat, resolve_targets},
    sink::fanout::Fanout,
};
use chrono::Utc;
use itertools::Itertools;
use kraken_contract::record::RecordCommand;
use kraken_core::WsSubscription;
use kraken_recording::{RecordingDeclaration, schema};
use std::path::{Path, PathBuf};

/// Plan what record captures: market-data channels only. Any channel the
/// recording schema can't store (e.g. the `executions`/`balances` account
/// feeds) is rejected up front — before planning a subscription or opening an
/// authenticated socket.
fn plan(cmd: &RecordCommand) -> Result<Vec<WsSubscription>> {
    kraken_recording::ensure_recordable(&cmd.channels)?;
    plan_subscriptions(&cmd.symbols, &cmd.channels, &cmd.options)
}

/// Run `kraken record`: plan the subscriptions, open the tape up front, then
/// stream through the shared [`stream::run`](crate::stream::run) driver into
/// the durable [`Sink`](kraken_recording::Sink) backend. The same `plan → build
/// sink → stream` flow `streamd` takes — record only swaps the stdout sink for
/// a persisting one, routing lifecycle diagnostics to stderr so stdout stays
/// clean. Socket drops are retried by the engine indefinitely (paced backoff) —
/// the session ends on shutdown, a fatal sink error, or every subscription
/// being rejected; the result folds that with the sink's finalize
/// (stamp/checkpoint) outcome.
pub(crate) async fn execute(cmd: &RecordCommand, ctx: &AppContext) -> Result<()> {
    // Derive the tape metadata from the planned subscriptions, then stream them.
    let ws_subscriptions = plan(cmd)?;

    // Market memory is global (never workspace-scoped): every tape recorded
    // anywhere is available to every workspace's lab, so the library compounds.
    let base = config::config_dir()?;
    // Reduce the name to one safe path segment so it cannot escape the tape
    // library.
    let tape_id = kraken_recording::sanitize_id(&cmd.tape);
    let meta = RecordingDeclaration {
        source: schema::SOURCE.to_string(),
        symbols: ws_subscriptions
            .iter()
            .flat_map(|sub| sub.symbols().iter().cloned())
            .unique()
            .collect(),
        channels: ws_subscriptions
            .iter()
            .map(|sub| sub.channel().to_string())
            .unique()
            .collect(),
        window_start: Utc::now(),
        cli_version: env!("CARGO_PKG_VERSION").to_string(),
    };

    // Open every requested sink up front (a busy tape or incompatible schema fails
    // here, before any socket opens), then stream through the shared
    // `stream::run` driver into the fan-out, which persists to each durable
    // backend and best-effort echoes to stdout.
    let plan = resolve_targets(&cmd.to);
    let tapes: Vec<PathBuf> = plan
        .durables
        .iter()
        .map(|backend| kraken_recording::resolve(&base, &tape_id, backend.extension()))
        .collect();
    let fanout = Fanout::build(&plan, &meta, ctx.format, |backend| {
        kraken_recording::resolve(&base, &tape_id, backend.extension())
    })?;

    // Every sink is open now (locks acquired, schemas ensured), so announce capture
    // start on stderr — one line per durable backend — before the socket opens.
    // It never prints for a run that failed to open; stdout stays reserved for
    // the data itself.
    for (backend, path) in plan.durables.iter().zip(&tapes) {
        eprintln!("{}", started_line(path, &meta, *backend));
    }

    let result = crate::stream::run(ws_subscriptions, None, None, fanout, None, ctx).await;

    // Only a session that actually finalized gets the summary: a failed session's
    // stderr must read started → error, not started → summary, or a wrapper
    // tailing the lifecycle records a completed capture for a recording that
    // never streamed.
    if result.is_ok() {
        // The captured-frame count (identical across mirrored backends, so read
        // it from the first) turns a quiet-market tape from a silent unknown
        // into an honest "0 events".
        let events = tapes
            .first()
            .and_then(|path| kraken_recording::frame_count(path));
        eprintln!("{}", summary_line(&tapes, events));
    }

    result
}

/// A one-line `record` notice for stderr announcing capture has begun — emitted
/// once the tape is open (lock acquired, schema ensured) and streaming is about
/// to start, so it never prints for a session that failed to open. Mirrors
/// [`summary_line`]; stdout stays data.
fn started_line(tape: &Path, meta: &RecordingDeclaration, format: RecordFormat) -> String {
    serde_json::json!({
        "event": "record",
        "type": "started",
        "ts": Utc::now(),
        "tape": tape.display().to_string(),
        "format": format.extension(),
        "symbols": meta.symbols,
        "channels": meta.channels,
    })
    .to_string()
}

/// A one-line `record` summary for stderr naming where the capture landed —
/// stdout is reserved for the data itself.
fn summary_line(tapes: &[PathBuf], events: Option<u64>) -> String {
    let tapes: Vec<String> = tapes.iter().map(|p| p.display().to_string()).collect();
    serde_json::json!({
        "event": "record",
        "type": "summary",
        "ts": Utc::now(),
        "tapes": tapes,
        "events": events,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kraken_contract::{subscription::SubscriptionOptions, value_types::SinkTarget};
    use kraken_core::SubscribableChannel;

    fn args(symbols: &[&str], channels: &[SubscribableChannel]) -> RecordCommand {
        RecordCommand {
            symbols: symbols.iter().map(|s| (*s).to_string()).collect(),
            channels: channels.to_vec(),
            options: SubscriptionOptions::default(),
            to: vec![SinkTarget::Jsonl],
            tape: "ds".into(),
        }
    }

    /// Resolve a record command into the subscriptions it streams, mirroring
    /// streamd.
    fn subscriptions(cmd: &RecordCommand) -> Vec<WsSubscription> {
        plan(cmd).expect("expected a streaming plan")
    }

    #[test]
    fn started_line_names_the_tape_and_what_is_recorded() {
        let meta = RecordingDeclaration {
            source: schema::SOURCE.to_string(),
            symbols: vec!["BTC/USD".into(), "ETH/USD".into()],
            channels: vec!["ticker".into()],
            window_start: "2026-01-01T00:00:00Z".parse().unwrap(),
            cli_version: "test".into(),
        };
        let line = started_line(Path::new("/data/test.duckdb"), &meta, RecordFormat::Duckdb);
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["event"], "record");
        assert_eq!(v["type"], "started");
        assert_eq!(v["tape"], "/data/test.duckdb");
        assert_eq!(v["format"], "duckdb");
        assert_eq!(v["symbols"], serde_json::json!(["BTC/USD", "ETH/USD"]));
        assert_eq!(v["channels"], serde_json::json!(["ticker"]));
    }

    #[test]
    fn dedupes_repeated_channels() {
        let subs = subscriptions(&args(
            &["BTC/USD"],
            &[SubscribableChannel::Trade, SubscribableChannel::Trade],
        ));
        assert_eq!(subs.len(), 1);
    }

    #[test]
    fn tape_name_is_sanitized_to_one_segment() {
        // `--tape` names one file; a traversal attempt is reduced to a single safe
        // path segment so it cannot escape the tape library.
        assert!(!kraken_recording::sanitize_id("../../etc/evil").contains('/'));
    }
}

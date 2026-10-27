//! Read-only reliability signals for live streams.
//!
//! A separate broadcast consumer measures latency, gaps, throughput, and
//! liveness without affecting capture. Advisory JSONL stays on stderr.

use chrono::{DateTime, Utc};
use kraken_core::{Channel, ChannelData, ChannelMessage, MethodResponse, endpoint::Endpoint};
use kraken_ws::Event;
use serde::Serialize;
use serde_with::skip_serializing_none;
use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};
use tokio::{
    sync::broadcast,
    time::{Instant, sleep_until},
};

/// Resolved monitor configuration. Built from the `--monitor*` flags in
/// [`AppContext`](crate::cli::AppContext); `None` there means the monitor is
/// off.
#[derive(Debug, Clone)]
pub(crate) struct MonitorConfig {
    /// How often a periodic `health` snapshot is emitted to stderr.
    pub(crate) health_interval: Duration,
    /// Emit a soft alert after this long with no market-data frame.
    pub(crate) stale_after: Duration,
    /// Emit a hard alert after this long with no heartbeat (connection-level
    /// liveness).
    pub(crate) heartbeat_timeout: Duration,
}

impl MonitorConfig {
    /// Build from the raw flag values (seconds), clamping each to at least 1s
    /// so a zero never produces a busy-loop of timers.
    pub(crate) fn new(
        health_interval_s: u64,
        stale_after_s: u64,
        heartbeat_timeout_s: u64,
    ) -> Self {
        Self {
            health_interval: Duration::from_secs(health_interval_s.max(1)),
            stale_after: Duration::from_secs(stale_after_s.max(1)),
            heartbeat_timeout: Duration::from_secs(heartbeat_timeout_s.max(1)),
        }
    }
}

/// Aggregates until every event sender closes.
pub(crate) async fn run_monitor(mut rx: broadcast::Receiver<Event>, cfg: MonitorConfig) {
    let mut monitor = Monitor::default();
    // Absolute deadlines preserve cadence across unrelated receive events.
    let mut health_deadline = Instant::now() + cfg.health_interval;
    let mut data_deadline = Instant::now() + cfg.stale_after;
    let mut hb_deadline = Instant::now() + cfg.heartbeat_timeout;

    loop {
        tokio::select! {
            // Unbiased: a high-volume feed keeps `rx.recv()` perpetually ready, and a biased
            // select would then starve the timers — never emitting a periodic `health` line
            // exactly when throughput diagnostics matter most. Fair polling still can't
            // false-trip the data/heartbeat watchdogs: each observed frame pushes their
            // deadlines forward, so `sleep_until` only completes once the feed is truly quiet.
            recv = rx.recv() => match recv {
                Ok(event) => {
                    let reset = monitor.observe(event, Utc::now());
                    let now = Instant::now();
                    if reset.data {
                        monitor.last_data = Some(now);
                        data_deadline = now + cfg.stale_after;
                    }
                    // Global liveness can mask one stalled endpoint in a mixed stream.
                    if reset.liveness {
                        monitor.last_liveness = Some(now);
                        hb_deadline = now + cfg.heartbeat_timeout;
                    }
                }
                // The ring overwrote frames this receiver never read — the feed is provably
                // alive (it's overflowing). Re-arm both watchdogs and fold the local drop in.
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    let now = Instant::now();
                    monitor.note_lag(n, now);
                    data_deadline = now + cfg.stale_after;
                    hb_deadline = now + cfg.heartbeat_timeout;
                }
                // Every sender gone: the stream has ended. Emit the final summary and stop.
                Err(broadcast::error::RecvError::Closed) => break,
            },

            () = sleep_until(health_deadline) => {
                emit(&monitor.snapshot(SnapshotKind::Health, &cfg));
                monitor.frames_in_window.clear();
                health_deadline = Instant::now() + cfg.health_interval;
            }

            () = sleep_until(data_deadline) => {
                emit(&stale_data_alert(cfg.stale_after));
                // Re-arm so a genuinely dead feed alerts at most once per interval, not in a
                // tight loop.
                data_deadline = Instant::now() + cfg.stale_after;
            }

            () = sleep_until(hb_deadline) => {
                // `liveness_alert` is phase-aware: before the first successful connect it
                // reports the feed never came up (with the failed-attempt count); while a
                // socket is connected but silent, a genuine stall. Between an observed
                // disconnect and the next reconnect it returns `None` — the session is already
                // `status: down` and the `no_data` watchdog covers the outage, so a "stalled
                // socket" alert would be false.
                if let Some(alert) = liveness_alert(
                    monitor.any_connected(),
                    monitor.total_connects(),
                    monitor.connect_failures(),
                    cfg.heartbeat_timeout,
                ) {
                    emit(&alert);
                }
                hb_deadline = Instant::now() + cfg.heartbeat_timeout;
            }
        }
    }

    emit(&monitor.snapshot(SnapshotKind::Summary, &cfg));
}

/// Which watchdog deadlines an observed event resets. `data` always implies
/// `liveness` (a data frame proves both are alive), so only the three named
/// combinations occur.
#[derive(Debug, Clone, Copy)]
struct Reset {
    data: bool,
    liveness: bool,
}

impl Reset {
    /// A market-data frame: resets both the data and the liveness watchdog.
    const DATA: Self = Self {
        data: true,
        liveness: true,
    };
    /// A heartbeat or (re)connect: proves the socket is alive, but carried no
    /// data.
    const LIVENESS: Self = Self {
        data: false,
        liveness: true,
    };
    /// A disconnect, failed connect, or ack: resets neither watchdog.
    const NONE: Self = Self {
        data: false,
        liveness: false,
    };
}

// ===========================================================================
// Output model — the typed shape of every stderr JSONL line. Serializing these
// *is* the output contract, so there is no hand-built JSON to drift from it.
// ===========================================================================

/// The kind of snapshot emitted to stderr.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum SnapshotKind {
    /// Periodic in-window snapshot (carries `fps`).
    Health,
    /// Final cumulative snapshot at end of stream.
    Summary,
}

/// One-word connection-health verdict, derived from `connected` and how long
/// since the last liveness signal. Lets a single line read as
/// healthy/stalled/down without a consumer cross-referencing `connected`
/// against `last_liveness_age_s`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    /// Connected, with a liveness signal within `heartbeat_timeout`.
    Healthy,
    /// Connected, but silent for longer than `heartbeat_timeout` — a half-open
    /// / stalled socket the transport hasn't yet torn down.
    Stalled,
    /// No socket connected (dropped, or never came up).
    Down,
}

/// One JSONL diagnostic line: the cumulative reliability picture plus a
/// per-channel breakdown.
#[derive(Debug, Serialize)]
struct Snapshot {
    /// Control-plane tag marking this as a `monitor`-produced line. Distinct
    /// from the data frames' `channel` (a real market-data channel), so a
    /// consumer routes a line by `event` (diagnostics) vs `channel` (data)
    /// without the two ever colliding.
    event: &'static str,
    /// Whether this is a periodic `health` line or the final `summary`.
    #[serde(rename = "type")]
    kind: SnapshotKind,
    /// Instant of this line, taken as `Utc::now()` when the snapshot is built.
    ts: DateTime<Utc>,
    /// Seconds the monitor has been running: `now − started`, rounded to 2 dp.
    uptime_s: f64,
    /// One-word verdict derived from `connected` and `last_liveness_age_s`:
    /// `down` if no socket is up, else `stalled` if the last liveness
    /// signal is older than `heartbeat_timeout`, else `healthy`.
    status: Status,
    /// Whether any endpoint currently holds a live socket (the OR across
    /// per-endpoint state).
    connected: bool,
    /// Reconnects across all endpoints: the sum of `(connects − 1)` per
    /// endpoint — every connect past each socket's first.
    reconnects: u64,
    /// Established sockets that later dropped: the count of `Disconnected`
    /// events.
    disconnects: u64,
    /// Connect attempts that failed before a socket came up: the count of
    /// `ConnectFailed` events. Stays 0 on a healthy feed; a climbing value
    /// with `connected: false` is the "no stream appears" signal.
    connect_failures: u64,
    /// Subscribe acknowledgements: the count of `Ack` events.
    subscribe_acks: u64,
    /// Subscribe / API errors: the count of `ApiError` events.
    subscribe_errors: u64,
    /// Frames the broadcast ring overwrote before this consumer read them: the
    /// sum of the `Lagged(n)` overflow counts.
    local_drops: u64,
    /// Frame timestamps that failed RFC3339 parsing (each is excluded from
    /// latency).
    parse_errors: u64,
    /// Inbound frames the connection dropped as unparseable (schema drift);
    /// they reach no sink, so this count is the only place a monitored
    /// session surfaces them.
    frames_unparsed: u64,
    /// Trade-id discontinuities observed: the number of per-symbol id jumps
    /// greater than one, across both gap kinds below.
    trade_gap_events: u64,
    /// Trades implied missing *across a reconnect*: the sum of `(id − prev_id −
    /// 1)` for jumps whose mark was set in an earlier connection epoch —
    /// outage loss the replayed snapshot didn't recover, not an in-band
    /// data fault.
    reconnect_gap_trades: u64,
    /// Trades implied missing *within one connection epoch*: the sum of `(id −
    /// prev_id − 1)` for jumps with no intervening reconnect — a genuine
    /// in-band drop, the count that signals a real data-quality problem.
    stream_gap_trades: u64,
    /// Seconds since the last market-data frame: `now − last_data`, rounded to
    /// 2 dp; `null` until the first frame.
    last_data_age_s: Option<f64>,
    /// Seconds since the last liveness signal (a data frame, heartbeat, or
    /// connect): `now − last_liveness`, rounded to 2 dp; `null` until the
    /// first.
    last_liveness_age_s: Option<f64>,
    /// Most recent ack's server-side processing time — `time_out − time_in`
    /// from its wire timestamps, in ms; `null` until an ack carries timing.
    server_latency_ms: Option<f64>,
    /// Most recent client/exchange clock offset — `recv_wall − time_out` at ack
    /// receipt, in ms; `null` until an ack carries timing.
    clock_offset_ms: Option<f64>,
    /// Per-endpoint connection breakdown, keyed by endpoint name (`auth` / `l3`
    /// / `public`), so a socket that is down or failing is visible even
    /// when the global `status` reads healthy off another endpoint.
    endpoints: BTreeMap<String, EndpointHealth>,
    /// Per-channel breakdown, ordered by channel name.
    channels: BTreeMap<String, ChannelHealth>,
}

/// Per-endpoint entry in a [`Snapshot`]. The global `status`/`connected`
/// collapse every socket into one verdict; this breakdown keeps a socket that
/// is down or failing to connect visible even when another endpoint keeps the
/// global picture healthy.
#[derive(Debug, Serialize)]
struct EndpointHealth {
    /// Whether this endpoint currently holds a live socket.
    connected: bool,
    /// Successful connects on this endpoint.
    connects: u64,
    /// Reconnects on this endpoint: `connects` past the first.
    reconnects: u64,
    /// Established sockets on this endpoint that later dropped.
    disconnects: u64,
    /// Connect attempts on this endpoint that failed before a socket came up.
    connect_failures: u64,
    /// Whether the endpoint gave up for good mid-session (its capture ended
    /// early).
    aborted: bool,
}

/// Per-channel entry in a [`Snapshot`].
#[skip_serializing_none]
#[derive(Debug, Serialize)]
struct ChannelHealth {
    /// Cumulative frames seen on this channel since the monitor started.
    frames: u64,
    /// Frames per second over the last health window: the window's frame count
    /// divided by `health_interval` seconds, rounded to 2 dp. Present only
    /// on `health` lines.
    fps: Option<f64>,
    /// Latency distribution for the channel, when its frames carry a network
    /// timestamp; absent otherwise.
    latency_ms: Option<LatencySummary>,
}

/// Compact latency distribution (ms) for one channel. Each sample is
/// `recv_wall_clock − frame_event_timestamp`.
#[derive(Debug, Serialize)]
struct LatencySummary {
    /// Number of latency samples.
    n: u64,
    /// Approximate median (computed as for `p95`).
    p50: f64,
    /// Approximate 95th percentile: the upper bound of the log-spaced histogram
    /// bucket the cumulative sample count crosses, clamped to the observed
    /// `max` (never above it).
    p95: f64,
    /// Approximate 99th percentile (computed as for `p95`).
    p99: f64,
    /// Smallest observed sample (may be negative when the client clock runs
    /// behind the exchange).
    min: f64,
    /// Largest observed sample.
    max: f64,
    /// Arithmetic mean: sum of samples divided by `n`.
    mean: f64,
}

/// Which watchdog raised an alert. Serializes to a stable, machine-routable
/// tag.
///
/// The shared `No` prefix is intentional — each variant names an *absence* —
/// and the resulting `no_data` / `no_heartbeat` / `no_connection` wire tags are
/// a deliberate part of the stderr contract, so we keep them over clippy's
/// suggestion to drop the common prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::enum_variant_names)]
enum AlertKind {
    /// No market-data frame within `stale_after` — a quiet market or a stalled
    /// stream.
    NoData,
    /// No heartbeat within `heartbeat_timeout` after at least one connect — the
    /// socket appears stalled.
    NoHeartbeat,
    /// No socket has come up yet — the endpoint is unreachable or the network
    /// is down.
    NoConnection,
}

/// One JSONL alert line: a watchdog tripped. Shares the `event: "monitor"` tag
/// with [`Snapshot`] so every monitor line carries the same discriminator;
/// `type` is the constant `"alert"` (mirroring the snapshot's
/// `health`/`summary`), and `alert` names which watchdog fired. Built by the
/// pure [`stale_data_alert`]/[`liveness_alert`] functions and written by
/// [`emit`], so an alert is a typed value with no hand-built JSON to drift.
#[skip_serializing_none]
#[derive(Debug, Serialize)]
struct Alert {
    /// Control-plane tag marking this as a `monitor`-produced line (see
    /// [`Snapshot::event`]).
    event: &'static str,
    /// Always `"alert"` — distinguishes this from the `health`/`summary`
    /// snapshots.
    #[serde(rename = "type")]
    kind: &'static str,
    /// Which watchdog fired.
    alert: AlertKind,
    /// Instant of this line, taken as `Utc::now()` when the alert is built.
    ts: DateTime<Utc>,
    /// Human-readable detail naming which watchdog fired and the threshold it
    /// crossed.
    message: String,
    /// Failed connect attempts so far — present only on a `no_connection`
    /// alert.
    connect_failures: Option<u64>,
}

impl Alert {
    fn new(alert: AlertKind, message: String, connect_failures: Option<u64>) -> Self {
        Self {
            event: "monitor",
            kind: "alert",
            alert,
            ts: Utc::now(),
            message,
            connect_failures,
        }
    }
}

/// Build the data-staleness alert: no market-data frame within `stale_after`.
fn stale_data_alert(stale_after: Duration) -> Alert {
    Alert::new(
        AlertKind::NoData,
        format!(
            "no market data for {}s — quiet market or a stalled stream",
            stale_after.as_secs()
        ),
        None,
    )
}

/// Build the liveness watchdog's alert for the current connection phase, or
/// `None` when staying silent is correct. Before the first successful connect
/// the feed never came up (unreachable host / network down — reported with the
/// failed-attempt count). While a socket is connected but silent past the
/// timeout, that is a genuine stall. Between an observed disconnect and the
/// next reconnect (`connected == false` with `connects > 0`) it returns `None`:
/// the session is knowingly down, so a `no_heartbeat` "stalled socket" alert
/// would be false — a stall is by definition still connected — and `status:
/// down` plus the `no_data` watchdog already surface the outage.
fn liveness_alert(
    connected: bool,
    total_connects: u64,
    connect_failures: u64,
    timeout: Duration,
) -> Option<Alert> {
    if total_connects == 0 {
        Some(Alert::new(
            AlertKind::NoConnection,
            format!(
                "no connection established after {}s ({connect_failures} failed attempt(s)) — \
                 endpoint unreachable or network down",
                timeout.as_secs()
            ),
            Some(connect_failures),
        ))
    } else if connected {
        Some(Alert::new(
            AlertKind::NoHeartbeat,
            format!(
                "no heartbeat for {}s — connection appears stalled",
                timeout.as_secs()
            ),
            None,
        ))
    } else {
        None
    }
}

/// Per-endpoint connection state. `streamd`/`record` open one socket per
/// endpoint (public data plus an authenticated socket), so liveness is tracked
/// per endpoint and the global picture derived from all of them — a drop on one
/// endpoint doesn't read as the whole stream going down while another is still
/// delivering data.
#[derive(Debug, Default, Clone, Copy)]
struct EndpointState {
    /// Whether this endpoint currently has a live socket.
    connected: bool,
    /// Successful connects on this endpoint; the count past the first are its
    /// reconnects.
    connects: u64,
    /// Established sockets on this endpoint that later dropped.
    disconnects: u64,
    /// Connect attempts on this endpoint that failed before a socket came up.
    connect_failures: u64,
    /// The endpoint gave up for good ([`Event::Aborted`]); no reconnect
    /// follows.
    aborted: bool,
}

/// Per-symbol trade-id state: the high-water mark and the connection epoch it
/// was last seen in. The epoch (a connect counter) lets a gap be attributed to
/// a reconnect — the mark was set in an earlier epoch, so the missing ids
/// straddle an outage — vs an in-stream drop.
#[derive(Debug, Clone, Copy)]
struct TradeSeq {
    id: u64,
    epoch: u64,
}

/// Bounded, allocation-light accumulator over the event stream.
///
/// Per-channel maps are keyed on the `Copy` [`Channel`] (a small fixed
/// catalogue) rather than its name, so folding a frame allocates nothing for
/// its key. Symbol-keyed maps stay `String`: symbols are unbounded and borrowed
/// from a frame that is dropped immediately.
#[derive(Debug, Default)]
struct Monitor {
    /// Start instant; `uptime_s` is measured from it.
    started: StartInstant,
    /// Per-channel latency histogram, feeding each channel's `latency_ms`.
    latency: HashMap<Channel, LatencyHist>,
    /// Cumulative frame count per channel, reported as each channel's `frames`.
    frames_total: HashMap<Channel, u64>,
    /// Frames per channel since the last health snapshot; divided by the
    /// interval for `fps`, then cleared each `health` line.
    frames_in_window: HashMap<Channel, u64>,
    /// Per-symbol trade-id high-water mark and the epoch it was set in; drives
    /// gap detection.
    last_trade_id: HashMap<String, TradeSeq>,
    /// Trade-id jumps greater than one, reported as `trade_gap_events`.
    trade_gap_events: u64,
    /// Missing trades summed from jumps that straddle a reconnect, reported as
    /// `reconnect_gap_trades`.
    reconnect_gap_trades: u64,
    /// Missing trades summed from jumps within one epoch, reported as
    /// `stream_gap_trades`.
    stream_gap_trades: u64,
    /// Per-endpoint connection state; every global connection figure
    /// (`connected`, `reconnects`, `total_connects`, `disconnects`,
    /// `connect_failures`), each channel's trade-gap epoch, and the
    /// per-endpoint `endpoints` breakdown are all derived from it.
    endpoints: HashMap<Endpoint, EndpointState>,
    /// Count of `Ack` events, reported as `subscribe_acks`.
    subscribe_acks: u64,
    /// Count of `ApiError` events, reported as `subscribe_errors`.
    subscribe_errors: u64,
    /// Sum of broadcast `Lagged(n)` overflow counts, reported as `local_drops`.
    local_drops: u64,
    /// Timestamps that failed RFC3339 parsing, reported as `parse_errors`.
    parse_errors: u64,
    /// `ParseFailure` events: frames dropped at the connection, reported as
    /// `frames_unparsed`.
    frames_unparsed: u64,
    /// Instant of the last market-data frame; `last_data_age_s` is measured
    /// from it.
    last_data: Option<Instant>,
    /// Instant of the last liveness signal (data frame, heartbeat, or connect);
    /// both `last_liveness_age_s` and `status` are measured from it.
    last_liveness: Option<Instant>,
    /// Most recent ack's `time_out − time_in`, reported as `server_latency_ms`.
    last_server_latency_ms: Option<f64>,
    /// Most recent ack's `recv_wall − time_out`, reported as `clock_offset_ms`.
    last_clock_offset_ms: Option<f64>,
}

impl Monitor {
    /// Fold one event into the accumulator, returning which watchdog deadlines
    /// it resets. `wall` is the local receive instant (wall clock),
    /// captured once by the caller.
    fn observe(&mut self, event: Event, wall: DateTime<Utc>) -> Reset {
        match event {
            Event::Message(frame) => self.observe_message(&frame, wall),
            Event::Heartbeat => Reset::LIVENESS,
            Event::Connected(endpoint) => {
                let state = self.endpoints.entry(endpoint).or_default();
                state.connected = true;
                state.connects += 1;
                Reset::LIVENESS
            }
            Event::Disconnected(endpoint) => {
                let state = self.endpoints.entry(endpoint).or_default();
                state.connected = false;
                state.disconnects += 1;
                Reset::NONE
            }
            // A failed attempt is the opposite of liveness: counted, but resetting neither
            // watchdog, so the heartbeat alert keeps firing while the feed is down.
            Event::ConnectFailed(endpoint) => {
                self.endpoints.entry(endpoint).or_default().connect_failures += 1;
                Reset::NONE
            }
            // A dropped inbound frame is a capture hole, not liveness: the actor
            // deliberately skips its idle reset for unparseable frames, and the
            // watchdogs mirror that so an all-junk feed still alerts.
            Event::ParseFailure(_) => {
                self.frames_unparsed += 1;
                Reset::NONE
            }
            // Terminal give-up: no reconnect follows, so the endpoint must read as
            // down-for-good in every later summary, not merely disconnected.
            Event::Aborted { endpoint, .. } => {
                let state = self.endpoints.entry(endpoint).or_default();
                state.connected = false;
                state.aborted = true;
                Reset::NONE
            }
            Event::Ack(resp) => {
                self.subscribe_acks += 1;
                self.record_ack_timing(&resp, wall);
                Reset::NONE
            }
            Event::ApiError(_) => {
                self.subscribe_errors += 1;
                Reset::NONE
            }
        }
    }

    /// Fold a local broadcast overflow (`Lagged(dropped)`) in. The overflow
    /// proves the feed is alive, so it refreshes the data and liveness
    /// instants (keeping `status` and the ages consistent with the
    /// watchdogs the caller re-arms). It also broke this consumer's
    /// continuity, so it forgets the per-symbol trade-id marks: the next trade
    /// re-seeds instead of being misread as an in-stream gap the primary
    /// sink never dropped.
    fn note_lag(&mut self, dropped: u64, now: Instant) {
        self.local_drops += dropped;
        self.last_data = Some(now);
        self.last_liveness = Some(now);
        self.last_trade_id.clear();
    }

    /// Fold one channel-data frame in: count it, measure latency on live
    /// frames, and track trade-id gaps.
    fn observe_message(&mut self, frame: &ChannelMessage, wall: DateTime<Utc>) -> Reset {
        let channel = frame.channel();
        *self.frames_total.entry(channel).or_default() += 1;
        *self.frames_in_window.entry(channel).or_default() += 1;

        // Latency from live `update` frames only. A `snapshot` replays historical data
        // on subscribe and every reconnect, so its timestamps are backfill age,
        // not network transit; OHLC carries the candle-start time and `Other`
        // carries none.
        if !frame.is_snapshot() {
            match &frame.body {
                ChannelData::Ticker(data) => {
                    self.record_latencies(channel, data.iter().map(|t| t.timestamp.as_str()), wall)
                }
                ChannelData::Trade(data) => {
                    self.record_latencies(channel, data.iter().map(|t| t.timestamp.as_str()), wall)
                }
                ChannelData::Book(data) => {
                    self.record_latencies(channel, data.iter().map(|b| b.timestamp.as_str()), wall)
                }
                // OHLC carries the candle-start time, not a transit timestamp; the
                // reference/account/system channels are not latency-bearing market data.
                ChannelData::Ohlc(_)
                | ChannelData::Instrument(_)
                | ChannelData::Executions(_)
                | ChannelData::Balances(_)
                | ChannelData::Level3(_)
                | ChannelData::Status(_) => {}
            }
        }

        // Trade-id gaps, on every trade frame including snapshots — the snapshot seeds
        // each symbol's high-water mark so the first live update isn't mistaken
        // for a gap. Gaps are classified against the *trade socket's* own
        // reconnect count, so a reconnect on an unrelated endpoint can't
        // recolour an in-stream drop as outage loss.
        if let ChannelData::Trade(data) = &frame.body {
            let epoch = self.channel_epoch(channel);
            for t in data {
                self.check_trade_gap(&t.symbol, t.trade_id, epoch);
            }
        }

        Reset::DATA
    }

    /// The reconnect generation of the socket that serves `channel`: connects
    /// seen on its endpoint. Trade-id gaps stamp and compare against this
    /// so a reconnect on a different endpoint never shifts a channel's gap
    /// classification.
    fn channel_epoch(&self, channel: Channel) -> u64 {
        let endpoint = match channel {
            Channel::Subscribable(sub) => sub.endpoint(),
            // System feeds ride the public socket.
            Channel::Status | Channel::Heartbeat => Endpoint::Public,
        };
        self.endpoints.get(&endpoint).map_or(0, |e| e.connects)
    }

    /// Fold each frame timestamp's latency (`wall − event_ts`) into the channel
    /// histogram; a timestamp that won't parse is counted in
    /// `parse_errors`, never fatal.
    fn record_latencies<'a>(
        &mut self,
        channel: Channel,
        timestamps: impl Iterator<Item = &'a str>,
        wall: DateTime<Utc>,
    ) {
        // The entry is resolved once; errors tally locally so the histogram borrow and
        // the counter don't fight over `self`.
        let mut parse_errors = 0;
        let histogram = self.latency.entry(channel).or_default();
        for ts in timestamps {
            match latency_ms(ts, wall) {
                Some(ms) => histogram.record(ms),
                None => parse_errors += 1,
            }
        }
        self.parse_errors += parse_errors;
    }

    /// Flag a gap when a symbol's `trade_id` (a per-symbol sequence number)
    /// jumps by more than one, attributing the missing trades to either a
    /// reconnect or an in-stream drop. `epoch` is the trade socket's
    /// connect count: if the mark was last set in an earlier epoch, the gap
    /// straddles a reconnect (outage loss, minus what the snapshot replayed);
    /// otherwise it is an in-band drop. Duplicate/replayed ids (≤ the mark)
    /// are not gaps and never lower it.
    fn check_trade_gap(&mut self, symbol: &str, id: u64, epoch: u64) {
        if let Some(prev) = self.last_trade_id.get(symbol).copied()
            && id > prev.id.saturating_add(1)
        {
            let missing = id - prev.id - 1;
            self.trade_gap_events += 1;
            if prev.epoch < epoch {
                self.reconnect_gap_trades += missing;
            } else {
                self.stream_gap_trades += missing;
            }
        }
        // Advance the mark only on a new high-water id, moving its epoch with it. A
        // duplicate or lower id (a reconnect snapshot replaying trades already
        // seen) must leave the mark in the epoch it was genuinely last seen in
        // — bumping the epoch on a replay would reclassify the next real jump
        // as an in-stream drop when it actually straddled the outage.
        let mark = self
            .last_trade_id
            .entry(symbol.to_string())
            .or_insert(TradeSeq { id, epoch });
        if id > mark.id {
            mark.id = id;
            mark.epoch = epoch;
        }
    }

    /// Derive a server-side processing latency (`time_out − time_in`) and a
    /// clock-offset estimate (`wall − time_out`) from an ack's wire
    /// timestamps, when present.
    fn record_ack_timing(&mut self, resp: &MethodResponse, wall: DateTime<Utc>) {
        let (Some(tin), Some(tout)) = (resp.time_in.as_deref(), resp.time_out.as_deref()) else {
            return;
        };
        let (Some(time_in), Some(time_out)) = (parse_rfc3339(tin), parse_rfc3339(tout)) else {
            return;
        };
        self.last_server_latency_ms = millis_between(time_in, time_out);
        self.last_clock_offset_ms = millis_between(time_out, wall);
    }

    fn any_connected(&self) -> bool {
        self.endpoints.values().any(|e| e.connected)
    }

    /// Connects summed across all endpoints; `0` means no socket has ever come
    /// up.
    fn total_connects(&self) -> u64 {
        self.endpoints.values().map(|e| e.connects).sum()
    }

    /// Established sockets that later dropped, summed across all endpoints.
    fn disconnects(&self) -> u64 {
        self.endpoints.values().map(|e| e.disconnects).sum()
    }

    /// Connect attempts that failed before a socket came up, summed across all
    /// endpoints.
    fn connect_failures(&self) -> u64 {
        self.endpoints.values().map(|e| e.connect_failures).sum()
    }

    /// Reconnects across every endpoint: connects past the first on each.
    fn reconnects(&self) -> u64 {
        self.endpoints
            .values()
            .map(|e| e.connects.saturating_sub(1))
            .sum()
    }

    /// Classify connection health from whether any socket is up and the
    /// (already-rounded) seconds since the last liveness signal — the same
    /// age shown in the snapshot, so the verdict and the number always
    /// agree.
    fn status(connected: bool, last_liveness_age_s: Option<f64>, cfg: &MonitorConfig) -> Status {
        if !connected {
            return Status::Down;
        }
        match last_liveness_age_s {
            Some(age) if age > cfg.heartbeat_timeout.as_secs_f64() => Status::Stalled,
            _ => Status::Healthy,
        }
    }

    /// Build a `health` or `summary` snapshot for stderr. Pure (no I/O), so the
    /// output shape is unit-testable on the typed value.
    fn snapshot(&self, kind: SnapshotKind, cfg: &MonitorConfig) -> Snapshot {
        let now = Instant::now();
        let connected = self.any_connected();
        let last_data_age_s = self.last_data.map(|t| round2((now - t).as_secs_f64()));
        let last_liveness_age_s = self.last_liveness.map(|t| round2((now - t).as_secs_f64()));
        let channels = self
            .frames_total
            .iter()
            .map(|(channel, &frames)| {
                // `fps` is a per-window rate, meaningful only on periodic health lines.
                let fps = matches!(kind, SnapshotKind::Health).then(|| {
                    let in_window = self.frames_in_window.get(channel).copied().unwrap_or(0);
                    round2(in_window as f64 / cfg.health_interval.as_secs_f64())
                });
                // Sample-less histograms (every timestamp failed to parse, or an empty
                // data array resolved the entry) must render as absent, not as zeros —
                // the field's contract is "absent unless measured".
                let latency_ms = self
                    .latency
                    .get(channel)
                    .filter(|hist| hist.count > 0)
                    .map(LatencySummary::from);
                // Render the `Channel` to its wire name only here, once per snapshot — not per
                // frame on the hot path.
                (
                    channel.to_string(),
                    ChannelHealth {
                        frames,
                        fps,
                        latency_ms,
                    },
                )
            })
            .collect();
        let endpoints = self
            .endpoints
            .iter()
            .map(|(endpoint, state)| {
                (
                    endpoint.to_string(),
                    EndpointHealth {
                        connected: state.connected,
                        connects: state.connects,
                        reconnects: state.connects.saturating_sub(1),
                        disconnects: state.disconnects,
                        connect_failures: state.connect_failures,
                        aborted: state.aborted,
                    },
                )
            })
            .collect();
        Snapshot {
            event: "monitor",
            kind,
            ts: Utc::now(),
            uptime_s: round2(self.started.elapsed().as_secs_f64()),
            status: Self::status(connected, last_liveness_age_s, cfg),
            connected,
            reconnects: self.reconnects(),
            disconnects: self.disconnects(),
            connect_failures: self.connect_failures(),
            subscribe_acks: self.subscribe_acks,
            subscribe_errors: self.subscribe_errors,
            local_drops: self.local_drops,
            parse_errors: self.parse_errors,
            frames_unparsed: self.frames_unparsed,
            trade_gap_events: self.trade_gap_events,
            reconnect_gap_trades: self.reconnect_gap_trades,
            stream_gap_trades: self.stream_gap_trades,
            last_data_age_s,
            last_liveness_age_s,
            server_latency_ms: self.last_server_latency_ms.map(round2),
            clock_offset_ms: self.last_clock_offset_ms.map(round2),
            endpoints,
            channels,
        }
    }
}

/// Per-channel latency histogram: exact count/sum/min/max plus log-spaced
/// buckets for approximate percentiles, all in milliseconds. Negative values (a
/// client clock running behind the exchange) are kept as-is in `min` — that
/// skew is itself a useful signal.
#[derive(Clone, Debug, Default)]
struct LatencyHist {
    count: u64,
    sum_ms: f64,
    min_ms: f64,
    max_ms: f64,
    /// One slot per bound in [`LAT_BUCKET_BOUNDS_MS`], plus a trailing overflow
    /// slot. The first recorded sample overwrites the zero-initialised
    /// `min_ms`/`max_ms`.
    buckets: [u64; LAT_BUCKET_BOUNDS_MS.len() + 1],
}

/// Upper bounds (ms) of the latency histogram buckets.
const LAT_BUCKET_BOUNDS_MS: [f64; 13] = [
    1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0,
];

impl LatencyHist {
    fn record(&mut self, ms: f64) {
        if self.count == 0 {
            self.min_ms = ms;
            self.max_ms = ms;
        } else {
            self.min_ms = self.min_ms.min(ms);
            self.max_ms = self.max_ms.max(ms);
        }
        self.count += 1;
        self.sum_ms += ms;
        let idx = LAT_BUCKET_BOUNDS_MS
            .iter()
            .position(|&bound| ms <= bound)
            .unwrap_or(LAT_BUCKET_BOUNDS_MS.len());
        self.buckets[idx] += 1;
    }

    /// Approximate `q`-quantile (0.0–1.0): the upper bound of the bucket the
    /// cumulative count crosses (overflow reads as the max), clamped to the
    /// observed max — a coarse bucket bound can exceed the largest sample,
    /// and an estimate above `max` reads as broken telemetry to anything
    /// routing on the health line.
    fn quantile(&self, q: f64) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let target = (q * self.count as f64).ceil() as u64;
        let mut cum = 0u64;
        for (i, &c) in self.buckets.iter().enumerate() {
            cum += c;
            if cum >= target {
                return LAT_BUCKET_BOUNDS_MS
                    .get(i)
                    .copied()
                    .unwrap_or(self.max_ms)
                    .min(self.max_ms);
            }
        }
        self.max_ms
    }

    fn mean_ms(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.sum_ms / self.count as f64
        }
    }
}

impl From<&LatencyHist> for LatencySummary {
    fn from(h: &LatencyHist) -> Self {
        Self {
            n: h.count,
            p50: round2(h.quantile(0.50)),
            p95: round2(h.quantile(0.95)),
            p99: round2(h.quantile(0.99)),
            min: round2(h.min_ms),
            max: round2(h.max_ms),
            mean: round2(h.mean_ms()),
        }
    }
}

/// Newtype so [`Monitor`] can `#[derive(Default)]` while `started` defaults to
/// "now".
#[derive(Debug)]
struct StartInstant(Instant);

impl StartInstant {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }
}

impl Default for StartInstant {
    fn default() -> Self {
        Self(Instant::now())
    }
}

fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Milliseconds from `from` to `to` (may be negative), or `None` on overflow.
fn millis_between(from: DateTime<Utc>, to: DateTime<Utc>) -> Option<f64> {
    Some((to - from).num_microseconds()? as f64 / 1000.0)
}

/// Latency in milliseconds of a frame whose event timestamp is `event_ts`,
/// observed at `now` (may be negative under clock skew).
fn latency_ms(event_ts: &str, now: DateTime<Utc>) -> Option<f64> {
    millis_between(parse_rfc3339(event_ts)?, now)
}

/// Round to 2 decimal places for compact, readable health output.
fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Write one monitor diagnostic line — a [`Snapshot`] or an [`Alert`] — as
/// JSONL to stderr (stdout is reserved for stream data). The value's
/// `Serialize` impl *is* the output contract, so there is no hand-built JSON to
/// drift from it.
#[expect(
    clippy::expect_used,
    reason = "Snapshot and Alert are plain derived-Serialize data; serialization cannot fail"
)]
fn emit<T: Serialize>(line: &T) {
    eprintln!(
        "{}",
        serde_json::to_string(line)
            .expect("monitor diagnostics are plain data and always serialize")
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use kraken_core::endpoint::Endpoint;

    fn msg(raw: &str) -> Event {
        Event::Message(ChannelMessage::parse(raw).expect("frame parses"))
    }

    /// The `Channel` key for a wire channel name, for probing the per-channel
    /// maps.
    fn ch(name: &str) -> Channel {
        name.parse().expect("channel name parses")
    }

    fn trade(symbol: &str, id: u64) -> Event {
        msg(&format!(
            r#"{{"channel":"trade","type":"update","data":[{{"symbol":"{symbol}","side":"buy",
               "price":1.0,"qty":1.0,"ord_type":"market","trade_id":{id},
               "timestamp":"2026-01-01T00:00:00.000000Z"}}]}}"#
        ))
    }

    fn trade_snapshot(symbol: &str, id: u64) -> Event {
        msg(&format!(
            r#"{{"channel":"trade","type":"snapshot","data":[{{"symbol":"{symbol}","side":"buy",
               "price":1.0,"qty":1.0,"ord_type":"market","trade_id":{id},
               "timestamp":"2026-01-01T00:00:00.000000Z"}}]}}"#
        ))
    }

    #[test]
    fn counts_frames_per_channel() {
        let mut m = Monitor::default();
        let now = Utc::now();
        m.observe(trade("BTC/USD", 1), now);
        m.observe(trade("BTC/USD", 2), now);
        m.observe(
            msg(
                r#"{"channel":"ticker","type":"update","data":[{"symbol":"BTC/USD","bid":1.0,
                "bid_qty":1.0,"ask":2.0,"ask_qty":1.0,"last":1.5,"volume":1.0,"vwap":1.0,"low":1.0,
                "high":2.0,"change":0.0,"change_pct":0.0,"timestamp":"2026-01-01T00:00:00Z"}]}"#,
            ),
            now,
        );
        assert_eq!(m.frames_total.get(&ch("trade")), Some(&2));
        assert_eq!(m.frames_total.get(&ch("ticker")), Some(&1));
    }

    #[test]
    fn flags_trade_id_gaps_and_ignores_duplicates() {
        let mut m = Monitor::default();
        let now = Utc::now();
        m.observe(trade("BTC/USD", 10), now); // first: establishes the mark
        m.observe(trade("BTC/USD", 11), now); // contiguous: no gap
        m.observe(trade("BTC/USD", 15), now); // jump of 4 → 3 missing
        m.observe(trade("BTC/USD", 15), now); // duplicate: not a gap, mark unchanged
        m.observe(trade("BTC/USD", 13), now); // replayed lower id: not a gap
        assert_eq!(m.trade_gap_events, 1);
        // No reconnect happened, so the gap is in-band, not outage loss.
        assert_eq!(m.stream_gap_trades, 3);
        assert_eq!(m.reconnect_gap_trades, 0);
    }

    #[test]
    fn a_reconnect_on_another_endpoint_does_not_mask_an_in_stream_trade_drop() {
        // Trades ride the public socket. An auth reconnect must not recolour a public
        // in-stream drop as outage loss — gap classification keys on the trade
        // socket's own reconnects.
        let mut m = Monitor::default();
        let now = Utc::now();
        m.observe(Event::Connected(Endpoint::Public), now);
        m.observe(Event::Connected(Endpoint::Auth), now);
        m.observe(trade("BTC/USD", 10), now);
        m.observe(trade("BTC/USD", 11), now);

        // The auth socket drops and reconnects; the public trade stream never broke.
        m.observe(Event::Disconnected(Endpoint::Auth), now);
        m.observe(Event::Connected(Endpoint::Auth), now);

        m.observe(trade("BTC/USD", 15), now); // 3 missing, entirely within the live public stream
        assert_eq!(m.trade_gap_events, 1);
        assert_eq!(
            m.stream_gap_trades, 3,
            "an unrelated auth reconnect must not hide the in-stream drop"
        );
        assert_eq!(m.reconnect_gap_trades, 0);
    }

    #[test]
    fn a_local_lag_does_not_manufacture_a_trade_gap() {
        // Frames the broadcast ring overwrites are a drop local to this consumer; the
        // exchange stream is contiguous, so the next trade's id jump must not
        // be booked as an in-stream gap (the metric that signals a real
        // data-quality problem).
        let mut m = Monitor::default();
        let now = Utc::now();
        m.observe(Event::Connected(Endpoint::Public), now);
        m.observe(trade("BTC/USD", 10), now);
        m.observe(trade("BTC/USD", 11), now);

        m.note_lag(500, Instant::now()); // the ring overwrote frames we never observed

        m.observe(trade("BTC/USD", 812), now); // big id jump — but it is our lag, not a drop
        assert_eq!(m.trade_gap_events, 0, "a local lag is not an in-stream gap");
        assert_eq!(m.stream_gap_trades, 0);
        assert_eq!(m.local_drops, 500);
    }

    #[test]
    fn a_local_lag_refreshes_liveness_so_status_is_not_stalled() {
        // A stale liveness instant would read as `stalled`, but a lag proves the feed
        // is alive and re-arms the watchdogs, so it must refresh the instant
        // the snapshot reads.
        let cfg = MonitorConfig::new(10, 30, 5); // heartbeat_timeout = 5s
        let mut m = Monitor::default();
        m.observe(Event::Connected(Endpoint::Public), Utc::now());
        m.last_liveness = Some(Instant::now() - Duration::from_secs(60));

        m.note_lag(1, Instant::now());

        assert_eq!(
            m.snapshot(SnapshotKind::Health, &cfg).status,
            Status::Healthy
        );
    }

    #[test]
    fn gaps_are_tracked_per_symbol() {
        let mut m = Monitor::default();
        let now = Utc::now();
        m.observe(trade("BTC/USD", 1), now);
        m.observe(trade("ETH/USD", 100), now);
        m.observe(trade("BTC/USD", 2), now); // contiguous for BTC
        m.observe(trade("ETH/USD", 105), now); // gap of 4 for ETH
        assert_eq!(m.trade_gap_events, 1);
        assert_eq!(m.stream_gap_trades, 4);
    }

    #[test]
    fn gap_across_a_reconnect_is_outage_loss_not_an_in_stream_drop() {
        let mut m = Monitor::default();
        let now = Utc::now();
        m.observe(Event::Connected(Endpoint::Public), now); // epoch 1
        m.observe(trade("BTC/USD", 10), now); // seeds the mark in epoch 1
        m.observe(trade("BTC/USD", 11), now); // contiguous

        // A reconnect advances the epoch; the replayed snapshot's first trade jumps the
        // id. Those missing ids straddle the outage, so they are reconnect
        // loss, not an in-band drop.
        m.observe(Event::Disconnected(Endpoint::Public), now);
        m.observe(Event::Connected(Endpoint::Public), now); // epoch 2 (reconnect)
        m.observe(trade_snapshot("BTC/USD", 16), now); // 11 -> 16 across the reconnect

        assert_eq!(m.trade_gap_events, 1);
        assert_eq!(m.reconnect_gap_trades, 4, "12..15 lost across the outage");
        assert_eq!(m.stream_gap_trades, 0, "nothing dropped in-band");
    }

    #[test]
    fn a_reconnect_snapshot_replaying_a_duplicate_still_attributes_the_gap_to_the_outage() {
        // Regression: a reconnect snapshot that replays an already-seen id must not
        // bump the mark's epoch. Stamping the current epoch on the replay used
        // to reclassify the next real jump as an in-stream drop, when it
        // straddled the outage.
        let mut m = Monitor::default();
        let now = Utc::now();
        m.observe(Event::Connected(Endpoint::Public), now); // epoch 1
        m.observe(trade("BTC/USD", 10), now); // seeds the mark in epoch 1
        m.observe(trade("BTC/USD", 11), now); // contiguous, still epoch 1

        m.observe(Event::Disconnected(Endpoint::Public), now);
        m.observe(Event::Connected(Endpoint::Public), now); // epoch 2 (reconnect)
        m.observe(trade_snapshot("BTC/USD", 11), now); // replayed duplicate: no advance, no gap
        m.observe(trade("BTC/USD", 16), now); // 11 -> 16 across the reconnect

        assert_eq!(m.trade_gap_events, 1);
        assert_eq!(
            m.reconnect_gap_trades, 4,
            "12..15 straddle the outage even though the snapshot replayed a duplicate first"
        );
        assert_eq!(m.stream_gap_trades, 0, "nothing dropped in-band");
    }

    #[test]
    fn snapshot_frames_are_counted_and_seed_gaps_but_skip_latency() {
        // The subscribe (and per-reconnect) snapshot replays historical trades; folding
        // their stale timestamps into latency pins every percentile to the
        // backfill age. A snapshot must still count as a frame and seed the gap
        // high-water mark, just not feed latency.
        let mut m = Monitor::default();
        let now = Utc::now();
        m.observe(trade_snapshot("BTC/USD", 10), now);
        assert_eq!(
            m.frames_total.get(&ch("trade")),
            Some(&1),
            "snapshot still counts as a received frame"
        );
        assert!(
            !m.latency.contains_key(&ch("trade")),
            "snapshot timestamps must not enter the latency histogram"
        );

        // A contiguous live update is not a gap (the snapshot seeded the mark) and
        // *does* record latency.
        m.observe(trade("BTC/USD", 11), now);
        assert_eq!(m.trade_gap_events, 0, "snapshot seeded the high-water mark");
        assert_eq!(
            m.latency.get(&ch("trade")).map(|h| h.count),
            Some(1),
            "only the live update fed latency"
        );
    }

    #[test]
    fn records_latency_for_market_channels_but_not_ohlc() {
        let mut m = Monitor::default();
        let now = Utc::now();
        m.observe(trade("BTC/USD", 1), now);
        m.observe(
            msg(
                r#"{"channel":"ohlc","type":"update","data":[{"symbol":"BTC/USD","open":1.0,
                "high":2.0,"low":0.5,"close":1.5,"vwap":1.2,"trades":7,"volume":1.0,
                "interval_begin":"2026-01-01T00:00:00.000000000Z","interval":1}]}"#,
            ),
            now,
        );
        assert!(m.latency.contains_key(&ch("trade")));
        assert!(
            !m.latency.contains_key(&ch("ohlc")),
            "ohlc latency is candle-start, excluded"
        );
    }

    #[test]
    fn a_sample_less_latency_histogram_renders_absent_not_zeros() {
        // A tolerated timestamp-parse failure resolves the histogram entry without ever
        // feeding it a sample; `latency_ms` must stay absent ("absent unless
        // measured"), not serialize as an all-zeros distribution.
        let cfg = MonitorConfig::new(10, 30, 5);
        let mut m = Monitor::default();
        let now = Utc::now();
        m.observe(
            msg(
                r#"{"channel":"trade","type":"update","data":[{"symbol":"BTC/USD","side":"buy",
                   "price":1.0,"qty":1.0,"ord_type":"market","trade_id":1,
                   "timestamp":"not-a-timestamp"}]}"#,
            ),
            now,
        );
        let snap = m.snapshot(SnapshotKind::Health, &cfg);
        let health = snap.channels.get("trade").expect("channel is reported");
        assert!(health.latency_ms.is_none(), "no sample was measured");
        assert_eq!(m.parse_errors, 1, "the failed timestamp is still counted");

        // The first parseable timestamp heals it.
        m.observe(trade("BTC/USD", 2), now);
        let snap = m.snapshot(SnapshotKind::Health, &cfg);
        let health = snap.channels.get("trade").expect("channel is reported");
        assert!(
            health.latency_ms.is_some(),
            "one real sample renders the block"
        );
    }

    #[test]
    fn counts_reconnects_and_tracks_connection_state() {
        let mut m = Monitor::default();
        let now = Utc::now();
        m.observe(Event::Connected(Endpoint::Public), now); // initial connect, not a reconnect
        m.observe(Event::Disconnected(Endpoint::Public), now);
        m.observe(Event::Connected(Endpoint::Public), now); // reconnect #1
        assert_eq!(m.reconnects(), 1);
        assert_eq!(m.disconnects(), 1);
        assert!(m.any_connected());
    }

    #[test]
    fn snapshot_surfaces_a_failing_endpoint_behind_a_healthy_global_status() {
        // A global-only status collapses every socket into one verdict; the
        // per-endpoint block must keep an endpoint that never came up visible
        // even while another keeps status healthy.
        let cfg = MonitorConfig::new(10, 30, 5);
        let mut m = Monitor::default();
        let now = Utc::now();
        m.observe(Event::Connected(Endpoint::Public), now); // public is up
        m.observe(Event::ConnectFailed(Endpoint::Auth), now); // auth only ever fails to connect
        m.observe(Event::ConnectFailed(Endpoint::Auth), now);

        let snap = m.snapshot(SnapshotKind::Health, &cfg);
        assert_eq!(
            snap.status,
            Status::Healthy,
            "public keeps the global status healthy"
        );
        let auth = snap
            .endpoints
            .get("auth")
            .expect("auth endpoint is reported");
        assert!(!auth.connected);
        assert_eq!(auth.connects, 0);
        assert_eq!(
            auth.connect_failures, 2,
            "the auth socket that never came up stays visible"
        );
    }

    #[test]
    fn per_endpoint_state_keeps_the_global_picture_honest() {
        // `streamd`/`record` open a public and an authenticated socket. Two *initial*
        // connects on different endpoints are not a reconnect, and one endpoint
        // dropping while the other still streams is not a whole-stream outage.
        let mut m = Monitor::default();
        let now = Utc::now();
        m.observe(Event::Connected(Endpoint::Public), now); // initial connect #1
        m.observe(Event::Connected(Endpoint::Auth), now); // initial connect #2, different socket
        assert_eq!(
            m.reconnects(),
            0,
            "two endpoints' first connects are not reconnects"
        );
        assert!(m.any_connected());

        m.observe(Event::Disconnected(Endpoint::Auth), now);
        assert!(
            m.any_connected(),
            "the public socket is still up, so the stream is not down"
        );

        m.observe(Event::Connected(Endpoint::Auth), now); // auth reconnect
        assert_eq!(m.reconnects(), 1, "only the auth socket reconnected");

        m.observe(Event::Disconnected(Endpoint::Public), now);
        m.observe(Event::Disconnected(Endpoint::Auth), now);
        assert!(
            !m.any_connected(),
            "both endpoints down → the stream is down"
        );
    }

    #[test]
    fn an_aborted_endpoint_reads_as_down_for_good() {
        // An abort now reaches consumers (the pump degrades instead of truncating), so
        // the summary must show the endpoint as dead — connected: false plus the
        // aborted marker — while any sibling endpoint stays healthy.
        let mut m = Monitor::default();
        let now = Utc::now();
        m.observe(Event::Connected(Endpoint::Auth), now);
        m.observe(Event::Connected(Endpoint::Public), now);
        let reset = m.observe(
            Event::Aborted {
                endpoint: Endpoint::Auth,
                cause: kraken_ws::AbortCause::AllSubscriptionsRejected { reason: None },
            },
            now,
        );
        assert!(
            !reset.data && !reset.liveness,
            "a give-up proves nothing live"
        );

        let snap = m.snapshot(SnapshotKind::Summary, &MonitorConfig::new(10, 30, 5));
        let auth = &snap.endpoints["auth"];
        assert!(auth.aborted && !auth.connected);
        let public = &snap.endpoints["public"];
        assert!(
            !public.aborted && public.connected,
            "the sibling stays healthy"
        );
    }

    #[test]
    fn connect_failures_are_counted_and_reset_neither_watchdog() {
        // A feed that never comes up (unreachable host / network down): the only events
        // are failed connect attempts. They must be visible structurally —
        // `connect_failures` climbs while `connected` stays false and
        // `connects` stays 0 — so the snapshot can distinguish "the stream
        // never appeared" from a healthy, quiet feed.
        let mut m = Monitor::default();
        let now = Utc::now();
        let reset = m.observe(Event::ConnectFailed(Endpoint::Public), now);
        assert!(
            !reset.data && !reset.liveness,
            "a failed connect proves neither data nor liveness, so the watchdogs keep firing"
        );
        m.observe(Event::ConnectFailed(Endpoint::Public), now);
        assert_eq!(m.connect_failures(), 2);
        assert_eq!(m.total_connects(), 0, "a failed attempt is not a connect");
        assert!(
            !m.any_connected(),
            "a failed attempt never marks a socket connected"
        );

        let snap = m.snapshot(SnapshotKind::Summary, &MonitorConfig::new(10, 30, 5));
        assert_eq!(snap.connect_failures, 2);
        assert!(!snap.connected);
    }

    #[test]
    fn ack_timestamps_yield_server_latency() {
        let mut m = Monitor::default();
        let resp = MethodResponse {
            reply: kraken_core::MethodReply::Subscribe(None),
            success: Some(true),
            error: None,
            symbol: None,
            req_id: None,
            time_in: Some("2026-01-01T00:00:00.000000Z".into()),
            time_out: Some("2026-01-01T00:00:00.050000Z".into()),
        };
        m.observe(Event::Ack(resp), Utc::now());
        assert_eq!(m.subscribe_acks, 1);
        assert_eq!(m.last_server_latency_ms, Some(50.0));
    }

    #[test]
    fn health_snapshot_reports_channels_and_latency() {
        let mut m = Monitor::default();
        let now = Utc::now();
        m.observe(trade("BTC/USD", 1), now);
        let cfg = MonitorConfig::new(10, 30, 5);
        let snap = m.snapshot(SnapshotKind::Health, &cfg);
        assert!(matches!(snap.kind, SnapshotKind::Health));
        let trade = snap.channels.get("trade").expect("trade channel present");
        assert_eq!(trade.frames, 1);
        let latency = trade.latency_ms.as_ref().expect("latency recorded");
        assert!(latency.n >= 1);
        assert!(trade.fps.is_some(), "health line carries fps");
    }

    #[test]
    fn status_classifies_connection_health() {
        let cfg = MonitorConfig::new(10, 30, 5); // heartbeat_timeout = 5s

        assert_eq!(
            Monitor::status(false, None, &cfg),
            Status::Down,
            "never connected"
        );
        assert_eq!(
            Monitor::status(true, Some(0.1), &cfg),
            Status::Healthy,
            "fresh liveness"
        );
        assert_eq!(
            Monitor::status(true, Some(12.0), &cfg),
            Status::Stalled,
            "silent past the heartbeat timeout"
        );
        assert_eq!(
            Monitor::status(false, Some(0.1), &cfg),
            Status::Down,
            "no socket overrides a recent liveness age"
        );
    }

    #[test]
    fn latency_histogram_quantiles_track_the_distribution() {
        let mut h = LatencyHist::default();
        for _ in 0..99 {
            h.record(5.0);
        }
        h.record(900.0); // one slow sample
        assert_eq!(h.count, 100);
        assert!(h.quantile(0.50) <= 5.0);
        assert!(h.quantile(0.99) >= 5.0);
        assert_eq!(h.max_ms, 900.0);
    }

    #[test]
    fn percentile_estimates_never_exceed_the_observed_max() {
        // Both samples land in the (20, 50] bucket well below its bound: the raw
        // bucket estimate would read 50.0 against max 30.0 — impossible telemetry.
        let mut h = LatencyHist::default();
        h.record(25.0);
        h.record(30.0);
        assert_eq!(h.quantile(0.50), 30.0);
        assert_eq!(h.quantile(0.99), 30.0);
        assert_eq!(h.max_ms, 30.0);
    }

    #[test]
    fn stale_data_alert_names_the_quiet_window() {
        let a = stale_data_alert(Duration::from_secs(30));
        assert_eq!(a.event, "monitor");
        assert_eq!(a.kind, "alert");
        assert_eq!(a.alert, AlertKind::NoData);
        assert!(a.message.contains("30s"), "got: {}", a.message);
        assert!(
            a.connect_failures.is_none(),
            "a data-staleness alert carries no connect context"
        );
    }

    #[test]
    fn liveness_alert_is_phase_aware() {
        // Never connected: the feed never came up — a `no_connection` alert carrying
        // the failed-attempt count, so a consumer can tell it apart from a
        // stalled socket.
        let cold = liveness_alert(false, 0, 3, Duration::from_secs(5)).expect("never-up alerts");
        assert_eq!(cold.alert, AlertKind::NoConnection);
        assert_eq!(cold.connect_failures, Some(3));
        assert!(
            cold.message.contains("3 failed attempt"),
            "got: {}",
            cold.message
        );

        // Connected before going silent: a genuine stall, with no connect-failure
        // context.
        let stalled = liveness_alert(true, 1, 0, Duration::from_secs(5))
            .expect("a live silent socket alerts");
        assert_eq!(stalled.alert, AlertKind::NoHeartbeat);
        assert!(stalled.connect_failures.is_none());
        assert!(
            stalled.message.contains("stalled"),
            "got: {}",
            stalled.message
        );

        // Connected earlier, now disconnected and reconnecting (`connected == false`
        // with `connects > 0`): the session is knowingly down, so a
        // `no_heartbeat` "stalled" alert would be false. The watchdog stays
        // silent — `status: down` and the `no_data` watchdog already surface
        // the outage.
        assert!(
            liveness_alert(false, 1, 0, Duration::from_secs(5)).is_none(),
            "no stall alert while knowingly disconnected"
        );
    }

    #[test]
    fn alert_serializes_as_a_tagged_jsonl_line() {
        // The serialized shape *is* the stderr contract: the same `event`/`type`
        // discriminator as a snapshot, plus a stable `alert` tag a consumer can
        // route on.
        let alert = liveness_alert(false, 0, 2, Duration::from_secs(5)).expect("never-up alerts");
        let json = serde_json::to_string(&alert).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["event"], "monitor");
        assert_eq!(v["type"], "alert");
        assert_eq!(v["alert"], "no_connection");
        assert_eq!(v["connect_failures"], 2);
        assert!(v["message"].is_string());
    }

    #[tokio::test]
    async fn run_monitor_ends_when_the_broadcast_closes() {
        let (tx, rx) = broadcast::channel(16);
        tx.send(Event::Connected(Endpoint::Public)).unwrap();
        tx.send(trade("BTC/USD", 1)).unwrap();
        tx.send(Event::Heartbeat).unwrap();
        drop(tx); // every sender gone → recv() yields Closed → the task must return
        // Guard against a hang: the task should finish promptly on Closed.
        tokio::time::timeout(
            Duration::from_secs(5),
            run_monitor(rx, MonitorConfig::new(10, 30, 5)),
        )
        .await
        .expect("monitor should stop when the broadcast closes");
    }
}

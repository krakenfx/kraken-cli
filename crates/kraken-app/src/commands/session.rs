//! Recorded windows over a scope's shared account journal.
//!
//! Start and stop markers delimit evidence while trading continues through the
//! same journal; live stops reconcile venue history before closing.

use super::{Execute, rest_bridge, workspace_guard};
use crate::{
    cli::AppContext,
    commands::{paper::value_account, subscription::plan_subscriptions},
    config,
    errors::{KrakenError, Result},
    output::{CommandOutput, rounded},
    process,
    record::{RecordFormat, SinkPlan, resolve_targets},
    session::{
        decision::{Decision, DecisionKind},
        manifest::{
            PaperRef, RecordingBackend, RecordingRef, SessionManifest, SessionOutcome,
            SessionState, SessionWindow, StrategyRef,
        },
    },
    sink::fanout::Fanout,
    stream,
    telemetry::CLIENT_VERSION,
};
use chrono::{DateTime, Utc};
use itertools::Itertools;
use kraken_contract::session::{
    Decisions, List, Note, SessionCommand, SessionRequestCommand, Show, Start, StateCommand,
    StateGet, StateSet, Stop,
};
use kraken_paper::{
    AccountEvent, CommandEntry, PaperState, VenueSnapshot,
    account::{Origin, PaperAccount},
};
use kraken_recording::{RecordingDeclaration, schema};
use kraken_session::session::{
    START_MARKER, STOP_MARKER, session_decisions_path, session_dir, session_file_path, tape_file,
};
use kraken_workspace::{
    WorkspaceManifest,
    session::{SessionId, SessionRecord, SessionRefSpec, SessionStatus, VenueFill},
};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Bridge the CLI/record `RecordFormat` to the manifest's `RecordingBackend` at
/// the command boundary, so the `session` module needs no `record` dependency.
impl From<RecordFormat> for RecordingBackend {
    fn from(format: RecordFormat) -> Self {
        match format {
            RecordFormat::Jsonl => Self::Jsonl,
            RecordFormat::Duckdb => Self::Duckdb,
        }
    }
}

impl Execute for Start {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        start(&self, ctx).await?;
        Ok(CommandOutput::inline())
    }

    fn renders_inline(&self) -> bool {
        true
    }
}

impl Execute for Stop {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self { session } = self;
        stop(&session, ctx).await
    }
}

impl Execute for Show {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self { session } = self;
        show(&session, ctx).await
    }
}

impl Execute for List {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        list(ctx)
    }
}

impl Execute for Note {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            kind,
            symbol,
            reason,
            order_id,
        } = self;
        note(
            super::domain_bridge::decision_kind(kind),
            symbol,
            reason,
            order_id,
            ctx,
        )
    }
}

impl Execute for Decisions {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self { session } = self;
        decisions(&session, ctx)
    }
}

impl Execute for StateGet {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self { session } = self;
        state_get(&session, ctx)
    }
}

impl Execute for StateSet {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            round,
            legs_done,
            zone,
            last_action_at,
            note,
        } = self;
        state_set(
            kraken_session::session::SessionCursor {
                round,
                legs_done,
                zone: zone.map(super::domain_bridge::zone),
                last_action_at,
                note,
            },
            ctx,
        )
    }
}

/// Where a session's prices came from: `live` or a replayed tape.
///
/// Not `Source`, which read as a claim about the money: a session in a paper
/// workspace also shows `live` here, because the simulator prices off the live
/// feed. The mode row is the only thing that says whether the funds are real.
const MARKET_DATA_HEADER: &str = "Market data";

/// How a session in this scope learns what happened inside its window.
///
/// The distinction that matters is between the two real-money scopes. The whole
/// account owns every venue trade in the window, so it heals from trade history
/// wholesale. A live workspace owns only the orders it recorded, so healing it
/// the same way would attribute another strategy's fills to it — it reconciles
/// by transaction ID instead.
#[derive(Debug, Clone)]
pub(crate) enum SessionLedger {
    /// The simulator: fills come from folding market data.
    Simulated,
    /// The whole real account.
    Account,
    /// A live workspace's slice of the real account. Carries the manifest that
    /// proved the mode, so reconciliation at session close cannot silently skip
    /// on a second read of it.
    Allocation(Box<WorkspaceManifest>),
}

/// Where this context's journal — and therefore its sessions — live.
pub(crate) fn scope_dir(ctx: &AppContext) -> Result<(PathBuf, SessionLedger)> {
    match workspace_guard::active_mode(ctx)? {
        workspace_guard::ActiveMode::Paper(_) => Ok((ctx.scoped_base()?, SessionLedger::Simulated)),
        workspace_guard::ActiveMode::Master => Ok((config::config_dir()?, SessionLedger::Account)),
        workspace_guard::ActiveMode::Live(manifest) => Ok((
            ctx.scoped_base()?,
            SessionLedger::Allocation(manifest.into()),
        )),
    }
}

/// Resolve a session ref in the current scope and read its window as a
/// timeline — the one seam explain, replay, and lab score share.
pub(crate) fn read_session_window(
    ctx: &AppContext,
    session_ref: &str,
) -> Result<(SessionId, kraken_session::timeline::SessionTimeline)> {
    let spec: SessionRefSpec = session_ref.parse().unwrap_or(SessionRefSpec::Latest);
    let (scope, _) = scope_dir(ctx)?;
    let (session_id, manifest) = kraken_workspace::session::resolve(&scope, &spec)?;
    let timeline = kraken_session::session::read_window(kraken_session::session::SessionTracks {
        journal: scope.join(kraken_workspace::JOURNAL_FILE),
        session_dir: session_dir(&scope, session_id.ordinal()),
        manifest,
    })
    .map_err(kraken_workspace::WorkspaceError::from)?;
    Ok((session_id, timeline))
}

/// Guard the read-back verbs (`explain pnl`, `lab score`) against a session
/// with no summary to anchor on, with a message true to *why* — and before the
/// tape read, so a recording session gets this instead of the reader's opaque
/// "tape is in use". A recording session *can* be stopped (the actionable retry
/// cue); an aborted one never will, so telling the caller to stop it is
/// impossible advice. A stopped session passes through.
pub(crate) fn ensure_session_stopped(ctx: &AppContext, session_ref: &str) -> Result<()> {
    let (status, session_id) = session_status(ctx, session_ref)?;
    match readback_block(status, &session_id.to_string()) {
        None => Ok(()),
        Some(message) => Err(KrakenError::Validation(message)),
    }
}

/// Guard `replay` against a still-recording session, whose tape the live
/// recorder holds locked — with the same session-aware cue the other read-back
/// verbs give, not the reader's opaque "tape is in use". Unlike explain/score,
/// an *aborted* session IS replayable (its partial tape reads back for a
/// post-mortem), so only a recording session is refused.
pub(crate) fn ensure_session_replayable(ctx: &AppContext, session_ref: &str) -> Result<()> {
    let (status, session_id) = session_status(ctx, session_ref)?;
    if status == SessionStatus::Recording {
        return Err(KrakenError::Validation(format!(
            "session '{session_id}' is still recording — stop it with 'kraken session stop' \
             first, then replay it"
        )));
    }
    Ok(())
}

/// Resolve a session ref to its derived status — the shared front half of the
/// read-back guards, so they agree on what "recording" means.
fn session_status(ctx: &AppContext, session_ref: &str) -> Result<(SessionStatus, SessionId)> {
    let spec: SessionRefSpec = session_ref.parse().unwrap_or(SessionRefSpec::Latest);
    let (scope, _) = scope_dir(ctx)?;
    let (session_id, manifest) = kraken_workspace::session::resolve(&scope, &spec)?;
    let dir = session_dir(&scope, session_id.ordinal());
    Ok((derived_status(&dir, &manifest), session_id))
}

/// The message blocking a read-back verb on a non-stopped session, or `None`
/// when it may proceed. Pure so the status→guidance mapping is pinned without a
/// workspace: an aborted session is never told to stop (its process is gone —
/// impossible advice), a recording one always is (the actionable retry cue).
fn readback_block(status: SessionStatus, session_id: &str) -> Option<String> {
    match status {
        SessionStatus::Stopped => None,
        SessionStatus::Recording => Some(format!(
            "session '{session_id}' is still recording — stop it with 'kraken session stop' \
             first; only a stopped session can be read back"
        )),
        SessionStatus::Aborted => Some(format!(
            "session '{session_id}' was aborted before it stopped; its window never closed, so it \
             has no P&L to anchor on — only a stopped session can be read back"
        )),
        SessionStatus::Damaged => Some(format!(
            "session '{session_id}' has an unreadable session.json — it cannot be read back"
        )),
    }
}

/// A run must persist to at least one durable backend: a stdout-only `--to`
/// leaves no tape to trade against or replay, and no recording lock for the
/// liveness derivation — reject it up front.
fn ensure_durable_plan(plan: &SinkPlan) -> Result<()> {
    if plan.durables.is_empty() {
        return Err(KrakenError::Validation(
            "a session must record durably; pass --to duckdb and/or jsonl (stdout-only leaves no \
             tape to trade against or replay)"
                .into(),
        ));
    }
    Ok(())
}

/// Build the manifest's optional [`StrategyRef`] from
/// `--strategy`/`--strategy-params`.
///
/// Params are opaque JSON parsed here rather than by a clap value parser, so a
/// malformed value surfaces through the standard JSON error envelope.
fn resolve_strategy(
    strategy: Option<&str>,
    params: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<Option<StrategyRef>> {
    let Some(name) = strategy else {
        if params.is_some() {
            return Err(KrakenError::Validation(
                "--strategy-params requires --strategy".into(),
            ));
        }
        return Ok(None);
    };
    if name.trim().is_empty() {
        return Err(KrakenError::Validation(
            "--strategy must not be empty".into(),
        ));
    }
    Ok(Some(StrategyRef {
        name: name.to_string(),
        // The manifest records any JSON; the flag only ever supplies an object.
        params: params.cloned().map(serde_json::Value::Object),
    }))
}

/// The journal marker that cuts a session window; the only [`CommandEntry`]
/// values that ever carry a `run` stamp.
fn marker(name: &str, run: &SessionId) -> CommandEntry {
    CommandEntry {
        name: name.to_string(),
        session: Some(run.to_string()),
        ..CommandEntry::default()
    }
}

/// Open a session window and record its market tape in the foreground; the
/// caller backgrounds the process so an agent gets the printed session id
/// immediately.
pub(crate) async fn start(args: &Start, ctx: &AppContext) -> Result<()> {
    let Start {
        symbols,
        channels,
        options,
        to,
        label,
        strategy,
        strategy_params,
        experiment,
        from,
        speed,
        window,
    } = args;
    // Reject any channel the recorder can't persist before planning or opening a
    // socket.
    kraken_recording::ensure_recordable(channels)?;
    // The params are already an object by the time they get here — clap parsed
    // them, or a tool call sent one. What is left to judge is the pairing.
    let strategy_ref = resolve_strategy(strategy.as_deref(), strategy_params.as_ref())?;
    let source_ref = from
        .as_deref()
        .map(str::parse::<kraken_recording::TapeRef>)
        .transpose()?;
    if source_ref.is_some() {
        ensure_speed(*speed)?;
    }
    // Fail fast on a malformed --for before opening a socket or a sink.
    // The window grammar lives in kraken-lab (a sealed `live:<window>` must
    // agree with what --for accepts); frame the error against this flag.
    let duration = window
        .as_deref()
        .map(|window| {
            kraken_lab::parse_window(window)
                .map_err(|why| KrakenError::Validation(format!("--for '{window}': {why}")))
        })
        .transpose()?;
    // A replay session records whatever the source tape holds; subscriptions
    // exist only on the live path.
    let subs = match source_ref {
        Some(_) => Vec::new(),
        None => plan_subscriptions(symbols, channels, options)?,
    };

    let (scope, ledger) = scope_dir(ctx)?;

    // A replay source settles fills from recorded prices, and the real
    // account's journal mirrors real venue orders — folding simulated fills
    // onto them would fabricate executions. Refuse before
    // anything is allocated; live workspaces refuse in the workspace guard.
    if source_ref.is_some() && matches!(ledger, SessionLedger::Account) {
        return Err(KrakenError::Validation(
            "session start --from settles fills from recorded prices, which cannot be folded into \
             the real account's journal — run the replay inside a paper workspace (--workspace or \
             KRAKEN_WORKSPACE)"
                .to_string(),
        ));
    }

    // A replay leg of a frozen plan must run the exact sealed bytes. `lab next`
    // checks this when it proposes the start; enforce it here too so a manual
    // start — or a re-record in the gap after the proposal — can't slip a
    // drifted tape past the seal and later satisfy the plan by tape ref
    // alone. No-op for an unfrozen experiment or an unplanned source. When it
    // does hash, keep the digest: `load_replay_source` stamps session.json with
    // the same value, so the tape is hashed once, not twice.
    let lab = kraken_lab::Lab::new(&scope).with_library(config::config_dir()?);
    let sealed_hash = if let (Some(exp), Some(tape)) = (experiment.as_deref(), source_ref.as_ref())
        && let Ok(loaded) = lab.show(exp)
    {
        lab.ensure_planned_source(&loaded, &tape.to_string())?
    } else {
        None
    };

    // Load the whole replay source up front: a bad ref, an unfinalized
    // tape, or an empty tape must fail before any sink opens.
    let replay = source_ref
        .map(|tape| load_replay_source(&scope, tape, *speed, sealed_hash))
        .transpose()?;

    let (declared_symbols, declared_channels) = match &replay {
        // The run records what the tape holds — declared verbatim.
        Some(replay) => (
            replay.declaration.symbols.clone(),
            replay.declaration.channels.clone(),
        ),
        None => (
            subs.iter()
                .flat_map(|sub| sub.symbols().iter().cloned())
                .unique()
                .collect(),
            subs.iter()
                .map(|sub| sub.channel().to_string())
                .unique()
                .collect(),
        ),
    };

    // Fold stale venue verdicts BEFORE the start marker cuts the window open:
    // window membership is positional, so a terminal order left over from an
    // earlier session that reconciliation folds later — after the marker —
    // would attribute its fill to this session's evidence. Heals before the
    // journal opens, like `finalize_session`: never hold the writer lock
    // across a network await. Fails closed on an unconfirmed fold: opening
    // the window anyway would let the next mid-session sync land the stale
    // fill inside it, the very misattribution this reconcile exists to stop.
    if let SessionLedger::Allocation(workspace) = &ledger {
        let (confirmed, _) =
            super::journal_mirror::reconcile_live_workspace(ctx, workspace).await?;
        if !confirmed {
            return Err(KrakenError::Validation(
                "the venue could not confirm this workspace's open orders; a session window \
                 opened now could absorb an earlier order's fill into its evidence — retry when \
                 the venue is reachable"
                    .to_string(),
            ));
        }
    }

    // Open the journal BEFORE allocating: the writer lock is what serializes
    // two concurrent `session start`s onto distinct ordinals. On the real
    // account the first open anchors the journal with a venue snapshot.
    let account = match &ledger {
        SessionLedger::Account => super::journal_mirror::open_attached(ctx).await?,
        // A workspace journal is already seeded by `workspace create`, so this
        // never re-anchors; it takes the same path for one lock discipline.
        SessionLedger::Allocation(_) => {
            super::journal_mirror::open_attached_at(scope.join(kraken_workspace::JOURNAL_FILE), ctx)
                .await?
        }
        SessionLedger::Simulated => {
            PaperAccount::open_at(scope.join(kraken_workspace::JOURNAL_FILE), Origin::Cli)?
        }
    };
    let session_id = kraken_workspace::session::allocate(&scope, label.as_deref())?;
    let dir = session_dir(&scope, session_id.ordinal());

    // Freeze the opening anchor: the account's equity now, best-effort
    // valuation (an unpriceable position is disclosed, never guessed).
    let state = account.state()?;
    let (opening_equity, opening_complete) = value_account(ctx.spot(), state).await;
    let currency = state.starting_currency.clone();

    let started_at = Utc::now();
    let meta = RecordingDeclaration {
        source: schema::SOURCE.to_string(),
        symbols: declared_symbols,
        channels: declared_channels,
        window_start: started_at,
        cli_version: CLIENT_VERSION.to_string(),
    };

    // One recording per durable backend; each tape sits beside session.json.
    let plan = resolve_targets(to);
    ensure_durable_plan(&plan)?;
    let recordings: Vec<RecordingRef> = plan
        .durables
        .iter()
        .map(|&backend| RecordingRef {
            backend: backend.into(),
            file: tape_file(backend.into()),
            schema_version: schema::SCHEMA_VERSION.to_string(),
            symbols: meta.symbols.clone(),
            channels: meta.channels.clone(),
        })
        .collect();
    let window = SessionWindow {
        session: session_id.to_string(),
        started_at,
        opening_equity,
        opening_complete,
        ended_at: None,
    };
    let manifest = SessionManifest::new(
        session_id.to_string(),
        CLIENT_VERSION,
        std::process::id(),
        recordings,
        PaperRef {
            starting_balance: opening_equity,
            currency,
        },
        window,
    )
    .with_strategy(strategy_ref)
    .with_experiment(experiment.clone())
    .with_source(replay.as_ref().map(|r| r.source.clone()))
    .with_label(label.clone());

    // Write session.json before opening any sink — tape.* must never exist
    // without its contract — then cut the window open with the durable
    // start marker.
    let run_file = session_file_path(&scope, session_id.ordinal());
    manifest.save(&run_file)?;
    let mut account = account;
    account.append(&account.stamp_at(
        vec![AccountEvent::Command(marker(START_MARKER, &session_id))],
        started_at,
    ))?;
    // Release the journal: trades must flow through it while the session records.
    drop(account);

    // Open every requested sink — tape locks + schema are validated before
    // any socket opens — then announce + stream through the fan-out.
    let fanout = Fanout::build(&plan, &meta, ctx.format, |backend| {
        dir.join(tape_file(backend.into()))
    })?;

    // Announce only after the stream is live: both pumps signal `ready` on the
    // first (real or synthetic) subscribe ack, so a `start` that never comes up
    // prints no `session_started` and just returns the streaming error. The
    // announce effect and its failure policy live here, at the call site: an
    // announcement that cannot be delivered (stdout gone) returns at once,
    // dropping the streaming future — the drop-triggered teardown the WS stack
    // is built on — since streaming on for a caller that never received its
    // handle helps nobody.
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let announce = || emit_line(&session_started_line(&dir, &manifest));
    let streamed = match replay {
        Some(replay) => {
            stream_and_announce(
                stream::run_replay(
                    replay.frames,
                    replay.source,
                    Some(ready_tx),
                    fanout,
                    duration,
                    replay.guard,
                    stream::shutdown_signal(),
                ),
                ready_rx,
                announce,
            )
            .await
        }
        None => {
            stream_and_announce(
                stream::run(subs, None, Some(ready_tx), fanout, duration, ctx),
                ready_rx,
                announce,
            )
            .await
        }
    };

    // The recorder owns the seal: any graceful end closes the window in
    // process — a `--for` timer, an exhausted replay tape, a socket close, or
    // a caught SIGINT/SIGTERM/SIGHUP (`shutdown_signal`). So Ctrl-C on a live
    // session seals it exactly as `session stop` would, at the instant recording
    // stopped; `session stop` just signals this same path and reports the result.
    // Only a stream *error* leaves the window open — the error is the answer,
    // and the aborted status never blocks the next start.
    if streamed.is_ok() {
        // The pump has drained; stamp the end now, before finalize's work.
        let end = Utc::now();
        let manifest = finalize_session(&scope, session_id, manifest, ctx, end, ledger).await?;
        emit_line(&session_stopped_line(&manifest))
    } else {
        streamed
    }
}

async fn stream_and_announce(
    session: impl Future<Output = Result<()>>,
    ready_rx: tokio::sync::oneshot::Receiver<()>,
    announce: impl FnOnce() -> Result<()>,
) -> Result<()> {
    tokio::pin!(session);
    tokio::select! {
        // The stream ended before it ever became ready: surface its outcome unannounced.
        result = &mut session => return result,
        ready = ready_rx => {
            // Err means the sender dropped unfired (the stream is winding down); the
            // stream's own outcome below is the answer, so there is nothing to announce.
            if ready.is_ok() {
                announce()?;
            }
        }
    }
    session.await
}

/// A validated replay source: frames, the tape's own declaration, the affine
/// provenance block, and the opened source itself — held for the pump's
/// lifetime so its shared read lock releases at teardown, like dropping the
/// live client.
struct ReplaySource {
    frames: Vec<kraken_recording::MarketEvent>,
    declaration: RecordingDeclaration,
    source: kraken_session::manifest::SessionSource,
    guard: kraken_recording::TapeSource,
}

fn ensure_speed(speed: f64) -> Result<()> {
    if !speed.is_finite() || !(kraken_replay::MIN_SPEED..=kraken_replay::MAX_SPEED).contains(&speed)
    {
        return Err(KrakenError::Validation(format!(
            "--speed must be between {} and {}",
            kraken_replay::MIN_SPEED,
            kraken_replay::MAX_SPEED
        )));
    }
    Ok(())
}

fn load_replay_source(
    base: &Path,
    tape: kraken_recording::TapeRef,
    speed: f64,
    content_hash: Option<String>,
) -> Result<ReplaySource> {
    use kraken_recording::{CaptureSource, Source};
    // JSONL-only, same as the fold's reader — the pump and every later paper
    // command must read the same file (kraken_replay owns the resolution).
    let path = kraken_replay::resolve_source_path(&config::config_dir()?, base, &tape)?;
    let guard = kraken_recording::TapeSource::open(&path)?;
    let capture = guard.capture()?;
    let frames: Vec<_> = guard.read()?.collect();
    // Only a finalized tape replays: an unfinalized one is live or crashed,
    // and its completeness is unknown by definition.
    if capture.summary.is_none() {
        return Err(KrakenError::Validation(format!(
            "tape '{tape}' is not finalized (still recording, or crashed mid-capture); replay \
             needs a finalized tape — see 'kraken tape list'"
        )));
    }
    let Some(first) = frames.first() else {
        return Err(KrakenError::Validation(format!(
            "tape '{tape}' holds no frames to replay"
        )));
    };
    // Record the source tape's hash so a completed run carries proof of the
    // exact bytes it replayed. The lab FSM checks this against a sealed replay
    // leg — the seal enforcement that does NOT depend on --experiment being
    // passed at start. Reuse the hash the seal guard already
    // streamed for this exact tape when it ran; otherwise hash it now. Same
    // `sha256_file` on both paths, so the two hashes are byte-comparable.
    let content_hash = match content_hash {
        Some(hash) => hash,
        None => kraken_lab::sha256_file(&path)?,
    };
    let source = kraken_session::manifest::SessionSource {
        tape: tape.to_string(),
        speed,
        anchor: first.at,
        started_at: Utc::now(),
        content_hash: Some(content_hash),
    };
    Ok(ReplaySource {
        source,
        declaration: capture.meta,
        frames,
        guard,
    })
}

/// The one-line `session_started` JSON a backgrounding caller parses for the
/// session handle. Printed when the readiness signal fires on the first
/// subscribe ack — a `start` that never comes up prints nothing and exits
/// non-zero.
fn session_started_line(dir: &Path, manifest: &SessionManifest) -> String {
    serde_json::json!({
        "type": "session_started",
        "session": manifest.window.session,
        "label": manifest.label,
        "pid": manifest.pid,
        "session_dir": dir.display().to_string(),
        "recordings": manifest.recordings,
        "window": {
            "started_at": manifest.window.started_at,
            "opening_equity": manifest.window.opening_equity.to_string(),
            "opening_complete": manifest.window.opening_complete,
        },
    })
    .to_string()
}

/// The closing `session_stopped` JSON a `--for`/replay session prints after it
/// self-finalizes, mirroring the `session_started` line so a caller watching
/// stdout sees the window open and close on the same stream.
fn session_stopped_line(manifest: &SessionManifest) -> String {
    serde_json::json!({
        "type": "session_stopped",
        "session": manifest.window.session,
        "status": manifest.status.to_string(),
        "summary": manifest.summary,
    })
    .to_string()
}

fn emit_line(line: &str) -> Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(line.as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

/// The session.json pid as a signalable `i32`. Pids are small; an out-of-range
/// value maps to `0`, which the liveness probe treats as "not individually
/// signalable" (reported alive).
fn signalable_pid(manifest: &SessionManifest) -> i32 {
    i32::try_from(manifest.pid).unwrap_or(0)
}

/// The tape paths this session's liveness is read from, or `None` when there is
/// no durable tape to consult — the caller then falls back to probing the pid.
fn recorded_tape_paths(dir: &Path, manifest: &SessionManifest) -> Option<Vec<PathBuf>> {
    if manifest.recordings.is_empty() {
        return None;
    }
    Some(
        manifest
            .recordings
            .iter()
            .map(|rec| dir.join(&rec.file))
            .collect(),
    )
}

/// Whether this session's recorder is still alive, read from its recording
/// lock(s): the kernel releases them the moment the process dies — a signal a
/// recycled pid can't spoof. Safe on async paths: each probe is one
/// non-blocking try-lock, microseconds, not worth `spawn_blocking`.
fn recorder_alive(dir: &Path, manifest: &SessionManifest) -> bool {
    match recorded_tape_paths(dir, manifest) {
        None => process::process_alive(signalable_pid(manifest)),
        Some(paths) => paths.iter().any(|p| kraken_recording::is_lock_held(p)),
    }
}

/// How long [`stop`] waits for a signalled recorder to release its recording
/// lock — generous enough for a DuckDB checkpoint on a session-sized tape;
/// exceeding it returns a timeout, never a premature close.
const STOP_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
/// Poll interval while waiting for the lock release; each probe is one
/// non-blocking try-lock, so 50ms is responsive at negligible cost.
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Wait for a just-signalled recorder to release its recording locks (the
/// kernel drops them on any exit, clean or not); then the caller may close
/// the window. Timeout and poll are injected so tests can drive the timeout
/// path.
///
/// # Errors
/// [`KrakenError::Timeout`] if the lock is still held after `timeout` — the
/// caller must NOT close the window then: leaving it open keeps the status
/// derivation honest.
async fn await_recorder_exit(
    dir: &Path,
    manifest: &SessionManifest,
    timeout: Duration,
    poll: Duration,
) -> Result<()> {
    let start = Instant::now();
    loop {
        if !recorder_alive(dir, manifest) {
            return Ok(());
        }
        if start.elapsed() >= timeout {
            return Err(KrakenError::Timeout(format!(
                "session {} recorder (pid {}) was signalled but still holds its recording lock \
                 after {}s; it may still be flushing. Re-run `kraken session stop` or check \
                 `kraken session show`.",
                manifest.window.session,
                manifest.pid,
                timeout.as_secs()
            )));
        }
        tokio::time::sleep(poll).await;
    }
}

/// Wait for a signalled recorder to stamp its own seal (`ended_at`) into
/// session.json — the recorder owns the close (it finalizes on the caught
/// signal), so `stop` reports its result instead of finalizing a second time.
/// `None` on timeout: the recorder released its lock but never sealed (it
/// died mid-finalize), so the caller must close the window from durable state.
async fn await_session_sealed(
    run_file: &Path,
    timeout: Duration,
    poll: Duration,
) -> Option<SessionManifest> {
    let start = Instant::now();
    loop {
        if let Ok(manifest) = SessionManifest::load(run_file)
            && manifest.window.ended_at.is_some()
        {
            return Some(manifest);
        }
        if start.elapsed() >= timeout {
            return None;
        }
        tokio::time::sleep(poll).await;
    }
}

/// Close the window: stamp `ended_at` and the summary the timeline consumers
/// anchor on. Pure — the clock is injected.
fn finalize(
    mut manifest: SessionManifest,
    final_value: Decimal,
    ended_at: DateTime<Utc>,
) -> SessionManifest {
    manifest.status = SessionState::Stopped;
    manifest.summary = Some(SessionOutcome {
        ended_at,
        final_value,
        pnl: final_value - manifest.window.opening_equity,
    });
    manifest.window.ended_at = Some(ended_at);
    manifest
}

/// Stop a session and close its window. Idempotent: an already-stopped session
/// is returned unchanged.
///
/// `stop` is a supervisor over the recorder, which owns the seal: it SIGTERMs
/// the recorder, waits for it to release its lock and stamp its own `ended_at`
/// (the same graceful close a Ctrl-C takes), and reports that — no second
/// finalize. It closes the window itself only as a fallback: the recorder was
/// already dead (an aborted session), or it released its lock but died before
/// sealing. A lock-release timeout leaves the window open.
async fn stop(session_ref: &str, ctx: &AppContext) -> Result<CommandOutput> {
    let spec: SessionRefSpec = session_ref.parse().unwrap_or(SessionRefSpec::Latest);
    let (scope, ledger) = scope_dir(ctx)?;
    let (session_id, manifest) = kraken_workspace::session::resolve(&scope, &spec)?;

    if manifest.window.ended_at.is_some() {
        return stopped_output(&manifest, "already stopped");
    }

    let dir = session_dir(&scope, session_id.ordinal());
    // Signal only if the recording lock proves the recorder is still running —
    // gating on the lock, not the bare pid, avoids SIGTERM-ing an unrelated
    // process that inherited a recycled pid.
    if recorder_alive(&dir, &manifest) {
        // Wrap the pid rejection with what the user can act on: the session
        // and session.json's real pid. Other error kinds pass through intact.
        process::terminate(signalable_pid(&manifest)).map_err(|e| match e {
            KrakenError::Validation(msg) => KrakenError::Validation(format!(
                "cannot stop session {session_id} (session.json pid {}): {msg}",
                manifest.pid
            )),
            other => other,
        })?;
        // Wait for the lock release so the window is never closed while the
        // recorder still holds — and writes to — its tape, then for the
        // recorder's own seal. Report that seal rather than writing a second.
        await_recorder_exit(&dir, &manifest, STOP_SHUTDOWN_TIMEOUT, STOP_POLL_INTERVAL).await?;
        let run_file = session_file_path(&scope, session_id.ordinal());
        if let Some(sealed) =
            await_session_sealed(&run_file, STOP_SHUTDOWN_TIMEOUT, STOP_POLL_INTERVAL).await
        {
            return stopped_output(&sealed, "stopped");
        }
    }

    // Fallback: no live recorder (an aborted session), or it released its lock but
    // never sealed. Close from durable state. `now()` is the stop instant; for
    // an aborted session this is later than the crash — the known limitation the
    // post-hoc abort-point seal will address.
    let end = Utc::now();
    let manifest = finalize_session(&scope, session_id, manifest, ctx, end, ledger).await?;
    stopped_output(&manifest, "stopped")
}

/// Close a session from durable state: reconcile (real account), settle and
/// value, append the stop marker, stamp the window. Shared by `session stop`
/// and a `--for`/replay self-stop.
async fn finalize_session(
    scope: &Path,
    session_id: SessionId,
    manifest: SessionManifest,
    ctx: &AppContext,
    end: DateTime<Utc>,
    ledger: SessionLedger,
) -> Result<SessionManifest> {
    // Both heal BEFORE the journal opens: never hold the journal's writer lock
    // across a network await. A live workspace heals by transaction ID rather
    // than from trade history — only the orders it recorded are its own, so
    // wholesale healing would credit it another strategy's fills.
    let mut healing = None;
    match &ledger {
        SessionLedger::Account => {
            healing = fetch_venue_window(ctx, manifest.window.started_at, end).await;
        }
        SessionLedger::Allocation(workspace) => {
            super::journal_mirror::reconcile_live_workspace(ctx, workspace).await?;
        }
        SessionLedger::Simulated => {}
    }

    let journal = scope.join(kraken_workspace::JOURNAL_FILE);
    let mut account = PaperAccount::open_at(journal, Origin::Cli)?;

    let final_value = match &manifest.source {
        Some(source) => {
            // A replay fold appends simulated fills, which only a simulated
            // ledger can absorb. The real account is a bug-era
            // start (refused above now); a live workspace is an aborted paper
            // replay that survived promotion. Fail closed on both.
            if !matches!(ledger, SessionLedger::Simulated) {
                return Err(KrakenError::Validation(
                    "this replay session sits on a ledger of real executions (the real account or \
                     a live workspace); settling it would fabricate fills in that journal, so its \
                     window cannot be closed"
                        .to_string(),
                ));
            }
            // Bound the closing fold to `end` — the instant the pump was told
            // to stop, captured BEFORE the shutdown drain. The pump is paced,
            // so at `end` it had emitted exactly the source frames with
            // `at <= tape_time(end)`; folding to a later `now()` (through the
            // SIGTERM wait) would settle fills on frames the session's tape never
            // recorded and the scorer never replays.
            let mut venue = super::replay_venue::ReplayVenue::for_run(
                &config::config_dir()?,
                scope,
                &session_dir(scope, session_id.ordinal()),
                source.clone(),
                Some(end),
            )?;
            venue.fold_due(&mut account)?;
            venue.value(account.state()?).0
        }
        None => {
            // Heal the journal from venue truth first (real account): fills
            // the mirror missed enter under their venue txids — idempotent,
            // a second stop appends nothing — then the balance snapshot.
            if let Some((fills, snapshot)) = healing {
                reconcile(&mut account, fills, snapshot, end)?;
            }
            value_account(ctx.spot(), account.state()?).await.0
        }
    };

    // The stop marker cuts the window in the journal — everything above
    // (healed fills included) is inside it. Durable append, never audit: a
    // disk fault must fail the stop, not leave the window open silently.
    account.append(&account.stamp_at(
        vec![AccountEvent::Command(marker(STOP_MARKER, &session_id))],
        end,
    ))?;
    drop(account);

    // Stamp the same `end` everywhere, so a post-stop fold and the window
    // agree on the closing instant.
    let manifest = finalize(manifest, final_value, end);
    manifest.save(&session_file_path(scope, session_id.ordinal()))?;
    Ok(manifest)
}

/// The venue's fills inside the window plus its balance snapshot —
/// best-effort by design: an unreachable venue must not make a session
/// unstoppable; what it misses is disclosed (`complete: false`) and the next
/// reconciliation heals it.
async fn fetch_venue_window(
    ctx: &AppContext,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Option<(Vec<VenueFill>, VenueSnapshot)> {
    match venue_window(ctx, from, to).await {
        Ok(result) => Some(result),
        Err(err) => {
            tracing::warn!(%err, "venue history unavailable; closing the window unreconciled");
            Some((
                Vec::new(),
                VenueSnapshot {
                    balances: Default::default(),
                    complete: false,
                    anchor: None,
                },
            ))
        }
    }
}

async fn venue_window(
    ctx: &AppContext,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<(Vec<VenueFill>, VenueSnapshot)> {
    use kraken_rest::spot::account::{Bound, TradesHistoryQuery, Window};
    let query = TradesHistoryQuery {
        window: Window {
            start: Some(Bound::Time(from.timestamp())),
            end: Some(Bound::Time(to.timestamp() + 1)),
            ..Window::default()
        },
        ..TradesHistoryQuery::default()
    };
    let history = ctx.spot_account()?.sign(&query).await?;
    let fills = venue_fills(history);

    // Through the mirror's reader, not a second parse of the same payload: it
    // normalizes the venue's legacy X/Z codes, and both writers land snapshots
    // in the same journal, which must hold one spelling per asset.
    let snapshot = VenueSnapshot {
        balances: super::journal_mirror::venue_balances(ctx).await?,
        complete: true,
        anchor: None,
    };
    Ok((fills, snapshot))
}

/// Venue fills into the reconciliation shape. A fill whose time is out of
/// range is skipped with a warning — one odd entry must not lose the rest.
fn venue_fills(history: kraken_rest::spot::account::TradesHistory) -> Vec<VenueFill> {
    history
        .trades
        .into_iter()
        .filter_map(|(txid, fill)| {
            let Some(time) = rest_bridge::instant(&fill.time) else {
                tracing::warn!(txid = %txid, "skipping a venue fill with an unreadable time");
                return None;
            };
            Some(VenueFill {
                txid: txid.to_string(),
                order_txid: fill.ordertxid.to_string(),
                pair: fill.pair.into(),
                side: fill.side,
                volume: fill.vol,
                price: fill.price,
                fee: fill.fee,
                cost: fill.cost,
                time,
            })
        })
        .collect()
}

/// Append the venue fills the journal has not seen (keyed by venue txid) at
/// their venue instants, then adopt the venue's balances as one `Reconciled`
/// snapshot. One durable batch.
fn reconcile(
    account: &mut PaperAccount,
    fills: Vec<VenueFill>,
    snapshot: VenueSnapshot,
    end: DateTime<Utc>,
) -> Result<()> {
    let known = account.state()?.filled_trades.clone();
    let mut records = Vec::new();
    for fill in kraken_workspace::session::missing_fills(&known, &fills) {
        let Some(trade) = venue_fill_as_trade(fill) else {
            tracing::warn!(txid = %fill.txid, pair = %fill.pair, "venue fill not mappable; left visible in drift");
            continue;
        };
        records.extend(account.stamp_at(vec![AccountEvent::OrderFilled { trade }], fill.time));
    }
    // An incomplete snapshot (venue unreachable, or a partial balance fetch)
    // must never be adopted: `Reconciled` takes the venue's balances wholesale,
    // so an empty/partial one would durably overwrite the journal and close the
    // window at near-zero equity — exactly what the "closing unreconciled" warn
    // above says did NOT happen. Recovered fills stay (additive, idempotent);
    // the balance truth waits for a reachable venue on the next stop.
    if snapshot.complete {
        records.extend(account.stamp_at(vec![AccountEvent::Reconciled(snapshot)], end));
    }
    Ok(account.append(&records)?)
}

/// A venue fill in the journal's trade shape, joined by txid like the
/// mirror's submissions: `id` = venue trade txid, `order_id` = order txid.
fn venue_fill_as_trade(fill: &VenueFill) -> Option<kraken_paper::PaperTrade> {
    let (pair, base, quote) = kraken_paper::parse_pair(&fill.pair).ok()?;
    Some(kraken_paper::PaperTrade {
        id: fill.txid.clone(),
        order_id: fill.order_txid.clone(),
        pair,
        base,
        quote,
        side: fill.side,
        volume: fill.volume,
        price: fill.price,
        fee: fill.fee,
        cost: fill.cost,
        filled_at: fill.time,
        reference_quote: None,
    })
}

/// Build the `stop` result: the closed window as JSON plus a compact table.
fn stopped_output(manifest: &SessionManifest, note: &str) -> Result<CommandOutput> {
    let summary = serde_json::to_value(&manifest.summary)?;
    let (final_value, pnl) = manifest.summary.as_ref().map_or_else(
        || ("-".to_string(), "-".to_string()),
        |s| {
            (
                format!("{:.2}", rounded(s.final_value, 2)),
                format!("{:+.2}", rounded(s.pnl, 2)),
            )
        },
    );
    let pairs = vec![
        ("Session".to_string(), manifest.window.session.clone()),
        ("Status".to_string(), manifest.status.to_string()),
        ("Final Value".to_string(), final_value),
        ("P&L".to_string(), pnl),
    ];
    Ok(CommandOutput::key_value(
        pairs,
        serde_json::json!({
            "group": "session",
            "type": "session_stopped",
            "session": manifest.window.session,
            "label": manifest.label,
            "status": manifest.status.to_string(),
            "note": note,
            "window": serde_json::to_value(&manifest.window)?,
            "summary": summary,
        }),
    ))
}

/// Fold a session window's account track into its end-of-window state: the
/// fills and open orders that happened INSIDE this session, never the whole
/// account's. A trade made outside the window is not in `window.events`, so it
/// cannot inflate this session's counts.
fn windowed_state(events: &[kraken_session::timeline::event::TimelineEvent]) -> PaperState {
    let mut state = PaperState::default();
    for event in events {
        if let kraken_session::timeline::event::EventPayload::Account(record) = &event.payload {
            state.apply(record.ts, &record.event);
        }
    }
    state
}

/// Report one session: its contract, derived status, and a valuation — the
/// stored summary for a closed window, a live valuation for an open one.
async fn show(session_ref: &str, ctx: &AppContext) -> Result<CommandOutput> {
    let (scope, ledger) = scope_dir(ctx)?;
    let (mut out, window_anchor) = show_in(&scope, session_ref, ctx, &ledger).await?;
    // A session in a live workspace describes real money, and nothing else in
    // its table says so: `Market data: live` is about the feed, not the funds.
    // Pre-promotion history earned simulated money and is stamped as such.
    if let SessionLedger::Allocation(workspace) = &ledger {
        workspace_guard::stamp_session_money(&mut out, workspace, &scope, window_anchor)?;
    }
    Ok(out)
}

/// The output plus the resolved window's money-source anchor (its end, or
/// its start when never sealed) — the caller stamps the money source, and
/// re-resolving the ref there could race a concurrent stop.
async fn show_in(
    scope: &Path,
    session_ref: &str,
    ctx: &AppContext,
    ledger: &SessionLedger,
) -> Result<(CommandOutput, Option<DateTime<Utc>>)> {
    let spec: SessionRefSpec = session_ref.parse().unwrap_or(SessionRefSpec::Latest);
    let (session_id, manifest) = kraken_workspace::session::resolve(scope, &spec)?;
    let dir = session_dir(scope, session_id.ordinal());
    let status = derived_status(&dir, &manifest);
    let window_anchor = manifest
        .window
        .ended_at
        .or(Some(manifest.window.started_at));

    // The displayed counts (trades, open orders) describe THE WINDOW in every
    // case — the fold of this session's own slice — so "Trades" means the same
    // thing whether the session is recording or stopped. Read the account
    // window tape-free: a recording session's tape is held by the recorder, so
    // the full timeline read would fail, but the counts are journal events.
    // Only the *valuation* differs: a stopped session marks at its sealed
    // summary, an open one live.
    let journal = scope.join(kraken_workspace::JOURNAL_FILE);
    let window_events = kraken_session::session::read_account_window(&journal, &manifest)
        .map_err(kraken_workspace::WorkspaceError::from)?;
    let window_state = windowed_state(&window_events);

    // A sealed (stopped) session values at its stored summary — a trade made
    // after `session stop` can never move a finished session.
    if let Some(summary) = manifest.summary.clone() {
        let out = show_output(&manifest, status, &window_state, summary.final_value, true)?;
        return Ok((out, window_anchor));
    }

    let (current_value, valuation_complete) = match &manifest.source {
        // Replay: fold the tape to now and value the account from its marks.
        // The lock is held only across local file work — no network round-trip.
        Some(source) => {
            // Even this read-only show folds durable fills first:
            // on a ledger of real executions — the real account, or a live
            // workspace whose aborted paper replay survived promotion — that
            // would write simulated executions into the journal. Fail closed.
            if !matches!(ledger, SessionLedger::Simulated) {
                return Err(KrakenError::Validation(
                    "this replay session sits on a ledger of real executions (the real account or \
                     a live workspace); reading its open window would fold simulated fills into \
                     that journal — see 'kraken session list' for its manifest instead"
                        .to_string(),
                ));
            }
            // A closed window values at its sealed end, never past it, so
            // `show` can't fold new fills into the journal.
            let ceiling = manifest.window.ended_at;
            let mut venue = super::replay_venue::ReplayVenue::for_run(
                &config::config_dir()?,
                scope,
                &dir,
                source.clone(),
                ceiling,
            )?;
            let mut account =
                PaperAccount::open_at(scope.join(kraken_workspace::JOURNAL_FILE), Origin::Cli)?;
            venue.fold_due(&mut account)?;
            let account_state = account.state()?.clone();
            drop(account);
            venue.value(&account_state)
        }
        None => {
            // Clone-and-drop before the await: a read-only show must not hold
            // the journal's exclusive lock across a network round-trip, or
            // concurrent commands fail "in use" for its whole duration.
            let account_state = {
                let account =
                    PaperAccount::open_at(scope.join(kraken_workspace::JOURNAL_FILE), Origin::Cli)?;
                account.state()?.clone()
            };
            value_account(ctx.spot(), &account_state).await
        }
    };
    let out = show_output(
        &manifest,
        status,
        &window_state,
        current_value,
        valuation_complete,
    )?;
    Ok((out, window_anchor))
}

/// Status derived like the crate's listing does: a closed window is stopped;
/// an open one is recording iff a recorder holds the tape.
fn derived_status(dir: &Path, manifest: &SessionManifest) -> SessionStatus {
    if manifest.window.ended_at.is_some() {
        SessionStatus::Stopped
    } else if recorder_alive(dir, manifest) {
        SessionStatus::Recording
    } else {
        SessionStatus::Aborted
    }
}

/// Percentage P&L against the opening equity (`0` when it is non-positive).
fn pnl_percent(pnl: Decimal, opening: Decimal) -> Decimal {
    if opening > Decimal::ZERO {
        pnl / opening * dec!(100)
    } else {
        Decimal::ZERO
    }
}

fn show_output(
    manifest: &SessionManifest,
    status: SessionStatus,
    state: &PaperState,
    current_value: Decimal,
    valuation_complete: bool,
) -> Result<CommandOutput> {
    let pnl = current_value - manifest.window.opening_equity;
    let pnl_pct = pnl_percent(pnl, manifest.window.opening_equity);
    let pairs = vec![
        ("Session".to_string(), manifest.window.session.clone()),
        (
            "Label".to_string(),
            manifest.label.clone().unwrap_or_else(|| "-".to_string()),
        ),
        // The knobs a driver was started with must read back through the
        // same command that reports the run — without these
        // rows, `--strategy-params` is write-only.
        (
            "Strategy".to_string(),
            manifest
                .strategy
                .as_ref()
                .map_or_else(|| "-".to_string(), |strategy| strategy.name.clone()),
        ),
        (
            "Params".to_string(),
            manifest
                .strategy
                .as_ref()
                .and_then(|strategy| strategy.params.as_ref())
                .map_or_else(|| "-".to_string(), ToString::to_string),
        ),
        ("Status".to_string(), status.to_string()),
        (
            MARKET_DATA_HEADER.to_string(),
            manifest.source.as_ref().map_or_else(
                || "live".to_string(),
                |s| format!("replay @{}x · {}", s.speed, s.tape),
            ),
        ),
        (
            "Symbols".to_string(),
            manifest
                .recordings
                .first()
                .map(|r| r.symbols.join(", "))
                .unwrap_or_default(),
        ),
        (
            "Current Value".to_string(),
            format!(
                "{:.2} {}",
                rounded(current_value, 2),
                manifest.paper.currency
            ),
        ),
        (
            "Window P&L".to_string(),
            format!(
                "{} ({}%)",
                crate::output::signed_or_sub_resolution(pnl, 2),
                crate::output::signed_or_sub_resolution(pnl_pct, 2)
            ),
        ),
        ("Trades".to_string(), state.filled_trades.len().to_string()),
    ];
    let recordings = serde_json::to_value(&manifest.recordings)?;
    let summary = serde_json::to_value(&manifest.summary)?;
    Ok(CommandOutput::key_value(
        pairs,
        serde_json::json!({
            "group": "session",
            "session": manifest.window.session,
            "label": manifest.label,
            "status": status.to_string(),
            "pid": manifest.pid,
            "created_at": manifest.created_at,
            "recordings": recordings,
            "strategy": serde_json::to_value(&manifest.strategy)?,
            "source": serde_json::to_value(&manifest.source)?,
            "window": serde_json::to_value(&manifest.window)?,
            "valuation": {
                "current_value": current_value,
                "pnl": pnl,
                "pnl_pct": pnl_pct,
                "valuation_complete": valuation_complete,
                "total_trades": state.filled_trades.len(),
                "open_orders": state.open_orders.len(),
            },
            "summary": summary,
        }),
    ))
}

/// A run command with no active workspace silently targets the default (real)
/// account, where an empty inventory reads as "nothing recorded" rather than
/// "you forgot to pick a workspace". Disambiguate on stderr.
fn warn_when_unscoped(ctx: &AppContext) {
    if ctx.workspace.is_none() {
        tracing::warn!(
            "no active workspace; targeting the default account. Set KRAKEN_WORKSPACE or \
             --workspace to scope to a paper workspace"
        );
    }
}

/// List the scope's sessions with their derived status — an offline inventory
/// (no prices fetched). Damaged session.json files are described, not fatal.
fn list(ctx: &AppContext) -> Result<CommandOutput> {
    warn_when_unscoped(ctx);
    let (scope, ledger) = scope_dir(ctx)?;
    let rows: Vec<SessionRecord> = kraken_workspace::session::list(&scope)?;
    let table: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                r.session.to_string(),
                r.label.clone().unwrap_or_else(|| "-".to_string()),
                r.status.to_string(),
                r.source.clone().unwrap_or_else(|| "live".to_string()),
                r.experiment.clone().unwrap_or_default(),
                r.started_at
                    .map(kraken_recording::format_instant)
                    .unwrap_or_else(|| "-".to_string()),
                r.ended_at
                    .map(kraken_recording::format_instant)
                    .unwrap_or_else(|| "-".to_string()),
            ]
        })
        .collect();
    let headers = vec![
        "Session".to_string(),
        "Label".to_string(),
        "Status".to_string(),
        MARKET_DATA_HEADER.to_string(),
        "Experiment".to_string(),
        "Started".to_string(),
        "Ended".to_string(),
    ];
    let sessions = serde_json::to_value(&rows)?;
    let mut out = CommandOutput::new(serde_json::json!({ "sessions": sessions }), headers, table);
    if let SessionLedger::Allocation(workspace) = &ledger {
        workspace_guard::warn_live_workspace(workspace);
        out.stamp_live_scope(&workspace.name);
        // The listing describes a live scope, but rows anchored before
        // promotion are paper history — name them, or the scope stamp
        // presents every past run as real-money evidence. Tolerant like the
        // rest of the listing: an unreadable journal costs the disclosure,
        // never the inventory (show/score on the session still fail loud).
        let live_since = workspace_guard::live_since(&scope).unwrap_or_else(|err| {
            tracing::warn!(%err, "could not read the journal for the paper-era disclosure");
            None
        });
        let paper_era: Vec<String> = rows
            .iter()
            .filter(|row| {
                workspace_guard::predates_live(row.ended_at.or(row.started_at), live_since)
            })
            .map(|row| row.session.to_string())
            .collect();
        out.stamp_paper_era_sessions(&paper_era);
    }
    Ok(out)
}

/// Read one session's decision log verbatim — the counter a looping agent
/// gates its rounds on, without ever learning the on-disk layout. Offline: no
/// network, no DB; an empty (or never-written) log is `count: 0`, never an
/// error, so "no rounds yet" stays distinguishable from a missing session.
fn decisions(session_ref: &str, ctx: &AppContext) -> Result<CommandOutput> {
    let spec: SessionRefSpec = session_ref.parse().unwrap_or(SessionRefSpec::Latest);
    let (scope, _) = scope_dir(ctx)?;
    let (session_id, _) = kraken_workspace::session::resolve(&scope, &spec)?;
    let decisions = kraken_session::session::read_decisions(&scope, session_id.ordinal())
        .map_err(kraken_workspace::WorkspaceError::from)?;

    let headers = ["Time", "Kind", "Symbol", "Reason", "Order"]
        .map(String::from)
        .to_vec();
    let rows: Vec<Vec<String>> = decisions
        .iter()
        .map(|d| {
            vec![
                kraken_recording::format_instant(d.timestamp),
                d.kind.to_string(),
                d.symbol.clone().unwrap_or_else(|| "-".to_string()),
                d.reason.clone(),
                d.order_id.clone().unwrap_or_else(|| "-".to_string()),
            ]
        })
        .collect();
    let mut out = CommandOutput::new(
        serde_json::json!({
            "group": "session",
            "session": session_id.to_string(),
            "count": decisions.len(),
            "decisions": serde_json::to_value(&decisions)?,
        }),
        headers,
        rows,
    );
    if let Some(workspace) = ctx.workspace.as_deref() {
        out.stamp_workspace(workspace);
    }
    Ok(out)
}

/// Read a session's typed state cell — the loop's O(1) grounding read.
/// Unset reads as the empty cursor with a null `updated_at`, loudly
/// distinguishable from a missing session (a validation refusal).
fn state_get(session_ref: &str, ctx: &AppContext) -> Result<CommandOutput> {
    let spec: SessionRefSpec = session_ref.parse().unwrap_or(SessionRefSpec::Latest);
    let (scope, _) = scope_dir(ctx)?;
    let (session_id, _) = kraken_workspace::session::resolve(&scope, &spec)?;
    let state = kraken_session::session::read_state(&scope, session_id.ordinal())
        .map_err(kraken_workspace::WorkspaceError::from)?;
    state_output(session_id, state, "state", ctx)
}

/// Replace the ACTIVE session's state cell — working memory belongs to a
/// live window, so with nothing recording this refuses exactly like `note`.
fn state_set(
    cursor: kraken_session::session::SessionCursor,
    ctx: &AppContext,
) -> Result<CommandOutput> {
    let (scope, _) = scope_dir(ctx)?;
    let Some((session_id, _)) = kraken_workspace::session::active(&scope)? else {
        return Err(kraken_workspace::WorkspaceError::NoActiveSession.into());
    };
    let state = kraken_session::session::write_state(&scope, session_id.ordinal(), cursor)
        .map_err(kraken_workspace::WorkspaceError::from)?;
    state_output(session_id, Some(state), "state_set", ctx)
}

/// One render for get and set: the stamped cursor, unset fields omitted.
fn state_output(
    session_id: SessionId,
    state: Option<kraken_session::session::SessionState>,
    kind: &str,
    ctx: &AppContext,
) -> Result<CommandOutput> {
    let (updated_at, cursor) = state
        .map(|s| (Some(s.updated_at), s.cursor))
        .unwrap_or_default();
    let pairs = vec![
        ("Session".to_string(), session_id.to_string()),
        (
            "Updated".to_string(),
            updated_at
                .map(kraken_recording::format_instant)
                .unwrap_or_else(|| "-".to_string()),
        ),
        ("Cursor".to_string(), serde_json::to_string(&cursor)?),
    ];
    let mut out = CommandOutput::key_value(
        pairs,
        serde_json::json!({
            "group": "session",
            "type": kind,
            "session": session_id.to_string(),
            "updated_at": updated_at,
            "cursor": serde_json::to_value(&cursor)?,
        }),
    );
    if let Some(workspace) = ctx.workspace.as_deref() {
        out.stamp_workspace(workspace);
    }
    Ok(out)
}

/// Append a decision (the "why") to the ACTIVE session's `decisions.jsonl`. No
/// network or DB; refuses when nothing is recording — a rationale must land
/// in the window it explains.
fn note(
    kind: DecisionKind,
    symbol: Option<String>,
    reason: String,
    order_id: Option<String>,
    ctx: &AppContext,
) -> Result<CommandOutput> {
    let (scope, _) = scope_dir(ctx)?;
    let Some((session_id, _)) = kraken_workspace::session::active(&scope)? else {
        return Err(kraken_workspace::WorkspaceError::NoActiveSession.into());
    };
    let decision = crate::session::decision::append(
        &session_decisions_path(&scope, session_id.ordinal()),
        kind,
        symbol,
        reason,
        order_id,
    )?;
    note_output(session_id, &decision)
}

/// Build the `note` result: the appended decision as JSON plus a compact table.
fn note_output(run: SessionId, decision: &Decision) -> Result<CommandOutput> {
    let pairs = vec![
        ("Session".to_string(), run.to_string()),
        ("Kind".to_string(), decision.kind.to_string()),
        (
            "Symbol".to_string(),
            decision.symbol.clone().unwrap_or_else(|| "-".to_string()),
        ),
        ("Reason".to_string(), decision.reason.clone()),
    ];
    let decision_json = serde_json::to_value(decision)?;
    Ok(CommandOutput::key_value(
        pairs,
        serde_json::json!({
            "group": "session",
            "type": "note_appended",
            "session": run.to_string(),
            "decision": decision_json,
        }),
    ))
}

#[cfg(test)]
mod speed_tests {
    use super::ensure_speed;

    #[test]
    fn speed_bounds_reject_non_finite_and_out_of_range() {
        for bad in [f64::NAN, f64::INFINITY, 0.0, 0.009, 1000.1] {
            assert!(ensure_speed(bad).is_err(), "{bad}");
        }
        for ok in [0.01, 1.0, 1000.0] {
            assert!(ensure_speed(ok).is_ok(), "{ok}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kraken_contract::value_types::SinkTarget;
    use kraken_session::manifest::PaperRef;
    use rust_decimal_macros::dec;

    fn manifest(recordings: Vec<RecordingRef>) -> SessionManifest {
        SessionManifest::new(
            "s1".to_string(),
            "0.0.0-test",
            4242,
            recordings,
            PaperRef {
                starting_balance: dec!(10_000),
                currency: "USD".to_string(),
            },
            SessionWindow {
                session: "s1".to_string(),
                started_at: "2026-01-01T00:00:00Z".parse().unwrap(),
                opening_equity: dec!(10_000),
                opening_complete: true,
                ended_at: None,
            },
        )
        .with_label(Some("momentum-s1".to_string()))
    }

    fn jsonl_recording() -> RecordingRef {
        RecordingRef {
            backend: RecordingBackend::Jsonl,
            file: "tape.jsonl".to_string(),
            schema_version: schema::SCHEMA_VERSION.to_string(),
            symbols: vec!["BTC/USD".to_string()],
            channels: vec!["ticker".to_string()],
        }
    }

    /// Regression: an unreachable venue yields an empty `complete: false`
    /// snapshot. `reconcile` must not adopt it — doing so durably wipes the
    /// master journal and closes `session stop` at near-zero equity.
    #[test]
    fn reconcile_keeps_balances_when_the_venue_snapshot_is_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let mut account =
            PaperAccount::open_at(dir.path().join("journal.jsonl"), Origin::Cli).unwrap();
        account
            .commit(
                vec![AccountEvent::Reset(kraken_paper::PaperConfig {
                    balance: dec!(10_000),
                    currency: "USD".to_string(),
                    fee_rate: dec!(0),
                    slippage_rate: dec!(0),
                })],
                CommandEntry {
                    name: "seed".to_string(),
                    ..CommandEntry::default()
                },
            )
            .unwrap();
        let before = account.state().unwrap().balances.clone();

        reconcile(
            &mut account,
            Vec::new(),
            VenueSnapshot {
                balances: Default::default(),
                complete: false,
                anchor: None,
            },
            "2026-01-01T00:05:00Z".parse().unwrap(),
        )
        .unwrap();

        assert_eq!(
            account.state().unwrap().balances,
            before,
            "an incomplete venue snapshot must never overwrite the journal balances"
        );
    }

    /// The demo's sink contract: the stdout echo keeps `kraken playground`
    /// visibly alive, and JSONL as the sole durable keeps its window
    /// replayable via `--from session:s<n>` (replay resolution is JSONL-only).
    #[test]
    fn demo_plan_echoes_to_stdout_and_records_a_replayable_jsonl_tape() {
        let demo = Start::demo(
            vec!["BTC/USD".to_string()],
            "recipe-playground-dca".to_string(),
            "1h".to_string(),
        );
        let plan = resolve_targets(&demo.to);
        assert!(plan.echo, "the demo must stream its tape to stdout");
        assert_eq!(plan.durables, vec![RecordFormat::Jsonl]);
    }

    /// Provenance is first-class on the grounding surface: a replayed session's
    /// `show` names its tape and speed in both renders, and a live session says
    /// so explicitly — replay evidence can never masquerade as live.
    #[test]
    fn show_output_carries_the_sessions_provenance() {
        use kraken_paper::PaperState;
        use kraken_session::manifest::SessionSource;

        let mut replayed = manifest(vec![jsonl_recording()]);
        replayed.source = Some(SessionSource {
            tape: "tape:jun-crash".to_string(),
            speed: 4.0,
            anchor: "2026-01-01T00:00:00Z".parse().unwrap(),
            started_at: "2026-01-01T00:00:01Z".parse().unwrap(),
            content_hash: None,
        });
        let out = show_output(
            &replayed,
            SessionStatus::Stopped,
            &PaperState::default(),
            dec!(10_000),
            true,
        )
        .unwrap();
        assert_eq!(out.data["source"]["tape"], "tape:jun-crash");
        assert!(
            out.rows
                .iter()
                .any(|r| r[0] == MARKET_DATA_HEADER && r[1] == "replay @4x · tape:jun-crash"),
            "rows: {:?}",
            out.rows
        );

        let live = manifest(vec![jsonl_recording()]);
        let out = show_output(
            &live,
            SessionStatus::Recording,
            &PaperState::default(),
            dec!(10_000),
            true,
        )
        .unwrap();
        assert_eq!(out.data["source"], serde_json::Value::Null);
        assert!(
            out.rows
                .iter()
                .any(|r| r[0] == MARKET_DATA_HEADER && r[1] == "live")
        );
    }

    /// The strategy stamp reads back through `show`: the knobs a
    /// driver was started with must not be write-only. A session without one
    /// stays clean — null in JSON, placeholder rows in the table.
    #[test]
    fn show_output_carries_the_strategy_stamp() {
        use kraken_paper::PaperState;
        use kraken_session::manifest::StrategyRef;

        let mut driven = manifest(vec![jsonl_recording()]);
        driven.strategy = Some(StrategyRef {
            name: "recipe-playground-price-alert".to_string(),
            params: Some(serde_json::json!({"upper": 70000})),
        });
        let out = show_output(
            &driven,
            SessionStatus::Stopped,
            &PaperState::default(),
            dec!(10_000),
            true,
        )
        .unwrap();
        assert_eq!(
            out.data["strategy"]["name"],
            "recipe-playground-price-alert"
        );
        assert_eq!(out.data["strategy"]["params"]["upper"], 70000);
        assert!(
            out.rows
                .iter()
                .any(|r| r[0] == "Strategy" && r[1] == "recipe-playground-price-alert"),
            "rows: {:?}",
            out.rows
        );
        assert!(
            out.rows
                .iter()
                .any(|r| r[0] == "Params" && r[1].contains("70000"))
        );

        let bare = manifest(vec![jsonl_recording()]);
        let out = show_output(
            &bare,
            SessionStatus::Stopped,
            &PaperState::default(),
            dec!(10_000),
            true,
        )
        .unwrap();
        assert_eq!(out.data["strategy"], serde_json::Value::Null);
        assert!(out.rows.iter().any(|r| r[0] == "Strategy" && r[1] == "-"));
    }

    /// The seal that keeps a stopped session's numbers honest: `session show`
    /// counts the fills the windowed read yields — never the whole account.
    /// Two fills in the window fold to two, whatever else the account did
    /// outside it.
    #[test]
    fn windowed_state_folds_only_the_windows_account_events() {
        use kraken_core::OrderSide;
        use kraken_paper::account::Origin;
        use kraken_session::timeline::{
            SessionTimeline,
            fixtures::{SessionBuilder, manifest},
        };

        let mut builder = SessionBuilder::with_rates(dec!(10_000), dec!(0), dec!(0));
        let mark = (dec!(100), dec!(100));
        builder.market_fill(
            "2026-01-01T00:00:01Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTC/USD",
            dec!(0.001),
            mark,
        );
        builder.market_fill(
            "2026-01-01T00:00:02Z",
            Origin::Cli,
            OrderSide::Sell,
            "BTC/USD",
            dec!(0.001),
            mark,
        );
        let timeline = SessionTimeline {
            events: builder.events,
            capture: None,
            manifest: manifest(vec![]),
            newer_records_skipped: 0,
        };

        let state = windowed_state(&timeline.events);
        assert_eq!(
            state.filled_trades.len(),
            2,
            "only the two fills inside the window are counted"
        );
    }

    #[test]
    fn resolve_strategy_requires_a_name_for_params() {
        let params = serde_json::Map::new();
        assert!(resolve_strategy(None, Some(&params)).is_err());
        assert!(resolve_strategy(Some(""), None).is_err());
        let rounds = serde_json::json!({"rounds": 10});
        let strategy = resolve_strategy(Some("dca"), rounds.as_object())
            .unwrap()
            .unwrap();
        assert_eq!(strategy.name, "dca");
        assert_eq!(strategy.params, Some(rounds));
        assert!(resolve_strategy(None, None).unwrap().is_none());
    }

    #[test]
    fn start_requires_a_durable_backend() {
        let plan = resolve_targets(&[SinkTarget::Stdout]);
        let err = ensure_durable_plan(&plan).unwrap_err();
        assert!(err.to_string().contains("record durably"), "{err}");
        let plan = resolve_targets(&[SinkTarget::Jsonl, SinkTarget::Stdout]);
        assert!(ensure_durable_plan(&plan).is_ok());
    }

    #[test]
    fn finalize_closes_the_window_and_anchors_pnl_on_opening_equity() {
        let closed = finalize(
            manifest(vec![]),
            dec!(10_150),
            "2026-01-01T01:00:00Z".parse().unwrap(),
        );
        assert_eq!(closed.status, SessionState::Stopped);
        let summary = closed.summary.expect("summary stamped");
        assert_eq!(summary.final_value, dec!(10_150));
        assert_eq!(summary.pnl, dec!(150), "pnl = final - opening_equity");
        assert_eq!(closed.window.ended_at, Some(summary.ended_at));
    }

    #[test]
    fn derived_status_reconciles_the_window_against_recorder_liveness() {
        let dir = tempfile::tempdir().unwrap();
        // Open window, no lock on tape.jsonl: aborted.
        let open = manifest(vec![jsonl_recording()]);
        assert_eq!(derived_status(dir.path(), &open), SessionStatus::Aborted);
        // Held tape lock: recording.
        let _lock =
            kraken_recording::FileLock::acquire(&dir.path().join("tape.jsonl"), "test recorder")
                .unwrap();
        assert_eq!(derived_status(dir.path(), &open), SessionStatus::Recording);
        // A closed window is stopped regardless of locks.
        let closed = finalize(open, dec!(10_000), Utc::now());
        assert_eq!(derived_status(dir.path(), &closed), SessionStatus::Stopped);
    }

    #[test]
    fn readback_block_never_tells_an_aborted_session_to_stop() {
        // A stopped session reads back; the others are blocked with guidance
        // that matches reality — the aborted case must not advise a stop that
        // can never happen (the process is gone), the recording case must.
        assert_eq!(readback_block(SessionStatus::Stopped, "s1"), None);

        let recording = readback_block(SessionStatus::Recording, "s1").unwrap();
        assert!(
            recording.contains("kraken session stop"),
            "recording is stoppable — keep the retry cue: {recording}"
        );

        let aborted = readback_block(SessionStatus::Aborted, "s1").unwrap();
        assert!(
            !aborted.contains("kraken session stop"),
            "an aborted session can't be stopped — never advise it: {aborted}"
        );
        assert!(aborted.contains("aborted"), "name the cause: {aborted}");
    }

    #[test]
    fn readback_block_guidance_names_no_specific_verb() {
        // Four verbs share this guard (explain, lab score, lab curve, recipe
        // create) — guidance promising any one of them misleads callers of
        // the others.
        for status in [
            SessionStatus::Recording,
            SessionStatus::Aborted,
            SessionStatus::Damaged,
        ] {
            let blocked = readback_block(status, "s1").unwrap();
            for verb in ["explain", "score", "curve", "recipe"] {
                assert!(
                    !blocked.contains(verb),
                    "guidance must stay verb-neutral, found '{verb}': {blocked}"
                );
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn await_recorder_exit_returns_when_the_lock_is_released() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = manifest(vec![jsonl_recording()]);
        let tape = dir.path().join("tape.jsonl");
        let lock = kraken_recording::FileLock::acquire(&tape, "test recorder").unwrap();
        let waiter = await_recorder_exit(
            dir.path(),
            &manifest,
            Duration::from_secs(5),
            Duration::from_millis(10),
        );
        drop(lock);
        waiter.await.expect("released lock ends the wait");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn await_recorder_exit_times_out_while_the_lock_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = manifest(vec![jsonl_recording()]);
        let tape = dir.path().join("tape.jsonl");
        let _lock = kraken_recording::FileLock::acquire(&tape, "test recorder").unwrap();
        let err = await_recorder_exit(
            dir.path(),
            &manifest,
            Duration::from_millis(50),
            Duration::from_millis(10),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, KrakenError::Timeout(_)), "{err}");
    }

    #[tokio::test]
    async fn await_run_sealed_returns_once_the_recorder_stamps_ended_at() {
        let dir = tempfile::tempdir().unwrap();
        let run_file = dir.path().join("session.json");
        let mut sealed = manifest(vec![jsonl_recording()]);
        sealed.window.ended_at = Some("2026-01-01T00:01:00Z".parse().unwrap());
        sealed.save(&run_file).unwrap();
        let result =
            await_session_sealed(&run_file, Duration::from_secs(5), Duration::from_millis(10))
                .await;
        assert!(result.is_some(), "a sealed session.json ends the wait");
    }

    #[tokio::test]
    async fn await_run_sealed_times_out_when_the_recorder_never_seals() {
        let dir = tempfile::tempdir().unwrap();
        let run_file = dir.path().join("session.json");
        // Unsealed: `ended_at` stays None, as it would if the recorder died
        // mid-finalize — the supervisor must fall back to closing it itself.
        manifest(vec![jsonl_recording()]).save(&run_file).unwrap();
        let result = await_session_sealed(
            &run_file,
            Duration::from_millis(50),
            Duration::from_millis(10),
        )
        .await;
        assert!(
            result.is_none(),
            "an unsealed session.json times out to None"
        );
    }

    #[test]
    fn a_venue_fill_keeps_its_txid_amounts_and_sub_second_time() {
        let history = serde_json::from_value(serde_json::json!({
            "trades": {
                "TAAAAA-11111-000001": {
                    "ordertxid": "OAAAAA-11111-000001",
                    "pair": "XXBTZUSD",
                    "time": 1767225600.5,
                    "type": "buy",
                    "ordertype": "limit",
                    "price": "50000.0",
                    "cost": "5000.0",
                    "fee": "13.0",
                    "vol": "0.1",
                    "misc": ""
                }
            },
            "count": 1
        }))
        .unwrap();
        let fills = venue_fills(history);
        assert_eq!(fills[0].txid, "TAAAAA-11111-000001");
        assert_eq!(fills[0].volume, dec!(0.1));
        assert_eq!(fills[0].side, kraken_core::OrderSide::Buy);
        assert_eq!(
            fills[0].time,
            DateTime::from_timestamp(1767225600, 500_000_000).unwrap()
        );
    }
}

delegate_execute!(SessionCommand { Start, Request });
delegate_execute!(SessionRequestCommand {
    Stop,
    Show,
    List,
    Note,
    Decisions,
    State
});
delegate_execute!(StateCommand { Get, Set });

//! Recorded windows over a scope's shared account journal.
//!
//! This module owns the artifact and reader; allocation and reference policy
//! remain above the storage layer.

use crate::{
    Result, SessionError,
    manifest::{RecordingBackend, SessionManifest},
    timeline::{SessionTimeline, event::TimelineEvent},
};
use chrono::{DateTime, Utc};
use kraken_paper::{
    AccountEvent, PaperState, VenueSnapshot, WindowAnchor,
    account::{AccountRecord, RECORD_VERSION},
};
use kraken_recording::{JsonlSource, line_seq};
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::path::{Path, PathBuf};

/// The session contract file inside a session directory.
pub const SESSION_FILE: &str = "session.json";

/// The only journal commands allowed to carry a session stamp.
pub const START_MARKER: &str = "session start";
pub const STOP_MARKER: &str = "session stop";

/// `<scope>/sessions/` — every session of the scope's journal.
pub fn sessions_root(scope: &Path) -> PathBuf {
    scope.join("sessions")
}

/// `<scope>/sessions/s<n>/`.
pub fn session_dir(scope: &Path, ordinal: u32) -> PathBuf {
    sessions_root(scope).join(format!("s{ordinal}"))
}

/// `<scope>/sessions/s<n>/session.json`.
pub fn session_file_path(scope: &Path, ordinal: u32) -> PathBuf {
    session_dir(scope, ordinal).join(SESSION_FILE)
}

/// `<scope>/sessions/s<n>/decisions.jsonl` — the session's own rationale log,
/// the one [`read_window`] joins to fills.
pub fn session_decisions_path(scope: &Path, ordinal: u32) -> PathBuf {
    session_dir(scope, ordinal).join("decisions.jsonl")
}

/// The agent state cell inside a session directory — the loop's durable
/// cursor, distinct from the evidential decision log.
pub const STATE_FILE: &str = "state.json";

/// Cap on the serialized state document: it is a cursor, not a database.
/// Sized generously above any sane round/leg/zone bookkeeping.
pub const MAX_STATE_BYTES: usize = 64 * 1024;

/// `<scope>/sessions/s<n>/state.json`.
pub fn session_state_path(scope: &Path, ordinal: u32) -> PathBuf {
    session_dir(scope, ordinal).join(STATE_FILE)
}

/// Agent-owned loop state, intentionally excluded from evidence and seals.
///
/// The cursor nests under its own key (never flattened): serde's
/// `deny_unknown_fields` — the typo guard this artifact depends on — does
/// not compose with `flatten`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionState {
    /// Stamped by [`write_state`] on every replace.
    pub updated_at: DateTime<Utc>,
    pub cursor: SessionCursor,
}

/// Typed cursor fields that reject configuration typos.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCursor {
    /// The last COMPLETED round — a firing acts as `round + 1` and writes it
    /// back once done (DCA and rebalance gates).
    pub round: Option<u32>,
    /// Pairs already executed in the round in flight — rebalance crash
    /// recovery resumes at the first missing leg.
    pub legs_done: Option<Vec<String>>,
    /// Price zone for crossing-based strategies (alert on the change, not
    /// the level).
    pub zone: Option<Zone>,
    /// Instant of the loop's last order — spacing gates (`min_spacing_s`).
    pub last_action_at: Option<DateTime<Utc>>,
    /// Free-text context for a human reading the artifact; never parsed.
    pub note: Option<String>,
}

/// Where price sits relative to a strategy's band.
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum Zone {
    Above,
    Within,
    Below,
}

/// Reads an optional state cell, refusing damage instead of assuming round
/// zero.
///
/// No read lock: the cell is replaced atomically as a whole file, so a
/// concurrent read sees the old or the new document, never a torn one.
pub fn read_state(scope: &Path, ordinal: u32) -> Result<Option<SessionState>> {
    let path = session_state_path(scope, ordinal);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let state: SessionState = serde_json::from_str(&raw).map_err(|err| {
        SessionError::Damaged(format!(
            "session state at {} is unreadable ({err}); fix or delete the file",
            path.display()
        ))
    })?;
    Ok(Some(state))
}

/// Replaces the state atomically; loop construction guarantees one writer.
pub fn write_state(scope: &Path, ordinal: u32, cursor: SessionCursor) -> Result<SessionState> {
    let state = SessionState {
        updated_at: Utc::now(),
        cursor,
    };
    let size = serde_json::to_vec(&state)
        .map_err(|err| SessionError::Rejected(format!("session state is unserializable: {err}")))?
        .len();
    if size > MAX_STATE_BYTES {
        return Err(SessionError::Rejected(format!(
            "session state is {size} bytes; the cap is {MAX_STATE_BYTES} — keep it a cursor, not \
             a database"
        )));
    }
    kraken_recording::write_json_atomic(&session_state_path(scope, ordinal), &state)?;
    Ok(state)
}

/// Reads known decisions, skipping forward-compatible kinds with a warning.
pub fn read_decisions(scope: &Path, ordinal: u32) -> Result<Vec<crate::decision::Decision>> {
    let path = session_decisions_path(scope, ordinal);
    let _guard = crate::timeline::read_guard(&path, "session decision log")?;
    let Some(source) = JsonlSource::<crate::decision::Decision>::open(path.clone()) else {
        return Ok(Vec::new());
    };
    let mut decisions = Vec::new();
    for (line, decision) in source.read_numbered()? {
        if matches!(decision.kind, crate::decision::DecisionKind::Unknown) {
            tracing::warn!(path = %path.display(), line, "skipping unknown decision kind");
            continue;
        }
        decisions.push(decision);
    }
    Ok(decisions)
}

/// The session's market recording file name for one backend: `tape.<ext>`.
pub fn tape_file(backend: RecordingBackend) -> String {
    match backend {
        RecordingBackend::Duckdb => "tape.duckdb".to_string(),
        RecordingBackend::Jsonl => "tape.jsonl".to_string(),
    }
}

/// Parse a strict session ordinal: `s<n>`, digits only, no leading zeros, n ≥
/// 1. Anything else — including `r0`, `r01`, `R3` — is not a session directory.
pub fn parse_ordinal(name: &str) -> Option<u32> {
    let digits = name.strip_prefix('s')?;
    if digits.is_empty() || digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The three files a windowed read spans, resolved by the caller — the
/// reader takes paths, not layout policy, so it never learns what a
/// workspace is.
pub struct SessionTracks {
    /// The scope's shared account journal.
    pub journal: PathBuf,
    /// The session's directory: `decisions.jsonl` and `tape.*` live here.
    pub session_dir: PathBuf,
    pub manifest: SessionManifest,
}

/// Read one session's window of the shared journal as a [`SessionTimeline`], so
/// every timeline consumer (explain, score, replay) works on sessions
/// unchanged.
///
/// The strict journal prefix becomes one synthetic `Attached` epoch at the
/// frozen opening anchor. The window, decisions, and tape then share the
/// timeline's `(at, track, seq)` order.
pub fn read_window(tracks: SessionTracks) -> Result<SessionTimeline> {
    let SessionTracks {
        journal,
        session_dir,
        manifest: run,
    } = tracks;

    let mut events = Vec::new();
    let capture = crate::timeline::read_market_track_in(&session_dir, &run, &mut events)?;

    // Writer-order locks keep account events and their rationale in one snapshot.
    let decisions = session_dir.join("decisions.jsonl");
    let _journal_guard = crate::timeline::read_guard(&journal, "paper account journal")?;
    let _decisions_guard = crate::timeline::read_guard(&decisions, "session decision log")?;
    crate::timeline::read_decisions_track(&decisions, &mut events)?;
    let newer_records_skipped = read_windowed_account_track(&journal, &run, &mut events)?;

    events.sort_by_key(TimelineEvent::key);
    Ok(SessionTimeline {
        events,
        capture,
        manifest: run,
        newer_records_skipped,
    })
}

/// This session's own account-journal window — the synthesized anchor epoch
/// plus the journal slice `(start..stop]` — WITHOUT reading the market tape.
///
/// [`read_window`] reads the tape too, which a *recording* session holds
/// locked; but the counts a live `session show` reports (trades, open orders)
/// are account-journal events, never tape frames. This is the tape-free read
/// that is therefore safe on open and stopped sessions alike.
pub fn read_account_window(
    journal: &Path,
    manifest: &SessionManifest,
) -> Result<Vec<TimelineEvent>> {
    let mut events = Vec::new();
    let _guard = crate::timeline::read_guard(journal, "paper account journal")?;
    read_windowed_account_track(journal, manifest, &mut events)?;
    events.sort_by_key(TimelineEvent::key);
    Ok(events)
}

/// Fold the prefix, synthesize the anchor epoch, slice the window.
fn read_windowed_account_track(
    journal: &Path,
    run: &SessionManifest,
    events: &mut Vec<TimelineEvent>,
) -> Result<usize> {
    let session_id = run.window.session.as_str();
    let Some(source) = JsonlSource::<AccountRecord>::open(journal.to_path_buf()) else {
        // No journal at all: the synthesized epoch alone is the window —
        // an account that never traded before its first session.
        events.push(synthesized_epoch(run, PaperState::default(), 0));
        return Ok(0);
    };

    let records: Vec<(usize, AccountRecord)> = source.read_numbered()?.collect();
    let start = records
        .iter()
        .position(|(_, record)| is_marker(record, START_MARKER, session_id))
        .ok_or_else(|| {
            SessionError::Rejected(format!(
                "session '{session_id}' has no start marker in the journal — the journal may have \
                 been replaced since the session was recorded"
            ))
        })?;
    let stop = records[start + 1..]
        .iter()
        .position(|(_, record)| is_marker(record, STOP_MARKER, session_id))
        .map(|offset| start + 1 + offset);

    // The opening state: everything up to AND including the start marker,
    // folded strictly. A record from a newer build here would silently skew
    // the opening balances, so it refuses like the account's own projection.
    let mut opening = PaperState::default();
    for (line, record) in &records[..=start] {
        if record.v > RECORD_VERSION {
            return Err(SessionError::Rejected(format!(
                "journal line {line} was written by a newer kraken (v{}); upgrade to read this \
                 session's opening state",
                record.v
            )));
        }
        opening.apply(record.ts, &record.event);
    }
    let marker_seq = line_seq(records[start].0);
    events.push(synthesized_epoch(run, opening, marker_seq));

    // The window slice, `(start..stop]`: tolerant like the whole-session
    // read — a newer record is dropped and disclosed, and money consumers
    // refuse on the count.
    let end = stop.map_or(records.len(), |s| s + 1);
    let mut newer_skipped = 0usize;
    for (line, record) in &records[start + 1..end] {
        if record.v > RECORD_VERSION {
            tracing::warn!(journal = %journal.display(), line, v = record.v, "skipping newer-version journal record");
            newer_skipped += 1;
            continue;
        }
        if matches!(record.event, AccountEvent::Unknown) {
            tracing::warn!(journal = %journal.display(), line, "skipping unknown account event kind");
            continue;
        }
        events.push(TimelineEvent::account(
            record.ts,
            line_seq(*line),
            record.clone(),
        ));
    }
    Ok(newer_skipped)
}

fn is_marker(record: &AccountRecord, name: &str, session_id: &str) -> bool {
    matches!(
        &record.event,
        AccountEvent::Command(entry)
            if entry.name == name && entry.session.as_deref() == Some(session_id)
    )
}

/// The one in-memory epoch a window opens with: the folded opening balances
/// as an `Attached` snapshot carrying the frozen `session.json` anchor. Stamped
/// at `started_at` with the start marker's own seq, so it sorts before every
/// sliced event (their journal lines are all later) and never reaches disk.
fn synthesized_epoch(run: &SessionManifest, opening: PaperState, seq: i64) -> TimelineEvent {
    let snapshot = VenueSnapshot {
        balances: opening.balances.into_iter().collect(),
        complete: run.window.opening_complete,
        anchor: Some(WindowAnchor {
            equity: run.window.opening_equity,
            currency: run.paper.currency.clone(),
            fee_rate: opening.fee_rate,
            slippage_rate: opening.slippage_rate,
        }),
    };
    TimelineEvent::account(
        run.window.started_at,
        seq,
        AccountRecord {
            v: RECORD_VERSION,
            ts: run.window.started_at,
            origin: kraken_paper::account::Origin::Cli,
            event: AccountEvent::Attached(snapshot),
        },
    )
}

#[cfg(test)]
mod state_tests {
    use super::*;

    #[test]
    fn state_round_trips_typed_and_omits_unset_fields_on_the_wire() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cursor = SessionCursor {
            round: Some(3),
            zone: Some(Zone::Above),
            legs_done: Some(vec!["BTC/USD".to_string()]),
            ..SessionCursor::default()
        };
        let written = write_state(dir.path(), 1, cursor.clone()).expect("write");
        let read = read_state(dir.path(), 1).expect("read").expect("present");
        assert_eq!(read, written);
        assert_eq!(read.cursor, cursor);

        // Unset cases stay off the wire, so the artifact reads as exactly
        // what the loop tracks.
        let raw =
            std::fs::read_to_string(session_state_path(dir.path(), 1)).expect("state on disk");
        assert!(raw.contains("\"zone\": \"above\""), "{raw}");
        assert!(!raw.contains("last_action_at"), "{raw}");
    }

    #[test]
    fn unset_state_reads_none_and_damaged_state_refuses_loud() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(read_state(dir.path(), 1).expect("read").is_none());

        std::fs::create_dir_all(session_dir(dir.path(), 1)).expect("mkdir");
        std::fs::write(session_state_path(dir.path(), 1), "{ not json").expect("write");
        let err = read_state(dir.path(), 1).expect_err("must refuse");
        assert!(
            matches!(err, SessionError::Damaged(_)),
            "a corrupt cursor must never read as round zero: {err}"
        );
    }

    #[test]
    fn unknown_cursor_keys_refuse_instead_of_steering_silently() {
        // A typo'd case ("rond") must fail the read, not vanish: this file is
        // agent-authored configuration, not tolerated ingest.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(session_dir(dir.path(), 1)).expect("mkdir");
        std::fs::write(
            session_state_path(dir.path(), 1),
            r#"{"updated_at":"2026-01-01T00:00:00Z","cursor":{"rond":3}}"#,
        )
        .expect("write");
        let err = read_state(dir.path(), 1).expect_err("must refuse");
        assert!(err.to_string().contains("rond"), "{err}");
    }

    #[test]
    fn oversized_state_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cursor = SessionCursor {
            note: Some("x".repeat(MAX_STATE_BYTES)),
            ..SessionCursor::default()
        };
        let err = write_state(dir.path(), 1, cursor).expect_err("oversized");
        assert!(err.to_string().contains("cursor, not a database"), "{err}");
    }
}

//! Durable session metadata and lifecycle state.
//!
//! Additive fields and serde defaults preserve minor-version compatibility.

use super::{MANIFEST_VERSION, Result, SessionError, is_compatible};
use chrono::{DateTime, Utc};
use kraken_recording::write_json_atomic;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::path::Path;

/// The durable description of a recorded session.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionManifest {
    /// Manifest schema version; a differing MAJOR is incompatible (see
    /// [`super::is_compatible`]).
    #[serde(default = "default_manifest_version")]
    pub manifest_version: String,
    /// CLI version that wrote the manifest.
    pub cli_version: String,
    /// The session name (its directory under `sessions/`).
    pub id: String,
    /// PID of the background recorder, so `stop` can signal it.
    pub pid: u32,
    /// The market recordings captured into this session — one per durable
    /// backend.
    pub recordings: Vec<RecordingRef>,
    /// The isolated paper account seeded for this session.
    pub paper: PaperRef,
    /// The strategy driving the session, if one was declared.
    pub strategy: Option<StrategyRef>,
    /// Opaque experiment label; session provisioning does not interpret it.
    pub experiment: Option<String>,
    /// Present for replay; absence is the authoritative live/replay
    /// discriminator.
    pub source: Option<SessionSource>,
    /// Lifecycle status; persisted intent only — the effective status is
    /// derived from the recording lock at read time.
    #[serde(default)]
    pub status: SessionState,
    /// End-of-run summary, present once the session is finalized.
    pub summary: Option<SessionOutcome>,
    /// Instant the session was created.
    pub created_at: DateTime<Utc>,
    /// Optional human handle (`--label`), resolvable like an ordinal.
    pub label: Option<String>,
    /// The window this manifest cut into its journal.
    pub window: SessionWindow,
}

/// The window cut into the journal. Written at start, closed at stop; the
/// opening anchor is frozen here and never recomputed, so a score today and
/// a score next month anchor on the same number.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionWindow {
    /// The window's own id — also the value its journal markers carry.
    pub session: String,
    pub started_at: DateTime<Utc>,
    /// The account's equity when the window opened, in the account currency.
    #[serde(with = "rust_decimal::serde::str")]
    pub opening_equity: Decimal,
    /// False when the opening valuation could not price every position —
    /// disclosed, never guessed.
    pub opening_complete: bool,
    /// `None` while the window is open (or crashed without a stop).
    pub ended_at: Option<DateTime<Utc>>,
}

/// Pointer to the market recording a session captured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingRef {
    /// Storage backend the tape was written with.
    pub backend: RecordingBackend,
    /// Tape file name relative to the session directory.
    pub file: String,
    /// `record` schema version at write time
    /// ([`kraken_recording::schema::SCHEMA_VERSION`]).
    pub schema_version: String,
    /// Trading pairs captured (e.g. `["BTC/USD","ETH/USD"]`).
    pub symbols: Vec<String>,
    /// Channels captured (e.g. `["trade","book","ohlc"]`).
    pub channels: Vec<String>,
}

/// The store a recording was written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum RecordingBackend {
    /// The embedded DuckDB store (`record-duckdb` feature).
    Duckdb,
    /// The dependency-free JSONL sink.
    Jsonl,
}

/// The starting shape of a session's isolated paper account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaperRef {
    /// Opening balance in [`currency`](PaperRef::currency).
    pub starting_balance: Decimal,
    /// Quote currency of the opening balance (e.g. `"USD"`).
    pub currency: String,
}

/// The strategy driving a session's decisions.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrategyRef {
    /// Strategy identifier (e.g. a recipe skill name).
    pub name: String,
    /// Opaque strategy parameters, passed through verbatim.
    pub params: Option<serde_json::Value>,
}

/// Persisted lifecycle state; OS locks determine whether a recorder is actually
/// live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, strum::Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum SessionState {
    /// The recorder is running (the manifest's initial state).
    #[default]
    Running,
    /// The recorder no longer holds its locks — stopped, crashed, or killed.
    Stopped,
}

/// Replay provenance and the affine mapping between tape and session time.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSource {
    /// The tape ref in display form (`tape:<name>` / `run:<ref>`).
    pub tape: String,
    /// Playback speed: tape seconds per session wall second.
    pub speed: f64,
    /// The first tape frame's instant — the affine anchor t₀.
    pub anchor: DateTime<Utc>,
    /// The session wall instant the pump started — the affine anchor w₀.
    pub started_at: DateTime<Utc>,
    /// sha256 of the source tape this session replayed — the session's own
    /// record of the bytes the lab FSM checks against a sealed replay leg.
    /// `None` on a manifest written before hashes were recorded.
    pub content_hash: Option<String>,
}

impl SessionSource {
    /// Map a tape instant into session wall time: `w₀ + (t − t₀)/speed`.
    pub fn session_time(&self, tape: DateTime<Utc>) -> DateTime<Utc> {
        let elapsed = (tape - self.anchor).num_microseconds().unwrap_or(i64::MAX);
        // Manifests are editable; invalid speed and anchors must not panic readers.
        let scaled = if self.speed > 0.0 {
            (elapsed as f64 / self.speed) as i64
        } else {
            0
        };
        self.started_at
            .checked_add_signed(chrono::Duration::microseconds(scaled))
            .unwrap_or(DateTime::<Utc>::MAX_UTC)
    }

    /// Map a session wall instant onto the tape: `t₀ + (w − w₀)·speed`.
    pub fn tape_time(&self, session: DateTime<Utc>) -> DateTime<Utc> {
        let elapsed = (session - self.started_at)
            .num_microseconds()
            .unwrap_or(i64::MAX);
        let scaled = (elapsed as f64 * self.speed) as i64;
        self.anchor
            .checked_add_signed(chrono::Duration::microseconds(scaled))
            .unwrap_or(DateTime::<Utc>::MAX_UTC)
    }
}

/// The end-of-run summary written when a session is finalized.
///
/// Named `SessionOutcome` (not `Summary`) to avoid confusion with the unrelated
/// `CaptureSummary` in the binary's sink driver.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionOutcome {
    /// Instant the session ended.
    pub ended_at: DateTime<Utc>,
    /// Portfolio value at the close, in the paper account's currency.
    pub final_value: Decimal,
    /// Profit/loss against the starting balance.
    pub pnl: Decimal,
}

fn default_manifest_version() -> String {
    MANIFEST_VERSION.to_string()
}

impl SessionManifest {
    /// A fresh manifest for a starting session: version and creation time
    /// stamped; `Running` with no summary yet. `cli_version` is the writing
    /// binary's version — a parameter, so the crate never learns where the
    /// caller keeps it.
    pub fn new(
        id: String,
        cli_version: impl Into<String>,
        pid: u32,
        recordings: Vec<RecordingRef>,
        paper: PaperRef,
        window: SessionWindow,
    ) -> Self {
        Self {
            manifest_version: MANIFEST_VERSION.to_string(),
            cli_version: cli_version.into(),
            id,
            pid,
            recordings,
            paper,
            strategy: None,
            experiment: None,
            source: None,
            status: SessionState::Running,
            summary: None,
            created_at: Utc::now(),
            label: None,
            window,
        }
    }

    /// Attach the human handle (`--label`), resolvable like an ordinal.
    pub fn with_label(mut self, label: Option<String>) -> Self {
        self.label = label;
        self
    }

    /// Label the manifest with the template that drove it, so
    /// replay/explain-pnl/lab can attribute the session to a hypothesis — the
    /// playground itself runs no strategy.
    pub fn with_strategy(mut self, strategy: Option<StrategyRef>) -> Self {
        self.strategy = strategy;
        self
    }

    /// Link the session to a lab experiment. The label is the durable run
    /// link `discover_sessions` trusts over the session-name convention.
    pub fn with_experiment(mut self, experiment: Option<String>) -> Self {
        self.experiment = experiment;
        self
    }

    /// Stamp replay provenance — this is what flips every reader onto the
    /// tape-marked, REST-free path.
    pub fn with_source(mut self, source: Option<SessionSource>) -> Self {
        self.source = source;
        self
    }

    /// Read and validate a manifest from `path`. A read failure is `Io`, an
    /// undecodable manifest `Damaged`, an incompatible MAJOR
    /// `IncompatibleManifest`; the guard is feature-free so `status`/`list`
    /// work without the `duckdb` backend.
    pub fn load(path: &Path) -> Result<Self> {
        let data = std::fs::read_to_string(path)?;
        let manifest: Self =
            serde_json::from_str(&data).map_err(|e| SessionError::Damaged(e.to_string()))?;
        if !is_compatible(&manifest.manifest_version) {
            return Err(SessionError::IncompatibleManifest {
                found: manifest.manifest_version,
                expected: MANIFEST_VERSION.to_string(),
            });
        }
        Ok(manifest)
    }

    /// Atomically write this manifest to `path`.
    pub fn save(&self, path: &Path) -> Result<()> {
        Ok(write_json_atomic(path, self)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn sample() -> SessionManifest {
        SessionManifest {
            manifest_version: MANIFEST_VERSION.to_string(),
            cli_version: "0.3.2".to_string(),
            id: "btc-dip".to_string(),
            pid: 4242,
            recordings: vec![RecordingRef {
                backend: RecordingBackend::Jsonl,
                file: "BTC-USD.jsonl".to_string(),
                schema_version: "2.0".to_string(),
                symbols: vec!["BTC/USD".to_string()],
                channels: vec!["trade".to_string(), "book".to_string()],
            }],
            paper: PaperRef {
                starting_balance: dec!(10_000.0),
                currency: "USD".to_string(),
            },
            strategy: Some(StrategyRef {
                name: "recipe-playground-dca".to_string(),
                params: Some(serde_json::json!({ "interval_s": 60 })),
            }),
            experiment: Some("dca-robust-1".to_string()),
            source: Some(SessionSource {
                tape: "tape:jun-crash".to_string(),
                speed: 10.0,
                anchor: "2026-06-01T00:00:00Z".parse().unwrap(),
                started_at: "2026-07-02T10:00:00Z".parse().unwrap(),
                content_hash: Some("sha256:jun-crash".to_string()),
            }),
            status: SessionState::Stopped,
            summary: Some(SessionOutcome {
                ended_at: "2026-07-02T10:30:00Z".parse().unwrap(),
                final_value: dec!(10_250.5),
                pnl: dec!(250.5),
            }),
            created_at: "2026-07-02T10:00:00Z".parse().unwrap(),
            label: Some("dca-robust-1-s1".to_string()),
            window: SessionWindow {
                session: "s1".to_string(),
                started_at: "2026-07-02T10:00:00Z".parse().unwrap(),
                opening_equity: dec!(10_000.0),
                opening_complete: true,
                ended_at: Some("2026-07-02T10:30:00Z".parse().unwrap()),
            },
        }
    }

    #[test]
    fn manifest_serializes_with_the_pinned_1_0_keys() {
        // Encode-side pin: a serialize-side key rename would pass the round-trip
        // and decode-fixture tests unnoticed.
        let json = serde_json::to_value(sample()).unwrap();
        let pinned = serde_json::json!({
            "manifest_version": "1.0",
            "cli_version": "0.3.2",
            "id": "btc-dip",
            "pid": 4242,
            "recordings": [{
                "backend": "jsonl",
                "file": "BTC-USD.jsonl",
                "schema_version": "2.0",
                "symbols": ["BTC/USD"],
                "channels": ["trade", "book"]
            }],
            "paper": { "starting_balance": 10000.0, "currency": "USD" },
            "strategy": {
                "name": "recipe-playground-dca",
                "params": { "interval_s": 60 }
            },
            "experiment": "dca-robust-1",
            "source": {
                "tape": "tape:jun-crash",
                "speed": 10.0,
                "anchor": "2026-06-01T00:00:00Z",
                "started_at": "2026-07-02T10:00:00Z",
                "content_hash": "sha256:jun-crash"
            },
            "status": "stopped",
            "summary": {
                "ended_at": "2026-07-02T10:30:00Z",
                "final_value": 10250.5,
                "pnl": 250.5
            },
            "created_at": "2026-07-02T10:00:00Z",
            "label": "dca-robust-1-s1",
            "window": {
                "session": "s1",
                "started_at": "2026-07-02T10:00:00Z",
                "opening_equity": "10000.0",
                "opening_complete": true,
                "ended_at": "2026-07-02T10:30:00Z"
            }
        });
        assert_eq!(json, pinned);
    }

    #[test]
    fn unknown_fields_and_missing_optionals_use_defaults() {
        // A lean manifest from a future build: an unknown key, and every optional
        // (strategy/summary) plus the defaulted version/status omitted.
        let json = r#"{
            "cli_version": "0.9.9",
            "id": "s1",
            "pid": 7,
            "recordings": [{
                "backend": "duckdb",
                "file": "BTC-USD.duckdb",
                "schema_version": "2.0",
                "symbols": ["BTC/USD"],
                "channels": ["trade"]
            }],
            "paper": { "starting_balance": 500.0, "currency": "EUR" },
            "window": {
                "session": "s1",
                "started_at": "2026-07-02T10:00:00Z",
                "opening_equity": "500.0",
                "opening_complete": true,
                "ended_at": null
            },
            "created_at": "2026-07-02T10:00:00Z",
            "future_field": {"added": "later"}
        }"#;
        let manifest: SessionManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.manifest_version, MANIFEST_VERSION);
        assert_eq!(manifest.status, SessionState::Running);
        assert!(manifest.strategy.is_none());
        assert!(manifest.experiment.is_none());
        assert!(manifest.source.is_none());
        assert!(manifest.summary.is_none());
    }

    #[test]
    fn load_rejects_incompatible_major_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");

        let mut manifest = sample();
        manifest.manifest_version = "2.0".to_string(); // future MAJOR
        manifest.save(&path).unwrap();
        assert!(SessionManifest::load(&path).is_err());

        manifest.manifest_version = "1.7".to_string(); // newer MINOR, same MAJOR
        manifest.save(&path).unwrap();
        assert!(SessionManifest::load(&path).is_ok());
    }

    #[test]
    fn affine_mapping_round_trips_between_tape_and_session_time() {
        let source = SessionSource {
            tape: "tape:jun-crash".to_string(),
            speed: 10.0,
            anchor: "2026-06-01T00:00:00Z".parse().unwrap(),
            started_at: "2026-07-02T10:00:00Z".parse().unwrap(),
            content_hash: None,
        };
        // One tape minute at 10× lands six session seconds after w₀.
        let tape: DateTime<Utc> = "2026-06-01T00:01:00Z".parse().unwrap();
        let session = source.session_time(tape);
        assert_eq!(
            session,
            "2026-07-02T10:00:06Z".parse::<DateTime<Utc>>().unwrap()
        );
        assert_eq!(source.tape_time(session), tape);
    }

    #[test]
    fn save_then_load_round_trips_through_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/manifest.json"); // parent created by save
        let manifest = sample();
        manifest.save(&path).unwrap();
        assert_eq!(SessionManifest::load(&path).unwrap(), manifest);
    }
}

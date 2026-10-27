//! Application facade for experiment planning, scoring, and comparison.
//!
//! Orchestration lives here; the binary only resolves scope and renders models.

use crate::{
    LabError, Result, Scorecard, SessionRef, Verdict,
    experiment::{Criteria, Experiment, PlannedSession, SessionPlan},
    plan::{NextAction, NextState, SessionObservation, SessionStatus},
    replay::Replay,
    sha256_file,
};
use kraken_recording::{Source, TapeBackend, TapeRef, TapeSource, TapeState, describe_tape};
use kraken_session::{
    manifest::SessionManifest,
    session::{SessionTracks, read_window, session_dir, session_file_path},
};
use std::path::PathBuf;

/// Local copy of the scope contract; this lower layer cannot depend on
/// workspaces.
const JOURNAL_FILE: &str = "journal.jsonl";

/// The lab's entry point. Owns the scope root, so the binary threads it once
/// at construction rather than into every call.
pub struct Lab {
    base: PathBuf,
    /// Global tape library, distinct from workspace-local experiment state.
    library: PathBuf,
}

/// A scored run, optionally judged against a frozen experiment.
#[derive(Debug)]
pub struct ScoredSession {
    pub card: Scorecard,
    /// Present only when [`Lab::score`] was given an experiment to judge
    /// against; carries the seal so the CLI can attribute the verdict.
    pub judged: Option<Judged>,
}

/// A scorecard's verdict against a named, sealed experiment.
#[derive(Debug)]
pub struct Judged {
    pub experiment: String,
    pub frozen: String,
    pub verdict: Verdict,
}

/// Every discovered run of an experiment, scored and judged side by side.
#[derive(Debug)]
pub struct Comparison {
    pub experiment: String,
    pub frozen: String,
    pub outcomes: Vec<SessionResult>,
}

/// One session's column: the session it names plus its mechanical result.
#[derive(Debug)]
pub struct SessionResult {
    pub session: SessionRef,
    pub result: OutcomeResult,
}

/// A run either scored-and-judged, or a named failure — never an omission.
#[derive(Debug)]
pub enum OutcomeResult {
    Scored {
        card: Box<Scorecard>,
        verdict: Verdict,
    },
    /// `category: detail`, pre-formatted so the CLI renders a named failure
    /// column verbatim without knowing the crate's error taxonomy.
    Failed { error: String },
}

impl Comparison {
    /// Mechanically-scored, passing sessions. Aborted or unscoreable runs never
    /// count — the promotion gate's direct input.
    pub fn pass_count(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| {
                matches!(&outcome.result, OutcomeResult::Scored { verdict, .. } if verdict.pass)
            })
            .count()
    }

    pub fn total(&self) -> usize {
        self.outcomes.len()
    }
}

impl Lab {
    pub fn new(base: impl Into<PathBuf>) -> Self {
        let base = base.into();
        Self {
            library: base.clone(),
            base,
        }
    }

    /// Point `tape:<name>` resolution at the global shared library — the
    /// binary passes the config root whenever the lab runs inside a
    /// workspace scope.
    #[must_use]
    pub fn with_library(mut self, library: impl Into<PathBuf>) -> Self {
        self.library = library.into();
        self
    }

    /// Freeze a pre-registered experiment: resolve and validate the session
    /// plan (binding each replay leg to its sealed bytes), then seal and
    /// store it.
    pub fn freeze(
        &self,
        name: &str,
        hypothesis: &str,
        strategy: Option<String>,
        criteria: Criteria,
        runs: &[String],
    ) -> Result<Experiment> {
        let session_plan = self.resolve_session_plan(runs)?;
        crate::store::freeze(
            &self.base,
            name,
            hypothesis,
            strategy,
            criteria,
            session_plan,
        )
    }

    /// Load a frozen experiment, verifying its seal.
    /// Every sealed experiment in this scope — the promotion checklist's
    /// evidence walk starts here.
    pub fn experiments(&self) -> Result<Vec<String>> {
        crate::store::experiments(&self.base)
    }

    pub fn show(&self, name: &str) -> Result<Experiment> {
        crate::store::load(&self.base, name)
    }

    /// The marked equity fold with its full point series — what
    /// `Scorecard.curve` summarizes. Exposed so consumers can compute their
    /// own metrics (Sharpe, rolling drawdown) without re-deriving the fold
    /// from the journal and tape. The whole [`Replay`] comes back, not just
    /// the curve: `epochs_skipped` and `caveats` are the fold's honesty
    /// disclosures, and a series without them would misrepresent a
    /// reset-truncated or hole-y recording. Refuses exactly what scoring
    /// refuses — no summary, a partial journal, no account epoch — raising
    /// the same `ReplayError` variants, so the refusal text matches
    /// `lab score`'s byte for byte.
    pub fn curve(&self, ordinal: u32) -> Result<Replay> {
        let manifest = SessionManifest::load(&session_file_path(&self.base, ordinal))?;
        if manifest.summary.is_none() {
            return Err(kraken_replay::ReplayError::NotStopped {
                session: manifest.id,
            }
            .into());
        }
        let timeline = read_window(SessionTracks {
            journal: self.base.join(JOURNAL_FILE),
            session_dir: session_dir(&self.base, ordinal),
            manifest,
        })?;
        if timeline.newer_records_skipped > 0 {
            return Err(kraken_replay::ReplayError::NewerJournal {
                skipped: timeline.newer_records_skipped,
            }
            .into());
        }
        crate::replay(&timeline).map_err(|err| match err {
            // Scoring surfaces this condition through explain-pnl's variant;
            // raise the same one so the two verbs refuse identically.
            LabError::NoEpoch { session } => kraken_replay::ReplayError::NoEpoch { session }.into(),
            other => other,
        })
    }

    /// Scores a stopped session, verifying any experiment seal before folding.
    pub fn score(&self, ordinal: u32, judge_against: Option<&str>) -> Result<ScoredSession> {
        let experiment = judge_against
            .map(|name| crate::store::load(&self.base, name))
            .transpose()?;
        let card = self.read_and_score(ordinal)?;
        let judged = experiment.map(|experiment| Judged {
            verdict: experiment.criteria.evaluate(&card),
            experiment: experiment.name,
            frozen: experiment.frozen,
        });
        Ok(ScoredSession { card, judged })
    }

    /// Compare every discovered run of an experiment: each re-scored from disk
    /// and judged against the sealed criteria, failures kept as named columns.
    /// Runs attribute to the sealed name, so discovery keys off that identity.
    pub fn compare(&self, name: &str) -> Result<Comparison> {
        let experiment = crate::store::load(&self.base, name)?;
        let outcomes = crate::store::discover_sessions(&self.base, &experiment.name)?
            .into_iter()
            .map(|run| {
                let result = self.score_and_judge(run.ordinal, &experiment);
                SessionResult {
                    session: run,
                    result,
                }
            })
            .collect();
        Ok(Comparison {
            experiment: experiment.name,
            frozen: experiment.frozen,
            outcomes,
        })
    }

    /// What the sealed plan says to do now. When it proposes starting a replay
    /// leg, the sealed tape is re-checked here too — so `lab next` never hands
    /// out a start command for a tape that has drifted from its seal.
    pub fn next(&self, name: &str) -> Result<NextAction> {
        let experiment = crate::store::load(&self.base, name)?;
        let observations = self.observe_sessions(&experiment.name)?;
        let action = crate::plan::next_action(&experiment, &observations)?;
        if let (NextState::Start, Some(source)) = (action.state, action.source.as_deref()) {
            self.ensure_planned_source(&experiment, source)?;
        }
        Ok(action)
    }

    /// Refuse a replay source whose bytes no longer match the plan's sealed
    /// `content_hash`; a no-op (→ `None`) for an unplanned source. On success
    /// it returns the verified hash so `run start` can stamp the
    /// manifest without re-hashing the same tape. Public because a manual
    /// `run start` enforces the seal through it too.
    pub fn ensure_planned_source(
        &self,
        experiment: &Experiment,
        source: &str,
    ) -> Result<Option<String>> {
        let Some(plan) = &experiment.session_plan else {
            return Ok(None);
        };
        // The plan owns how a replay leg is identified; the guard only hashes
        // and compares, so start- and completion-time enforcement can't drift.
        let Some(sealed) = plan.sealed_replay_hash(source) else {
            return Ok(None);
        };
        let tape: TapeRef = source.parse()?;
        let path = kraken_replay::resolve_source_path(&self.library, &self.base, &tape)?;
        let current = sha256_file(&path)?;
        if current != sealed {
            return Err(LabError::Spec(format!(
                "tape '{source}' no longer matches the frozen plan (sealed {sealed}, current \
                 {current}); a re-recorded tape must not satisfy a frozen session plan — register \
                 a new experiment"
            )));
        }
        Ok(Some(current))
    }

    /// The single scoring seam: read a session's window of the shared journal
    /// and fold it into a scorecard. A future pre-score gate or provenance
    /// check lands here.
    fn read_and_score(&self, ordinal: u32) -> Result<Scorecard> {
        let manifest = SessionManifest::load(&session_file_path(&self.base, ordinal))?;
        let timeline = read_window(SessionTracks {
            journal: self.base.join(JOURNAL_FILE),
            session_dir: session_dir(&self.base, ordinal),
            manifest,
        })?;
        crate::score(&timeline)
    }

    fn score_and_judge(&self, ordinal: u32, experiment: &Experiment) -> OutcomeResult {
        match self.read_and_score(ordinal) {
            Ok(card) => OutcomeResult::Scored {
                verdict: experiment.criteria.evaluate(&card),
                card: Box::new(card),
            },
            Err(err) => OutcomeResult::Failed {
                error: format!("{}: {err}", err.category()),
            },
        }
    }

    /// Observe every discovered session's lifecycle from its `session.json` and
    /// recorder lock. Completed = finalized with a summary; Running = the
    /// recorder still holds a lock on a tape; anything else (crashed, damaged,
    /// summaryless) is Aborted — it can never count toward the plan.
    fn observe_sessions(&self, experiment: &str) -> Result<Vec<SessionObservation>> {
        let mut observations = Vec::new();
        for run in crate::store::discover_sessions(&self.base, experiment)? {
            let run_file = session_file_path(&self.base, run.ordinal);
            let (status, source, content_hash) = match SessionManifest::load(&run_file) {
                Ok(manifest) => {
                    let src = manifest.source.as_ref();
                    let source = src.map(|source| source.tape.clone());
                    let content_hash = src.and_then(|source| source.content_hash.clone());
                    let status = if manifest.summary.is_some() {
                        SessionStatus::Completed
                    } else if self.recorder_alive(run.ordinal, &manifest) {
                        SessionStatus::Running
                    } else {
                        SessionStatus::Aborted
                    };
                    (status, source, content_hash)
                }
                // Discovery already skips a damaged session.json; this arm covers
                // the race where it broke between the two reads.
                Err(err) => {
                    tracing::warn!(session = %run.session, error = %err,
                        "session.json unreadable; treating the session as aborted");
                    (SessionStatus::Aborted, None, None)
                }
            };
            observations.push(SessionObservation {
                ordinal: run.ordinal,
                session: run.session,
                status,
                source,
                content_hash,
            });
        }
        Ok(observations)
    }

    /// Whether a session's recorder still holds a recording lock on a tape in
    /// the session directory — the kernel drops it the instant the process
    /// dies, a signal a recycled pid can't spoof. A lab run always records,
    /// so this is exactly its liveness.
    fn recorder_alive(&self, ordinal: u32, manifest: &SessionManifest) -> bool {
        let dir = session_dir(&self.base, ordinal);
        manifest
            .recordings
            .iter()
            .any(|recording| kraken_recording::is_lock_held(&dir.join(&recording.file)))
    }

    /// Parse and resolve `--session` entries. Replay entries bind name → bytes
    /// here, at freeze time — the only moment that binding is made: the tape
    /// must exist and be finalized, and its content hash goes into the seal so
    /// a re-recorded tape can never satisfy the frozen plan.
    fn resolve_session_plan(&self, runs: &[String]) -> Result<Option<SessionPlan>> {
        if runs.is_empty() {
            return Ok(None);
        }
        let plan = runs
            .iter()
            .map(|run| self.resolve_planned_run(run))
            .collect::<Result<Vec<_>>>()?;
        Ok(Some(SessionPlan(plan)))
    }

    fn resolve_planned_run(&self, run: &str) -> Result<PlannedSession> {
        if let Some(window) = run.strip_prefix("live:") {
            let window = window.trim();
            if window.is_empty() {
                return Err(LabError::Spec(
                    "--session live: needs a window (e.g. live:24h)".into(),
                ));
            }
            // The seal is immutable, so a window `run start --for` can't run
            // would brick the plan. Validate it now with the exact parser
            // that command uses, not just a non-empty check.
            crate::window::parse_window(window).map_err(|why| {
                LabError::Spec(format!(
                    "--session live:{window}: invalid window ({why}) — e.g. live:24h or live:2h \
                     30m"
                ))
            })?;
            return Ok(PlannedSession::Live {
                window: window.to_string(),
            });
        }
        if let Some(tape) = run.strip_prefix("replay:") {
            let tape: TapeRef = tape.parse()?;
            let path = kraken_replay::resolve_source_path(&self.library, &self.base, &tape)?;
            // Only a finalized tape can be sealed: hashing a still-growing file
            // would brick the plan the moment more bytes land.
            let entry = describe_tape(&tape, &path, TapeBackend::Jsonl);
            if entry.state != TapeState::Finalized {
                return Err(LabError::Spec(format!(
                    "tape '{tape}' is not finalized (still recording, or crashed mid-capture); a \
                     sealed session plan needs a finalized tape — see 'kraken tape list'"
                )));
            }
            // Finalized only means a sidecar summary exists, not that any frames
            // were captured. An empty tape hashes and seals fine, but `run start`
            // refuses it ("holds no frames to replay"), so the sealed leg
            // would strand the plan with an un-runnable next step. Require here
            // what the runtime requires: at least one frame.
            let source = TapeSource::open(&path)?;
            if source.read()?.next().is_none() {
                return Err(LabError::Spec(format!(
                    "tape '{tape}' holds no frames to replay; a sealed session plan needs a \
                     non-empty tape — see 'kraken tape list'"
                )));
            }
            return Ok(PlannedSession::Replay {
                tape: tape.to_string(),
                content_hash: sha256_file(&path)?,
            });
        }
        Err(LabError::Spec(format!(
            "invalid --session entry {run:?}: expected live:<window> or replay:<ref>"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use std::path::Path;

    fn criteria() -> Criteria {
        Criteria {
            min_return_pct: Some(dec!(1.0)),
            max_drawdown_pct: None,
            min_fills: Some(1),
        }
    }

    /// Write a finalized JSONL tape under `tapes/<name>`; an empty
    /// `frames` slice models a tape that finalized having captured nothing.
    fn write_finalized_recording(base: &Path, name: &str, frames: &[&str]) {
        let recordings = kraken_recording::tapes_root(base);
        std::fs::create_dir_all(&recordings).unwrap();
        let tape = frames.iter().map(|f| format!("{f}\n")).collect::<String>();
        std::fs::write(recordings.join(format!("{name}.jsonl")), tape).unwrap();
        std::fs::write(
            recordings.join(format!("{name}.jsonl.meta.json")),
            serde_json::json!({
                "schema_version": "1.0",
                "source": "kraken-spot-ws-v2",
                "symbols": ["BTC/USD"],
                "channels": ["ticker"],
                "window_start": "2026-01-01T00:00:00Z",
                "cli_version": "0.0.0-test",
                "window_end": "2026-01-01T00:00:20Z",
                "events_dropped": 0,
                "frames_unparsed": 0,
                "reconnect_count": 0
            })
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn freeze_rejects_a_live_window_the_duration_parser_cannot_run() {
        let base = tempfile::tempdir().unwrap();
        let lab = Lab::new(base.path());
        for bad in ["live:10", "live:soon", "live:10x", "live:2 fortnights"] {
            let err = lab
                .freeze("m1", "h", None, criteria(), &[bad.to_string()])
                .unwrap_err();
            assert!(matches!(err, LabError::Spec(_)), "{bad}: got {err}");
            assert!(err.to_string().contains("invalid window"), "{bad}: {err}");
        }
    }

    #[test]
    fn freeze_accepts_the_windows_the_duration_parser_accepts() {
        let base = tempfile::tempdir().unwrap();
        // Includes a compound span to pin that the seal path carries the space
        // humantime allows straight through to the stored window.
        for (n, good) in ["live:20s", "live:5m", "live:1h", "live:24h", "live:2h 30m"]
            .into_iter()
            .enumerate()
        {
            let lab = Lab::new(base.path());
            let experiment = lab
                .freeze(&format!("m{n}"), "h", None, criteria(), &[good.to_string()])
                .unwrap();
            let plan = experiment.session_plan.expect("a plan was frozen");
            assert!(
                matches!(plan.0.as_slice(), [PlannedSession::Live { .. }]),
                "{good}"
            );
        }
    }

    #[test]
    fn ensure_planned_source_refuses_a_drifted_replay_tape() {
        let base = tempfile::tempdir().unwrap();
        let recordings = kraken_recording::tapes_root(base.path());
        std::fs::create_dir_all(&recordings).unwrap();
        let tape = recordings.join("crash.jsonl");
        std::fs::write(&tape, "the sealed frames\n").unwrap();
        let sealed = sha256_file(&tape).unwrap();
        let experiment = Experiment {
            name: "m1".into(),
            hypothesis: "h".into(),
            strategy: None,
            criteria: Criteria::default(),
            created_at: "2026-01-01T00:00:00Z".parse().unwrap(),
            session_plan: Some(SessionPlan(vec![PlannedSession::Replay {
                tape: "tape:crash".into(),
                content_hash: sealed,
            }])),
            frozen: "sha256:test".into(),
        };
        let lab = Lab::new(base.path());

        // Untouched bytes still satisfy the seal, and the verified hash comes
        // back for the caller to reuse.
        assert!(
            lab.ensure_planned_source(&experiment, "tape:crash")
                .unwrap()
                .is_some()
        );

        // Re-recorded under the same ref: refused.
        std::fs::write(&tape, "re-recorded, different bytes\n").unwrap();
        let err = lab
            .ensure_planned_source(&experiment, "tape:crash")
            .unwrap_err();
        assert!(matches!(err, LabError::Spec(_)), "got: {err}");
        assert!(
            err.to_string()
                .contains("no longer matches the frozen plan"),
            "got: {err}"
        );
    }

    #[test]
    fn freeze_rejects_an_empty_finalized_replay_tape() {
        let base = tempfile::tempdir().unwrap();
        write_finalized_recording(base.path(), "empty", &[]);
        let err = Lab::new(base.path())
            .freeze(
                "m1",
                "h",
                None,
                criteria(),
                &["replay:tape:empty".to_string()],
            )
            .unwrap_err();
        assert!(matches!(err, LabError::Spec(_)), "got: {err}");
        assert!(err.to_string().contains("holds no frames"), "got: {err}");
    }

    #[test]
    fn freeze_accepts_a_finalized_replay_tape_with_frames() {
        let base = tempfile::tempdir().unwrap();
        let frame = r#"{"channel":"ticker","type":"update","data":[{"symbol":"BTC/USD","bid":50000.0,"bid_qty":1.0,"ask":50000.0,"ask_qty":1.0,"last":50000.0,"volume":100.0,"vwap":50000.0,"low":50000.0,"high":50000.0,"change":0.0,"change_pct":0.0,"timestamp":"2026-01-01T00:00:00Z"}]}"#;
        write_finalized_recording(base.path(), "full", &[frame]);
        let experiment = Lab::new(base.path())
            .freeze(
                "m1",
                "h",
                None,
                criteria(),
                &["replay:tape:full".to_string()],
            )
            .unwrap();
        let plan = experiment.session_plan.expect("a plan was frozen");
        assert!(matches!(plan.0.as_slice(), [PlannedSession::Replay { .. }]));
    }
}

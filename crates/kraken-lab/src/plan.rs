//! The derived — never stored — next-action machine over a frozen session plan.
//!
//! Pre-registering *which* runs (and how many) closes the last goalpost:
//! without a sealed plan, an agent can keep running until a pass appears and
//! stop there. The plan itself is sealed-spec data and lives in
//! [`experiment`](crate::experiment); progress is always re-derived here from
//! the session directories, so there is no state file to drift.

use crate::{
    LabError, Result,
    experiment::{Experiment, PlannedSession},
};
use serde::Serialize;

/// One discovered session's observed lifecycle, derived by the caller from its
/// `session.json` and the recorder lock — never persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionObservation {
    pub ordinal: u32,
    /// The session's id, `s<n>`.
    pub session: String,
    pub status: SessionStatus,
    /// The replay source ref the session's manifest records; `None` = live.
    pub source: Option<String>,
    /// The sha256 of the tape the session actually replayed; `None` for a live
    /// session or a manifest written before hashes were recorded. Together
    /// with `source` this binds a session to a *sealed* replay entry — a
    /// re-recorded tape has a different hash and so cannot satisfy the
    /// frozen leg.
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    /// The recorder still holds its locks.
    Running,
    /// Finalized with a summary — scoreable evidence.
    Completed,
    /// Stopped or crashed without a summary; never counts toward the plan.
    /// Its ordinal stays claimed forever, so evidence never gets renamed.
    Aborted,
}

/// What the plan says to do now.
#[derive(Debug, Serialize)]
pub struct NextAction {
    pub state: NextState,
    /// The label of the session to start, or the id (`s<n>`) of the one
    /// running.
    pub session: Option<String>,
    /// The `--from` value for a replay entry; `None` for live runs.
    pub source: Option<String>,
    /// A ready-to-run `kraken session start` line, where derivable.
    pub command: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NextState {
    Start,
    Running,
    PlanComplete,
}

impl NextState {
    /// The lowercase tag, identical to the serde `snake_case` repr — the
    /// table renderer wants the string directly, not via a JSON round-trip.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Running => "running",
            Self::PlanComplete => "plan_complete",
        }
    }
}

/// The speed `lab next` proposes for replay entries: fast enough that a
/// recorded day screens in an hour or two, disclosed by the reaction-time
/// caveat; the skill names adjusting it as the one permitted edit.
const PROPOSED_REPLAY_SPEED: f64 = 10.0;

/// Derive the next action from the sealed plan and the observed runs.
/// Entries are consumed by *matching* completed runs, in order — a replay
/// entry only by a session of that tape, a live entry only by a sourceless
/// run — so a crashed run finalized later, or a replay of the wrong tape,
/// can never stand in for the live session the plan still owes. Aborted runs
/// consume nothing but keep their ordinals claimed.
pub(crate) fn next_action(
    experiment: &Experiment,
    runs: &[SessionObservation],
) -> Result<NextAction> {
    let Some(plan) = &experiment.session_plan else {
        return Err(LabError::Spec(format!(
            "experiment '{}' has no session plan; a frozen spec is immutable, so register a new \
             experiment with --session entries to use lab next",
            experiment.name
        )));
    };
    if let Some(running) = runs.iter().find(|run| run.status == SessionStatus::Running) {
        return Ok(NextAction {
            state: NextState::Running,
            session: Some(running.session.clone()),
            source: None,
            command: None,
            reason: format!(
                "session {} is still in progress; stop and score it, then ask again",
                running.session
            ),
        });
    }
    let mut unconsumed: Vec<&SessionObservation> = runs
        .iter()
        .filter(|run| run.status == SessionStatus::Completed)
        .collect();
    let total = plan.0.len();
    let mut satisfied = 0usize;
    let mut pending = None;
    for entry in &plan.0 {
        match unconsumed.iter().position(|run| entry_matches(entry, run)) {
            Some(position) => {
                unconsumed.remove(position);
                satisfied += 1;
            }
            None => {
                pending = Some(entry);
                break;
            }
        }
    }
    let Some(entry) = pending else {
        return Ok(NextAction {
            state: NextState::PlanComplete,
            session: None,
            source: None,
            command: None,
            reason: format!(
                "all {total} planned sessions are complete; run 'kraken lab compare {}'",
                experiment.name
            ),
        });
    };
    // A prediction for the proposed label only: the workspace's run allocator
    // is authoritative at start time (under the journal lock), so a
    // concurrent start simply shifts the label — never the evidence.
    let next_ordinal = runs.iter().map(|run| run.ordinal).max().unwrap_or(0) + 1;
    let label = format!("{}-s{next_ordinal}", experiment.name);
    let aborted = runs
        .iter()
        .filter(|run| run.status == SessionStatus::Aborted)
        .count();
    let strategy = experiment
        .strategy
        .as_deref()
        .map(|s| format!(" --strategy {s}"))
        .unwrap_or_default();
    let (source, market) = match entry {
        // The sealed `live:<window>` is a real timer: --for makes the session
        // stop and finalize itself after the window, so a live leg needs no
        // manual `session stop`. A replay leg ends when its tape runs out.
        // Quote the window: humantime accepts compound spans (`2h 30m`), and an
        // unquoted space would split the copy-pasted command's argv.
        PlannedSession::Live { window } => (
            None,
            format!(" --symbols <SYMBOLS> --channels ticker,trade --for '{window}'"),
        ),
        PlannedSession::Replay { tape, .. } => (Some(tape.clone()), String::new()),
    };
    let source_flags = source
        .as_deref()
        .map(|tape| format!(" --from {tape} --speed {PROPOSED_REPLAY_SPEED}"))
        .unwrap_or_default();
    let command = format!(
        "kraken session start --label {label} --experiment {}{strategy}{source_flags}{market}",
        experiment.name
    );
    let mut reason = format!(
        "start planned session {} of {total} ({})",
        satisfied + 1,
        entry.label()
    );
    if aborted > 0 {
        reason.push_str(&format!(
            "; {aborted} aborted session(s) kept their ordinals and count for nothing"
        ));
    }
    if !unconsumed.is_empty() {
        reason.push_str(&format!(
            "; {} completed session(s) match no pending plan entry and count for nothing",
            unconsumed.len()
        ));
    }
    Ok(NextAction {
        state: NextState::Start,
        session: Some(label),
        source,
        command: Some(command),
        reason,
    })
}

/// Whether a completed run can consume a plan entry: source *identity and
/// bytes*, not arrival order, is the binding. A replay entry needs both the
/// tape ref and the sealed `content_hash` — a session of the right ref but a
/// re-recorded tape carries a different hash and is refused (fail-closed: a
/// run with no recorded hash never satisfies a sealed replay leg). This is the
/// seal enforcement that holds regardless of how the session was started.
fn entry_matches(entry: &PlannedSession, run: &SessionObservation) -> bool {
    match entry {
        PlannedSession::Live { .. } => run.source.is_none(),
        PlannedSession::Replay { tape, content_hash } => {
            run.source.as_deref() == Some(tape)
                && run.content_hash.as_deref() == Some(content_hash.as_str())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experiment::{Criteria, SessionPlan};
    use chrono::Utc;

    fn experiment(plan: Option<SessionPlan>) -> Experiment {
        Experiment {
            name: "m1".to_string(),
            hypothesis: "h".to_string(),
            strategy: Some("recipe-dca".to_string()),
            criteria: Criteria::default(),
            created_at: Utc::now(),
            session_plan: plan,
            frozen: "sha256:test".to_string(),
        }
    }

    fn plan() -> SessionPlan {
        SessionPlan(vec![
            PlannedSession::Replay {
                tape: "tape:jun-crash".to_string(),
                content_hash: "sha256:abc".to_string(),
            },
            PlannedSession::Live {
                window: "24h".to_string(),
            },
        ])
    }

    /// The replay content_hash `plan()` seals — a session must carry it to
    /// consume the replay leg.
    const SEALED_HASH: &str = "sha256:abc";

    fn observed(ordinal: u32, status: SessionStatus) -> SessionObservation {
        SessionObservation {
            ordinal,
            session: format!("s{ordinal}"),
            status,
            source: None,
            content_hash: None,
        }
    }

    fn observed_replay(ordinal: u32, status: SessionStatus, tape: &str) -> SessionObservation {
        observed_replay_hashed(ordinal, status, tape, Some(SEALED_HASH))
    }

    fn observed_replay_hashed(
        ordinal: u32,
        status: SessionStatus,
        tape: &str,
        content_hash: Option<&str>,
    ) -> SessionObservation {
        SessionObservation {
            source: Some(tape.to_string()),
            content_hash: content_hash.map(str::to_string),
            ..observed(ordinal, status)
        }
    }

    #[test]
    fn plan_less_experiment_is_refused_with_the_flag_to_use() {
        let err = next_action(&experiment(None), &[]).unwrap_err();
        assert!(err.to_string().contains("--session"), "{err}");
    }

    #[test]
    fn fresh_plan_starts_r1_on_the_first_entry() {
        let action = next_action(&experiment(Some(plan())), &[]).unwrap();
        assert_eq!(action.state, NextState::Start);
        assert_eq!(action.session.as_deref(), Some("m1-s1"));
        assert_eq!(action.source.as_deref(), Some("tape:jun-crash"));
        // The full proposed line, pinned: the skill quotes it verbatim.
        assert_eq!(
            action.command.as_deref(),
            Some(
                "kraken session start --label m1-s1 --experiment m1 --strategy recipe-dca --from \
                 tape:jun-crash --speed 10"
            )
        );
    }

    #[test]
    fn a_running_run_blocks_the_next_start() {
        let action = next_action(
            &experiment(Some(plan())),
            &[observed(1, SessionStatus::Running)],
        )
        .unwrap();
        assert_eq!(action.state, NextState::Running);
        assert_eq!(action.session.as_deref(), Some("s1"));
        assert!(action.command.is_none());
    }

    #[test]
    fn a_sourceless_run_cannot_consume_a_replay_entry() {
        let action = next_action(
            &experiment(Some(plan())),
            &[observed(1, SessionStatus::Completed)],
        )
        .unwrap();
        assert_eq!(action.state, NextState::Start);
        assert_eq!(
            action.source.as_deref(),
            Some("tape:jun-crash"),
            "the replay entry is still pending"
        );
        assert_eq!(action.session.as_deref(), Some("m1-s2"));
    }

    #[test]
    fn aborted_runs_claim_ordinals_but_consume_no_entry() {
        let action = next_action(
            &experiment(Some(plan())),
            &[
                observed(1, SessionStatus::Aborted),
                observed_replay(2, SessionStatus::Completed, "tape:jun-crash"),
            ],
        )
        .unwrap();
        assert_eq!(action.state, NextState::Start);
        // Entry 2 of 2 (live), on a fresh ordinal — never a reused one.
        assert_eq!(action.session.as_deref(), Some("m1-s3"));
        assert!(action.reason.contains("aborted"), "{}", action.reason);
        // The live leg carries its sealed window as a real --for timer, quoted
        // so a compound span survives the copy-pasted argv.
        let command = action.command.unwrap();
        assert!(command.contains("--for '24h'"), "{command}");
        assert!(
            command.contains("--symbols <SYMBOLS> --channels ticker,trade"),
            "{command}"
        );
    }

    #[test]
    fn a_complete_plan_points_at_compare() {
        let action = next_action(
            &experiment(Some(plan())),
            &[
                observed_replay(1, SessionStatus::Completed, "tape:jun-crash"),
                observed(2, SessionStatus::Completed),
            ],
        )
        .unwrap();
        assert_eq!(action.state, NextState::PlanComplete);
        assert!(action.reason.contains("lab compare m1"));
    }

    #[test]
    fn a_replay_run_of_the_wrong_bytes_does_not_consume_the_leg() {
        // The seal's core promise: a completed run of the right tape ref but
        // a re-recorded tape (different hash) — or one carrying no hash at all —
        // must NOT satisfy the sealed replay leg. The plan still owes it, so the
        // run counts for nothing regardless of how it was started.
        for hash in [Some("sha256:tampered"), None] {
            let action = next_action(
                &experiment(Some(plan())),
                &[observed_replay_hashed(
                    1,
                    SessionStatus::Completed,
                    "tape:jun-crash",
                    hash,
                )],
            )
            .unwrap();
            assert_eq!(action.state, NextState::Start, "hash {hash:?}");
            assert_eq!(
                action.source.as_deref(),
                Some("tape:jun-crash"),
                "the sealed replay leg is still owed (hash {hash:?})"
            );
            assert_eq!(action.session.as_deref(), Some("m1-s2"), "hash {hash:?}");
            assert!(
                action.reason.contains("count for nothing"),
                "hash {hash:?}: {}",
                action.reason
            );
        }
    }
}

#![doc = include_str!("../README.md")]

mod app;
mod experiment;
mod hash;
pub mod mark;
mod plan;
mod replay;
mod score;
mod store;
mod trades;
mod window;

pub use app::{Comparison, Judged, Lab, OutcomeResult, ScoredSession, SessionResult};
pub use experiment::{Check, Criteria, Experiment, PlannedSession, SessionPlan, Verdict};
pub use hash::sha256_file;
// The planner types cross upward (the CLI renders `NextAction`), but the
// planner and the store are orchestration primitives the `Lab` facade owns:
// the binary reaches them only through `Lab`, so `next_action`/`freeze`/`load`/
// `discover_sessions` stay crate-internal and the seam is enforced by the compiler.
pub use plan::{NextAction, NextState, SessionObservation, SessionStatus};
pub use replay::{Curve, Point, Replay, replay};
pub use score::{CurveSummary, Metrics, ScoreSource, Scorecard, TradeStats, score};
pub use store::{SessionRef, experiments_root};
pub use window::parse_window;

pub type Result<T, E = LabError> = std::result::Result<T, E>;

/// The lab layer's failure modes. The binary maps each variant onto the JSON
/// envelope exactly once, in its `errors` module.
#[derive(Debug, thiserror::Error)]
pub enum LabError {
    /// A window with no account epoch — folding from a default account would
    /// fabricate an equity line for money that never existed.
    /// → envelope `validation`.
    #[error("session '{session}' has no paper account epoch in its window; nothing to replay")]
    NoEpoch { session: String },
    /// A frozen experiment that doesn't exist. → envelope `validation`.
    #[error("experiment '{0}' not found; run 'kraken lab new {0}' first")]
    ExperimentNotFound(String),
    /// A freeze under a name already sealed — a spec is immutable by design.
    /// → envelope `validation`.
    #[error(
        "experiment '{0}' already exists; a frozen spec is immutable — choose a new name or show \
         it with 'kraken lab show {0}'"
    )]
    ExperimentExists(String),
    /// A spec the lab refuses to freeze (bad name, no hypothesis, no
    /// criteria). → envelope `validation`.
    #[error("{0}")]
    Spec(String),
    /// Stored lab data (an experiment file) the schema can't decode — or
    /// whose frozen hash no longer matches. → envelope `parse`.
    #[error("{0}")]
    Damaged(String),
    /// Filesystem failure around the lab's store. → envelope `io`.
    #[error(transparent)]
    Store(#[from] std::io::Error),
    /// A recording the plan resolution touched (a replay tape ref, its
    /// catalog entry, or its frames) could not be read; the envelope mapping
    /// delegates to the recording bridge.
    #[error(transparent)]
    Recording(#[from] kraken_recording::Error),
    /// Reading a session's session timeline failed; the envelope mapping
    /// delegates to the session bridge. The crate now reads sessions itself
    /// (the facade scores and compares from disk), so this failure mode is
    /// first-class.
    #[error(transparent)]
    Session(#[from] kraken_session::SessionError),
    /// A scorecard's explain-pnl input failed; the envelope mapping delegates
    /// to the replay bridge (NotStopped→validation, Emit→io, Render→parse).
    #[error(transparent)]
    Replay(#[from] kraken_replay::ReplayError),
}

impl LabError {
    /// The stable envelope category this failure maps to. Kept in sync with
    /// the binary's error bridge so the crate can label a failed comparison
    /// column ("validation: …") without a round-trip through the CLI error
    /// type. The binary's `category_codes_match_the_envelope_contract` test
    /// pins the wire strings.
    pub fn category(&self) -> &'static str {
        use kraken_replay::ReplayError;
        use kraken_session::SessionError;
        match self {
            Self::NoEpoch { .. }
            | Self::ExperimentNotFound(_)
            | Self::ExperimentExists(_)
            | Self::Spec(_) => "validation",
            Self::Damaged(_) => "parse",
            // Recording failures only surface during plan resolution (the New
            // command's top-level error, mapped precisely by the binary's
            // recording bridge), never in a compare column, so a coarse "io"
            // here is a label of last resort, not a contract.
            Self::Store(_) | Self::Recording(_) => "io",
            Self::Session(e) => match e {
                SessionError::Rejected(_) => "validation",
                SessionError::IncompatibleManifest { .. } => "config",
                SessionError::Damaged(_) => "parse",
                SessionError::Recording(_) | SessionError::Io(_) => "io",
            },
            Self::Replay(e) => match e {
                ReplayError::NotStopped { .. }
                | ReplayError::NoEpoch { .. }
                | ReplayError::Source(_) => "validation",
                ReplayError::NewerJournal { .. } => "config",
                ReplayError::Emit(_) => "io",
                ReplayError::Render(_) => "parse",
            },
        }
    }
}

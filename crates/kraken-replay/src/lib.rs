#![doc = include_str!("../README.md")]

mod explain;
mod playback;
mod source;

pub use explain::{
    explain_pnl,
    report::{
        Anchor, ComponentKind, ComponentLine, Coverage, EndMarkSource, MissedFill,
        MissedFillSection, MissedOutcome, OriginSlice, PnlReport, SymbolWindow, TradeLine,
        WindowSummary,
    },
};
pub use playback::{MAX_SPEED, MIN_SPEED, PlaybackClock, replay_events};
pub use source::resolve_source_path;

pub type Result<T, E = ReplayError> = std::result::Result<T, E>;

/// The replay layer's failure modes. The binary maps each variant onto the
/// JSON envelope exactly once, in its `errors` module.
#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    /// A run with no final summary to anchor a P&L explanation on. The
    /// hint is byte-stable — it is the agent skill's retry cue.
    /// → envelope `validation`.
    #[error(
        "session '{session}' has no final summary to anchor on; run 'kraken session stop' first"
    )]
    NotStopped { session: String },
    /// Journal records newer than this build can read: a P&L decomposed
    /// from a partial account track would report confident, wrong numbers.
    /// → envelope `config`.
    #[error(
        "{skipped} account journal record(s) are newer than this build supports and were not \
         read; a P&L decomposed from a partial journal would be wrong; upgrade kraken"
    )]
    NewerJournal { skipped: usize },
    /// A journal with no `Initialized`/`Reset` to anchor the final epoch on.
    /// → envelope `validation`.
    #[error(
        "session '{session}' has no account initialization on its journal to anchor a final epoch \
         on"
    )]
    NoEpoch { session: String },
    /// A source ref that resolves to no JSONL file (missing, or DuckDB-only).
    /// → envelope `validation`.
    #[error("{0}")]
    Source(String),
    /// The playback sink failed. → envelope `io`.
    #[error(transparent)]
    Emit(#[from] std::io::Error),
    /// Rendering an event for emission failed. → envelope `parse`.
    #[error(transparent)]
    Render(#[from] serde_json::Error),
}

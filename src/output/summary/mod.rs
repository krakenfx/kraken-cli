//! The compact `key:value` line wire values show in `-o table` output.
//!
//! Presentation only: the wire types live in `kraken-core`; how each renders as a
//! table line is this binary's concern, implemented as a local trait on the foreign
//! types — [`channel`] payloads and [`method`] results, mirroring the crate's split —
//! plus the merged session-[`timeline`] payloads `kraken replay` renders.
use std::fmt::Write as _;

use itertools::Itertools;

mod channel;
mod method;
mod timeline;

/// A compact, single-line `key:value` description of a wire value for `-o table` output.
///
/// Implemented for every wire type that appears in a table row — method-reply results and
/// the channel-data payloads alike — so a caller renders any of them the same way. Build
/// the line with `SummaryLine` rather than hand-rolling `format!`, so absent fields are
/// dropped and the formatting stays uniform. The `-o json` path is unaffected; it always
/// serializes the full typed value.
pub(crate) trait Summarize {
    fn summary(&self) -> String;
}

/// A list of summarizable entries summarizes as each entry's line, joined by `, ` —
/// the one formatting rule for every multi-entry payload and reply.
impl<T: Summarize> Summarize for Vec<T> {
    fn summary(&self) -> String {
        self.iter().map(T::summary).join(", ")
    }
}

/// Builds the compact `key:value` line a value shows in `-o table` output for [`Summarize`].
///
/// Absent fields are skipped, so the line carries only what the value actually has; the
/// full value is always available via `-o json`. Centralized so every type formats — and,
/// for replies, surfaces `warnings` — the same way instead of hand-rolling it. Wraps one
/// growing buffer rather than a joined `Vec<String>`: this sits on the per-frame table
/// render path, where a heap allocation per field is the dominant cost.
#[derive(Default)]
struct SummaryLine(String);

impl SummaryLine {
    /// Append `key:value`.
    fn field(mut self, key: &str, value: impl std::fmt::Display) -> Self {
        if !self.0.is_empty() {
            self.0.push(' ');
        }
        self.0.push_str(key);
        self.0.push(':');
        // fmt::Write on `String` cannot fail; the discard is the infallibility, not
        // a swallowed error.
        let _ = write!(self.0, "{value}");
        self
    }

    /// Append `key:value` only when `value` is present.
    fn opt(self, key: &str, value: Option<impl std::fmt::Display>) -> Self {
        match value {
            Some(value) => self.field(key, value),
            None => self,
        }
    }

    /// Append `warnings:N` when the reply carries any — the operator's cue to re-run with
    /// `-o json` for the messages. An empty list reads as no warnings.
    fn warnings(self, warnings: Option<&[String]>) -> Self {
        self.opt(
            "warnings",
            warnings.filter(|w| !w.is_empty()).map(<[String]>::len),
        )
    }

    fn build(self) -> String {
        self.0
    }
}

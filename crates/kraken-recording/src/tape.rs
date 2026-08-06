//! Tape identity and the read-only catalog behind `kraken tape list`.
//!
//! A printed [`TapeRef`] is a public contract: the replay `--source` flag
//! and sealed lab session plans consume the exact `Display` form, so the grammar
//! is explicit-prefix only (`tape:<name>` for the global library,
//! `run:<ref>` for a session's own tape) and parse/display round-trip by law.
//! Growth (content hashes, time slices) must be additive to the string form.

use std::fmt;
use std::path::Path;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::error::{Error, Result};
use crate::frames::capture::{RecordingIntegrity, load_sidecar, tapes_root};
use crate::lock::is_lock_held;

/// A namespaced pointer to recorded market data: a standalone `kraken record`
/// tape, or the tape a playground session captured for itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TapeRef {
    Recording { name: String },
    Session { name: String },
}

impl TapeRef {
    pub fn name(&self) -> &str {
        match self {
            Self::Recording { name } | Self::Session { name } => name,
        }
    }
}

impl FromStr for TapeRef {
    type Err = Error;

    /// Explicit prefixes only — no magic resolution order, so a ref means
    /// the same thing in every future consumer.
    fn from_str(s: &str) -> Result<Self> {
        let (namespace, name) = s.split_once(':').ok_or_else(|| {
            Error::Rejected(format!(
                "invalid tape ref {s:?}: expected 'tape:<name>' or 'session:<ref>'"
            ))
        })?;
        if !kraken_core::name::is_safe_name_segment(name) {
            return Err(Error::Rejected(format!(
                "invalid tape name {name:?}: use only letters, digits, '.', '-', or '_' \
                 (no '/', spaces, or '.'/'..')."
            )));
        }
        let name = name.to_owned();
        match namespace {
            "tape" => Ok(Self::Recording { name }),
            "session" => Ok(Self::Session { name }),
            other => Err(Error::Rejected(format!(
                "unknown tape namespace {other:?}: expected 'tape' or 'session'"
            ))),
        }
    }
}

impl fmt::Display for TapeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Recording { name } => write!(f, "tape:{name}"),
            Self::Session { name } => write!(f, "session:{name}"),
        }
    }
}

/// Storage backend of one tape file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TapeBackend {
    Jsonl,
    Duckdb,
}

impl TapeBackend {
    /// The file extension this backend reads and writes.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jsonl => "jsonl",
            Self::Duckdb => "duckdb",
        }
    }

    /// Backends this build can read — the one place the `duckdb` feature
    /// gate lives, so no caller duplicates cfg knowledge.
    pub fn supported() -> &'static [Self] {
        #[cfg(feature = "duckdb")]
        {
            &[Self::Jsonl, Self::Duckdb]
        }
        #[cfg(not(feature = "duckdb"))]
        {
            &[Self::Jsonl]
        }
    }
}

impl fmt::Display for TapeBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.extension())
    }
}

/// What a tape file can say about its completeness at list time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TapeState {
    /// Finalized with integrity counters — replayable as recorded.
    Finalized,
    /// Being written now, or crashed mid-capture; completeness unknown.
    Unfinalized,
    /// Present but unreadable under this build's contract (see `detail`).
    Damaged,
}

/// One catalog row. Never an error by construction: a damaged tape is a
/// described row, so one bad file cannot kill the listing.
#[derive(Debug, Serialize)]
pub struct TapeEntry {
    /// The [`TapeRef`] display form — the string every other command accepts.
    pub tape: String,
    pub backend: TapeBackend,
    pub state: TapeState,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub symbols: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_start: Option<DateTime<Utc>>,
    /// The finalize stamp, verbatim — window end and gap counters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integrity: Option<RecordingIntegrity>,
    /// Frames the tape holds, so an empty capture is distinguishable from a
    /// dense one at a glance. `None` when the backend can't be read (damaged
    /// or held by a live writer) — shown as unknown, never a misleading zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events: Option<u64>,
    /// Whether a writer holds the tape right now.
    pub in_use: bool,
    /// Why the row is `Damaged` — and only then: a live capture is healthy,
    /// its metadata is just not readable yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl TapeEntry {
    /// The catalog's total order: tape ref, then backend. Defined once so
    /// `list_recordings` and the `tape list` merge sort by the same key.
    pub fn sort_key(&self) -> (&str, TapeBackend) {
        (self.tape.as_str(), self.backend)
    }
}

/// Describe one tape file without ever failing: metadata when readable,
/// a `Damaged` row naming the reason when not.
pub fn describe_tape(tape: &TapeRef, path: &Path, backend: TapeBackend) -> TapeEntry {
    let in_use = is_lock_held(path);
    let manifest = match backend {
        #[cfg(feature = "duckdb")]
        TapeBackend::Duckdb => duckdb_manifest(path),
        _ => load_sidecar(path, "list"),
    };
    match manifest {
        Ok(manifest) => {
            let state = if manifest.summary.is_some() {
                TapeState::Finalized
            } else {
                TapeState::Unfinalized
            };
            TapeEntry {
                tape: tape.to_string(),
                backend,
                state,
                symbols: manifest.meta.symbols,
                channels: manifest.meta.channels,
                window_start: Some(manifest.meta.window_start),
                integrity: manifest.summary,
                events: frame_count(path),
                in_use,
                detail: None,
            }
        }
        // A held writer lock is a live capture, not damage — the metadata is
        // simply not readable yet (DuckDB keeps it inside the locked file).
        Err(err) => {
            let state = if in_use {
                TapeState::Unfinalized
            } else {
                TapeState::Damaged
            };
            TapeEntry {
                tape: tape.to_string(),
                backend,
                state,
                symbols: Vec::new(),
                channels: Vec::new(),
                window_start: None,
                integrity: None,
                events: None,
                in_use,
                detail: (state == TapeState::Damaged).then(|| err.to_string()),
            }
        }
    }
}

/// Frames a tape holds, read cheaply at list time and keyed off the file
/// extension: DuckDB's persisted `last_seq` high-water mark, or a JSONL line
/// count (one frame per line). `None` when the file can't be read (a
/// live-locked or damaged tape), so a caller shows "unknown" rather than a
/// misleading zero.
pub fn frame_count(path: &Path) -> Option<u64> {
    match path.extension().and_then(|ext| ext.to_str()) {
        #[cfg(feature = "duckdb")]
        Some("duckdb") => crate::frames::DuckdbSource::open(path)
            .ok()?
            .frame_count()
            .ok()
            .flatten(),
        Some("jsonl") => jsonl_line_count(path),
        _ => None,
    }
}

/// One JSONL frame per line, so the committed line count is the frame count.
fn jsonl_line_count(path: &Path) -> Option<u64> {
    use std::io::BufRead;
    let file = std::fs::File::open(path).ok()?;
    u64::try_from(std::io::BufReader::new(file).lines().count()).ok()
}

/// Read a DuckDB tape's self-description through the read-only source
/// (shared lock; a live writer rejects and the caller degrades the row).
#[cfg(feature = "duckdb")]
fn duckdb_manifest(path: &Path) -> Result<crate::frames::capture::RecordingManifest> {
    use crate::frames::CaptureSource;
    crate::frames::DuckdbSource::open(path)?.capture()
}

/// Every tape under `<base>/tapes/`, sorted by ref then backend.
/// A missing tapes directory is an empty catalog, not an error.
/// On a build without the `duckdb` feature, `.duckdb` files are skipped
/// entirely — that build can neither describe nor replay them.
pub fn list_recordings(base: &Path) -> Result<Vec<TapeEntry>> {
    let entries = match std::fs::read_dir(tapes_root(base)) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut rows = Vec::new();
    for entry in entries {
        let path = entry?.path();
        let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        // Sidecars, locks, and foreign files match no supported backend.
        let Some(&backend) = TapeBackend::supported()
            .iter()
            .find(|backend| backend.extension() == extension)
        else {
            continue;
        };
        // A stem outside the name grammar cannot be referenced by any ref
        // string, so it cannot be listed under one either.
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if !kraken_core::name::is_safe_name_segment(name) {
            continue;
        }
        let tape = TapeRef::Recording {
            name: name.to_owned(),
        };
        rows.push(describe_tape(&tape, &path, backend));
    }
    rows.sort_unstable_by(|a, b| a.sort_key().cmp(&b.sort_key()));
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::frames::capture::resolve;

    #[test]
    fn frame_count_reads_jsonl_lines_and_is_unknown_for_foreign_files() {
        // One JSONL frame per line, so the count is the line count; a file that
        // matches no backend reads as unknown (None), never a misleading 0.
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("t.jsonl");
        std::fs::write(&jsonl, "frame-a\nframe-b\nframe-c\n").unwrap();
        assert_eq!(frame_count(&jsonl), Some(3));

        let empty = dir.path().join("empty.jsonl");
        std::fs::write(&empty, "").unwrap();
        assert_eq!(
            frame_count(&empty),
            Some(0),
            "a quiet capture is 0, not unknown"
        );

        let foreign = dir.path().join("notes.txt");
        std::fs::write(&foreign, "x").unwrap();
        assert_eq!(frame_count(&foreign), None);
        assert_eq!(frame_count(&dir.path().join("gone.jsonl")), None);
    }

    #[test]
    fn ref_parses_both_namespaces_and_rejects_the_rest() {
        assert_eq!(
            "tape:jun-crash".parse::<TapeRef>().unwrap(),
            TapeRef::Recording {
                name: "jun-crash".into()
            }
        );
        assert_eq!(
            "session:btc-dip".parse::<TapeRef>().unwrap(),
            TapeRef::Session {
                name: "btc-dip".into()
            }
        );
        for bad in [
            "jun-crash",
            "tapes:jun-crash",
            "tape:",
            "tape:a/b",
            "tape:..",
            "session:a b",
            // The pre-release path-shaped grammar is gone, not aliased.
            "recordings/jun-crash",
            "sessions/btc-dip",
        ] {
            assert!(bad.parse::<TapeRef>().is_err(), "{bad}");
        }
    }

    proptest! {
        #[test]
        fn ref_display_round_trips(name in "[A-Za-z0-9._-]{1,32}", session in proptest::bool::ANY) {
            prop_assume!(kraken_core::name::is_safe_name_segment(&name));
            let tape = if session {
                TapeRef::Session { name }
            } else {
                TapeRef::Recording { name }
            };
            prop_assert_eq!(tape.to_string().parse::<TapeRef>().unwrap(), tape);
        }
    }

    #[test]
    fn missing_recordings_dir_lists_empty_not_an_error() {
        let base = tempfile::tempdir().unwrap();
        assert!(list_recordings(base.path()).unwrap().is_empty());
    }

    /// A held writer lock is a live capture: unfinalized, in use, and no
    /// damage detail — never a false "something is wrong" signal.
    #[test]
    fn live_writer_reads_unfinalized_and_in_use_not_damaged() {
        let base = tempfile::tempdir().unwrap();
        let path = resolve(base.path(), "live", "jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "").unwrap();
        let _writer = crate::lock::FileLock::acquire(&path, "capture").unwrap();

        let entry = describe_tape(
            &TapeRef::Recording {
                name: "live".into(),
            },
            &path,
            TapeBackend::Jsonl,
        );
        assert_eq!(entry.state, TapeState::Unfinalized);
        assert!(entry.in_use);
        assert!(entry.detail.is_none());
    }

    #[test]
    fn damaged_sidecar_is_a_described_row_not_a_listing_failure() {
        let base = tempfile::tempdir().unwrap();
        let path = resolve(base.path(), "broken", "jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{}\n").unwrap();

        let rows = list_recordings(base.path()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tape, "tape:broken");
        assert_eq!(rows[0].state, TapeState::Damaged);
        assert!(rows[0].detail.is_some());
        assert!(!rows[0].in_use);
    }
}

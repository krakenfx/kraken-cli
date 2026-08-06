//! Resolving a replay-source ref to the file playback reads. Shared by the
//! replay venue, the `run start --from` path, and the lab's seal
//! enforcement — the one place that knows a source is JSONL-on-disk.

use std::path::{Path, PathBuf};

use kraken_recording::TapeRef;
use kraken_session::manifest::RecordingBackend;
use kraken_session::session::{parse_ordinal, session_dir, tape_file};

use crate::{ReplayError, Result};

/// The JSONL file backing a replay source, erroring if it is absent. Replay
/// reads JSONL only, so a tape recorded to DuckDB alone resolves to a
/// missing path and is refused with the re-record hint.
///
/// Two bases, deliberately: `tape:<name>` addresses the GLOBAL shared
/// library (`library`; market memory is never workspace-scoped), while
/// `session:s<n>` addresses the caller's scope's sessions tree (`scope`). In an
/// unscoped context the two coincide.
///
/// A `run:<ref>` addresses runs by ordinal (`session:s<n>`); label resolution is
/// CLI-layer policy and sealed lab plans store ordinal refs, so anything
/// else is refused here.
pub fn resolve_source_path(library: &Path, scope: &Path, tape: &TapeRef) -> Result<PathBuf> {
    let path = match tape {
        TapeRef::Recording { name } => kraken_recording::resolve(library, name, "jsonl"),
        TapeRef::Session { name } => {
            let Some(ordinal) = parse_ordinal(name) else {
                return Err(ReplayError::Source(format!(
                    "session ref '{tape}' does not name a session ordinal; replay sources address \
                     sessions as session:s<n> — resolve a label to its ordinal first (see 'kraken session \
                     list')"
                )));
            };
            session_dir(scope, ordinal).join(tape_file(RecordingBackend::Jsonl))
        }
    };
    if path.exists() {
        return Ok(path);
    }
    // "recorded to DuckDB alone" is only true when the DuckDB sibling exists;
    // an absent tape gets a plain not-found instead of that misleading hint.
    let message = match tape {
        TapeRef::Recording { name }
            if kraken_recording::resolve(library, name, "duckdb").exists() =>
        {
            format!(
                "tape '{tape}' was recorded to DuckDB only; replay reads JSONL — re-record \
                 it with --to jsonl (see 'kraken tape list')"
            )
        }
        TapeRef::Recording { .. } => {
            format!("tape '{tape}' not found (see 'kraken tape list')")
        }
        TapeRef::Session { .. } => {
            format!("session '{tape}' has no recorded tape (see 'kraken session list')")
        }
    };
    Err(ReplayError::Source(message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_ordinal_ref_resolves_to_the_run_tape() {
        let base = tempfile::tempdir().unwrap();
        let dir = session_dir(base.path(), 3);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("tape.jsonl"), "{}\n").unwrap();

        let tape: TapeRef = "session:s3".parse().unwrap();
        let path = resolve_source_path(base.path(), base.path(), &tape).unwrap();
        assert_eq!(path, dir.join("tape.jsonl"));
    }

    #[test]
    fn run_label_ref_is_refused_with_the_ordinal_grammar() {
        let base = tempfile::tempdir().unwrap();
        let tape: TapeRef = "session:m1-s3".parse().unwrap();
        let err = resolve_source_path(base.path(), base.path(), &tape).unwrap_err();
        assert!(matches!(err, ReplayError::Source(_)), "got: {err}");
        assert!(err.to_string().contains("session:s<n>"), "got: {err}");
    }

    #[test]
    fn missing_session_tape_points_at_session_list() {
        let base = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(session_dir(base.path(), 3)).unwrap();

        let tape: TapeRef = "session:s3".parse().unwrap();
        let err = resolve_source_path(base.path(), base.path(), &tape).unwrap_err();
        assert!(
            err.to_string().contains("has no recorded tape"),
            "got: {err}"
        );
    }

    #[test]
    fn absent_tape_reads_not_found_without_the_duckdb_hint() {
        let base = tempfile::tempdir().unwrap();
        let tape: TapeRef = "tape:nope".parse().unwrap();
        let err = resolve_source_path(base.path(), base.path(), &tape).unwrap_err();
        assert!(err.to_string().contains("not found"), "got: {err}");
        assert!(!err.to_string().contains("DuckDB"), "got: {err}");
    }

    #[test]
    fn duckdb_only_tape_keeps_the_re_record_hint() {
        let base = tempfile::tempdir().unwrap();
        let duckdb = kraken_recording::resolve(base.path(), "qa", "duckdb");
        std::fs::create_dir_all(duckdb.parent().unwrap()).unwrap();
        std::fs::write(&duckdb, b"").unwrap();

        let tape: TapeRef = "tape:qa".parse().unwrap();
        let err = resolve_source_path(base.path(), base.path(), &tape).unwrap_err();
        assert!(err.to_string().contains("--to jsonl"), "got: {err}");
    }
}

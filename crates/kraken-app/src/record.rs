//! The CLI's `--to` vocabulary for recording commands. Storage itself —
//! tapes, sinks, sources, locks — lives in [`kraken_recording`]; this
//! module only turns flag values into a sink plan.

pub(crate) use kraken_contract::value_types::SinkTarget;

/// The recording backend selected by `--to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum RecordFormat {
    /// File-based DuckDB tape (the recording schema's read contract).
    Duckdb,
    /// Dependency-free JSONL files (always available; the DuckDB fallback).
    Jsonl,
}

impl RecordFormat {
    pub(crate) fn extension(self) -> &'static str {
        match self {
            Self::Duckdb => "duckdb",
            Self::Jsonl => "jsonl",
        }
    }
}

/// The resolved `--to` targets: the durable backends to persist to, plus
/// whether to also echo the live stream to stdout.
pub(crate) struct SinkPlan {
    pub durables: Vec<RecordFormat>,
    pub echo: bool,
}

/// Split a `--to` target list into its unique durable backends and a
/// stdout-echo flag. Duplicate durables are ignored; any non-empty combination
/// is valid.
pub(crate) fn resolve_targets(targets: &[SinkTarget]) -> SinkPlan {
    let mut durables = Vec::new();
    let mut echo = false;
    for target in targets {
        // Exhaustive on purpose: a new target must show up here as a compile
        // error, not silently drop from the plan.
        let durable = match target {
            SinkTarget::Stdout => {
                echo = true;
                continue;
            }
            SinkTarget::Duckdb => RecordFormat::Duckdb,
            SinkTarget::Jsonl => RecordFormat::Jsonl,
        };
        if !durables.contains(&durable) {
            durables.push(durable);
        }
    }
    SinkPlan { durables, echo }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_targets_dedupes_durables_and_flags_stdout() {
        let all = resolve_targets(&[SinkTarget::Duckdb, SinkTarget::Jsonl, SinkTarget::Stdout]);
        assert_eq!(all.durables, [RecordFormat::Duckdb, RecordFormat::Jsonl]);
        assert!(all.echo);

        let stdout_only = resolve_targets(&[SinkTarget::Stdout]);
        assert!(stdout_only.durables.is_empty());
        assert!(stdout_only.echo);

        let deduped = resolve_targets(&[SinkTarget::Duckdb, SinkTarget::Duckdb]);
        assert_eq!(deduped.durables, [RecordFormat::Duckdb]);
        assert!(!deduped.echo);
    }
}

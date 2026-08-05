//! Logging/tracing configuration and subscriber initialization.
//!
//! Resolution of format, level, and colour lives here — in the centralised
//! config layer, mirroring [`crate::cli`]. Installation of the global
//! subscriber is performed exactly once from the binary entrypoint
//! (`src/main.rs`); library code only ever *emits* `tracing` events and never
//! installs a subscriber (so tests and the REPL share one un-contended path).
//!
//! Hard rule: the subscriber writes to **stderr only**. stdout carries the
//! command result (JSON/table), the MCP JSON-RPC transport, and JSONL
//! streams, and must never receive a log line.

use std::io::IsTerminal;

/// Log output format for diagnostics on stderr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum LogFormat {
    /// Human-readable, multi-line, coloured — for interactive development.
    Pretty,
    /// Single-line structured JSON (JSONL, one object per line) — the
    /// machine-parseable format for agents and automated pipelines.
    Json,
    /// Single-line, compact human-readable.
    Compact,
}

/// Resolved logging configuration, assembled once before dispatch.
#[derive(Debug, Clone)]
pub struct LogConfig {
    format: LogFormat,
    ansi: bool,
    /// Fallback `EnvFilter` directive, used only when `RUST_LOG` is unset.
    default_directive: String,
}

impl LogConfig {
    /// Resolve logging configuration from CLI flags and the environment.
    ///
    /// Format precedence (high → low): `--log-format` / `KRAKEN_LOG_FORMAT`
    /// (clap applies flag > env) → **JSON**. JSON is the default everywhere so
    /// logs are machine-parseable (JSONL) for agents and pipelines out of the
    /// box — including the MCP stdio server, whose stdout carries JSON-RPC. A
    /// human-readable format is opt-in via `--log-format pretty` (or
    /// `KRAKEN_LOG_FORMAT=pretty`, e.g. exported in a shell profile for
    /// interactive work). This is intentionally independent of `-o`/`--output`,
    /// which formats the command *result* on stdout.
    ///
    /// Level: `RUST_LOG` owns it entirely; when it is set it wins outright.
    /// `verbose` (the `-v`/`--verbose` flag) is only convenience that raises the
    /// fallback directive (`kraken_cli=debug`) applied when `RUST_LOG` is unset.
    ///
    /// Takes the two inputs it needs directly rather than the whole [`Cli`](crate::cli::Cli), so
    /// the logging layer stays decoupled from the CLI surface and is trivially
    /// testable.
    #[must_use]
    pub fn resolve(log_format: Option<LogFormat>, verbose: bool) -> Self {
        let format = log_format.unwrap_or(LogFormat::Json);

        let ansi = matches!(format, LogFormat::Pretty | LogFormat::Compact)
            && std::io::stderr().is_terminal()
            && !crate::config::no_color();

        // Default verbosity, used only when RUST_LOG is unset. First-party warnings
        // and errors are always surfaced — every workspace crate that logs must be
        // listed here, or its safety warnings (disabled TLS verification, reconnects)
        // vanish at default verbosity. Dependency crates are held to ERROR so a
        // genuinely serious failure (TLS, connection) still shows, while their WARN
        // noise stays out of stderr. debug/info/trace stay silent unless `-v` or
        // RUST_LOG enables them — preserving the `2>/dev/null` contract.
        let default_directive = if verbose {
            "warn,kraken_cli=debug,kraken_ws=debug".to_string()
        } else {
            "error,kraken_cli=warn,kraken_ws=warn".to_string()
        };

        Self {
            format,
            ansi,
            default_directive,
        }
    }

    /// Install the global `tracing` subscriber.
    ///
    /// Idempotent and panic-free: failure or a second call is ignored, so
    /// logging can never abort a command. Always writes to stderr.
    pub fn init(&self) {
        use tracing_subscriber::EnvFilter;

        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(&self.default_directive));

        let builder = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr);

        match self.format {
            LogFormat::Json => {
                let _ = builder.json().with_ansi(false).try_init();
            }
            LogFormat::Compact => {
                let _ = builder.compact().with_ansi(self.ansi).try_init();
            }
            LogFormat::Pretty => {
                let _ = builder.pretty().with_ansi(self.ansi).try_init();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_format_is_json() {
        // JSON is the machine-parseable default everywhere when no flag/env is set.
        assert_eq!(LogConfig::resolve(None, false).format, LogFormat::Json);
    }

    #[test]
    fn explicit_log_format_is_respected() {
        assert_eq!(
            LogConfig::resolve(Some(LogFormat::Pretty), false).format,
            LogFormat::Pretty
        );
        assert_eq!(
            LogConfig::resolve(Some(LogFormat::Json), false).format,
            LogFormat::Json
        );
        assert_eq!(
            LogConfig::resolve(Some(LogFormat::Compact), false).format,
            LogFormat::Compact
        );
    }

    #[test]
    fn json_never_colorizes() {
        assert!(!LogConfig::resolve(Some(LogFormat::Json), false).ansi);
    }

    #[test]
    fn default_is_silent_except_first_party_warnings() {
        // No -v / RUST_LOG: warnings/errors from every workspace crate that logs
        // surface (kraken_ws carries the disabled-TLS and reconnect warnings),
        // dependencies only at ERROR — no debug/info noise, no dependency WARN
        // leakage.
        assert_eq!(
            LogConfig::resolve(None, false).default_directive,
            "error,kraken_cli=warn,kraken_ws=warn"
        );
    }

    #[test]
    fn verbose_raises_first_party_default_to_debug() {
        let cfg = LogConfig::resolve(None, true);
        assert!(cfg.default_directive.contains("kraken_cli=debug"));
        assert!(cfg.default_directive.contains("kraken_ws=debug"));
    }

    #[test]
    fn trace_level_is_statically_compiled_in() {
        // Guards against a `release_max_level_*` cap in Cargo.toml. The
        // trace-level HTTP diagnostics in `client.rs` and the documented
        // `RUST_LOG=kraken_cli::client=trace` only work if `trace!` events
        // survive compilation. A cap below TRACE compiles them out of
        // release/dist builds, so this assertion fails there the moment one is
        // reintroduced (it is a no-op in debug builds, where the cap does not
        // apply). Run under `cargo test --release` to exercise it.
        use tracing::level_filters::{LevelFilter, STATIC_MAX_LEVEL};
        assert_eq!(
            STATIC_MAX_LEVEL,
            LevelFilter::TRACE,
            "trace-level diagnostics must stay compiled in (no release_max_level_* cap)"
        );
    }
}

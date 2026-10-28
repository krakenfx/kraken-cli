//! The one-shot `kraken <command>` front-end.
//!
//! The thinnest of the three adapters: parse argv with the engine's own clap
//! tree, hand the result to the engine, render what comes back, and turn it
//! into an exit code. Every decision about what a command *means* lives in
//! [`kraken_app`].

// Tests assert with unwrap/panic by design; the workspace lints deny them elsewhere.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

use clap::{
    Parser,
    error::{ContextKind, ContextValue, ErrorKind},
};
use kraken_app::{AppContext, Cli, errors::KrakenError, logging::LogConfig, output::OutputFormat};
use std::process::{self, ExitCode};

/// Parse argv, run the one command it names, and return the process's exit
/// code.
///
/// Returns rather than exits so the caller can still drain its runtime on the
/// error path — a `process::exit` here would kill a mid-flight sink finalizer.
pub async fn run() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        // Pre-command, nothing spawned yet — clap's own exit is safe here.
        Err(err) => exit_on_parse_error(err),
    };

    // Install the tracing subscriber once, before AppContext assembly (which
    // can emit warnings). Logs are JSON on stderr by default; a logging-init
    // failure never aborts the command.
    LogConfig::resolve(
        cli.globals.connection.log_format,
        cli.globals.connection.verbose,
    )
    .init();

    // Format is needed to render any error from context assembly, so resolve it
    // before the (consuming) command match below.
    let format = cli
        .globals
        .interactive
        .output
        .unwrap_or(OutputFormat::Table);

    let ctx = match AppContext::from_cli(&cli) {
        Ok(ctx) => ctx,
        Err(e) => {
            kraken_app::output::render_error(format, &e);
            return ExitCode::FAILURE;
        }
    };

    match kraken_app::run(&ctx, cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            kraken_app::output::render_error(ctx.format, &e);
            ExitCode::FAILURE
        }
    }
}

/// Subcommands that left `kraken`, and what to run instead, so a command or an
/// MCP client config from an older release gets a way forward.
const MOVED_SUBCOMMANDS: &[(&str, &str)] = &[
    (
        "mcp",
        "`kraken mcp` is now the `kraken-mcp` binary: point your MCP client at `kraken-mcp`. `-s` \
         and `--allow-dangerous` work as before; pass API keys as KRAKEN_API_KEY and \
         KRAKEN_API_SECRET",
    ),
    (
        "shell",
        "`kraken shell` is now the `kraken-shell` binary: run `kraken-shell`. Pass API keys as \
         KRAKEN_API_KEY and KRAKEN_API_SECRET, or store them with `kraken auth set`",
    ),
];

/// The way forward for an unrecognized subcommand that used to exist.
fn moved_subcommand_hint(err: &clap::Error) -> Option<&'static str> {
    if err.kind() != ErrorKind::InvalidSubcommand {
        return None;
    }
    let Some(ContextValue::String(name)) = err.get(ContextKind::InvalidSubcommand) else {
        return None;
    };
    MOVED_SUBCOMMANDS
        .iter()
        .find(|(old, _)| old == name)
        .map(|(_, hint)| *hint)
}

/// Render a clap parse failure and exit. `--help`/`--version` print normally
/// (exit 0); a real usage error becomes a JSON error envelope when `-o json`
/// was requested — keeping the JSON-or-JSON-error contract for automation —
/// otherwise clap's standard human message.
fn exit_on_parse_error(mut err: clap::Error) -> ! {
    let moved = moved_subcommand_hint(&err);
    if let Some(hint) = moved {
        err.insert(
            ContextKind::Suggested,
            ContextValue::StyledStrs(vec![hint.into()]),
        );
    }
    if err.use_stderr() && wants_json(std::env::args_os()) {
        // Keep clap's whole diagnostic body — every line up to the blank line before
        // the usage/help footer — joined onto one line. Multi-line diagnostics
        // (the list of missing required arguments, allowed values) must survive
        // into the envelope; only the footer is noise there. The body starts
        // with "error: "; drop it so the message isn't a doubled "Validation
        // error: error: ..." (Validation's Display adds its own).
        let rendered = err.render().to_string();
        let body = rendered
            .lines()
            .take_while(|line| !line.trim().is_empty())
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(" ");
        let message = body.strip_prefix("error: ").unwrap_or(&body);
        // The tip sits below the blank line the body stops at, so it is added back
        // here.
        let message = match moved {
            Some(hint) => format!("{message}; {hint}"),
            None => message.to_owned(),
        };
        kraken_app::output::render_error(OutputFormat::Json, &KrakenError::Validation(message));
        process::exit(2);
    }
    err.exit();
}

/// Whether `-o json` was requested. Hand-scanned from argv because clap can't
/// recover a global flag once parsing has failed (`ignore_errors` drops
/// everything after the offending arg), and because `args_os` — unlike
/// `env::args` — never panics on a non-UTF8 argument.
fn wants_json(args: impl IntoIterator<Item = std::ffi::OsString>) -> bool {
    let mut args = args.into_iter().skip(1);
    while let Some(arg) = args.next() {
        // A non-UTF8 arg can't be the ASCII `-o json`; skip it. Bundled shorts (`-vo
        // json`) are not decomposed and fall back to clap's plain-text error.
        let Some(arg) = arg.to_str() else { continue };
        let is_json = match arg {
            "-o" | "--output" => args.next().is_some_and(|v| v.to_str() == Some("json")),
            other => matches!(
                other
                    .strip_prefix("--output=")
                    .or_else(|| other.strip_prefix("-o="))
                    .or_else(|| other.strip_prefix("-o")),
                Some("json")
            ),
        };
        if is_json {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::wants_json;

    fn argv(rest: &[&str]) -> Vec<std::ffi::OsString> {
        std::iter::once("kraken")
            .chain(rest.iter().copied())
            .map(std::ffi::OsString::from)
            .collect()
    }

    #[test]
    fn detects_json_in_every_spelling_and_position() {
        for form in [
            vec!["-o", "json"],
            vec!["--output", "json"],
            vec!["-o=json"],
            vec!["--output=json"],
            vec!["-ojson"],
        ] {
            assert!(wants_json(argv(&form)), "form: {form:?}");
        }
        // After the offending argument — the position the clap reparse could not
        // recover.
        assert!(wants_json(argv(&[
            "ws",
            "add-order",
            "--order-type",
            "bogus",
            "-o",
            "json"
        ])));
    }

    #[test]
    fn absent_or_other_format_is_not_json() {
        assert!(!wants_json(argv(&[
            "ws",
            "add-order",
            "--order-type",
            "limit"
        ])));
        assert!(!wants_json(argv(&["-o", "table"])));
        // A "json" value belonging to another flag must not be read as `--output json`.
        assert!(!wants_json(argv(&["--symbol", "json"])));
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_argument_is_skipped_not_panicked_on() {
        // Regression: iterating env::args() (not args_os) would panic here. A non-UTF8
        // arg must be skipped, and a following `-o json` still detected.
        use std::os::unix::ffi::OsStringExt;
        let args = vec![
            std::ffi::OsString::from("kraken"),
            std::ffi::OsString::from_vec(vec![0xFF]),
            std::ffi::OsString::from("-o"),
            std::ffi::OsString::from("json"),
        ];
        assert!(wants_json(args));
    }
}

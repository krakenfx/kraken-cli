// Tests assert with unwrap/panic by design; the workspace lints deny them elsewhere.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

use std::process::{self, ExitCode};
use std::time::Duration;

use clap::Parser;
use kraken_cli::errors::KrakenError;
use kraken_cli::logging::LogConfig;
use kraken_cli::output::OutputFormat;
use kraken_cli::{AppContext, Cli};

/// How long runtime shutdown may wait on outstanding blocking work (a wedged sink
/// `CHECKPOINT` on a spawn_blocking thread) after `run` returns. Without this bound the
/// runtime drop joins those threads indefinitely and the exiting process hangs.
const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

fn main() -> ExitCode {
    // rmcp serves MCP tool calls on spawned worker tasks, and `execute_command`
    // builds a large future (the `Command` enum spans 150+ variants). Tokio's
    // default 2 MiB worker stack overflows on that future, aborting the process on
    // the first tool call. 8 MiB matches the main-thread default with headroom.
    // Worker stacks are committed lazily, so the larger size is effectively free:
    // non-streaming commands await their work on the main thread, and the tasks the
    // streaming commands (monitor, websocket) spawn are small.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(8 * 1024 * 1024)
        .build()
    {
        Ok(runtime) => runtime,
        // Pre-runtime, so pre-subscriber and pre-CLI-parse: a bare stderr line
        // and exit 1 are all the contract we can honor this early.
        Err(e) => {
            eprintln!("kraken: failed to build tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    // `run` returns the code rather than exiting so the drain below also runs on
    // the error path — `process::exit` here would kill a mid-flight sink finalizer.
    let code = runtime.block_on(run());
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
    code
}

async fn run() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        // Pre-command, nothing spawned yet — clap's own exit is safe here.
        Err(err) => exit_on_parse_error(err),
    };

    // Install the tracing subscriber once, before AppContext assembly (which
    // can emit warnings). Logs are JSON on stderr by default; a logging-init
    // failure never aborts the command.
    LogConfig::resolve(cli.log_format, cli.verbose).init();

    // Format is needed to render any error from context assembly, so resolve it
    // before the (consuming) command match below.
    let format = cli.output.unwrap_or(OutputFormat::Table);

    let ctx = match AppContext::from_cli(&cli) {
        Ok(ctx) => ctx,
        Err(e) => {
            kraken_cli::output::render_error(format, &e);
            return ExitCode::FAILURE;
        }
    };

    match kraken_cli::run(&ctx, cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            kraken_cli::output::render_error(ctx.format, &e);
            ExitCode::FAILURE
        }
    }
}

/// Render a clap parse failure and exit. `--help`/`--version` print normally (exit 0); a real
/// usage error becomes a JSON error envelope when `-o json` was requested — keeping the
/// JSON-or-JSON-error contract for automation — otherwise clap's standard human message.
fn exit_on_parse_error(err: clap::Error) -> ! {
    if err.use_stderr() && wants_json(std::env::args_os()) {
        // Keep clap's whole diagnostic body — every line up to the blank line before the
        // usage/help footer — joined onto one line. Multi-line diagnostics (the list of
        // missing required arguments, allowed values) must survive into the envelope; only
        // the footer is noise there. The body starts with "error: "; drop it so the message
        // isn't a doubled "Validation error: error: ..." (Validation's Display adds its own).
        let rendered = err.render().to_string();
        let body = rendered
            .lines()
            .take_while(|line| !line.trim().is_empty())
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(" ");
        let message = body.strip_prefix("error: ").unwrap_or(&body).to_owned();
        kraken_cli::output::render_error(OutputFormat::Json, &KrakenError::Validation(message));
        process::exit(2);
    }
    err.exit();
}

/// Whether `-o json` was requested. Hand-scanned from argv because clap can't recover a global
/// flag once parsing has failed (`ignore_errors` drops everything after the offending arg), and
/// because `args_os` — unlike `env::args` — never panics on a non-UTF8 argument.
fn wants_json(args: impl IntoIterator<Item = std::ffi::OsString>) -> bool {
    let mut args = args.into_iter().skip(1);
    while let Some(arg) = args.next() {
        // A non-UTF8 arg can't be the ASCII `-o json`; skip it. Bundled shorts (`-vo json`) are
        // not decomposed and fall back to clap's plain-text error.
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
        // After the offending argument — the position the clap reparse could not recover.
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
        // Regression: iterating env::args() (not args_os) would panic here. A non-UTF8 arg must
        // be skipped, and a following `-o json` still detected.
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

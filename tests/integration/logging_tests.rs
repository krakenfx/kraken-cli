//! Logging/observability contract tests.
//!
//! These lock in the two hard guarantees of the tracing migration:
//! 1. Enabling logging never changes a byte of stdout (the agent/JSON contract
//!    and the `2>/dev/null` invocation pattern must be unaffected).
//! 2. Diagnostics are emitted as structured JSON on stderr, never stdout.
//!
//! `paper status` against a fresh `$HOME` is a deterministic, offline command:
//! it returns a stable "not initialized" JSON validation envelope on stdout and
//! touches no network.
use assert_cmd::Command;
use tempfile::tempdir;

#[allow(deprecated)]
fn kraken() -> Command {
    Command::cargo_bin("kraken").unwrap()
}

/// Enabling logging must not change a single byte of stdout.
#[test]
fn logging_does_not_change_stdout() {
    let home = tempdir().unwrap();

    let run = |logging: bool| -> Vec<u8> {
        let mut cmd = kraken();
        cmd.env("HOME", home.path())
            .env_remove("RUST_LOG")
            .env_remove("KRAKEN_LOG_FORMAT")
            .args(["paper", "status", "-o", "json"]);
        if logging {
            cmd.env("RUST_LOG", "trace").args(["--log-format", "json"]);
        }
        cmd.output().unwrap().stdout
    };

    let silent = run(false);
    let loud = run(true);

    assert_eq!(
        silent, loud,
        "stdout must be byte-identical whether or not logging is enabled"
    );
    // ...and it is the expected single-line JSON envelope.
    let value: serde_json::Value = serde_json::from_slice(&silent).unwrap();
    assert_eq!(value["error"], "validation");
}

/// Diagnostics go to stderr as NDJSON; stdout stays the clean result envelope.
#[test]
fn json_logs_go_to_stderr_not_stdout() {
    let home = tempdir().unwrap();

    // A non-Kraken URL override (with the danger flag) makes context assembly
    // emit a WARN — a deterministic, offline diagnostic.
    let output = kraken()
        .env("HOME", home.path())
        .env_remove("RUST_LOG")
        .env("KRAKEN_SPOT_URL", "https://example.com")
        .env("KRAKEN_DANGER_ALLOW_ANY_URL_HOST", "1")
        .args(["paper", "status", "-o", "json", "--log-format", "json"])
        .output()
        .unwrap();

    // stdout: exactly the JSON result envelope (parses as a single object).
    let stdout_value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stdout_value["error"], "validation");

    // stderr: the first line is a structured WARN log object.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let first_line = stderr.lines().next().unwrap_or_default();
    let log: serde_json::Value =
        serde_json::from_str(first_line).expect("first stderr line must be a JSON log object");
    assert_eq!(log["level"], "WARN");
    // No credential field ever appears in diagnostics.
    assert!(!stderr.contains("api_secret"));
}

/// Silent by default: with no `RUST_LOG`/`-v`, stderr carries no debug/info
/// noise and stdout is unaffected.
#[test]
fn silent_by_default_no_debug_or_info() {
    let home = tempdir().unwrap();

    let output = kraken()
        .env("HOME", home.path())
        .env_remove("RUST_LOG")
        .env_remove("KRAKEN_LOG_FORMAT")
        .args(["paper", "status", "-o", "json"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("\"level\":\"DEBUG\"") && !stderr.contains("\"level\":\"INFO\""),
        "default verbosity must not emit DEBUG/INFO; stderr was: {stderr}"
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"], "validation");
}

/// The tracing subscriber is installed once at startup, so a per-command
/// `--verbose`/`--log-format` inside the REPL cannot take effect. The shell must
/// say so once rather than accept the flag as a silent no-op.
#[test]
fn shell_warns_once_on_per_line_logging_flags() {
    let home = tempdir().unwrap();

    // Two flagged lines plus one unflagged line: the notice must appear exactly
    // once (first flagged use), not per command and not for the unflagged one.
    let output = kraken()
        .env("HOME", home.path())
        .env_remove("RUST_LOG")
        .env_remove("KRAKEN_LOG_FORMAT")
        .arg("shell")
        .write_stdin(
            "paper status --verbose\npaper status --log-format pretty\npaper status\nexit\n",
        )
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    let notices = stderr
        .lines()
        .filter(|l| l.contains("logging is configured once"))
        .count();
    assert_eq!(
        notices, 1,
        "expected exactly one per-line logging-flag notice; stderr was: {stderr}"
    );
}

/// A shell session that never uses a per-line logging flag stays quiet — the
/// notice is not emitted spuriously.
#[test]
fn shell_silent_without_per_line_logging_flags() {
    let home = tempdir().unwrap();

    let output = kraken()
        .env("HOME", home.path())
        .env_remove("RUST_LOG")
        .env_remove("KRAKEN_LOG_FORMAT")
        .arg("shell")
        .write_stdin("paper status\nexit\n")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("logging is configured once"),
        "notice must not fire without a per-line logging flag; stderr was: {stderr}"
    );
}

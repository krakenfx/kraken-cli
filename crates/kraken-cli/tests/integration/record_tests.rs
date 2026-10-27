//! End-to-end CLI wiring tests for `kraken record`.
//!
//! These run the real binary and assert the command parses and validates
//! correctly. Validation runs before any socket or file is touched, so no
//! live/mock server is needed. The capture-and-read path (frames -> DuckDB ->
//! read back) is covered deterministically by the in-crate sink/pipeline tests,
//! which can drive the typed sink without a network.

use super::common::config_dir_in;
use assert_cmd::Command;
use predicates::prelude::*;

#[allow(deprecated)]
fn kraken() -> Command {
    Command::cargo_bin("kraken").unwrap()
}

#[test]
fn record_ignores_the_active_workspace_and_writes_the_global_library() {
    // Market memory is global: even inside a workspace, record opens its sinks
    // under the flat tapes/ dir. Sinks open before the socket connects,
    // so the layout is observable while the recorder retries the refused
    // endpoint — record never exits on connect failure, so the test reaps it.
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(config_dir_in(&home).join("workspaces").join("w1")).unwrap();

    #[allow(deprecated)]
    let bin = assert_cmd::cargo::cargo_bin("kraken");
    let mut recorder = std::process::Command::new(bin)
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .env_remove("KRAKEN_SESSION")
        .env_remove("KRAKEN_WORKSPACE")
        .env("KRAKEN_WS_PUBLIC_URL", "wss://127.0.0.1:1")
        .env("KRAKEN_DANGER_ALLOW_ANY_URL_HOST", "1")
        .env("KRAKEN_WS_RECONNECT_BASE_MS", "1")
        .args([
            "--workspace",
            "w1",
            "record",
            "--symbols",
            "BTC/USD",
            "--channels",
            "ticker",
            "--to",
            "jsonl",
            "-o",
            "json",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    let global_tape = config_dir_in(&home).join("tapes").join("default.jsonl");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !global_tape.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    // Kill may race a crash-exit; wait() reaps either way.
    let _ = recorder.kill();
    recorder.wait().unwrap();

    assert!(
        global_tape.exists(),
        "the tape must land in the global library"
    );
    assert!(
        !config_dir_in(&home)
            .join("workspaces")
            .join("w1")
            .join("tapes")
            .exists(),
        "the workspace tree must not grow a tapes dir"
    );
}

#[test]
fn record_only_non_recordable_channels_is_rejected_as_validation() {
    // `executions` is an account feed record doesn't capture, so with no recordable
    // channel alongside it the command's validation rejects the invocation with a
    // JSON envelope on stdout (before any socket).
    kraken()
        .args([
            "record",
            "--symbols",
            "BTC/USD",
            "--channels",
            "executions",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"error\":\"validation\""));
}

#[test]
fn record_without_channels_is_rejected_as_validation() {
    kraken()
        .args(["record", "--symbols", "BTC/USD", "-o", "json"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"error\":\"validation\""));
}

#[test]
fn record_unknown_backend_is_rejected_at_parse_time() {
    // `--to bogus` is not a `RecordFormat` value, so clap rejects it during
    // argument parsing (exit code 2, usage error on stderr).
    kraken()
        .args([
            "record",
            "--symbols",
            "BTC/USD",
            "--channels",
            "trades",
            "--to",
            "bogus",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid value 'bogus'"));
}

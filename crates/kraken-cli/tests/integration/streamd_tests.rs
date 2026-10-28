//! End-to-end CLI wiring tests for `kraken streamd`.
//!
//! These run the real binary and assert the command parses, validates, and
//! renders error envelopes correctly — the validation path runs before any
//! socket is opened, so no live/mock server is needed. The streaming engine
//! itself (subscribe, reconnect/resubscribe, rejection eviction, token
//! injection) is covered deterministically by the in-crate engine tests.

use assert_cmd::Command;
use predicates::prelude::*;

#[allow(deprecated)]
fn kraken() -> Command {
    Command::cargo_bin("kraken").unwrap()
}

#[test]
fn streamd_unknown_channel_is_rejected_at_parse_time() {
    // `--channels` is a clap `ValueEnum`, so an unknown channel is rejected during
    // argument parsing (exit code 2). With `-o json` the usage error is
    // rendered as a JSON validation envelope on stdout — carrying clap's
    // message — rather than raw text on stderr.
    kraken()
        .args([
            "streamd",
            "start",
            "--symbols",
            "BTC/USD",
            "--channels",
            "bogus",
            "-o",
            "json",
        ])
        .assert()
        .code(2)
        .stdout(
            predicate::str::contains("\"error\":\"validation\"")
                .and(predicate::str::contains("invalid value 'bogus'")),
        );
}

#[test]
fn streamd_level3_is_rejected_as_validation() {
    kraken()
        .args([
            "streamd",
            "start",
            "--symbols",
            "BTC/USD",
            "--channels",
            "level3",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"error\":\"validation\""));
}

#[test]
fn streamd_market_channel_without_symbols_is_rejected() {
    kraken()
        .args([
            "streamd",
            "start",
            "--symbols",
            "",
            "--channels",
            "ticker",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"error\":\"validation\""));
}

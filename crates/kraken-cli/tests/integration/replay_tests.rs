//! End-to-end CLI wiring tests for `kraken replay`.
//!
//! These run the real binary and assert the argument/session contract: with no
//! runs on record the default `--run latest` is a validation error naming
//! what exists, an out-of-range speed is a parser error (exit 2), and
//! runtime failures use the JSON envelope on stdout under `-o json`.
//! `HOME`/`XDG_CONFIG_HOME` point at a temp dir so no developer state can
//! pre-exist. Playback itself is pinned by the unit tests in
//! `src/commands/replay.rs` on a paused clock.

use super::common::kraken_in;
use predicates::prelude::*;

#[test]
fn replay_with_no_runs_names_what_exists() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["replay", "-o", "json"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("session 'latest' not found"))
        .stdout(predicate::str::contains("kraken session start"));
}

#[test]
fn replay_of_an_unknown_run_is_a_validation_error() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["replay", "--session", "s9", "-o", "json"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("not found"));
}

#[test]
fn replay_rejects_an_out_of_range_speed_at_the_parser() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["replay", "--session", "s1", "--speed", "0"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("speed"));
}

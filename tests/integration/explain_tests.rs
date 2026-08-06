//! End-to-end CLI wiring tests for `kraken explain pnl`.
//!
//! These run the real binary and assert the session contract: no runs on record
//! and an unknown session ref are validation errors in the JSON envelope, and
//! the ref defaults to `latest`. The decomposition itself is pinned by
//! kraken-replay's unit tests.

use predicates::prelude::*;

use super::common::{kraken, kraken_in};

#[test]
fn explain_pnl_with_no_runs_names_what_exists() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["explain", "pnl", "-o", "json"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("session 'latest' not found"));
}

#[test]
fn explain_pnl_of_an_unknown_run_is_a_validation_error() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["explain", "pnl", "--session", "ghost", "-o", "json"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("not found"));
}

#[test]
fn explain_pnl_help_names_the_run_ref() {
    kraken()
        .args(["explain", "pnl", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--session"))
        .stdout(predicate::str::contains("latest"));
}

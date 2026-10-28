//! End-to-end CLI wiring tests for `kraken explain pnl`.
//!
//! These run the real binary and assert the session contract: no runs on record
//! and an unknown session ref are validation errors in the JSON envelope, and
//! the ref defaults to `latest`. The decomposition itself is pinned by
//! kraken-replay's unit tests.

use super::common::kraken_in;
use predicates::prelude::*;

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

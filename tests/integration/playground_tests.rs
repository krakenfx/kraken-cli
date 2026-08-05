//! End-to-end CLI wiring tests for the bare `kraken playground` demo: one
//! command that composes existing primitives — create-or-reuse the
//! `playground` paper workspace, print how to watch, start a recorded run.
//! No new state, no engine; these tests pin the composition's contract.

use predicates::prelude::*;

use super::common::{config_dir_in, kraken, kraken_in};

#[test]
fn bare_playground_parses_without_a_subcommand() {
    kraken()
        .args(["playground", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--symbols"))
        .stdout(predicate::str::contains("--template"))
        .stdout(predicate::str::contains("--for"));
}

/// The demo against a dead WS endpoint: the reconnect loop never comes up
/// (the harness timeout reaps it), but the workspace provisioning and the
/// guidance happen first — and a second invocation must reuse the account,
/// never re-fund it.
#[test]
fn demo_creates_the_playground_workspace_once_and_reuses_it() {
    let home = tempfile::tempdir().unwrap();
    let demo = |home: &tempfile::TempDir| {
        kraken_in(home)
            .env("KRAKEN_WS_PUBLIC_URL", "wss://127.0.0.1:1")
            .env("KRAKEN_DANGER_ALLOW_ANY_URL_HOST", "1")
            .env("KRAKEN_WS_RECONNECT_BASE_MS", "1")
            .timeout(std::time::Duration::from_secs(15))
            .args(["playground", "-o", "json"])
            .output()
            .unwrap()
    };

    let first = demo(&home);
    assert!(
        !first.status.success(),
        "the dead stream fails the session: {first:?}"
    );
    let stderr = String::from_utf8_lossy(&first.stderr);
    assert!(
        stderr.contains("playground workspace created"),
        "first invocation provisions: {stderr}"
    );
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            config_dir_in(&home)
                .join("workspaces")
                .join("playground")
                .join("workspace.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["capital"], "10000");
    assert_eq!(manifest["mode"], "paper");

    let second = demo(&home);
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        !stderr.contains("playground workspace created"),
        "reuse never re-provisions: {stderr}"
    );
    // The account is untouched: same capital, no second epoch.
    let balance = kraken_in(&home)
        .args(["workspace", "balance", "playground", "-o", "json"])
        .output()
        .unwrap();
    assert!(balance.status.success());
    let json: serde_json::Value = serde_json::from_slice(&balance.stdout).unwrap();
    assert_eq!(json["balances"]["USD"]["total"], "10000");
}

/// A live-mode manifest under the demo's name refuses at provision — the demo
/// must never adopt an account whose fills could be real. (Named live
/// workspaces are contract-accepted on disk but runtime-refused.)
#[test]
fn demo_refuses_a_live_mode_playground_workspace() {
    let home = tempfile::tempdir().unwrap();
    let dir = config_dir_in(&home).join("workspaces").join("playground");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("workspace.json"),
        serde_json::json!({
            "workspace_version": "1.0",
            "cli_version": "0.0.0-test",
            "name": "playground",
            "capital": "10000",
            "currency": "USD",
            "mode": "live",
            "fee_rate": "0.0026",
            "slippage_rate": "0",
            "created_at": "2026-01-01T00:00:00Z",
        })
        .to_string(),
    )
    .unwrap();

    kraken_in(&home)
        .args(["playground", "-o", "json"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("simulated fills only"));
}

/// The trade example names the first `--symbols` pair — guidance pointing at
/// an unrecorded pair would leave the window without marks to score it by.
#[test]
fn demo_prints_the_export_hint_and_watch_lines_for_the_recorded_symbol() {
    let home = tempfile::tempdir().unwrap();
    let out = kraken_in(&home)
        .env("KRAKEN_WS_PUBLIC_URL", "wss://127.0.0.1:1")
        .env("KRAKEN_DANGER_ALLOW_ANY_URL_HOST", "1")
        .env("KRAKEN_WS_RECONNECT_BASE_MS", "1")
        .timeout(std::time::Duration::from_secs(15))
        .args(["playground", "--symbols", "ETH/USD", "-o", "json"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    for needle in [
        "export KRAKEN_WORKSPACE=playground",
        "kraken session show",
        "kraken session stop",
        "kraken order buy ETH/USD",
    ] {
        assert!(stderr.contains(needle), "{needle} missing in: {stderr}");
    }
}

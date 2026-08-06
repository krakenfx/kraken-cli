use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;

use super::common::{config_dir_in, kraken_in};

#[allow(deprecated)]
fn kraken() -> Command {
    let mut cmd = Command::cargo_bin("kraken").unwrap();
    // Hermetic by default: the retired `KRAKEN_SESSION` is scrubbed for one
    // release as defense against stale developer environments (the binary
    // now ignores it, but a set value in CI logs would mislead debugging).
    cmd.env_remove("KRAKEN_SESSION");
    cmd
}

/// [`kraken_in`] with REST pointed at a dead local port, so best-effort
/// mark-to-market paths (paper status, reconcile) fail fast without ever
/// opening a live socket.
fn kraken_offline_in(home: &tempfile::TempDir) -> Command {
    let mut cmd = kraken_in(home);
    cmd.env("KRAKEN_SPOT_URL", "https://127.0.0.1:9/")
        .env("KRAKEN_DANGER_ALLOW_ANY_URL_HOST", "1");
    cmd
}

// --- REPL scope stickiness ---

/// A workspace for REPL scope tests: created via the real command so the
/// existence gate passes.
fn create_workspace(home: &tempfile::TempDir, name: &str) {
    kraken_in(home)
        .args([
            "workspace",
            "create",
            name,
            "--capital",
            "1000",
            "--mode",
            "paper",
            "-o",
            "json",
        ])
        .assert()
        .success();
}

#[test]
fn repl_env_workspace_does_not_override_startup_workspace_flag() {
    // `kraken shell --workspace w1` with $KRAKEN_WORKSPACE=w2 exported: the
    // explicit startup flag wins; the constant env must not silently
    // re-scope every line.
    let home = tempfile::tempdir().unwrap();
    create_workspace(&home, "w1");
    create_workspace(&home, "w2");
    let out = kraken_in(&home)
        .env("KRAKEN_WORKSPACE", "w2")
        .args(["shell", "--workspace", "w1"])
        .write_stdin("paper balance -o json\n")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"workspace\":\"w1\""),
        "startup flag wins over env: {stdout}"
    );
}

#[test]
fn repl_per_line_workspace_flag_overrides_startup_workspace() {
    let home = tempfile::tempdir().unwrap();
    create_workspace(&home, "w1");
    create_workspace(&home, "w2");
    let out = kraken_in(&home)
        .args(["shell", "--workspace", "w1"])
        .write_stdin("paper balance --workspace w2 -o json\n")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"workspace\":\"w2\""),
        "explicit per-line flag re-scopes that line: {stdout}"
    );
}

// --- URL override rejection tests (REQ-005, REQ-020) ---

#[test]
fn env_var_rejects_http_spot_url() {
    kraken()
        .env("KRAKEN_SPOT_URL", "http://api.kraken.com")
        .env_remove("KRAKEN_FUTURES_URL")
        .env_remove("KRAKEN_WS_PUBLIC_URL")
        .env_remove("KRAKEN_WS_AUTH_URL")
        .args(["status"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Insecure").or(predicate::str::contains("rejected")));
}

#[test]
fn env_var_rejects_http_futures_url() {
    kraken()
        .env("KRAKEN_FUTURES_URL", "http://futures.kraken.com")
        .env_remove("KRAKEN_SPOT_URL")
        .env_remove("KRAKEN_WS_PUBLIC_URL")
        .env_remove("KRAKEN_WS_AUTH_URL")
        .args(["futures", "instruments"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Insecure").or(predicate::str::contains("rejected")));
}

#[test]
fn env_var_rejects_ws_public_url() {
    kraken()
        .env("KRAKEN_WS_PUBLIC_URL", "ws://ws.kraken.com/v2")
        .env_remove("KRAKEN_SPOT_URL")
        .env_remove("KRAKEN_FUTURES_URL")
        .env_remove("KRAKEN_WS_AUTH_URL")
        .args(["status"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Insecure").or(predicate::str::contains("rejected")));
}

#[test]
fn env_var_rejects_ws_auth_url() {
    kraken()
        .env("KRAKEN_WS_AUTH_URL", "ws://ws-auth.kraken.com/v2")
        .env_remove("KRAKEN_SPOT_URL")
        .env_remove("KRAKEN_FUTURES_URL")
        .env_remove("KRAKEN_WS_PUBLIC_URL")
        .args(["status"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Insecure").or(predicate::str::contains("rejected")));
}

#[test]
fn env_var_spot_url_override_used() {
    kraken()
        .env("KRAKEN_SPOT_URL", "https://api.kraken.com/override-test")
        .env_remove("KRAKEN_FUTURES_URL")
        .env_remove("KRAKEN_WS_PUBLIC_URL")
        .env_remove("KRAKEN_WS_AUTH_URL")
        .args(["status", "-v"])
        .assert()
        .failure() // will fail because the base path is invalid
        .stderr(predicate::str::contains("api.kraken.com/override-test"));
}

#[test]
fn env_var_futures_url_override_used() {
    kraken()
        .env(
            "KRAKEN_FUTURES_URL",
            "https://futures.kraken.com/override-test",
        )
        .env_remove("KRAKEN_SPOT_URL")
        .env_remove("KRAKEN_WS_PUBLIC_URL")
        .env_remove("KRAKEN_WS_AUTH_URL")
        .args(["futures", "instruments", "-v"])
        .assert()
        .failure() // will fail because the base path is invalid
        .stderr(predicate::str::contains("futures.kraken.com/override-test"));
}

#[test]
fn paper_commands_work_after_failed_auth() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .env_remove("KRAKEN_API_KEY")
        .env_remove("KRAKEN_API_SECRET")
        .args(["balance"])
        .assert()
        .failure();

    kraken_in(&home)
        .env_remove("KRAKEN_API_KEY")
        .env_remove("KRAKEN_API_SECRET")
        .args(["paper", "init", "w1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("w1"));

    kraken_in(&home)
        .env_remove("KRAKEN_API_KEY")
        .env_remove("KRAKEN_API_SECRET")
        .args(["paper", "balance", "w1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("USD"));
}

#[test]
fn paper_market_buy_fails_gracefully_without_network() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["paper", "init", "w1"])
        .assert()
        .success();

    kraken_offline_in(&home)
        .args(["--workspace", "w1", "paper", "buy", "BTCUSD", "0.1"])
        .assert()
        .failure();
}

#[test]
fn paper_market_sell_fails_gracefully_without_network() {
    let home = tempfile::tempdir().unwrap();
    let ws_dir = config_dir_in(&home).join("workspaces").join("w1");
    fs::create_dir_all(&ws_dir).unwrap();
    fs::write(
        ws_dir.join("workspace.json"),
        r#"{"workspace_version":"1.0","cli_version":"0.0.0-test","name":"w1","capital":"5000.0","currency":"USD","mode":"paper","fee_rate":"0.0026","slippage_rate":"0.0","created_at":"2026-01-01T00:00:00Z"}"#,
    )
    .unwrap();

    // A hand-written account journal with BTC holdings — also pins the on-disk
    // wire format: any drift in the record envelope or event shapes breaks
    // this fixture loudly.
    let log = concat!(
        r#"{"v":1,"ts":"2026-01-01T00:00:00+00:00","origin":"cli","event":"initialized","balance":"5000.0","currency":"USD","fee_rate":"0.0026","slippage_rate":"0.0"}"#,
        "\n",
        r#"{"v":1,"ts":"2026-01-02T00:00:00+00:00","origin":"cli","event":"order_filled","trade":{"id":"PAPER-00002","order_id":"PAPER-00001","pair":"BTCUSD","base":"BTC","quote":"USD","side":"buy","volume":"1.0","price":"4000.0","fee":"10.4","cost":"4000.0","filled_at":"2026-01-02T00:00:00+00:00"}}"#,
        "\n"
    );
    fs::write(ws_dir.join("journal.jsonl"), log).unwrap();

    // The funded holdings make the market sell reach the price fetch; the
    // unroutable venue makes that fetch fail — graceful error, no fill.
    kraken_in(&home)
        .env("KRAKEN_SPOT_URL", "https://api.kraken.com/override-test/")
        .args(["--workspace", "w1", "paper", "sell", "BTCUSD", "0.05"])
        .assert()
        .failure();
}

#[test]
fn paper_init_state_lives_under_the_workspaces_tree() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["paper", "init", "w1"])
        .assert()
        .success();

    let ws = config_dir_in(&home).join("workspaces").join("w1");
    assert!(
        ws.join("workspace.json").exists(),
        "init must write the workspace manifest at {ws:?}"
    );
    assert!(
        ws.join("journal.jsonl").exists(),
        "init must seed the workspace journal at {ws:?}"
    );

    // The retired flat spot account and the pre-workspace layouts must not
    // reappear (paper/ itself may exist later, but only for futures state).
    for legacy in ["paper/events.jsonl", "paper/state.json", "paper.json"] {
        assert!(
            !config_dir_in(&home).join(legacy).exists(),
            "legacy state file must not exist after fresh init: {legacy}"
        );
    }
}

#[test]
fn paper_init_custom_balance_and_currency() {
    let home = tempfile::tempdir().unwrap();
    // `--balance` survives as an alias of `--capital`.
    kraken_in(&home)
        .args([
            "paper",
            "init",
            "w1",
            "--balance",
            "5000",
            "--currency",
            "EUR",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("5000"))
        .stdout(predicate::str::contains("EUR"));
}

#[test]
fn paper_init_json_output_parsed() {
    // The JSON output is the WorkspaceManifest verbatim; money fields are
    // Decimal strings, never floats.
    let home = tempfile::tempdir().unwrap();
    let output = kraken_in(&home)
        .args(["paper", "init", "w1", "--output", "json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output must be valid JSON");
    assert_eq!(json["workspace_version"], "1.0");
    assert_eq!(json["name"], "w1");
    assert_eq!(json["mode"], "paper");
    assert_eq!(json["capital"], "10000");
    assert_eq!(json["currency"], "USD");
    assert_eq!(json["fee_rate"], "0.0026");
    assert_eq!(json["slippage_rate"], "0");
}

#[test]
fn paper_reset_returns_to_manifest_capital() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["paper", "init", "w1", "--capital", "10000"])
        .assert()
        .success();
    // Reset is destructive-confirmed; `--yes` stands in for the prompt.
    let output = kraken_in(&home)
        .args(["--yes", "paper", "reset", "w1", "-o", "json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output must be valid JSON");
    assert_eq!(json["group"], "workspace");
    assert_eq!(json["type"], "reset");
    assert_eq!(json["workspace"], "w1");
    assert_eq!(json["capital"], "10000");
    assert_eq!(json["currency"], "USD");
}

#[test]
fn paper_orders_shows_empty_table() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["paper", "init", "w1"])
        .assert()
        .success();
    kraken_in(&home)
        .args(["--workspace", "w1", "paper", "orders"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PAPER]"))
        .stdout(predicate::str::contains("No open orders"));
}

#[test]
fn paper_cancel_all_without_orders() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["paper", "init", "w1"])
        .assert()
        .success();
    kraken_in(&home)
        .args(["--workspace", "w1", "paper", "cancel-all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PAPER]"));
}

#[test]
fn paper_cancel_nonexistent_order_fails() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["paper", "init", "w1"])
        .assert()
        .success();
    kraken_in(&home)
        .args(["--workspace", "w1", "paper", "cancel", "PAPER-99999"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn bare_paper_status_targets_the_unfunded_global_account() {
    // With no workspace in scope, status addresses the
    // global paper account; before any init that account does not exist.
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["paper", "status"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Workspace 'global' not found"));
}

#[test]
fn paper_market_buy_fails_gracefully_with_error_message() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["paper", "init", "w1"])
        .assert()
        .success();
    kraken_offline_in(&home)
        .args(["--workspace", "w1", "paper", "buy", "BTCUSD", "0.1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error"));
}

#[test]
fn paper_status_json_output_parsed() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["paper", "init", "w1"])
        .assert()
        .success();
    let output = kraken_offline_in(&home)
        .args(["paper", "status", "w1", "--output", "json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output must be valid JSON");
    assert_eq!(json["mode"], "paper");
    assert_eq!(json["workspace"], "w1");
    assert!(json["valuation_complete"].is_boolean());
}

#[test]
fn futures_paper_help_shows_subcommands() {
    kraken()
        .args(["futures", "paper", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("reset"))
        .stdout(predicate::str::contains("balance"))
        .stdout(predicate::str::contains("buy"))
        .stdout(predicate::str::contains("sell"))
        .stdout(predicate::str::contains("orders"))
        .stdout(predicate::str::contains("cancel"))
        .stdout(predicate::str::contains("cancel-all"))
        .stdout(predicate::str::contains("positions"))
        .stdout(predicate::str::contains("fills"))
        .stdout(predicate::str::contains("history"))
        .stdout(predicate::str::contains("leverage"))
        .stdout(predicate::str::contains("set-leverage"))
        .stdout(predicate::str::contains("status"))
        .stdout(predicate::str::contains("batch-order"))
        .stdout(predicate::str::contains("order-status"))
        .stdout(predicate::str::contains("edit-order"));
}

#[test]
fn futures_paper_init_table_output_labeled() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[FUTURES PAPER]"));
}

#[test]
fn futures_paper_init_json_output_labeled() {
    let dir = tempfile::tempdir().unwrap();
    let output = kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "init", "--output", "json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"mode\""),
        "JSON output must contain mode field"
    );
    assert!(
        stdout.contains("\"futures_paper\""),
        "JSON mode must be futures_paper"
    );
}

#[test]
fn futures_paper_init_json_output_parsed() {
    let dir = tempfile::tempdir().unwrap();
    let output = kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "init", "--output", "json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output must be valid JSON");
    assert_eq!(json["mode"], "futures_paper");
    assert_eq!(json["starting_collateral"], 10000.0);
}

#[test]
fn futures_paper_init_custom_balance_and_currency() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args([
            "futures",
            "paper",
            "init",
            "--balance",
            "50000",
            "--currency",
            "EUR",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("50000"))
        .stdout(predicate::str::contains("EUR"));
}

#[test]
fn futures_paper_init_prevents_double_init() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "init"])
        .assert()
        .success();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "init"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already initialized"));
}

#[test]
fn futures_paper_commands_fail_without_init() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "balance"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not initialized"));
}

#[test]
fn futures_paper_status_fails_without_init() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "status"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not initialized"));
}

#[test]
fn futures_paper_balance_table_labeled() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "init"])
        .assert()
        .success();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "balance"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[FUTURES PAPER]"));
}

#[test]
fn futures_paper_status_table_labeled() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "init"])
        .assert()
        .success();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[FUTURES PAPER]"));
}

#[test]
fn futures_paper_status_json_output_parsed() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "init"])
        .assert()
        .success();
    let output = kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "status", "--output", "json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output must be valid JSON");
    assert_eq!(json["mode"], "futures_paper");
    assert!(json["starting_collateral"].is_number());
    assert!(json["collateral"].is_number());
    assert!(json["equity"].is_number());
    assert!(json["currency"].is_string());
}

#[test]
fn futures_paper_history_table_labeled() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "init"])
        .assert()
        .success();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "history"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[FP]"));
}

#[test]
fn futures_paper_reset_works() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "init"])
        .assert()
        .success();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "reset"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[FUTURES PAPER]"))
        .stdout(predicate::str::contains("reset"));
}

#[test]
fn futures_paper_orders_shows_empty_table() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "init"])
        .assert()
        .success();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "orders"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[FP]"));
}

#[test]
fn futures_paper_cancel_all_without_orders() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "init"])
        .assert()
        .success();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "cancel-all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[FUTURES PAPER]"));
}

#[test]
fn futures_paper_cancel_nonexistent_order_fails() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "init"])
        .assert()
        .success();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "cancel", "--order-id", "FP-99999"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn futures_paper_positions_empty() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "init"])
        .assert()
        .success();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "positions"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[FP]"));
}

#[test]
fn futures_paper_fills_empty() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "init"])
        .assert()
        .success();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "fills"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[FP]"));
}

#[test]
fn futures_paper_leverage_shows_empty() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "init"])
        .assert()
        .success();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "leverage"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[FP]"));
}

#[test]
fn futures_paper_set_leverage_works() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "init"])
        .assert()
        .success();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "set-leverage", "PF_XBTUSD", "10"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[FUTURES PAPER]"))
        .stdout(predicate::str::contains("PF_XBTUSD"))
        .stdout(predicate::str::contains("10"));
}

#[test]
fn futures_paper_commands_work_after_failed_auth() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .env_remove("KRAKEN_API_KEY")
        .env_remove("KRAKEN_API_SECRET")
        .args(["balance"])
        .assert()
        .failure();

    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .env_remove("KRAKEN_API_KEY")
        .env_remove("KRAKEN_API_SECRET")
        .args(["futures", "paper", "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[FUTURES PAPER]"));

    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .env_remove("KRAKEN_API_KEY")
        .env_remove("KRAKEN_API_SECRET")
        .args(["futures", "paper", "balance"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[FUTURES PAPER]"));
}

#[test]
fn futures_paper_state_in_correct_path() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "init"])
        .assert()
        .success();

    let mac_path = dir
        .path()
        .join("Library/Application Support/kraken/paper/futures_state.json");
    let xdg_path = dir.path().join(".config/kraken/paper/futures_state.json");
    assert!(
        mac_path.exists() || xdg_path.exists(),
        "State file must be at kraken/paper/futures_state.json; checked {mac_path:?} and {xdg_path:?}"
    );
}

#[test]
fn futures_paper_state_isolated_from_spot() {
    let home = tempfile::tempdir().unwrap();

    kraken_in(&home)
        .args(["paper", "init", "w1"])
        .assert()
        .success();

    kraken_in(&home)
        .args(["futures", "paper", "init"])
        .assert()
        .success();

    kraken_in(&home)
        .args(["paper", "balance", "w1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("USD"));

    kraken_in(&home)
        .args(["futures", "paper", "balance"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[FUTURES PAPER]"));

    // Futures keeps its flat paper/ state file; the spot account lives in the
    // workspace tree, so the retired flat spot journal must not reappear.
    let cfg = config_dir_in(&home);
    assert!(cfg.join("paper/futures_state.json").exists());
    assert!(!cfg.join("paper/events.jsonl").exists());
}

#[test]
fn futures_paper_buy_fails_gracefully_without_network() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "init"])
        .assert()
        .success();

    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .env(
            "KRAKEN_FUTURES_URL",
            "https://futures.kraken.com/override-test/",
        )
        .args([
            "futures",
            "paper",
            "buy",
            "PF_XBTUSD",
            "1",
            "--type",
            "market",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::is_empty().not());
}

#[test]
fn futures_paper_buy_rejects_reserved_symbol_chars() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args([
            "futures",
            "paper",
            "buy",
            "PF_XBT/USD",
            "1",
            "--type",
            "market",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must not contain '/'"));
}

#[test]
fn futures_paper_sell_fails_gracefully_without_network() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["futures", "paper", "init"])
        .assert()
        .success();

    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .env(
            "KRAKEN_FUTURES_URL",
            "https://futures.kraken.com/override-test/",
        )
        .args([
            "futures",
            "paper",
            "sell",
            "PF_XBTUSD",
            "1",
            "--type",
            "market",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::is_empty().not());
}

// --- event-sourced account log ---

/// The ticket's acceptance flow, fully offline: init, a resting limit buy, a
/// cancel, and a rejected order against a workspace must reconstruct the full
/// ordered lifecycle from the workspace's `journal.jsonl` alone.
#[test]
fn account_log_reconstructs_order_lifecycle_in_order() {
    let home = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        // Reconcile's price fetch must fail fast and gracefully offline.
        let mut cmd = kraken_offline_in(&home);
        cmd.args(args);
        cmd
    };

    run(&["paper", "init", "w1", "--capital", "10000"])
        .assert()
        .success();
    run(&[
        "--workspace",
        "w1",
        "paper",
        "buy",
        "BTCUSD",
        "0.1",
        "--type",
        "limit",
        "--price",
        "1.0",
    ])
    .assert()
    .success();
    run(&["--workspace", "w1", "paper", "cancel", "PAPER-00001"])
        .assert()
        .success();
    // Oversized reserve -> engine rejection, recorded on the log.
    run(&[
        "--workspace",
        "w1",
        "paper",
        "buy",
        "BTCUSD",
        "1000000",
        "--type",
        "limit",
        "--price",
        "1.0",
    ])
    .assert()
    .failure();

    let log_path = config_dir_in(&home)
        .join("workspaces")
        .join("w1")
        .join("journal.jsonl");
    let records: Vec<serde_json::Value> = fs::read_to_string(&log_path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();

    let events: Vec<&str> = records
        .iter()
        .map(|r| r["event"].as_str().unwrap())
        .collect();
    // Init (workspace create) seeds only the capital epoch — it is account
    // provisioning, not a trade command, so it writes no command audit entry.
    assert_eq!(
        events,
        [
            "initialized",
            "order_submitted",
            "command",
            "order_cancelled",
            "command",
            "order_rejected",
            "command",
        ],
        "full log: {records:#?}"
    );

    // The lifecycle joins by order id, and the cancel's timestamp is the
    // record's own — the information the snapshot could never hold.
    assert_eq!(records[1]["order"]["id"], "PAPER-00001");
    assert_eq!(records[3]["order"]["id"], "PAPER-00001");
    assert!(records[3]["ts"].as_str().unwrap() >= records[1]["ts"].as_str().unwrap());
    assert_eq!(records[5]["category"], "validation");
    assert_eq!(records[6]["outcome"]["result"], "err");
    // Command audit entries carry origin and post-command balances.
    assert_eq!(records[2]["origin"], "cli");
    assert_eq!(records[2]["balances"]["USD"], 10000.0);
}

#[test]
fn ws_symbol_channels_require_at_least_one_pair() {
    // An empty pairs list used to open a socket that subscribed to nothing and idled
    // forever (the server answers with silence, not a rejection); the parser now
    // refuses it up front. `instrument` stays pair-less by design: whole catalogue.
    for channel in ["ticker", "trades", "book", "ohlc", "level3"] {
        kraken()
            .args(["ws", channel])
            .assert()
            .failure()
            .code(2)
            .stderr(predicate::str::contains("PAIRS"));
    }
    // With -o json the same refusal keeps the machine contract: a validation
    // envelope on stdout, still exit 2.
    kraken()
        .args(["ws", "ticker", "-o", "json"])
        .assert()
        .code(2)
        .stdout(predicate::str::contains(r#""error":"validation""#));
}

#[test]
fn env_var_rejects_zero_ws_reconnect_base() {
    // Zero is the fixed point of the multiplicative backoff (0*2=0): the schedule
    // would never escalate and a down venue would be dialed in a hot loop.
    kraken()
        .env("KRAKEN_WS_RECONNECT_BASE_MS", "0")
        .args(["ws", "ticker", "BTC/USD"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("1.."));
}

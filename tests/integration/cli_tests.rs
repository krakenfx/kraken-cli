use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;

#[allow(deprecated)]
fn kraken() -> Command {
    Command::cargo_bin("kraken").unwrap()
}

#[test]
fn help_flag_shows_usage() {
    kraken()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Kraken CLI"))
        .stdout(predicate::str::contains("Usage:"));
}

#[test]
fn version_flag_shows_version() {
    kraken()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("kraken"));
}

#[test]
fn no_args_prints_help() {
    kraken()
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage:"));
}

#[test]
fn unknown_command_fails() {
    kraken().arg("nonexistent-command").assert().failure();
}

#[test]
fn order_help_shows_subcommands() {
    kraken()
        .args(["order", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("buy"))
        .stdout(predicate::str::contains("sell"))
        .stdout(predicate::str::contains("cancel"));
}

#[test]
fn futures_help_shows_subcommands() {
    kraken()
        .args(["futures", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("instruments"))
        .stdout(predicate::str::contains("tickers"))
        .stdout(predicate::str::contains("order"))
        .stdout(predicate::str::contains("feeschedules"))
        .stdout(predicate::str::contains("accounts"))
        .stdout(predicate::str::contains("positions"))
        .stdout(predicate::str::contains("ws"));
}

#[test]
fn futures_edit_order_help_shows_args() {
    kraken()
        .args(["futures", "edit-order", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("order-id"));
}

#[test]
fn futures_batch_order_help_shows_args() {
    kraken()
        .args(["futures", "batch-order", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("json"));
}

#[test]
fn futures_cancel_help_shows_args() {
    kraken()
        .args(["futures", "cancel", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("order-id"));
}

#[test]
fn futures_ws_help_shows_channels() {
    kraken()
        .args(["futures", "ws", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ticker"))
        .stdout(predicate::str::contains("fills"))
        .stdout(predicate::str::contains("book"));
}

#[test]
fn futures_ws_ticker_help_shows_markets() {
    kraken()
        .args(["futures", "ws", "ticker", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("MARKETS").or(predicate::str::contains("Market symbols")));
}

#[test]
fn futures_ws_trades_help_shows_markets() {
    kraken()
        .args(["futures", "ws", "trades", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("MARKETS").or(predicate::str::contains("Market symbols")));
}

#[test]
fn futures_ws_trade_alias_still_works() {
    kraken()
        .args(["futures", "ws", "trade", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Stream futures trades"));
}

#[test]
fn futures_ws_trades_requires_markets() {
    kraken()
        .args(["futures", "ws", "trades"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("MARKETS").and(predicate::str::contains("required")));
}

#[test]
fn futures_ws_ticker_requires_markets() {
    kraken()
        .args(["futures", "ws", "ticker"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("MARKETS").and(predicate::str::contains("required")));
}

#[test]
fn futures_ws_book_requires_markets() {
    kraken()
        .args(["futures", "ws", "book"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("MARKETS").and(predicate::str::contains("required")));
}

#[test]
fn ws_help_shows_subcommands() {
    kraken()
        .args(["ws", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ticker"))
        .stdout(predicate::str::contains("trades"))
        .stdout(predicate::str::contains("book"))
        .stdout(predicate::str::contains("executions"))
        .stdout(predicate::str::contains("balances"))
        .stdout(predicate::str::contains("instrument"))
        .stdout(predicate::str::contains("spread").not())
        .stdout(predicate::str::contains("own-trades").not())
        .stdout(predicate::str::contains("open-orders").not());
}

#[test]
fn mcp_help_shows_safe_default_services() {
    kraken()
        .args(["mcp", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[default: market,account,paper]"));
}

#[test]
fn mcp_help_shows_allow_dangerous_flag() {
    kraken()
        .args(["mcp", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--allow-dangerous"))
        .stdout(predicate::str::contains(
            "Skip per-call confirmation for dangerous tools",
        ));
}

#[test]
fn auth_help_shows_subcommands() {
    kraken()
        .args(["auth", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("set"))
        .stdout(predicate::str::contains("show"))
        .stdout(predicate::str::contains("test"))
        .stdout(predicate::str::contains("reset"));
}

#[test]
fn output_flag_accepts_table_and_json() {
    kraken()
        .args(["--output", "table", "--help"])
        .assert()
        .success();

    kraken()
        .args(["--output", "json", "--help"])
        .assert()
        .success();
}

#[test]
fn output_flag_rejects_invalid() {
    kraken()
        .args(["--output", "xml", "--help"])
        .assert()
        .failure();
}

#[test]
fn api_secret_and_stdin_are_mutually_exclusive() {
    kraken()
        .args(["--api-secret", "foo", "--api-secret-stdin", "status"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn balance_without_credentials_fails() {
    kraken()
        .env_remove("KRAKEN_API_KEY")
        .env_remove("KRAKEN_API_SECRET")
        .env("HOME", "/nonexistent")
        .args(["balance", "--output", "json"])
        .assert()
        .failure();
}

#[test]
fn json_error_is_valid_json() {
    let output = kraken()
        .env_remove("KRAKEN_API_KEY")
        .env_remove("KRAKEN_API_SECRET")
        .env("HOME", "/nonexistent")
        .args(["balance", "--output", "json"])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.is_empty() {
        let parsed: serde_json::Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|_| panic!("Expected valid JSON, got: {stdout}"));
        assert!(
            parsed.get("error").is_some(),
            "Expected error field in JSON"
        );
    }
}

#[test]
fn earn_help_shows_subcommands() {
    kraken()
        .args(["earn", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("allocate"))
        .stdout(predicate::str::contains("strategies"));
}

#[test]
fn deposit_help_shows_subcommands() {
    kraken()
        .args(["deposit", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("methods"))
        .stdout(predicate::str::contains("addresses"))
        .stdout(predicate::str::contains("status"));
}

#[test]
fn withdrawal_help_shows_subcommands() {
    kraken()
        .args(["withdrawal", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("methods"))
        .stdout(predicate::str::contains("addresses"))
        .stdout(predicate::str::contains("cancel"));
}

#[test]
fn auth_set_futures_secret_requires_futures_key() {
    kraken()
        .args([
            "auth",
            "set",
            "--api-key",
            "spot-key",
            "--api-secret",
            "spot-secret",
            "--futures-api-secret",
            "orphan-secret",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--futures-api-key"));
}

#[test]
fn auth_set_futures_secret_stdin_requires_futures_key() {
    kraken()
        .args([
            "auth",
            "set",
            "--api-key",
            "spot-key",
            "--api-secret",
            "spot-secret",
            "--futures-api-secret-stdin",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--futures-api-key"));
}

#[test]
fn auth_set_futures_secret_file_requires_futures_key() {
    kraken()
        .args([
            "auth",
            "set",
            "--api-key",
            "spot-key",
            "--api-secret",
            "spot-secret",
            "--futures-api-secret-file",
            "/tmp/nonexistent",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--futures-api-key"));
}

// --- Paper Trading tests ---

#[test]
fn paper_help_shows_subcommands() {
    kraken()
        .args(["paper", "--help"])
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
        .stdout(predicate::str::contains("history"))
        .stdout(predicate::str::contains("status"));
}

#[test]
fn paper_init_table_output_labeled() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PAPER]"));
}

#[test]
fn paper_init_json_output_labeled() {
    let dir = tempfile::tempdir().unwrap();
    let output = kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "init", "--output", "json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"mode\""),
        "JSON output must contain mode field"
    );
    assert!(stdout.contains("\"paper\""), "JSON mode must be paper");
}

#[test]
fn paper_balance_table_labeled() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "init"])
        .assert()
        .success();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "balance"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PAPER]"));
}

#[test]
fn paper_history_table_labeled() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "init"])
        .assert()
        .success();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "history"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PAPER]"));
}

#[test]
fn paper_status_table_labeled() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "init"])
        .assert()
        .success();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PAPER]"));
}

#[test]
fn paper_status_json_output_labeled() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "init"])
        .assert()
        .success();
    let output = kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "status", "--output", "json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"mode\""),
        "JSON output must contain mode field"
    );
    assert!(stdout.contains("\"paper\""), "JSON mode must be paper");
    assert!(
        stdout.contains("\"valuation_complete\""),
        "JSON must include valuation_complete"
    );
}

#[test]
fn paper_init_prevents_double_init() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "init"])
        .assert()
        .success();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "init"])
        .assert()
        .failure();
}

#[test]
fn paper_commands_fail_without_init() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "balance"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not initialized"));
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

// --- Paper Trading: additional integration tests ---

#[test]
fn paper_commands_work_after_failed_auth() {
    let dir = tempfile::tempdir().unwrap();
    // First, a real auth command fails (no credentials)
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .env_remove("KRAKEN_API_KEY")
        .env_remove("KRAKEN_API_SECRET")
        .args(["balance"])
        .assert()
        .failure();

    // Paper init should still work
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .env_remove("KRAKEN_API_KEY")
        .env_remove("KRAKEN_API_SECRET")
        .args(["paper", "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PAPER]"));

    // Paper balance should work
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .env_remove("KRAKEN_API_KEY")
        .env_remove("KRAKEN_API_SECRET")
        .args(["paper", "balance"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PAPER]"));
}

#[test]
fn paper_market_buy_fails_gracefully_without_network() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "init"])
        .assert()
        .success();

    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .env("KRAKEN_SPOT_URL", "https://api.kraken.com/override-test/")
        .args(["paper", "buy", "BTCUSD", "0.1"])
        .assert()
        .failure();
}

#[test]
fn paper_market_sell_fails_gracefully_without_network() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join(".config").join("kraken");
    let paper_dir = config_dir.join("paper");
    fs::create_dir_all(&paper_dir).unwrap();

    // Create a paper state with some BTC
    let state = serde_json::json!({
        "balances": {"USD": 5000.0, "BTC": 1.0},
        "reserved": {},
        "open_orders": [],
        "filled_trades": [],
        "starting_balance": 10000.0,
        "starting_currency": "USD",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
        "next_order_id": 1
    });
    fs::write(
        paper_dir.join("state.json"),
        serde_json::to_string_pretty(&state).unwrap(),
    )
    .unwrap();

    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .env("KRAKEN_SPOT_URL", "https://api.kraken.com/override-test/")
        .args(["paper", "sell", "BTCUSD", "0.05"])
        .assert()
        .failure();
}

#[test]
fn paper_state_in_subdirectory() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "init"])
        .assert()
        .success();

    // On macOS dirs::config_dir() = HOME/Library/Application Support;
    // on Linux it respects XDG_CONFIG_HOME.
    let mac_path = dir
        .path()
        .join("Library/Application Support/kraken/paper/state.json");
    let xdg_path = dir.path().join(".config/kraken/paper/state.json");
    assert!(
        mac_path.exists() || xdg_path.exists(),
        "State file must be at kraken/paper/state.json; checked {mac_path:?} and {xdg_path:?}"
    );

    let mac_legacy = dir
        .path()
        .join("Library/Application Support/kraken/paper.json");
    let xdg_legacy = dir.path().join(".config/kraken/paper.json");
    assert!(
        !mac_legacy.exists() && !xdg_legacy.exists(),
        "Legacy paper.json must not exist after fresh init"
    );
}

#[test]
fn paper_init_custom_balance_and_currency() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "init", "--balance", "5000", "--currency", "EUR"])
        .assert()
        .success()
        .stdout(predicate::str::contains("5000"))
        .stdout(predicate::str::contains("EUR"));
}

#[test]
fn paper_init_json_output_parsed() {
    let dir = tempfile::tempdir().unwrap();
    let output = kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "init", "--output", "json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output must be valid JSON");
    assert_eq!(json["mode"], "paper");
    assert_eq!(json["starting_balance"], 10000.0);
}

#[test]
fn paper_reset_works() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "init"])
        .assert()
        .success();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "reset"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PAPER]"))
        .stdout(predicate::str::contains("reset"));
}

#[test]
fn paper_orders_shows_empty_table() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "init"])
        .assert()
        .success();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "orders"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PAPER]"));
}

#[test]
fn paper_cancel_all_without_orders() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "init"])
        .assert()
        .success();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "cancel-all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PAPER]"));
}

#[test]
fn paper_cancel_nonexistent_order_fails() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "init"])
        .assert()
        .success();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "cancel", "PAPER-99999"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn paper_status_fails_without_init() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "status"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not initialized"));
}

#[test]
fn paper_market_buy_fails_gracefully_with_error_message() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "init"])
        .assert()
        .success();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .env("KRAKEN_SPOT_URL", "https://api.kraken.com/override-test/")
        .args(["paper", "buy", "BTCUSD", "0.1"])
        .assert()
        .failure()
        .stderr(predicate::str::is_empty().not());
}

#[test]
fn paper_status_json_output_parsed() {
    let dir = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "init"])
        .assert()
        .success();
    let output = kraken()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .args(["paper", "status", "--output", "json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output must be valid JSON");
    assert_eq!(json["mode"], "paper");
    assert!(json["valuation_complete"].is_boolean());
}

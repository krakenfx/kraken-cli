//! `kraken workspace` end to end: create-as-fund, the synthesized default
//! row, and the create=fund law feeding the paper trade path.

use super::common::{config_dir_in, kraken_in};
use predicates::prelude::*;

#[test]
fn workspace_create_funds_the_account_in_one_step() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "workspace",
            "create",
            "w1",
            "--capital",
            "10000",
            "--mode",
            "paper",
            "-o",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"capital\":\"10000\""))
        .stdout(predicate::str::contains("\"mode\":\"paper\""));

    let dir = config_dir_in(&home).join("workspaces").join("w1");
    assert!(dir.join("workspace.json").exists(), "contract written");
    let journal = std::fs::read_to_string(dir.join("journal.jsonl")).unwrap();
    assert!(
        journal.contains("\"initialized\""),
        "journal seeded with the capital epoch: {journal}"
    );
}

#[test]
fn workspace_create_then_paper_buy_trades_the_funded_account() {
    // The create=fund acceptance test: a limit order (offline-capable) against
    // the workspace journal reserves part of the created capital.
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "workspace",
            "create",
            "w1",
            "--capital",
            "10000",
            "--mode",
            "paper",
            "-o",
            "json",
        ])
        .assert()
        .success();
    kraken_in(&home)
        .args([
            "--workspace",
            "w1",
            "paper",
            "buy",
            "BTCUSD",
            "0.01",
            "--type",
            "limit",
            "--price",
            "100",
            "-o",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("PAPER-00001"));

    let out = kraken_in(&home)
        .args(["--workspace", "w1", "paper", "balance", "-o", "json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    // Cost (0.01 x 100) plus the 0.26% fee reserve, digit-exact.
    assert_eq!(json["balances"]["USD"]["reserved"], "1.002600");
    assert_eq!(json["workspace"], "w1");
}

#[test]
fn paper_trade_warns_when_no_run_is_recording() {
    // A trade with no active session lands on the account but in no session window,
    // so it warns rather than silently orphaning onto a stopped/aborted
    // session. A resting limit order keeps the assertion offline (no live fill
    // needed).
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "workspace",
            "create",
            "w1",
            "--capital",
            "10000",
            "--mode",
            "paper",
        ])
        .assert()
        .success();
    let out = kraken_in(&home)
        .args([
            "--workspace",
            "w1",
            "paper",
            "buy",
            "BTCUSD",
            "0.001",
            "--type",
            "limit",
            "--price",
            "100",
            "-o",
            "json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no active session"),
        "a runless trade must warn: {stderr}"
    );
}

#[test]
fn workspace_list_synthesizes_the_default_account_row() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "workspace",
            "create",
            "alpha",
            "--capital",
            "500",
            "--mode",
            "paper",
            "-o",
            "json",
        ])
        .assert()
        .success();
    let out = kraken_in(&home)
        .args(["workspace", "list", "-o", "json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let rows = json["workspaces"].as_array().unwrap();
    assert_eq!(rows[0]["name"], "default");
    assert_eq!(rows[0]["mode"], "live");
    assert!(rows[0].get("capital").is_none(), "venue-held, never local");
    assert_eq!(rows[1]["name"], "alpha");
    assert_eq!(rows[1]["capital"], "500");
}

#[test]
fn workspace_create_ignores_the_ambient_workspace() {
    // Create addresses accounts by name; it must never resolve the scope tree
    // (the workspace being born cannot be the active scope).
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .env("KRAKEN_WORKSPACE", "ghost")
        .args([
            "workspace",
            "create",
            "w1",
            "--capital",
            "100",
            "--mode",
            "paper",
            "-o",
            "json",
        ])
        .assert()
        .success();
}

#[test]
fn workspace_reset_returns_to_capital_and_needs_confirmation() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "workspace",
            "create",
            "w1",
            "--capital",
            "1000",
            "--mode",
            "paper",
            "-o",
            "json",
        ])
        .assert()
        .success();
    // Reserve some capital first so the reset visibly undoes it.
    kraken_in(&home)
        .args([
            "--workspace",
            "w1",
            "paper",
            "buy",
            "BTCUSD",
            "0.01",
            "--type",
            "limit",
            "--price",
            "100",
            "-o",
            "json",
        ])
        .assert()
        .success();
    kraken_in(&home)
        .args(["--yes", "workspace", "reset", "w1", "-o", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"capital\":\"1000\""));

    let out = kraken_in(&home)
        .args(["workspace", "balance", "w1", "-o", "json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["balances"]["USD"]["total"], "1000");
    assert_eq!(json["balances"]["USD"]["reserved"], "0");
}

#[test]
fn paper_init_is_workspace_create_under_another_name() {
    // One implementation, two names: init writes the same contract create does.
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "paper",
            "init",
            "sandbox",
            "--balance",
            "2500",
            "-o",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"capital\":\"2500\""))
        .stdout(predicate::str::contains("\"mode\":\"paper\""));
    assert!(
        config_dir_in(&home)
            .join("workspaces")
            .join("sandbox")
            .join("workspace.json")
            .exists(),
        "init wrote the workspace contract"
    );
}

#[test]
fn withdraw_refuses_inside_a_paper_workspace_naming_the_promotion_path() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "workspace",
            "create",
            "w1",
            "--capital",
            "1000",
            "--mode",
            "paper",
            "-o",
            "json",
        ])
        .assert()
        .success();
    kraken_in(&home)
        .args([
            "--workspace",
            "w1",
            "withdraw",
            "XBT",
            "cold",
            "0.1",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("no paper equivalent"))
        .stdout(predicate::str::contains("workspace promote"));
}

#[test]
fn ws_add_order_is_refused_at_dispatch_not_at_stream_time() {
    // The guard sits before the streaming bypass in the executor: a ws order
    // method refuses without opening any socket.
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "workspace",
            "create",
            "w1",
            "--capital",
            "1000",
            "--mode",
            "paper",
            "-o",
            "json",
        ])
        .assert()
        .success();
    kraken_in(&home)
        .args([
            "--workspace",
            "w1",
            "ws",
            "add-order",
            "--symbol",
            "BTC/USD",
            "--side",
            "buy",
            "--order-type",
            "market",
            "--order-qty",
            "0.1",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("no paper equivalent"));
}

#[test]
fn futures_paper_mutations_refuse_inside_a_workspace_but_reads_pass() {
    // Futures paper state is one global snapshot: a mutation
    // from inside a workspace would corrupt shared state; reads stay honest.
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "workspace",
            "create",
            "w1",
            "--capital",
            "1000",
            "--mode",
            "paper",
            "-o",
            "json",
        ])
        .assert()
        .success();
    kraken_in(&home)
        .args([
            "--workspace",
            "w1",
            "futures",
            "paper",
            "init",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("not workspace-scoped"));
    let read = kraken_in(&home)
        .args([
            "--workspace",
            "w1",
            "futures",
            "paper",
            "status",
            "-o",
            "json",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&read.stdout);
    assert!(
        !stdout.contains("not workspace-scoped"),
        "reads pass the guard (whatever else they report): {stdout}"
    );
}

#[test]
fn order_buy_inside_a_paper_workspace_fills_on_the_workspace_journal() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "workspace",
            "create",
            "w1",
            "--capital",
            "100000",
            "--mode",
            "paper",
            "-o",
            "json",
        ])
        .assert()
        .success();
    // A limit order rests deterministically (no live price fetch), and the
    // receipt is the unified envelope: mode identity + paper economics.
    let placed = kraken_in(&home)
        .args([
            "--workspace",
            "w1",
            "order",
            "buy",
            "BTC/USD",
            "0.01",
            "--type",
            "limit",
            "--price",
            "100",
            "-o",
            "json",
        ])
        .output()
        .unwrap();
    assert!(placed.status.success(), "order buy failed: {placed:?}");
    let receipt: serde_json::Value =
        serde_json::from_slice(&placed.stdout).expect("receipt is json");
    assert_eq!(receipt["mode"], "paper");
    assert_eq!(receipt["status"], "submitted");
    assert_eq!(receipt["side"], "buy");
    assert!(receipt["order_id"].as_str().is_some(), "receipt: {receipt}");
    assert!(
        receipt.get("venue").is_none(),
        "paper receipts carry no venue block"
    );
    // The same journal the paper verbs read: the order is visible both ways.
    let orders = kraken_in(&home)
        .args(["--workspace", "w1", "open-orders", "-o", "json"])
        .output()
        .unwrap();
    assert!(orders.status.success());
    let stdout = String::from_utf8_lossy(&orders.stdout);
    assert!(
        stdout.contains(receipt["order_id"].as_str().unwrap()),
        "routed open-orders must show the routed order: {stdout}"
    );
}

#[test]
fn order_cancel_inside_a_paper_workspace_cancels_on_the_workspace_journal() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "workspace",
            "create",
            "w1",
            "--capital",
            "100000",
            "--mode",
            "paper",
            "-o",
            "json",
        ])
        .assert()
        .success();
    let placed = kraken_in(&home)
        .args([
            "--workspace",
            "w1",
            "order",
            "buy",
            "BTC/USD",
            "0.01",
            "--type",
            "limit",
            "--price",
            "100",
            "-o",
            "json",
        ])
        .output()
        .unwrap();
    let receipt: serde_json::Value = serde_json::from_slice(&placed.stdout).unwrap();
    let order_id = receipt["order_id"].as_str().unwrap();
    kraken_in(&home)
        .args([
            "--workspace",
            "w1",
            "order",
            "cancel",
            order_id,
            "-o",
            "json",
        ])
        .assert()
        .success();
    let orders = kraken_in(&home)
        .args(["--workspace", "w1", "open-orders", "-o", "json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&orders.stdout);
    assert!(
        !stdout.contains(order_id),
        "cancelled order must leave the book: {stdout}"
    );
}

#[test]
fn balance_inside_a_workspace_shows_the_workspace_account() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "workspace",
            "create",
            "w1",
            "--capital",
            "1000",
            "--mode",
            "paper",
            "-o",
            "json",
        ])
        .assert()
        .success();
    // No credentials in the environment: reaching the venue would fail auth,
    // so success here proves the read was routed to the workspace journal.
    let read = kraken_in(&home)
        .args(["--workspace", "w1", "balance", "-o", "json"])
        .output()
        .unwrap();
    assert!(read.status.success(), "routed balance failed: {read:?}");
    let stdout = String::from_utf8_lossy(&read.stdout);
    assert!(
        stdout.contains("1000"),
        "workspace capital visible: {stdout}"
    );
}

#[test]
fn workspace_order_reason_lands_in_the_workspace_decision_log() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "workspace",
            "create",
            "w1",
            "--capital",
            "100000",
            "--mode",
            "paper",
            "-o",
            "json",
        ])
        .assert()
        .success();
    let placed = kraken_in(&home)
        .args([
            "--workspace",
            "w1",
            "order",
            "buy",
            "BTC/USD",
            "0.01",
            "--type",
            "limit",
            "--price",
            "100",
            "--reason",
            "support retest at 100",
            "-o",
            "json",
        ])
        .output()
        .unwrap();
    assert!(placed.status.success(), "order failed: {placed:?}");
    let receipt: serde_json::Value = serde_json::from_slice(&placed.stdout).unwrap();

    let decisions = std::fs::read_to_string(
        config_dir_in(&home)
            .join("workspaces")
            .join("w1")
            .join("decisions.jsonl"),
    )
    .expect("workspace decision log exists");
    let decision: serde_json::Value =
        serde_json::from_str(decisions.lines().next().unwrap()).unwrap();
    assert_eq!(decision["reason"], "support retest at 100");
    // Keyed by order id, so explain-pnl can join decision to fill.
    assert_eq!(decision["order_id"], receipt["order_id"]);
}

#[test]
fn venue_only_read_filters_are_refused_inside_a_paper_workspace() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "workspace",
            "create",
            "w1",
            "--capital",
            "1000",
            "--mode",
            "paper",
            "-o",
            "json",
        ])
        .assert()
        .success();
    kraken_in(&home)
        .args([
            "--workspace",
            "w1",
            "trades-history",
            "--ledgers",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("trades-history --ledgers"));
}

#[test]
fn unsimulatable_order_verbs_refuse_inside_a_paper_workspace() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "workspace",
            "create",
            "w1",
            "--capital",
            "1000",
            "--mode",
            "paper",
            "-o",
            "json",
        ])
        .assert()
        .success();
    // order edit hits the venue directly — the guard must stop it before I/O.
    kraken_in(&home)
        .args([
            "--workspace",
            "w1",
            "order",
            "edit",
            "OTXID-1",
            "--price",
            "50",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("order edit"));
}

#[test]
fn a_live_allocation_the_account_cannot_cover_is_refused_at_create() {
    let home = tempfile::tempdir().unwrap();
    // No credentials and no venue: the allocation cannot be checked against a
    // real balance, and an unverifiable claim is not a safer claim.
    kraken_in(&home)
        .args([
            "workspace",
            "create",
            "prod",
            "--capital",
            "1000",
            "--mode",
            "live",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("account balance unread"));

    // Nothing was written: a refused create leaves no half-made workspace.
    assert!(
        !config_dir_in(&home)
            .join("workspaces")
            .join("prod")
            .exists(),
        "a refused live create must not leave a workspace behind"
    );
}

#[test]
fn live_workspace_refuses_a_disallowed_pair_before_any_network_call() {
    let home = tempfile::tempdir().unwrap();
    plant_live_workspace(&home, "prod", Some(&["BTC/USD"]));

    kraken_in(&home)
        .args([
            "--workspace",
            "prod",
            "order",
            "buy",
            "ETH/USD",
            "0.01",
            "--type",
            "limit",
            "--price",
            "1",
            "--validate",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("not permitted"))
        // The allow-list is policy, not authentication: it must refuse without
        // ever reaching for a credential.
        .stdout(predicate::str::contains("auth").not());
}

#[test]
fn the_simulator_cannot_write_fills_into_a_live_workspace_ledger() {
    let home = tempfile::tempdir().unwrap();
    plant_live_workspace(&home, "prod", None);

    for verb in [
        vec!["paper", "buy", "BTC/USD", "0.01"],
        vec!["paper", "sell", "BTC/USD", "0.01"],
        vec!["paper", "orders"],
        vec!["paper", "history"],
        vec!["paper", "cancel-all"],
    ] {
        let mut args = vec!["--workspace", "prod"];
        args.extend(verb.iter().copied());
        args.extend(["-o", "json"]);
        kraken_in(&home)
            .args(&args)
            .assert()
            .failure()
            .stdout(predicate::str::contains("simulates fills"))
            .stdout(predicate::str::contains("is live"));
    }
}

/// What cannot settle a fill from recorded prices against a ledger of real
/// executions: replaying a tape, and sealing a plan that commits to replaying
/// one later. Recorded sessions and scoring are fine — only these two are
/// refused, and both by the one shared guard.
#[test]
fn replay_and_a_sealed_replay_leg_refuse_inside_a_live_workspace() {
    let home = tempfile::tempdir().unwrap();
    plant_live_workspace(&home, "prod", None);

    for verb in [
        vec![
            "session",
            "start",
            "--from",
            "tape:whatever",
            "--speed",
            "10",
        ],
        vec![
            "lab",
            "new",
            "exp1",
            "--hypothesis",
            "h",
            "--min-return-pct",
            "0",
            "--session",
            "replay:tape:whatever",
        ],
    ] {
        let mut args = vec!["--workspace", "prod"];
        args.extend(verb.iter().copied());
        args.extend(["-o", "json"]);
        kraken_in(&home)
            .args(&args)
            .assert()
            .failure()
            .stdout(predicate::str::contains(
                "settles fills from recorded prices",
            ))
            .stdout(predicate::str::contains("workspace 'prod' is live"));
    }
}

/// The promotion gate grades its own checklist through `Lab::compare` on the
/// workspace it is promoting, so refusing the user that same read left the
/// evidence behind a promotion unreadable in the workspace holding it. The
/// guard must let the lab reads through — reaching the lab's own "not found",
/// not the guard's refusal, is what tells the two apart.
#[test]
fn the_lab_reads_reach_the_lab_inside_a_live_workspace() {
    let home = tempfile::tempdir().unwrap();
    plant_live_workspace(&home, "prod", None);

    for verb in [
        vec!["lab", "show", "ghost"],
        vec!["lab", "compare", "ghost"],
        vec!["lab", "next", "ghost"],
    ] {
        let mut args = vec!["--workspace", "prod"];
        args.extend(verb.iter().copied());
        args.extend(["-o", "json"]);
        kraken_in(&home)
            .args(&args)
            .assert()
            .failure()
            .stdout(predicate::str::contains("settles fills from recorded prices").not())
            .stdout(predicate::str::contains("ghost"));
    }
}

/// A live-only plan is runnable in a live workspace, so sealing one there is
/// not refused: only the replay leg ever was.
#[test]
fn a_live_only_plan_seals_inside_a_live_workspace() {
    let home = tempfile::tempdir().unwrap();
    plant_live_workspace(&home, "prod", None);

    kraken_in(&home)
        .args([
            "--workspace",
            "prod",
            "lab",
            "new",
            "live-only",
            "--hypothesis",
            "h",
            "--min-return-pct",
            "0",
            "--session",
            "live:10s",
            "-o",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"frozen\":\"sha256:"));
}

/// The other direction of the same guard, and the one that matters most: a
/// paper workspace is the ONLY place the lab and replay can run, and the
/// promotion checklist reads its evidence out of that workspace's own scope. A
/// refusal here would make every workspace unpromotable, so pin both verbs.
#[test]
fn replay_and_the_lab_run_inside_a_paper_workspace() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "workspace",
            "create",
            "sandbox",
            "--capital",
            "10000",
            "--mode",
            "paper",
            "-o",
            "json",
        ])
        .assert()
        .success();

    kraken_in(&home)
        .args([
            "--workspace",
            "sandbox",
            "lab",
            "new",
            "exp1",
            "--hypothesis",
            "h",
            "--min-return-pct",
            "0",
            "-o",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\":\"exp1\""))
        .stdout(predicate::str::contains("\"frozen\":\"sha256:"));

    // Replay still fails on the missing tape — but on the tape, not the guard.
    kraken_in(&home)
        .args([
            "--workspace",
            "sandbox",
            "session",
            "start",
            "--from",
            "tape:whatever",
            "-o",
            "json",
        ])
        .assert()
        .stdout(predicate::str::contains("settles fills from recorded prices").not());
}

/// Recording live inside a live workspace is the point of the feature, so the
/// replay refusal must not catch it. `--for 0s` closes the window immediately.
#[test]
fn a_live_workspace_may_still_record_a_session() {
    let home = tempfile::tempdir().unwrap();
    plant_live_workspace(&home, "prod", None);

    kraken_in(&home)
        .args([
            "--workspace",
            "prod",
            "session",
            "start",
            "--symbols",
            "BTC/USD",
            "--channels",
            "ticker",
            "--for",
            "0s",
            "-o",
            "json",
        ])
        .assert()
        .stdout(predicate::str::contains("settles fills from recorded prices").not());
}

/// A live workspace contract on disk, without going through `workspace create`
/// (which verifies the allocation against a real balance).
fn plant_live_workspace(home: &tempfile::TempDir, name: &str, allowed_pairs: Option<&[&str]>) {
    let ws = config_dir_in(home).join("workspaces").join(name);
    std::fs::create_dir_all(&ws).unwrap();
    let mut manifest = serde_json::json!({
        "workspace_version": "1.0",
        "cli_version": "0.0.0",
        "name": name,
        "capital": "1000",
        "currency": "USD",
        "mode": "live",
        "fee_rate": "0.0026",
        "slippage_rate": "0",
        "created_at": "2026-01-01T00:00:00Z"
    });
    if let Some(pairs) = allowed_pairs {
        manifest["allowed_pairs"] = serde_json::json!(pairs);
    }
    std::fs::write(ws.join("workspace.json"), manifest.to_string()).unwrap();
}

#[test]
fn allow_pairs_workspace_refuses_an_unlisted_pair() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "workspace",
            "create",
            "scalper",
            "--capital",
            "2000",
            "--mode",
            "paper",
            "--allow-pairs",
            "BTC/USD,ETH/USD",
            "-o",
            "json",
        ])
        .assert()
        .success();
    // A listed pair trades; policy is checked in the engine's own grammar.
    kraken_in(&home)
        .args([
            "--workspace",
            "scalper",
            "paper",
            "buy",
            "xbtusd",
            "0.01",
            "--type",
            "limit",
            "--price",
            "100",
            "-o",
            "json",
        ])
        .assert()
        .success();
    kraken_in(&home)
        .args([
            "--workspace",
            "scalper",
            "paper",
            "buy",
            "SOLUSD",
            "1",
            "--type",
            "limit",
            "--price",
            "10",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("not permitted"))
        .stdout(predicate::str::contains("BTC/USD, ETH/USD"));
}

#[test]
fn deny_all_workspace_refuses_every_pair() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "workspace",
            "create",
            "vault",
            "--capital",
            "2000",
            "--mode",
            "paper",
            "--allow-pairs",
            "-o",
            "json",
        ])
        .assert()
        .success();
    kraken_in(&home)
        .args([
            "--workspace",
            "vault",
            "paper",
            "buy",
            "BTCUSD",
            "0.01",
            "--type",
            "limit",
            "--price",
            "100",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("deny-all"));
}

#[test]
fn workspace_promote_refusal_envelope_carries_the_checklist() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "workspace",
            "create",
            "w1",
            "--capital",
            "1000",
            "--mode",
            "paper",
            "-o",
            "json",
        ])
        .assert()
        .success();
    let out = kraken_in(&home)
        .args(["--yes", "workspace", "promote", "w1", "-o", "json"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "missing evidence must refuse before provisioning: {out:?}"
    );
    let envelope: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(envelope["error"], "validation");
    let checklist = &envelope["checklist"];
    assert_eq!(checklist["workspace"], "w1");
    assert_eq!(checklist["promotable"], false);
    let criteria = checklist["criteria"].as_array().unwrap();
    assert_eq!(
        criteria.len(),
        4,
        "only paper evidence is graded: {criteria:?}"
    );
    assert_eq!(
        checklist["blockers"].as_array().map(Vec::len),
        Some(0),
        "no session or open-order blockers exist"
    );
}

#[test]
fn workspace_promote_of_the_default_account_is_refused() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["--yes", "workspace", "promote", "-o", "json"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("nothing to promote"));
}

#[test]
fn workspace_report_totals_re_sum_from_its_own_rows() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "workspace",
            "create",
            "w1",
            "--capital",
            "1000",
            "--mode",
            "paper",
            "-o",
            "json",
        ])
        .assert()
        .success();
    let out = kraken_in(&home)
        .env("KRAKEN_SPOT_URL", "https://127.0.0.1:9/")
        .env("KRAKEN_DANGER_ALLOW_ANY_URL_HOST", "1")
        .args(["workspace", "report", "w1", "-o", "json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["workspace"], "w1");
    assert_eq!(report["mode"], "paper");
    assert_eq!(report["capital"], "1000");
    assert_eq!(report["sessions"], serde_json::json!([]));
    assert_eq!(
        report["sessions_pnl"], "0",
        "no rows, zero sum — digit-exact"
    );
    // A cash-only account values completely even with the venue dead.
    assert_eq!(report["equity"], "1000");
    assert_eq!(report["valuation_complete"], true);
}

/// The public paper contract: `paper reset --balance 7000` starts the
/// account over with new terms — restored as a delegate over
/// `workspace reset --capital`, updating the contract alongside the epoch.
#[test]
fn paper_reset_balance_reparameterizes_the_account_and_contract() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["paper", "init", "w1", "--capital", "5000", "-o", "json"])
        .assert()
        .success();
    let out = kraken_in(&home)
        .args([
            "--yes",
            "paper",
            "reset",
            "w1",
            "--balance",
            "7000",
            "-o",
            "json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["capital"], "7000");

    // Both the balances and the durable contract follow.
    let balance = kraken_in(&home)
        .args(["workspace", "balance", "w1", "-o", "json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&balance.stdout).unwrap();
    assert_eq!(json["balances"]["USD"]["total"], "7000");
    let contract: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            config_dir_in(&home)
                .join("workspaces")
                .join("w1")
                .join("workspace.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(contract["capital"], "7000");
}

#[test]
fn plain_reset_still_returns_to_the_created_capital() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["paper", "init", "w1", "--capital", "5000", "-o", "json"])
        .assert()
        .success();
    kraken_in(&home)
        .args(["--yes", "paper", "reset", "w1", "-o", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"capital\":\"5000\""));
}

//! The live allocation gate through the real binary, offline: the CLI's
//! https-only rule keeps a plain-http mock venue out of reach here, so these
//! tests pin the wiring end to end without a venue — the full gate behaviour
//! against a mock venue lives in `trade.rs`'s `allocation_gate` unit tests.

use super::common::{config_dir_in, kraken_in};
use base64::Engine as _;
use predicates::prelude::*;

/// An isolated CLI pointed at a venue that answers nothing: fake creds, an
/// unreachable https URL, and the host allowlist relaxed for loopback.
fn kraken_offline(home: &tempfile::TempDir) -> assert_cmd::Command {
    let mut cmd = kraken_in(home);
    cmd.env("KRAKEN_API_KEY", "test-api-key")
        .env(
            "KRAKEN_API_SECRET",
            base64::engine::general_purpose::STANDARD.encode(b"test_secret_key_bytes_32_chars!!"),
        )
        .env("KRAKEN_SPOT_URL", "https://127.0.0.1:1")
        .env("KRAKEN_DANGER_ALLOW_ANY_URL_HOST", "1");
    cmd
}

/// A funded live workspace without `workspace create`'s backing check: the
/// binary creates and funds it as paper, then the manifest's mode flips in
/// place — the same end state a promotion reaches, without needing a venue.
fn plant_funded_live_workspace(home: &tempfile::TempDir, name: &str) {
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
    let manifest_path = config_dir_in(home)
        .join("workspaces")
        .join(name)
        .join("workspace.json");
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest["mode"] = serde_json::json!("live");
    std::fs::write(&manifest_path, manifest.to_string()).unwrap();
}

/// The refusal needs no venue — that is the point: when the real balance
/// cannot be read the workspace stops spending, the same direction `workspace
/// create` fails, rather than trading blind against an unverifiable claim.
#[test]
fn an_unverifiable_backing_refuses_spending_without_a_venue() {
    let home = tempfile::tempdir().unwrap();
    plant_funded_live_workspace(&home, "prod");
    kraken_offline(&home)
        .args([
            "--workspace",
            "prod",
            "order",
            "buy",
            "BTC/USD",
            "0.03",
            "--type",
            "limit",
            "--price",
            "50000",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("\"refusal\":\"unverified\""))
        .stdout(predicate::str::contains("\"workspace\":\"prod\""))
        .stdout(predicate::str::contains(
            "cannot verify the account behind its allocation",
        ))
        .stdout(predicate::str::contains("still allowed"));
}

/// Opening a session window while the venue cannot confirm a mirrored open
/// order must fail closed: the reconcile runs inside `session start`, before
/// any marker or socket, and an unconfirmed fold refuses the start — else the
/// next mid-session sync would fold the stale fill inside the new window.
#[test]
fn session_start_refuses_when_open_orders_cannot_be_confirmed() {
    let home = tempfile::tempdir().unwrap();
    plant_funded_live_workspace(&home, "prod");
    let journal = config_dir_in(&home)
        .join("workspaces")
        .join("prod")
        .join("journal.jsonl");
    let open_order = serde_json::json!({
        "v": 1, "ts": "2026-01-01T00:01:00Z", "origin": "cli", "event": "order_submitted",
        "order": {
            "id": "OTXID-000000-0001", "pair": "BTCUSD", "base": "BTC", "quote": "USD",
            "side": "buy", "volume": "0.01", "price": "10000", "order_type": "limit",
            "reserved_asset": "USD", "reserved_amount": "100", "created_at": "2026-01-01T00:01:00Z"
        }
    });
    let mut contents = std::fs::read_to_string(&journal).unwrap();
    contents.push_str(&format!("{open_order}\n"));
    std::fs::write(&journal, contents).unwrap();

    kraken_offline(&home)
        .args([
            "--workspace",
            "prod",
            "session",
            "start",
            "--symbols",
            "BTC/USD",
            "--channels",
            "ticker",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains(
            "could not confirm this workspace's open orders",
        ));
    assert!(
        !config_dir_in(&home)
            .join("workspaces")
            .join("prod")
            .join("sessions")
            .exists(),
        "the refusal must precede any session allocation"
    );
}

/// The exit order passes the gate — its failure is the unreachable venue,
/// proving the refusal above came from the gate and not the network.
#[test]
fn the_recovering_sell_passes_the_gate_and_fails_only_at_the_venue() {
    let home = tempfile::tempdir().unwrap();
    plant_funded_live_workspace(&home, "prod");
    kraken_offline(&home)
        .args([
            "--workspace",
            "prod",
            "order",
            "sell",
            "BTC/USD",
            "0.001",
            "--type",
            "limit",
            "--price",
            "50000",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"error\":\"network\""))
        .stdout(predicate::str::contains("Insufficient").not());
}

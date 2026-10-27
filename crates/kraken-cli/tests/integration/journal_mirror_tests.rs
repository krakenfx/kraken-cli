//! Pins the ordering law end to end: the mirror journal
//! opens (and anchors) BEFORE the venue call, and a failed venue call never
//! fabricates an order. The happy path (txid mirroring) is unit-tested
//! in-process against a mock venue — the binary accepts https only, by design.

use super::common::{config_dir_in, kraken_in};

const TEST_SECRET: &str = "dGVzdF9zZWNyZXRfa2V5X2J5dGVzXzMyX2NoYXJzISE="; // base64, test-only

#[test]
fn master_order_anchors_the_journal_before_the_venue_and_never_fakes_an_order() {
    let home = tempfile::tempdir().unwrap();
    // A dead venue: connection refused on every request.
    let output = kraken_in(&home)
        .env("KRAKEN_API_KEY", "test-api-key")
        .env("KRAKEN_API_SECRET", TEST_SECRET)
        .env("KRAKEN_SPOT_URL", "https://127.0.0.1:9/")
        .env("KRAKEN_DANGER_ALLOW_ANY_URL_HOST", "1")
        .args([
            "order", "buy", "BTC/USD", "0.01", "--type", "limit", "--price", "100", "-o", "json",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success(), "the venue is down: {output:?}");

    // The mirror opened first: the journal exists, anchored incomplete
    // (the balance fetch failed too) — and holds NO order.
    let journal = std::fs::read_to_string(config_dir_in(&home).join("journal.jsonl")).unwrap();
    let events: Vec<serde_json::Value> = journal
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(events[0]["event"], "attached");
    assert_eq!(events[0]["complete"], false);
    assert!(
        !events.iter().any(|e| e["event"] == "order_submitted"),
        "a failed venue call must not fabricate an order: {events:?}"
    );
}

//! Local evidence mirror for live venue activity.
//!
//! The venue remains authoritative; transaction-ID reconciliation repairs
//! missed local observations.

use std::collections::BTreeMap;
use std::collections::HashMap;

use kraken_core::OrderSide;
use kraken_paper::account::{Origin, PaperAccount};
use kraken_paper::{AccountEvent, PaperOrder, PaperOrderType, VenueSnapshot, parse_pair};
use rust_decimal::Decimal;

use crate::cli::AppContext;
use crate::config;
use crate::errors::Result;

/// The real account's journal: `<config_dir>/journal.jsonl` — the same
/// file name a workspace journal carries, at the unscoped root.
pub(crate) fn master_journal_path() -> Result<std::path::PathBuf> {
    Ok(config::config_dir()?.join(kraken_workspace::JOURNAL_FILE))
}

/// Open the mirror, anchoring a fresh journal with the venue's balance
/// snapshot. Opens (and locks) BEFORE the venue call by design: a busy
/// journal refuses before any money moves, and the writer lock serializes
/// concurrent trades. The anchor fetch is best-effort — an offline snapshot
/// is disclosed as incomplete, never guessed and never blocking.
pub(crate) async fn open_attached(ctx: &AppContext) -> Result<PaperAccount> {
    open_attached_at(master_journal_path()?, ctx).await
}

/// [`open_attached`] with a caller-chosen journal — the seam unit tests
/// drive, since the real path is fixed under the user's config dir.
pub(crate) async fn open_attached_at(
    journal: std::path::PathBuf,
    ctx: &AppContext,
) -> Result<PaperAccount> {
    let mut account = PaperAccount::open_at(journal, Origin::from_mcp_mode(ctx.mcp_mode))?;
    if !account.is_initialized() {
        let snapshot = fetch_snapshot(ctx).await;
        let records = account.stamp(vec![AccountEvent::Attached(snapshot)]);
        account.append_if_uninitialized(&records)?;
    }
    Ok(account)
}

async fn fetch_snapshot(ctx: &AppContext) -> VenueSnapshot {
    match venue_balances(ctx).await {
        Ok(balances) => VenueSnapshot {
            balances,
            complete: true,
            anchor: None,
        },
        Err(err) => {
            tracing::warn!(%err, "could not fetch the venue balance snapshot; anchoring incomplete");
            VenueSnapshot {
                balances: BTreeMap::new(),
                complete: false,
                anchor: None,
            }
        }
    }
}

async fn venue_balances(ctx: &AppContext) -> Result<BTreeMap<String, Decimal>> {
    let (client, creds, otp) = ctx.spot_authed()?;
    let data = client
        .private_post("Balance", HashMap::new(), creds, otp, true)
        .await?;
    let mut balances = BTreeMap::new();
    if let Some(map) = data.as_object() {
        for (asset, amount) in map {
            if let Some(amount) = amount.as_str().and_then(|a| a.parse::<Decimal>().ok()) {
                balances.insert(asset.clone(), amount);
            }
        }
    }
    Ok(balances)
}

/// Record a venue-accepted order. Best-effort by design: the venue holds
/// the truth, and a local disk fault must never unreport a real order —
/// reconciliation heals the gap by txid.
pub(crate) fn mirror_submitted(
    account: &mut PaperAccount,
    txid: &str,
    side: OrderSide,
    pair: &str,
    order_type: PaperOrderType,
    volume: Decimal,
    price: Option<Decimal>,
) {
    let order = match observed_order(txid, side, pair, order_type, volume, price) {
        Ok(order) => order,
        Err(err) => {
            tracing::warn!(txid, %err, "venue order accepted but not mirrorable");
            return;
        }
    };
    let records = account.stamp(vec![AccountEvent::OrderSubmitted { order }]);
    account.audit(&records);
}

/// Record a venue-accepted cancel for an order the mirror knows. An unknown
/// txid (an order predating the mirror) is skipped — reconciliation owns it.
pub(crate) fn mirror_cancelled(account: &mut PaperAccount, txid: &str) {
    let Some(order) = account
        .state()
        .ok()
        .and_then(|state| state.open_orders.iter().find(|o| o.id == txid).cloned())
    else {
        tracing::warn!(
            txid,
            "cancel accepted for an order the mirror never saw; left to reconciliation"
        );
        return;
    };
    let records = account.stamp(vec![AccountEvent::OrderCancelled { order }]);
    account.audit(&records);
}

/// A venue order in the journal's order shape, honestly partial: the venue
/// owns reservations (amount zero) and a market order's price is unknown
/// until reconciliation observes the fill.
fn observed_order(
    txid: &str,
    side: OrderSide,
    pair: &str,
    order_type: PaperOrderType,
    volume: Decimal,
    price: Option<Decimal>,
) -> Result<PaperOrder> {
    let (api_pair, base, quote) = parse_pair(pair)?;
    Ok(PaperOrder {
        id: txid.to_string(),
        pair: api_pair,
        base,
        quote: quote.clone(),
        side,
        volume,
        price: price.unwrap_or(Decimal::ZERO),
        order_type,
        reserved_asset: quote,
        reserved_amount: Decimal::ZERO,
        created_at: chrono::Utc::now(),
    })
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use rust_decimal_macros::dec;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::config::{CredentialSource, Credentials};

    fn ctx(api_url: Option<String>, with_creds: bool) -> AppContext {
        AppContext {
            format: crate::output::OutputFormat::Json,
            api_url,
            futures_url: None,
            ws_public_url: None,
            ws_auth_url: None,
            ws_l3_url: None,
            ws_futures_url: None,
            ws_reconnect_base_ms: None,
            spot_credentials: with_creds.then(|| Credentials {
                api_key: secrecy::SecretString::from("test-api-key"),
                api_secret: secrecy::SecretString::from(
                    base64::engine::general_purpose::STANDARD
                        .encode(b"test_secret_key_bytes_32_chars!!"),
                ),
                source: CredentialSource::Flag,
            }),
            futures_credentials: None,
            api_secret: None,
            otp: None,
            workspace: None,
            force: true,
            accept_invalid_certs: false,
            allow_any_url_host: true,
            mcp_mode: false,
            monitor: None,
            spot_client: std::sync::OnceLock::new(),
            futures_client: std::sync::OnceLock::new(),
        }
    }

    async fn balance_venue() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/0/private/Balance"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "error": [],
                "result": { "ZUSD": "1000.00", "XXBT": "0.5" }
            })))
            .mount(&server)
            .await;
        server
    }

    fn journal_events(journal: &std::path::Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(journal)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn attach_anchors_a_fresh_journal_with_the_venue_snapshot() {
        let server = balance_venue().await;
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().join("journal.jsonl");
        let ctx = ctx(Some(server.uri()), true);
        drop(open_attached_at(journal.clone(), &ctx).await.unwrap());
        let events = journal_events(&journal);
        assert_eq!(events[0]["event"], "attached");
        assert_eq!(events[0]["complete"], true);
        assert_eq!(events[0]["balances"]["ZUSD"], "1000.00");
    }

    #[tokio::test]
    async fn attach_without_a_reachable_venue_discloses_an_incomplete_anchor() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().join("journal.jsonl");
        // No credentials: the anchor fetch fails, the attach must not.
        let ctx = ctx(None, false);
        drop(open_attached_at(journal.clone(), &ctx).await.unwrap());
        let events = journal_events(&journal);
        assert_eq!(events[0]["event"], "attached");
        assert_eq!(events[0]["complete"], false);
    }

    #[tokio::test]
    async fn reopening_the_mirror_never_reanchors() {
        let server = balance_venue().await;
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().join("journal.jsonl");
        let ctx = ctx(Some(server.uri()), true);
        drop(open_attached_at(journal.clone(), &ctx).await.unwrap());
        drop(open_attached_at(journal.clone(), &ctx).await.unwrap());
        let attaches = journal_events(&journal)
            .iter()
            .filter(|e| e["event"] == "attached")
            .count();
        assert_eq!(attaches, 1, "one anchor per journal lifetime");
    }

    #[tokio::test]
    async fn mirror_cancelled_closes_a_known_order_and_skips_an_unknown_txid() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().join("journal.jsonl");
        let ctx = ctx(None, false);
        let mut account = open_attached_at(journal.clone(), &ctx).await.unwrap();
        mirror_submitted(
            &mut account,
            "OTXID-KNOWN-000001",
            OrderSide::Buy,
            "BTC/USD",
            PaperOrderType::Limit,
            dec!(0.01),
            Some(dec!(100)),
        );
        mirror_cancelled(&mut account, "OTXID-NEVER-SEEN00");
        mirror_cancelled(&mut account, "OTXID-KNOWN-000001");
        let events = journal_events(&journal);
        let cancelled: Vec<_> = events
            .iter()
            .filter(|e| e["event"] == "order_cancelled")
            .collect();
        assert_eq!(cancelled.len(), 1, "unknown txid left to reconciliation");
        assert_eq!(cancelled[0]["order"]["id"], "OTXID-KNOWN-000001");
    }
}

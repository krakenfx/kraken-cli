//! Snapshot pins for every serialized workspace contract. These wire shapes
//! are what agents and scripts parse; a diff here is a deliberate contract
//! change, reviewed via `cargo insta review`, never an accident.

#![allow(clippy::unwrap_used)]

use chrono::{TimeZone, Utc};
use insta::assert_json_snapshot;
use kraken_workspace::{
    AssetBalance, ListedWorkspace, WORKSPACE_VERSION, WorkspaceBalances, WorkspaceManifest,
    WorkspaceMode, WorkspaceOverview,
    receipt::{OrderReceipt, PaperFill, ReceiptCore, ReceiptStatus, TradeMode},
};
use rust_decimal_macros::dec;

fn manifest() -> WorkspaceManifest {
    WorkspaceManifest {
        workspace_version: WORKSPACE_VERSION.to_string(),
        cli_version: "0.0.0-test".to_string(),
        name: "btc-momentum".to_string(),
        capital: dec!(10000),
        currency: "USD".to_string(),
        mode: WorkspaceMode::Paper,
        fee_rate: dec!(0.0026),
        slippage_rate: dec!(0),
        allowed_pairs: Some(vec!["BTC/USD".to_string(), "ETH/USD".to_string()]),
        created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
    }
}

#[test]
fn workspace_manifest_wire_shape_is_pinned() {
    assert_json_snapshot!(manifest());
}

/// The 1.0 key set, as a literal: an accidental rename or reorder of the
/// serialized fields must fail loud, independent of the snapshot file.
#[test]
fn manifest_1_0_keys_are_pinned() {
    let value = serde_json::to_value(manifest()).unwrap();
    let keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    // Struct-declaration order since serde_json preserve_order;
    // previously the alphabetical BTreeMap accident. Existing manifests parse
    // unchanged — JSON key order is not meaningful to the reader.
    assert_eq!(
        keys,
        [
            "workspace_version",
            "cli_version",
            "name",
            "capital",
            "currency",
            "mode",
            "fee_rate",
            "slippage_rate",
            "allowed_pairs",
            "created_at",
        ]
    );
}

#[test]
fn listed_workspaces_include_the_synthesized_default_row() {
    let rows = vec![
        ListedWorkspace {
            name: "default".to_string(),
            mode: WorkspaceMode::Live,
            capital: None,
            currency: None,
            created_at: None,
            damaged: None,
        },
        ListedWorkspace {
            name: "btc-momentum".to_string(),
            mode: WorkspaceMode::Paper,
            capital: Some(dec!(10000)),
            currency: Some("USD".to_string()),
            created_at: Some(Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()),
            damaged: None,
        },
    ];
    assert_json_snapshot!(rows);
}

#[test]
fn workspace_overview_wire_shape_is_pinned() {
    let overview: WorkspaceOverview = manifest().into();
    assert_json_snapshot!(overview);
}

#[test]
fn balances_wire_shape_is_pinned() {
    let balances = WorkspaceBalances {
        workspace: "btc-momentum".to_string(),
        mode: WorkspaceMode::Paper,
        balances: [
            (
                "USD".to_string(),
                AssetBalance {
                    total: dec!(9500.50),
                    reserved: dec!(120.25),
                    available: dec!(9380.25),
                },
            ),
            (
                "BTC".to_string(),
                AssetBalance {
                    total: dec!(0.01),
                    reserved: dec!(0),
                    available: dec!(0.01),
                },
            ),
        ]
        .into_iter()
        .collect(),
    };
    assert_json_snapshot!(balances);
}

fn receipt_core(mode: TradeMode, status: ReceiptStatus) -> ReceiptCore {
    ReceiptCore {
        mode,
        status,
        order_id: Some("OTEST1-RCPT0-000001".to_string()),
        pair: Some("BTC/USD".to_string()),
        side: Some(kraken_core::OrderSide::Buy),
        order_type: Some("limit".to_string()),
        volume: Some(dec!(0.01)),
        price: Some(dec!(50000)),
    }
}

#[test]
fn paper_order_receipt_wire_shape_is_pinned() {
    assert_json_snapshot!(OrderReceipt {
        core: receipt_core(TradeMode::Paper, ReceiptStatus::Filled),
        venue: None,
        paper: Some(PaperFill {
            trade_id: Some("PAPER-00002".to_string()),
            fee: Some(dec!(1.3)),
            cost: Some(dec!(500)),
        }),
    });
}

#[test]
fn live_order_receipt_wire_shape_is_pinned() {
    assert_json_snapshot!(OrderReceipt {
        core: receipt_core(TradeMode::Live, ReceiptStatus::Submitted),
        venue: Some(serde_json::json!({
            "descr": { "order": "buy 0.01000000 XBTUSD @ limit 50000.0" },
            "txid": ["OTEST1-RCPT0-000001"]
        })),
        paper: None,
    });
}

mod proptests {
    use super::*;
    use proptest::prelude::*;
    use rust_decimal::Decimal;

    proptest! {
        /// Digit-exactness is the law for money: any capital survives the
        /// serde round trip byte-for-byte.
        #[test]
        fn manifest_round_trips_through_serde(mantissa in 1i64..=i64::MAX, scale in 0u32..=9) {
            let mut subject = manifest();
            subject.capital = Decimal::new(mantissa, scale);
            let json = serde_json::to_string(&subject).unwrap();
            let back: WorkspaceManifest = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(back, subject);
        }
    }
}

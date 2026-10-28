//! The `balances` channel: asset balances and account-ledger transactions.
//!
//! Source: <https://docs.kraken.com/api/docs/websocket-v2/balances>. A snapshot entry
//! carries the per-wallet breakdown (`wallets`); an update entry carries the
//! ledger transaction (`amount`, `fee`, `ledger_id`, …). Both share `asset`,
//! `asset_class` and the resulting total `balance`; each shape's exclusive
//! fields are `Option`s.
//!
//! The ledger vocabulary fields decode into open enums: documented tokens are
//! typed variants, and anything newer lands in that enum's `Unknown(String)` —
//! the token preserved and re-serialized verbatim (a `#[serde(untagged)]`
//! fallback variant) — while tripping the unknown-vocabulary warn. Drift on the
//! account's own money records is observed, never a hard error and never a
//! narrowed record.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use strum::Display;

/// One balance snapshot entry or ledger transaction (see the module schema).
///
/// No `deny_unknown_fields`: an extensible inbound payload — a field Kraken
/// adds later is ignored rather than failing the decode, which would drop every
/// frame on the channel.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BalanceData {
    pub asset: String,
    /// The docs pin only the placeholder `"currency"` and keep the set open, so
    /// this stays a plain string — unlike the closed ledger vocabularies
    /// below, typed with a verbatim `Unknown` fallback. Identifiers
    /// (`wallet_id`, `user`) stay strings.
    pub asset_class: String,
    /// The total held across all wallets after this event.
    pub balance: Decimal,
    /// Snapshot only: the per-wallet breakdown.
    pub wallets: Option<Vec<WalletBalance>>,
    /// Update only: the signed change of this transaction.
    pub amount: Option<Decimal>,
    pub fee: Option<Decimal>,
    pub ledger_id: Option<String>,
    pub ref_id: Option<String>,
    pub timestamp: Option<String>,
    /// The broad ledger event type (`deposit`, `trade`, `transfer`, …).
    #[serde(rename = "type")]
    pub event_type: Option<LedgerEntryType>,
    pub subtype: Option<LedgerSubtype>,
    pub category: Option<LedgerCategory>,
    pub wallet_type: Option<WalletType>,
    pub wallet_id: Option<String>,
    pub user: Option<String>,
    /// Wire fields outside the pinned schema, preserved verbatim (see
    /// [`ExtraFields`](super::ExtraFields)).
    #[serde(flatten)]
    pub extra: super::ExtraFields,
}

/// One wallet's share of an asset in a snapshot entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WalletBalance {
    #[serde(rename = "type")]
    pub wallet_type: WalletType,
    pub id: String,
    pub balance: Decimal,
    /// Wire fields outside the pinned schema, preserved verbatim (see
    /// [`ExtraFields`](super::ExtraFields)).
    #[serde(flatten)]
    pub extra: super::ExtraFields,
}

impl BalanceData {
    /// Whether any ledger vocabulary field decoded through its `Unknown`
    /// fallback.
    pub(crate) fn has_unknown_vocabulary(&self) -> bool {
        matches!(self.event_type, Some(LedgerEntryType::Unknown(_)))
            || matches!(self.subtype, Some(LedgerSubtype::Unknown(_)))
            || matches!(self.category, Some(LedgerCategory::Unknown(_)))
            || matches!(self.wallet_type, Some(WalletType::Unknown(_)))
            || self.wallets.as_ref().is_some_and(|wallets| {
                wallets
                    .iter()
                    .any(|w| matches!(w.wallet_type, WalletType::Unknown(_)))
            })
    }
}

/// The broad ledger event a balance update records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[non_exhaustive]
pub enum LedgerEntryType {
    Deposit,
    Withdrawal,
    Trade,
    Margin,
    Adjustment,
    Rollover,
    Credit,
    Transfer,
    Settled,
    Staking,
    Sale,
    Reserve,
    Conversion,
    Dividend,
    Reward,
    CreatorFee,
    /// Vocabulary Kraken adds later; the token is preserved and re-serialized
    /// verbatim.
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Unknown(String),
}

/// The transfer flavour of a `transfer`-type ledger entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Display)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
#[non_exhaustive]
pub enum LedgerSubtype {
    SpotFromFutures,
    SpotToFutures,
    StakingFromSpot,
    SpotFromStaking,
    StakingToSpot,
    SpotToStaking,
    /// Vocabulary Kraken adds later; the token is preserved and re-serialized
    /// verbatim.
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Unknown(String),
}

/// The fine-grained ledger category of an update entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
#[non_exhaustive]
pub enum LedgerCategory {
    Deposit,
    Withdrawal,
    Trade,
    MarginTrade,
    MarginSettle,
    MarginConversion,
    Conversion,
    Credit,
    /// One word on the wire, unlike its kebab-case siblings.
    #[serde(rename = "marginrollover")]
    #[strum(serialize = "marginrollover")]
    MarginRollover,
    StakingRewards,
    Instant,
    EquityTrade,
    Airdrop,
    EquityDividend,
    RewardBonus,
    Nft,
    BlockTrade,
    /// Vocabulary Kraken adds later; the token is preserved and re-serialized
    /// verbatim.
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Unknown(String),
}

/// Which wallet flavour holds a balance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Display)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
#[non_exhaustive]
pub enum WalletType {
    Spot,
    Earn,
    /// Vocabulary Kraken adds later; the token is preserved and re-serialized
    /// verbatim.
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Unknown(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_ledger_vocabulary_decodes_typed() {
        let entry: BalanceData = serde_json::from_str(
            r#"{"asset":"BTC","asset_class":"currency","balance":1.0,"type":"creator_fee",
                "category":"margin-trade","subtype":"spottofutures","wallet_type":"earn"}"#,
        )
        .expect("documented tokens decode");
        assert_eq!(entry.event_type, Some(LedgerEntryType::CreatorFee));
        assert_eq!(entry.category, Some(LedgerCategory::MarginTrade));
        assert_eq!(entry.subtype, Some(LedgerSubtype::SpotToFutures));
        assert_eq!(entry.wallet_type, Some(WalletType::Earn));
        assert!(!entry.has_unknown_vocabulary());
    }

    #[test]
    fn an_undocumented_ledger_token_survives_verbatim_and_is_flagged() {
        // The account's own money record: an unrecognized token must neither fail the
        // frame (the pre-typing risk) nor be replaced by a placeholder on output — it
        // decodes as Unknown, re-serializes byte-equal, and trips the drift warn.
        let entry: BalanceData = serde_json::from_str(
            r#"{"asset":"BTC","asset_class":"currency","balance":1.0,
                "type":"yield_farming","category":"marginrollover"}"#,
        )
        .expect("an undocumented token still decodes");
        assert_eq!(
            entry.event_type,
            Some(LedgerEntryType::Unknown("yield_farming".into()))
        );
        assert_eq!(entry.category, Some(LedgerCategory::MarginRollover));
        assert!(entry.has_unknown_vocabulary());
        let out = serde_json::to_value(&entry).expect("serializes");
        assert_eq!(out["type"], "yield_farming", "the original token survives");
        assert_eq!(
            out["category"], "marginrollover",
            "the one-word rename holds"
        );
    }
}

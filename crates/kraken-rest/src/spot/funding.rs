//! Deposits, withdrawals and wallet transfers. Replies publish as sent:
//! Kraken's hyphenated keys and legacy asset codes included.
//!
//! Every request that moves money is never resent after a timeout: a resend
//! would withdraw or transfer twice.

use crate::{
    Delivery, KeyedRows,
    primitives::ListOrObject,
    spot::{AssetClass, Endpoint, Private, RebaseMultiplier, sealed::Sealed},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize};
use serde_with::skip_serializing_none;

/// `POST /0/private/DepositMethods`.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct DepositMethodsQuery {
    pub asset: String,
    pub aclass: Option<AssetClass>,
    pub rebase_multiplier: Option<RebaseMultiplier>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepositMethod {
    pub method: String,
    /// The most that can be deposited, or `false` for no limit.
    pub limit: DepositLimit,
    #[serde(default, with = "crate::decimal::string::option")]
    pub fee: Option<Decimal>,
    #[serde(
        rename = "fee-percentage",
        default,
        with = "crate::decimal::string::option"
    )]
    pub fee_percentage: Option<Decimal>,
    #[serde(
        rename = "address-setup-fee",
        default,
        with = "crate::decimal::string::option"
    )]
    pub address_setup_fee: Option<Decimal>,
    #[serde(rename = "gen-address")]
    pub gen_address: Option<bool>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub minimum: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DepositLimit {
    /// Always `false`: Kraken's way of saying "unlimited".
    Unlimited(bool),
    Amount(#[serde(with = "crate::decimal::string")] Decimal),
}

/// `POST /0/private/DepositAddresses`. Asking for a `new` address takes one
/// from the account's pool, so that request is never resent.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct DepositAddressesQuery {
    pub asset: String,
    pub method: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub new: bool,
    pub aclass: Option<AssetClass>,
    /// Required by some methods (Lightning) to size the invoice.
    pub amount: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepositAddress {
    pub address: String,
    pub expiretm: String,
    pub new: Option<bool>,
    pub tag: Option<String>,
    pub memo: Option<String>,
}

/// `DepositStatus`/`WithdrawStatus` share one filter; only the ledger differs.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct TransferFilter {
    pub asset: Option<String>,
    pub aclass: Option<AssetClass>,
    pub method: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    /// A previous page's `next_cursor`.
    pub cursor: Option<String>,
    pub limit: Option<u32>,
    pub rebase_multiplier: Option<RebaseMultiplier>,
}

/// `POST /0/private/DepositStatus`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DepositStatusQuery {
    #[serde(flatten)]
    pub filter: TransferFilter,
}

/// `POST /0/private/WithdrawStatus`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct WithdrawStatusQuery {
    #[serde(flatten)]
    pub filter: TransferFilter,
}

/// Recent deposits or withdrawals: a list, or a page of one when a cursor was
/// asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Transfers<P> {
    List(Vec<Transfer>),
    Page(P),
}

pub type Deposits = Transfers<DepositPage>;
pub type Withdrawals = Transfers<WithdrawalPage>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepositPage {
    // The public docs name it `deposit`, the API description `deposits`.
    #[serde(alias = "deposits")]
    pub deposit: Vec<Transfer>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawalPage {
    pub withdrawals: Vec<Transfer>,
    pub next_cursor: Option<String>,
}

impl<'de, P: Deserialize<'de>> Deserialize<'de> for Transfers<P> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match ListOrObject::deserialize(d)? {
            ListOrObject::List(rows) => Self::List(rows),
            ListOrObject::Object(page) => Self::Page(page),
        })
    }
}

/// One deposit or withdrawal.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfer {
    pub method: String,
    pub network: Option<String>,
    pub aclass: AssetClass,
    pub asset: String,
    pub refid: String,
    pub txid: String,
    /// The address or account the funds moved to or from.
    pub info: String,
    #[serde(with = "crate::decimal::string")]
    pub amount: Decimal,
    #[serde(default, with = "crate::decimal::string::option")]
    pub fee: Option<Decimal>,
    pub time: i64,
    pub status: TransferStatus,
    #[serde(rename = "status-prop")]
    pub status_prop: Option<TransferFlag>,
    pub originators: Option<Vec<String>>,
    pub key: Option<String>,
}

/// Where a transfer is in its lifecycle. Published as Kraken capitalises it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferStatus {
    Initial,
    Pending,
    EarlyConfirmed,
    Settled,
    Success,
    Failure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransferFlag {
    Return,
    Onhold,
    CancelPending,
    Canceled,
    CancelDenied,
}

/// `POST /0/private/WithdrawMethods`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct WithdrawMethodsQuery {
    pub asset: Option<String>,
    pub aclass: Option<AssetClass>,
    pub network: Option<String>,
    pub rebase_multiplier: Option<RebaseMultiplier>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawMethod {
    pub asset: String,
    pub method: String,
    pub method_id: Option<String>,
    pub network: Option<String>,
    pub network_id: Option<String>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub minimum: Option<Decimal>,
    pub fee: Option<WithdrawFee>,
    pub limits: Option<Vec<WithdrawLimit>>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawFee {
    pub aclass: AssetClass,
    pub asset: String,
    #[serde(default, with = "crate::decimal::string::option")]
    pub fee: Option<Decimal>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub fee_percentage: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawLimit {
    pub description: String,
    pub limit_type: String,
    /// Usage per window (`24h`, `30d`, …).
    pub limits: KeyedRows<String, LimitUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitUsage {
    #[serde(with = "crate::decimal::string")]
    pub maximum: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub remaining: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub used: Decimal,
}

/// `POST /0/private/WithdrawAddresses`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct WithdrawAddressesQuery {
    pub asset: Option<String>,
    pub aclass: Option<AssetClass>,
    pub method: Option<String>,
    pub key: Option<String>,
    pub verified: Option<bool>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawAddress {
    pub address: String,
    pub asset: String,
    pub method: String,
    pub key: String,
    pub tag: Option<String>,
    pub memo: Option<String>,
    pub verified: bool,
}

/// `POST /0/private/WithdrawInfo`: the fee and limit a withdrawal would incur.
#[derive(Debug, Clone, Serialize)]
pub struct WithdrawInfoQuery {
    pub asset: String,
    /// The withdrawal key: an address the account has saved, by name.
    pub key: String,
    #[serde(with = "crate::decimal::string")]
    pub amount: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawQuote {
    pub method: String,
    #[serde(with = "crate::decimal::string")]
    pub limit: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub amount: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub fee: Decimal,
}

/// `POST /0/private/Withdraw`.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct Withdraw {
    pub asset: String,
    pub key: String,
    #[serde(with = "crate::decimal::string")]
    pub amount: Decimal,
    pub aclass: Option<AssetClass>,
    /// Must match the address saved under `key`, as a guard against a
    /// swapped key.
    pub address: Option<String>,
    /// Refuse the withdrawal if the fee would exceed this.
    #[serde(default, with = "crate::decimal::string::option")]
    pub max_fee: Option<Decimal>,
    pub rebase_multiplier: Option<RebaseMultiplier>,
}

/// The reference a withdrawal or transfer is tracked by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferRef {
    pub refid: String,
}

/// `POST /0/private/WithdrawCancel`. The reply is whether the cancel took.
#[derive(Debug, Clone, Serialize)]
pub struct WithdrawCancel {
    pub asset: String,
    pub refid: String,
}

/// `POST /0/private/WalletTransfer`: Spot wallet to Futures wallet.
#[derive(Debug, Clone, Serialize)]
pub struct WalletTransfer {
    pub asset: String,
    pub from: String,
    pub to: String,
    #[serde(with = "crate::decimal::string")]
    pub amount: Decimal,
}

impl Sealed for DepositMethodsQuery {}
impl Endpoint for DepositMethodsQuery {
    const PATH: &'static str = "DepositMethods";
    type Access = Private;
    type Response = Vec<DepositMethod>;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

impl Sealed for DepositAddressesQuery {}
impl Endpoint for DepositAddressesQuery {
    const PATH: &'static str = "DepositAddresses";
    type Access = Private;
    type Response = Vec<DepositAddress>;

    fn delivery(&self) -> Delivery {
        Delivery::once_if(self.new)
    }
}

impl Sealed for DepositStatusQuery {}
impl Endpoint for DepositStatusQuery {
    const PATH: &'static str = "DepositStatus";
    type Access = Private;
    type Response = Deposits;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

impl Sealed for WithdrawStatusQuery {}
impl Endpoint for WithdrawStatusQuery {
    const PATH: &'static str = "WithdrawStatus";
    type Access = Private;
    type Response = Withdrawals;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

impl Sealed for WithdrawMethodsQuery {}
impl Endpoint for WithdrawMethodsQuery {
    const PATH: &'static str = "WithdrawMethods";
    type Access = Private;
    type Response = Vec<WithdrawMethod>;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

impl Sealed for WithdrawAddressesQuery {}
impl Endpoint for WithdrawAddressesQuery {
    const PATH: &'static str = "WithdrawAddresses";
    type Access = Private;
    type Response = Vec<WithdrawAddress>;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

impl Sealed for WithdrawInfoQuery {}
impl Endpoint for WithdrawInfoQuery {
    const PATH: &'static str = "WithdrawInfo";
    type Access = Private;
    type Response = WithdrawQuote;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

impl Sealed for Withdraw {}
impl Endpoint for Withdraw {
    const PATH: &'static str = "Withdraw";
    type Access = Private;
    type Response = TransferRef;
}

impl Sealed for WithdrawCancel {}
impl Endpoint for WithdrawCancel {
    const PATH: &'static str = "WithdrawCancel";
    type Access = Private;
    type Response = bool;
}

impl Sealed for WalletTransfer {}
impl Endpoint for WalletTransfer {
    const PATH: &'static str = "WalletTransfer";
    type Access = Private;
    type Response = TransferRef;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spot::envelope::decode_fixture;

    #[test]
    fn deposit_methods_decode_an_unlimited_limit() {
        let methods: Vec<DepositMethod> = decode_fixture("DepositMethods.json").unwrap();
        assert_eq!(methods[0].limit, DepositLimit::Unlimited(false));
        assert_eq!(
            serde_json::to_value(&methods[0]).unwrap()["gen-address"],
            true
        );
    }

    #[test]
    fn decodes_the_funding_replies() {
        let _: Vec<DepositAddress> = decode_fixture("DepositAddresses.json").unwrap();
        let _: Deposits = decode_fixture("DepositStatus.json").unwrap();
        let _: Vec<WithdrawMethod> = decode_fixture("WithdrawMethods.json").unwrap();
        let _: Vec<WithdrawAddress> = decode_fixture("WithdrawAddresses.json").unwrap();
        let _: WithdrawQuote = decode_fixture("WithdrawInfo.json").unwrap();
        let _: TransferRef = decode_fixture("Withdraw.json").unwrap();
        let _: Withdrawals = decode_fixture("WithdrawStatus.json").unwrap();
        assert!(decode_fixture::<bool>("WithdrawCancel.json").unwrap());
        let _: TransferRef = decode_fixture("WalletTransfer.json").unwrap();
    }

    /// `--cursor` turns either status reply into a page object.
    #[test]
    fn a_cursor_page_decodes_in_both_directions() {
        let rows: serde_json::Value = decode_fixture("WithdrawStatus.json").unwrap();
        let rows = &rows;
        let page = |key: &str| serde_json::json!({ key: rows, "next_cursor": "abc" });

        let withdrawals: Withdrawals = serde_json::from_value(page("withdrawals")).unwrap();
        assert!(
            matches!(withdrawals, Transfers::Page(WithdrawalPage { next_cursor: Some(c), .. }) if c == "abc")
        );
        for key in ["deposit", "deposits"] {
            let deposits: Deposits = serde_json::from_value(page(key)).unwrap();
            assert!(matches!(deposits, Transfers::Page(_)), "{key}");
        }
    }

    #[test]
    fn a_drifted_row_names_its_field_in_either_shape() {
        let row = r#"{"method":"Bitcoin","aclass":"currency","asset":"XXBT","refid":"r","txid":"t","info":"i","amount":"x","time":1,"status":"Success"}"#;
        for (json, at) in [
            (format!("[{row}]"), "[0].amount"),
            (
                format!(r#"{{"withdrawals":[{row}],"next_cursor":null}}"#),
                "withdrawals[0].amount",
            ),
        ] {
            let err = serde_path_to_error::deserialize::<_, Withdrawals>(
                &mut serde_json::Deserializer::from_str(&json),
            )
            .unwrap_err();
            assert_eq!(err.path().to_string(), at);
        }
    }

    #[test]
    fn a_new_deposit_address_is_never_resent() {
        let query = |new| DepositAddressesQuery {
            asset: "BTC".into(),
            method: "Bitcoin".into(),
            new,
            aclass: None,
            amount: None,
        };
        assert_eq!(query(true).delivery(), Delivery::AtMostOnce);
        assert_eq!(query(false).delivery(), Delivery::Retryable);
    }
}

//! Moving funds between the Futures wallets and to Spot. Never resent after a
//! lost reply: a resend would move the funds twice.

use super::{Endpoint, Method, Private, Reply, account::Acknowledged, sealed::Sealed};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// `POST /withdrawal`: Futures wallet to the Spot wallet.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WithdrawToSpot {
    pub currency: String,
    #[serde(with = "crate::decimal::string")]
    pub amount: Decimal,
    pub source_wallet: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Withdrawn {
    pub uid: String,
}

/// `POST /transfer`: between two Futures wallets (`main`, `flex`, a margin
/// account).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletTransfer {
    pub from_account: String,
    pub to_account: String,
    pub unit: String,
    #[serde(with = "crate::decimal::string")]
    pub amount: Decimal,
}

impl Sealed for WithdrawToSpot {}
impl Endpoint for WithdrawToSpot {
    const PATH: &'static str = "withdrawal";
    const METHOD: Method = Method::Post;
    type Access = Private;
    type Response = Reply<Withdrawn>;
}

impl Sealed for WalletTransfer {}
impl Endpoint for WalletTransfer {
    const PATH: &'static str = "transfer";
    const METHOD: Method = Method::Post;
    type Access = Private;
    type Response = Reply<Acknowledged>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::futures::envelope::decode_fixture;

    #[test]
    fn decodes_the_transfer_replies() {
        let withdrawn: Reply<Withdrawn> = decode_fixture("trading.post_withdrawal.json").unwrap();
        assert!(!withdrawn.body.uid.is_empty());
        let _: Reply<Acknowledged> =
            decode_fixture("trading.post_transfer.synthetic.json").unwrap();
    }
}

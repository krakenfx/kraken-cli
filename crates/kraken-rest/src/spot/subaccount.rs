//! Subaccounts: create one, move funds between accounts.

use crate::spot::{AssetClass, Endpoint, Private, sealed::Sealed};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// `POST /0/private/CreateSubaccount`. Never resent: a resend would race the
/// first for the username.
#[derive(Debug, Clone, Serialize)]
pub struct CreateSubaccount {
    pub username: String,
    pub email: String,
}

/// `POST /0/private/AccountTransfer`. Never resent after a timeout.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct AccountTransfer {
    pub asset: String,
    pub asset_class: Option<AssetClass>,
    #[serde(with = "crate::decimal::string")]
    pub amount: Decimal,
    /// Source account IIBAN or name.
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountTransferred {
    pub transfer_id: String,
    pub status: String,
}

impl Sealed for CreateSubaccount {}
impl Endpoint for CreateSubaccount {
    const PATH: &'static str = "CreateSubaccount";
    type Access = Private;
    type Response = bool;
}

impl Sealed for AccountTransfer {}
impl Endpoint for AccountTransfer {
    const PATH: &'static str = "AccountTransfer";
    type Access = Private;
    type Response = AccountTransferred;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spot::envelope::decode_fixture;

    #[test]
    fn decodes_the_replies() {
        assert!(decode_fixture::<bool>("CreateSubaccount.json").unwrap());
        let moved: AccountTransferred = decode_fixture("AccountTransfer.json").unwrap();
        assert_eq!(moved.status, "complete");
    }
}

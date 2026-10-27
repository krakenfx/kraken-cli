use serde::ser::{Serialize, SerializeMap, Serializer};
use std::collections::BTreeSet;
use strum::IntoEnumIterator;

/// Lets a grant create and delete its key.
pub(crate) const KEY_WRITE_SCOPE: &str = "account.fast-api-key:write";
/// Lets one account have a key on each machine. Without it, Kraken refuses a
/// second key.
pub(crate) const KEY_MULTIPLE_SCOPE: &str = "account.fast-api-key:multiple";

/// Withdrawals and withdrawal-address changes have no variant. Thus no caller
/// can create a key that moves funds off the account.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, strum::Display, strum::EnumIter,
)]
#[strum(serialize_all = "kebab-case")]
pub enum Permission {
    FundsQuery,
    /// Deposit methods, addresses and status. It cannot move funds off the
    /// account.
    FundsAdd,
    FundsEarn,
    TradesQueryOpen,
    TradesQueryClosed,
    TradesModify,
    TradesClose,
    LedgerQuery,
    ExportData,
    /// Gets the token that all private WebSocket channels use.
    #[strum(serialize = "websockets-token")]
    WebSocketsToken,
}

impl Permission {
    pub(crate) const fn scope(self) -> &'static str {
        match self {
            Self::FundsQuery => "account.fast-api-key:funds-query",
            Self::FundsAdd => "account.fast-api-key:funds-add",
            Self::FundsEarn => "account.fast-api-key:funds-earn",
            Self::TradesQueryOpen => "account.fast-api-key:trades-query-open",
            Self::TradesQueryClosed => "account.fast-api-key:trades-query-closed",
            Self::TradesModify => "account.fast-api-key:trades-modify",
            Self::TradesClose => "account.fast-api-key:trades-close",
            Self::LedgerQuery => "account.fast-api-key:ledger-query",
            Self::ExportData => "account.fast-api-key:export-data",
            Self::WebSocketsToken => "account.fast-api-key:create-websockets-token",
        }
    }

    /// Kraken rejects a request body with an unknown flag, so each name must
    /// match the Kraken API.
    pub(crate) const fn flag(self) -> &'static str {
        match self {
            Self::FundsQuery => "funds_query",
            Self::FundsAdd => "funds_add",
            Self::FundsEarn => "funds_earn",
            Self::TradesQueryOpen => "trades_query_open",
            Self::TradesQueryClosed => "trades_query_closed",
            Self::TradesModify => "trades_modify",
            Self::TradesClose => "trades_close",
            Self::LedgerQuery => "ledger_query",
            Self::ExportData => "export_data",
            Self::WebSocketsToken => "token_websockets_create",
        }
    }

    /// `None` for a flag that kraken-cli never requests.
    pub(crate) fn from_flag(flag: &str) -> Option<Self> {
        Self::iter().find(|permission| permission.flag() == flag)
    }

    /// The WebSocket token is read-only: trading on a socket also needs
    /// `TradesModify`.
    pub(crate) const fn is_read_only(self) -> bool {
        matches!(
            self,
            Self::FundsQuery
                | Self::TradesQueryOpen
                | Self::TradesQueryClosed
                | Self::LedgerQuery
                | Self::WebSocketsToken
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Permissions(BTreeSet<Permission>);

impl Permissions {
    pub fn all() -> Self {
        Permission::iter().collect()
    }

    pub fn read_only() -> Self {
        Permission::iter().filter(|p| p.is_read_only()).collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = Permission> + '_ {
        self.0.iter().copied()
    }

    pub(crate) fn contains(&self, permission: Permission) -> bool {
        self.0.contains(&permission)
    }

    /// The key-management scopes, then one scope for each permission.
    pub(crate) fn scopes(&self) -> Vec<&'static str> {
        [KEY_WRITE_SCOPE, KEY_MULTIPLE_SCOPE]
            .into_iter()
            .chain(self.iter().map(Permission::scope))
            .collect()
    }
}

impl FromIterator<Permission> for Permissions {
    fn from_iter<I: IntoIterator<Item = Permission>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

/// The key-permission object of the Kraken API: one `true` flag per
/// permission.
impl Serialize for Permissions {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for permission in self.iter() {
            map.serialize_entry(permission.flag(), &true)?;
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// The names, in scope form and flag form, of the rights that move funds
    /// off the account.
    const WITHDRAWAL: [&str; 6] = [
        "funds-withdraw",
        "withdrawal-address",
        "funds_withdraw",
        "withdrawal_address",
        "withdraw",
        "address",
    ];

    fn any_permissions() -> impl Strategy<Value = Permissions> {
        proptest::sample::subsequence(Permission::iter().collect::<Vec<_>>(), 0..=10)
            .prop_map(|subset| subset.into_iter().collect())
    }

    proptest! {
        #[test]
        fn no_permission_set_can_ask_for_a_withdrawal_right(permissions in any_permissions()) {
            let scopes = permissions.scopes().join(" ");
            let flags = serde_json::to_string(&permissions).unwrap();
            for word in WITHDRAWAL {
                prop_assert!(!scopes.contains(word), "{scopes}");
                prop_assert!(!flags.contains(word), "{flags}");
            }
        }

        #[test]
        fn every_request_can_manage_its_own_key(permissions in any_permissions()) {
            let scopes = permissions.scopes();
            prop_assert!(scopes.contains(&KEY_WRITE_SCOPE));
            prop_assert!(scopes.contains(&KEY_MULTIPLE_SCOPE));
        }
    }

    /// The scopes that the security review approved. A new scope needs a new
    /// review before this list grows.
    #[test]
    fn the_widest_sign_in_asks_for_the_approved_scopes_only() {
        assert_eq!(
            Permissions::all().scopes(),
            [
                "account.fast-api-key:write",
                "account.fast-api-key:multiple",
                "account.fast-api-key:funds-query",
                "account.fast-api-key:funds-add",
                "account.fast-api-key:funds-earn",
                "account.fast-api-key:trades-query-open",
                "account.fast-api-key:trades-query-closed",
                "account.fast-api-key:trades-modify",
                "account.fast-api-key:trades-close",
                "account.fast-api-key:ledger-query",
                "account.fast-api-key:export-data",
                "account.fast-api-key:create-websockets-token",
            ]
        );
    }

    #[test]
    fn flags_serialize_as_the_api_object() {
        let permissions: Permissions = [Permission::TradesModify, Permission::WebSocketsToken]
            .into_iter()
            .collect();
        assert_eq!(
            serde_json::to_value(permissions).unwrap(),
            serde_json::json!({ "trades_modify": true, "token_websockets_create": true })
        );
    }

    #[test]
    fn every_flag_parses_back_to_its_permission() {
        for permission in Permission::iter() {
            assert_eq!(Permission::from_flag(permission.flag()), Some(permission));
        }
        assert_eq!(Permission::from_flag("funds_withdraw"), None);
    }

    #[test]
    fn the_websocket_permission_reads_as_one_word() {
        assert_eq!(Permission::WebSocketsToken.to_string(), "websockets-token");
    }

    #[test]
    fn read_only_trades_nothing() {
        let read_only = Permissions::read_only();
        assert!(!read_only.contains(Permission::TradesModify));
        assert!(!read_only.contains(Permission::TradesClose));
        assert!(read_only.contains(Permission::WebSocketsToken));
    }
}

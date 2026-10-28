//! What actually stands behind a live workspace's allocation.
//!
//! A live workspace is a sub-ledger over the configured Kraken account, so its
//! allocation is a claim rather than a venue-enforced wall. The honest way to
//! say that is a number, not an adjective: this module reports what the
//! workspace claims, what the account holds, and what other live workspaces
//! have already claimed of it.

use rust_decimal::Decimal;
use serde::Serialize;

/// Whether the account can still stand behind the allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum BackingStatus {
    /// The account holds the allocation, over and above every other claim.
    Backed,
    /// Other live workspaces plus this one claim more than the account holds.
    /// New orders are refused: the ledger would be recording money that is
    /// not there.
    OverAllocated,
    /// The account balance could not be read, so the claim cannot be checked.
    Unverified,
}

/// What other live workspaces claim of one asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claims {
    pub total: Decimal,
    /// False when a neighbour's manifest could not be read, so `total` is a
    /// floor rather than the figure. A claim cannot be checked against a floor.
    pub complete: bool,
}

impl Claims {
    /// A complete tally of `total`.
    pub fn known(total: Decimal) -> Self {
        Self {
            total,
            complete: true,
        }
    }
}

/// The allocation set against the account behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Backing {
    pub status: BackingStatus,
    /// Asset the allocation is denominated in.
    pub asset: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub allocation: Decimal,
    /// `None` when the venue balance was unreachable.
    #[serde(with = "rust_decimal::serde::str_option")]
    pub account: Option<Decimal>,
    /// Claimed by *other* live workspaces in the same asset.
    #[serde(with = "rust_decimal::serde::str")]
    pub allocated_elsewhere: Decimal,
    /// Account balance minus every live claim, including this one.
    #[serde(with = "rust_decimal::serde::str_option")]
    pub unallocated: Option<Decimal>,
}

impl Backing {
    /// Set `allocation` against `account`, given what other live workspaces
    /// already claim of the same asset.
    pub fn evaluate(
        asset: impl Into<String>,
        allocation: Decimal,
        allocated_elsewhere: Claims,
        account: Option<Decimal>,
    ) -> Self {
        // Unverifiable either way round: an unreadable balance leaves the right
        // side of the comparison unknown, an unreadable neighbour manifest the
        // left. Neither may read as a satisfied check, so `unallocated` stays
        // `None` and every status below is read back off it.
        let unallocated = account
            .filter(|_| allocated_elsewhere.complete)
            .map(|held| held - allocated_elsewhere.total - allocation);
        Self {
            status: match unallocated {
                None => BackingStatus::Unverified,
                Some(headroom) if headroom < Decimal::ZERO => BackingStatus::OverAllocated,
                Some(_) => BackingStatus::Backed,
            },
            asset: asset.into(),
            allocation,
            account,
            allocated_elsewhere: allocated_elsewhere.total,
            unallocated,
        }
    }

    /// One line a human can act on, for stderr and table footers.
    pub fn summary(&self) -> String {
        let allocation = self.allocation;
        let asset = &self.asset;
        let Some(held) = self.account else {
            return format!(
                "allocation {allocation} {asset}; account balance unread, so backing is unverified"
            );
        };
        // The balance was read, so the unverifiable side is the neighbour
        // tally — blaming credentials here would send the user away from the
        // workspace that actually needs repair.
        let Some(headroom) = self.unallocated else {
            return format!(
                "allocation {allocation} {asset} of {held} {asset} held; another workspace's \
                 manifest is unreadable, so its claim — and this backing — cannot be verified"
            );
        };
        if headroom < Decimal::ZERO {
            return format!(
                "OVER-ALLOCATED: this workspace claims {allocation} {asset} and other live \
                 workspaces claim {} {asset}, but the account holds only {held} {asset}",
                self.allocated_elsewhere,
            );
        }
        format!(
            "allocation {allocation} {asset} of {held} {asset} held; {headroom} {asset} \
             unallocated"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn other_live_claims_count_against_the_same_balance() {
        let backing =
            Backing::evaluate("BTC", dec!(100), Claims::known(dec!(850)), Some(dec!(1000)));
        assert_eq!(backing.status, BackingStatus::Backed);
        assert_eq!(backing.unallocated, Some(dec!(50)));

        let crowded =
            Backing::evaluate("BTC", dec!(100), Claims::known(dec!(950)), Some(dec!(1000)));
        assert_eq!(crowded.status, BackingStatus::OverAllocated);
        assert_eq!(crowded.unallocated, Some(dec!(-50)));
    }

    /// An unreadable balance must not read as a satisfied check.
    #[test]
    fn an_unreadable_balance_is_unverified_not_backed() {
        let backing = Backing::evaluate("BTC", dec!(100), Claims::known(dec!(0)), None);
        assert_eq!(backing.status, BackingStatus::Unverified);
        assert!(backing.summary().contains("unverified"));
    }

    /// Nor may an unreadable neighbour: its claim is unknown, so the headroom
    /// this workspace would be told it has is a floor, not a figure. The
    /// summary must name the neighbour, not the balance that was read fine.
    #[test]
    fn an_unreadable_neighbour_is_unverified_not_backed() {
        let incomplete = Claims {
            total: dec!(0),
            complete: false,
        };
        let backing = Backing::evaluate("BTC", dec!(100), incomplete, Some(dec!(1000)));
        assert_eq!(backing.status, BackingStatus::Unverified);
        assert_eq!(backing.unallocated, None, "no headroom may be implied");
        let summary = backing.summary();
        assert!(summary.contains("manifest is unreadable"), "{summary}");
        assert!(!summary.contains("balance unread"), "{summary}");
    }
}

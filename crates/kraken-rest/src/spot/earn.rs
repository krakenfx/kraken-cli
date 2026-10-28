//! Earn: strategies, allocations, and moving funds in and out of them.
//! Replies publish as sent.

use crate::{
    Delivery,
    spot::{Endpoint, Private, sealed::Sealed},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// `POST /0/private/Earn/Strategies`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct StrategiesQuery {
    pub asset: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lock_type: Vec<LockKind>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ascending: bool,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Strategies {
    pub next_cursor: Option<String>,
    pub items: Vec<Strategy>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Strategy {
    pub id: String,
    pub asset: String,
    pub lock_type: LockTerms,
    pub apr_estimate: Option<AprEstimate>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub user_min_allocation: Option<Decimal>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub user_cap: Option<Decimal>,
    #[serde(with = "crate::decimal::string")]
    pub allocation_fee: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub deallocation_fee: Decimal,
    pub auto_compound: AutoCompound,
    pub yield_source: YieldSource,
    pub can_allocate: bool,
    pub can_deallocate: bool,
    pub allocation_restriction_info: Vec<RestrictionReason>,
}

/// How funds are locked, with the periods that apply to its kind (seconds).
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockTerms {
    #[serde(rename = "type")]
    pub kind: LockKind,
    pub payout_frequency: Option<u64>,
    pub bonding_period: Option<u64>,
    pub bonding_period_variable: Option<bool>,
    pub bonding_rewards: Option<bool>,
    pub exit_queue_period: Option<u64>,
    pub unbonding_period: Option<u64>,
    pub unbonding_period_variable: Option<bool>,
    pub unbonding_rewards: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockKind {
    Flex,
    Bonded,
    Timed,
    Instant,
}

/// Annual rate range, in percent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AprEstimate {
    #[serde(with = "crate::decimal::string")]
    pub low: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub high: Decimal,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoCompound {
    #[serde(rename = "type")]
    pub kind: AutoCompoundKind,
    /// For `optional`: whether it is on unless turned off.
    pub default: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoCompoundKind {
    Enabled,
    Disabled,
    Optional,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YieldSource {
    #[serde(rename = "type")]
    pub kind: YieldKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum YieldKind {
    Staking,
    OffChain,
}

/// Why the account may not allocate to a strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestrictionReason {
    Tier,
}

/// `POST /0/private/Earn/Allocations`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct AllocationsQuery {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ascending: bool,
    /// The asset `converted` amounts are stated in; USD when absent.
    pub converted_asset: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hide_zero_allocations: bool,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Allocations {
    pub converted_asset: String,
    #[serde(with = "crate::decimal::string")]
    pub total_allocated: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub total_rewarded: Decimal,
    pub next_cursor: Option<String>,
    pub items: Vec<StrategyAllocation>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyAllocation {
    pub strategy_id: String,
    pub native_asset: String,
    pub amount_allocated: AllocatedAmounts,
    pub total_rewarded: Amount,
    pub payout: Option<Payout>,
}

/// What is allocated, split by lifecycle stage.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllocatedAmounts {
    pub bonding: Option<AllocationStage>,
    pub exit_queue: Option<AllocationStage>,
    pub unbonding: Option<AllocationStage>,
    pub pending: Option<Amount>,
    pub total: Amount,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllocationStage {
    #[serde(with = "crate::decimal::string")]
    pub native: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub converted: Decimal,
    pub allocation_count: u32,
    pub allocations: Vec<StageAllocation>,
}

/// One allocation in a stage. Times are RFC 3339, as sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageAllocation {
    pub created_at: String,
    pub expires: String,
    #[serde(with = "crate::decimal::string")]
    pub native: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub converted: Decimal,
}

/// An amount in the strategy's asset and in the converted asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Amount {
    #[serde(with = "crate::decimal::string")]
    pub native: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub converted: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payout {
    pub period_start: String,
    pub period_end: String,
    pub accumulated_reward: Amount,
    pub estimated_reward: Amount,
}

/// `POST /0/private/Earn/Allocate`. Queues the move; poll
/// [`AllocateStatusQuery`]. Never resent after a timeout.
#[derive(Debug, Clone, Serialize)]
pub struct Allocate {
    pub strategy_id: String,
    #[serde(with = "crate::decimal::string")]
    pub amount: Decimal,
}

/// `POST /0/private/Earn/Deallocate`. Never resent after a timeout.
#[derive(Debug, Clone, Serialize)]
pub struct Deallocate {
    pub strategy_id: String,
    #[serde(with = "crate::decimal::string")]
    pub amount: Decimal,
}

/// `POST /0/private/Earn/AllocateStatus`.
#[derive(Debug, Clone, Serialize)]
pub struct AllocateStatusQuery {
    pub strategy_id: String,
}

/// `POST /0/private/Earn/DeallocateStatus`.
#[derive(Debug, Clone, Serialize)]
pub struct DeallocateStatusQuery {
    pub strategy_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveStatus {
    /// The last requested move has not settled yet.
    pub pending: bool,
}

impl Sealed for StrategiesQuery {}
impl Endpoint for StrategiesQuery {
    const PATH: &'static str = "Earn/Strategies";
    type Access = Private;
    type Response = Strategies;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

impl Sealed for AllocationsQuery {}
impl Endpoint for AllocationsQuery {
    const PATH: &'static str = "Earn/Allocations";
    type Access = Private;
    type Response = Allocations;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

impl Sealed for Allocate {}
impl Endpoint for Allocate {
    const PATH: &'static str = "Earn/Allocate";
    type Access = Private;
    type Response = bool;
}

impl Sealed for Deallocate {}
impl Endpoint for Deallocate {
    const PATH: &'static str = "Earn/Deallocate";
    type Access = Private;
    type Response = bool;
}

impl Sealed for AllocateStatusQuery {}
impl Endpoint for AllocateStatusQuery {
    const PATH: &'static str = "Earn/AllocateStatus";
    type Access = Private;
    type Response = MoveStatus;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

impl Sealed for DeallocateStatusQuery {}
impl Endpoint for DeallocateStatusQuery {
    const PATH: &'static str = "Earn/DeallocateStatus";
    type Access = Private;
    type Response = MoveStatus;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spot::envelope::decode_fixture;

    #[test]
    fn decodes_the_strategy_catalogue() {
        let strategies: Strategies = decode_fixture("Earn_Strategies.json").unwrap();
        assert_eq!(strategies.items[0].lock_type.kind, LockKind::Instant);
    }

    #[test]
    fn decodes_the_allocations() {
        let allocations: Allocations = decode_fixture("Earn_Allocations.json").unwrap();
        let bonding = allocations.items[0]
            .amount_allocated
            .bonding
            .as_ref()
            .unwrap();
        assert_eq!(bonding.allocation_count, 2);
    }

    #[test]
    fn decodes_the_moves() {
        assert!(decode_fixture::<bool>("Earn_Allocate.json").unwrap());
        assert!(
            !decode_fixture::<MoveStatus>("Earn_AllocateStatus.json")
                .unwrap()
                .pending
        );
    }

    #[test]
    fn lock_types_are_sent_as_an_array() {
        let query = StrategiesQuery {
            lock_type: vec![LockKind::Flex, LockKind::Bonded],
            ..StrategiesQuery::default()
        };
        assert_eq!(
            serde_json::to_string(&query).unwrap(),
            r#"{"lock_type":["flex","bonded"]}"#
        );
    }
}

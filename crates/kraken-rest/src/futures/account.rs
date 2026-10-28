//! Account data and preferences.

use super::{
    Endpoint, Greeks, MarginLevel, Method, PathSegment, Private, Reply, market::InstrumentType,
    sealed::Sealed,
};
use crate::KeyedRows;
use kraken_core::OrderSide;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::borrow::Cow;

/// `GET /accounts`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AccountsQuery {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Accounts {
    /// `cash`, `flex`, and one margin account per single-collateral contract.
    pub accounts: KeyedRows<String, Account>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Account {
    #[serde(rename = "cashAccount")]
    Cash(CashAccount),
    #[serde(rename = "multiCollateralMarginAccount")]
    Flex(Box<FlexAccount>),
    #[serde(rename = "marginAccount")]
    Margin(Box<MarginAccount>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CashAccount {
    pub balances: KeyedRows<String, DecimalText>,
}

/// A balance Kraken sends as a decimal string, published as sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DecimalText(#[serde(with = "crate::decimal::string")] pub Decimal);

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlexAccount {
    pub currencies: KeyedRows<String, FlexCurrency>,
    #[serde(with = "crate::decimal::json")]
    pub balance_value: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub portfolio_value: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub collateral_value: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub initial_margin: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub initial_margin_with_orders: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub maintenance_margin: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub pnl: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub unrealized_funding: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub total_unrealized: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub total_unrealized_as_margin: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub margin_equity: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub available_margin: Decimal,
    pub portfolio_margin_breakdown: Option<PortfolioMarginBreakdown>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub upnl_interest_rate: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlexCurrency {
    #[serde(with = "crate::decimal::json")]
    pub quantity: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub value: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub collateral: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub available: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortfolioMarginBreakdown {
    #[serde(with = "crate::decimal::json")]
    pub total_cross_asset_netted_market_risk: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub total_market_risk: Decimal,
    pub total_scenario_pnls: Option<Vec<super::Number>>,
    #[serde(with = "crate::decimal::json")]
    pub total_absolute_option_position_delta_notional: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub net_portfolio_delta: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub total_premium: Decimal,
    pub is_buy_only: bool,
    #[serde(with = "crate::decimal::json")]
    pub futures_maintenance_margin: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarginAccount {
    pub auxiliary: Auxiliary,
    pub balances: KeyedRows<String, DecimalText>,
    pub currency: String,
    pub margin_requirements: MarginRequirements,
    pub trigger_estimates: MarginRequirements,
}

/// `usd` value, portfolio value, unrealised P&L, available funds, funding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Auxiliary {
    #[serde(with = "crate::decimal::json")]
    pub usd: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub pv: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub pnl: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub af: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub funding: Decimal,
}

/// Initial, maintenance, liquidation and termination thresholds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarginRequirements {
    #[serde(with = "crate::decimal::json")]
    pub im: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub mm: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub lt: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub tt: Decimal,
}

/// `GET /openorders`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct OpenOrdersQuery {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenOrders {
    pub open_orders: Vec<OpenOrder>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenOrder {
    #[serde(rename = "order_id")]
    pub order_id: String,
    pub cli_ord_id: Option<String>,
    pub symbol: String,
    pub side: OrderSide,
    pub order_type: RestingOrderType,
    #[serde(default, with = "crate::decimal::json::option")]
    pub limit_price: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub stop_price: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub unfilled_size: Option<Decimal>,
    pub received_time: String,
    pub status: RestingOrderStatus,
    #[serde(with = "crate::decimal::json")]
    pub filled_size: Decimal,
    pub reduce_only: bool,
    pub trigger_signal: Option<TriggerSignal>,
    pub last_update_time: String,
    pub algo_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestingOrderType {
    Lmt,
    Stop,
    TakeProfit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RestingOrderStatus {
    Untouched,
    PartiallyFilled,
}

/// The price a stop watches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerSignal {
    Mark,
    Last,
    Spot,
    Index,
}

/// `GET /openpositions`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct OpenPositionsQuery {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenPositions {
    pub open_positions: Vec<OpenPosition>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenPosition {
    pub side: PositionSide,
    pub symbol: String,
    #[serde(with = "crate::decimal::json")]
    pub price: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub size: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub unrealized_pnl: Decimal,
    #[serialize_always]
    #[serde(default, with = "crate::decimal::json::option")]
    pub unrealized_funding: Option<Decimal>,
    pub pnl_currency: Option<String>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub max_fixed_leverage: Option<Decimal>,
    pub greeks: Option<Greeks>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionSide {
    Long,
    Short,
}

/// `GET /fills`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FillsQuery {
    /// Fills before this time, newest first.
    pub last_fill_time: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fills {
    pub fills: Vec<Fill>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fill {
    #[serde(rename = "fill_id")]
    pub fill_id: String,
    pub symbol: String,
    pub side: OrderSide,
    #[serde(rename = "order_id")]
    pub order_id: String,
    pub cli_ord_id: Option<String>,
    #[serde(with = "crate::decimal::json")]
    pub size: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub price: Decimal,
    pub fill_time: String,
    pub fill_type: FillType,
    #[serde(
        rename = "realized_pnl",
        default,
        with = "crate::decimal::json::option"
    )]
    pub realized_pnl: Option<Decimal>,
    #[serde(rename = "sequence_id")]
    pub sequence_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FillType {
    Maker,
    Taker,
    Liquidation,
    PartialLiquidation,
    Assignor,
    Assignee,
    TakerAfterEdit,
    UnwindBankrupt,
    UnwindCounterparty,
}

/// `GET /leveragepreferences`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct LeveragePreferencesQuery {
    pub symbol: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeveragePreferences {
    pub leverage_preferences: Vec<LeveragePreference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeveragePreference {
    pub symbol: String,
    #[serde(with = "crate::decimal::json")]
    pub max_leverage: Decimal,
}

/// `PUT /leveragepreferences`: fixed leverage for a contract; without
/// `maxLeverage` the contract returns to cross margin.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetLeveragePreference {
    pub symbol: String,
    #[serde(default, with = "crate::decimal::json::option")]
    pub max_leverage: Option<Decimal>,
}

/// `GET /pnlpreferences`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PnlPreferencesQuery {}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PnlPreferences {
    pub account_wide_pnl_currency: Option<String>,
    pub preferences: Vec<PnlPreference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PnlPreference {
    pub symbol: String,
    pub pnl_currency: String,
}

/// `PUT /pnlpreferences`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPnlPreference {
    pub symbol: String,
    pub pnl_preference: String,
}

/// A write that answers with its outcome alone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Acknowledged {}

/// `GET /notifications`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct NotificationsQuery {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notifications {
    pub notifications: Vec<Notification>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    #[serde(rename = "type")]
    pub kind: NotificationType,
    pub priority: NotificationPriority,
    pub note: String,
    pub effective_time: String,
    pub expected_downtime_minutes: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationType {
    NewFeature,
    BugFix,
    Settlement,
    General,
    Maintenance,
    Market,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationPriority {
    Low,
    Medium,
    High,
}

/// `GET /unwindqueue`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct UnwindQueueQuery {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnwindQueue {
    pub queue: Vec<UnwindPosition>,
}

/// Where a position stands in the auto-deleveraging queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnwindPosition {
    pub symbol: String,
    pub percentile: i32,
}

/// `GET /assignmentprogram/current`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AssignmentProgramsQuery {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignmentPrograms {
    pub participants: Vec<AssignmentParticipation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignmentParticipation {
    pub id: i64,
    pub participant: AssignmentParticipant,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignmentParticipant {
    pub contract_type: String,
    #[serialize_always]
    pub contract: Option<String>,
    #[serialize_always]
    #[serde(default, with = "crate::decimal::json::option")]
    pub max_size: Option<Decimal>,
    #[serialize_always]
    #[serde(default, with = "crate::decimal::json::option")]
    pub max_position: Option<Decimal>,
    pub accept_long: bool,
    pub accept_short: bool,
    pub time_frame: AssignmentTimeFrame,
    pub enabled: bool,
    #[serde(default, with = "crate::decimal::json::option")]
    pub cash_account_open_position_max_notional: Option<Decimal>,
    pub base_currency: Option<String>,
    pub quote_currency: Option<String>,
    pub minimum_profitability_per_assignment_long_bps: Option<i32>,
    pub minimum_profitability_per_assignment_short_bps: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AssignmentTimeFrame {
    Weekdays,
    Weekend,
    All,
}

/// `GET /feeschedules/volumes`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FeeScheduleVolumesQuery {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeeScheduleVolumes {
    /// 30-day USD volume per fee schedule uid.
    pub volumes_by_fee_schedule: KeyedRows<String, super::Number>,
}

/// `GET /subaccounts`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SubaccountsQuery {}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subaccounts {
    pub master_account_uid: Option<String>,
    pub subaccounts: Vec<Subaccount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subaccount {
    pub account_uid: String,
    pub full_name: Option<String>,
    pub email: String,
    pub holding_accounts: Vec<HoldingBalance>,
    pub futures_accounts: Vec<FuturesAccountMargin>,
    pub flex_account: SubaccountFlex,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldingBalance {
    pub currency: String,
    #[serde(with = "crate::decimal::json")]
    pub amount: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FuturesAccountMargin {
    pub name: String,
    #[serde(with = "crate::decimal::json")]
    pub available_margin: Decimal,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubaccountFlex {
    pub currencies: Vec<SubaccountFlexCurrency>,
    #[serde(with = "crate::decimal::json")]
    pub initial_margin: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub initial_margin_with_orders: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub maintenance_margin: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub balance_value: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub portfolio_value: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub collateral_value: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub pnl: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub unrealized_funding: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub total_unrealized: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub total_unrealized_as_margin: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub available_margin: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub margin_equity: Decimal,
    pub portfolio_margin_breakdown: Option<PortfolioMarginBreakdown>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubaccountFlexCurrency {
    pub currency: String,
    #[serde(with = "crate::decimal::json")]
    pub quantity: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub value: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub collateral: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub available: Decimal,
}

/// `GET /subaccount/{uid}/trading-enabled`.
#[derive(Debug, Clone, Serialize)]
pub struct SubaccountTradingQuery {
    #[serde(skip)]
    pub subaccount_uid: PathSegment,
}

/// `PUT /subaccount/{uid}/trading-enabled`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSubaccountTrading {
    #[serde(skip)]
    pub subaccount_uid: PathSegment,
    pub trading_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubaccountTrading {
    pub trading_enabled: bool,
}

/// `GET /trading/instruments`: the contracts this key may trade.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TradingInstrumentsQuery {
    pub contract_type: Option<InstrumentType>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradingInstruments {
    pub instruments: Vec<TradingInstrument>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TradingInstrument {
    pub symbol: String,
    pub pair: String,
    pub base: String,
    pub quote: String,
    #[serde(rename = "type")]
    pub kind: InstrumentType,
    pub underlying: Option<String>,
    pub underlying_future: Option<String>,
    #[serde(with = "crate::decimal::json")]
    pub tick_size: Decimal,
    #[serde(default, with = "crate::decimal::json::option")]
    pub contract_size: Option<Decimal>,
    #[serde(with = "crate::decimal::json")]
    pub minimum_trade_size: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub impact_mid_size: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub max_position_size: Decimal,
    #[serde(default, with = "crate::decimal::json::option")]
    pub max_open_interest_usd: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub max_open_interest_share: Option<Decimal>,
    pub opening_date: String,
    pub last_trading_time: Option<String>,
    #[serialize_always]
    pub isin: Option<String>,
    pub contract_value_trade_precision: i64,
    pub post_only: bool,
    pub suspended: bool,
    pub fee_schedule_uid: String,
    pub rebate_levels: KeyedRows<String, super::Number>,
    pub mtf: bool,
    pub tradfi: bool,
    pub restricted: bool,
    pub is_expired: bool,
    pub margin_levels: Option<Vec<MarginLevel>>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub funding_rate_coefficient: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub max_relative_funding_rate: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub min_relative_funding_rate: Option<Decimal>,
    pub description: Option<String>,
    pub maker_protection_millis: Option<i64>,
    pub option_type: Option<OptionType>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub strike_price: Option<Decimal>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionType {
    Call,
    Put,
}

/// `GET /transfers`. Kraken's description gives this reply no schema, so it is
/// the one Futures reply published unexamined.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransfersQuery {
    pub last_transfer_time: Option<String>,
}

macro_rules! private {
    ($request:ty, $method:ident, $path:literal, $response:ty) => {
        impl Sealed for $request {}
        impl Endpoint for $request {
            const PATH: &'static str = $path;
            const METHOD: Method = Method::$method;
            type Access = Private;
            type Response = $response;
        }
    };
}

private!(AccountsQuery, Get, "accounts", Reply<Accounts>);
private!(OpenOrdersQuery, Get, "openorders", Reply<OpenOrders>);
private!(
    OpenPositionsQuery,
    Get,
    "openpositions",
    Reply<OpenPositions>
);
private!(FillsQuery, Get, "fills", Reply<Fills>);
private!(
    LeveragePreferencesQuery,
    Get,
    "leveragepreferences",
    Reply<LeveragePreferences>
);
private!(
    SetLeveragePreference,
    Put,
    "leveragepreferences",
    Reply<Acknowledged>
);
private!(
    PnlPreferencesQuery,
    Get,
    "pnlpreferences",
    Reply<PnlPreferences>
);
private!(SetPnlPreference, Put, "pnlpreferences", Reply<Acknowledged>);
private!(
    NotificationsQuery,
    Get,
    "notifications",
    Reply<Notifications>
);
private!(UnwindQueueQuery, Get, "unwindqueue", Reply<UnwindQueue>);
private!(
    AssignmentProgramsQuery,
    Get,
    "assignmentprogram/current",
    Reply<AssignmentPrograms>
);
private!(
    FeeScheduleVolumesQuery,
    Get,
    "feeschedules/volumes",
    Reply<FeeScheduleVolumes>
);
private!(SubaccountsQuery, Get, "subaccounts", Reply<Subaccounts>);
private!(
    TradingInstrumentsQuery,
    Get,
    "trading/instruments",
    Reply<TradingInstruments>
);
private!(TransfersQuery, Get, "transfers", serde_json::Value);

impl Sealed for SubaccountTradingQuery {}
impl Endpoint for SubaccountTradingQuery {
    const PATH: &'static str = "subaccount/{uid}/trading-enabled";
    const METHOD: Method = Method::Get;
    type Access = Private;
    type Response = Reply<SubaccountTrading>;

    fn path(&self) -> Cow<'static, str> {
        format!("subaccount/{}/trading-enabled", self.subaccount_uid).into()
    }
}

impl Sealed for SetSubaccountTrading {}
impl Endpoint for SetSubaccountTrading {
    const PATH: &'static str = "subaccount/{uid}/trading-enabled";
    const METHOD: Method = Method::Put;
    type Access = Private;
    type Response = Reply<SubaccountTrading>;

    fn path(&self) -> Cow<'static, str> {
        format!("subaccount/{}/trading-enabled", self.subaccount_uid).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::futures::envelope::decode_fixture;

    #[test]
    fn decodes_the_account_replies() {
        let accounts: Reply<Accounts> = decode_fixture("trading.get_accounts.json").unwrap();
        assert!(matches!(
            accounts.body.accounts.get(&"cash".to_owned()),
            Some(Account::Cash(_))
        ));
        let _: Reply<OpenOrders> = decode_fixture("trading.get_openorders.json").unwrap();
        let _: Reply<OpenPositions> = decode_fixture("trading.get_openpositions.json").unwrap();
        let _: Reply<Fills> = decode_fixture("trading.get_fills.json").unwrap();
        let _: Reply<LeveragePreferences> =
            decode_fixture("trading.get_leveragepreferences.json").unwrap();
        let _: Reply<Acknowledged> =
            decode_fixture("trading.put_leveragepreferences.json").unwrap();
        let _: Reply<PnlPreferences> = decode_fixture("trading.get_pnlpreferences.json").unwrap();
        let _: Reply<Notifications> = decode_fixture("trading.get_notifications.json").unwrap();
        let _: Reply<UnwindQueue> = decode_fixture("trading.get_unwindqueue.json").unwrap();
        let _: Reply<AssignmentPrograms> =
            decode_fixture("trading.get_assignmentprogram_current.synthetic.json").unwrap();
        let _: Reply<FeeScheduleVolumes> =
            decode_fixture("trading.get_feeschedules_volumes.json").unwrap();
        let _: Reply<Subaccounts> = decode_fixture("trading.get_subaccounts.json").unwrap();
        let _: Reply<SubaccountTrading> =
            decode_fixture("trading.get_subaccount_trading-enabled.synthetic.json").unwrap();
        let _: Reply<TradingInstruments> =
            decode_fixture("trading.get_trading_instruments.synthetic.json").unwrap();
    }

    #[test]
    fn a_margin_account_is_told_apart_by_its_type() {
        let accounts: Reply<Accounts> = decode_fixture("trading.get_accounts.json").unwrap();
        let margin = accounts
            .body
            .accounts
            .values()
            .filter(|a| matches!(a, Account::Margin(_)))
            .count();
        assert!(margin >= 1);
    }
}

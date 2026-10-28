//! Typed enums (and shared `#[command(flatten)]` arg groups) for CLI arguments.

/// Wire form of a CLI value enum whose protocol string is exactly its clap
/// token. Where the REST token diverges (futures order types), the venue
/// crate's request type owns the mapping.
///
/// Implementors keep an explicit `#[value(name = ...)]` on every variant —
/// even where it matches clap's derived default — so a Rust variant rename
/// can never silently change the protocol string.
pub trait CliWire: clap::ValueEnum {
    #[expect(
        clippy::expect_used,
        reason = "to_possible_value is None only for #[value(skip)] variants; CliWire enums have \
                  none"
    )]
    fn as_wire(&self) -> String {
        self.to_possible_value()
            .expect("CLI value enums declare no skipped variants")
            .get_name()
            .to_owned()
    }
}

/// Asset class filter shared by market, account, and funding commands.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum AssetClass {
    #[value(name = "currency")]
    Currency,
    #[value(name = "tokenized_asset")]
    #[serde(rename = "tokenized_asset")]
    TokenizedAsset,
    #[value(name = "forex")]
    Forex,
}

impl CliWire for AssetClass {}

// Flattened into every command that takes the flag, so it is declared once.
#[derive(Debug, Clone, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RebaseOpts {
    /// Rebase multiplier for xstocks data (rebased or base).
    #[arg(long)]
    pub rebase_multiplier: Option<RebaseMultiplier>,
}

// Offset-style pagination, shared by closed-orders, trades-history and ledgers.
#[derive(Debug, Clone, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Pagination {
    /// Starting unix timestamp or ID (exclusive).
    #[arg(long)]
    pub start: Option<String>,
    /// Ending unix timestamp or ID (inclusive).
    #[arg(long)]
    pub end: Option<String>,
    /// Result offset for pagination.
    #[arg(long)]
    pub offset: Option<u64>,
    /// Omit count from result (faster for large histories).
    #[arg(long)]
    #[serde(default)]
    pub without_count: bool,
}

impl Pagination {
    /// Any window or offset requested — venue-only semantics the paper
    /// journal cannot honour, so mode routing refuses instead of ignoring.
    pub fn is_bounded(&self) -> bool {
        self.start.is_some() || self.end.is_some() || self.offset.is_some() || self.without_count
    }
}

/// Rebase multiplier for xstocks data.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum RebaseMultiplier {
    #[value(name = "rebased")]
    Rebased,
    #[value(name = "base")]
    Base,
}

impl CliWire for RebaseMultiplier {}

/// Which timestamp to use when filtering closed orders.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Closetime {
    #[value(name = "open")]
    Open,
    #[value(name = "close")]
    Close,
    #[value(name = "both")]
    Both,
}

impl CliWire for Closetime {}

/// Type of data to export in a report request.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum ReportType {
    #[value(name = "trades")]
    Trades,
    #[value(name = "ledgers")]
    Ledgers,
}

impl CliWire for ReportType {}

/// Price levels per side for the grouped order book.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum GroupedDepth {
    #[value(name = "10")]
    #[serde(rename = "10")]
    D10,
    #[value(name = "25")]
    #[serde(rename = "25")]
    D25,
    #[value(name = "100")]
    #[serde(rename = "100")]
    D100,
    #[value(name = "250")]
    #[serde(rename = "250")]
    D250,
    #[value(name = "1000")]
    #[serde(rename = "1000")]
    D1000,
}

impl CliWire for GroupedDepth {}

/// Tick grouping within each price level of the grouped order book.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Grouping {
    #[value(name = "1")]
    #[serde(rename = "1")]
    G1,
    #[value(name = "5")]
    #[serde(rename = "5")]
    G5,
    #[value(name = "10")]
    #[serde(rename = "10")]
    G10,
    #[value(name = "25")]
    #[serde(rename = "25")]
    G25,
    #[value(name = "50")]
    #[serde(rename = "50")]
    G50,
    #[value(name = "100")]
    #[serde(rename = "100")]
    G100,
    #[value(name = "250")]
    #[serde(rename = "250")]
    G250,
    #[value(name = "500")]
    #[serde(rename = "500")]
    G500,
    #[value(name = "1000")]
    #[serde(rename = "1000")]
    G1000,
}

impl CliWire for Grouping {}

/// Price levels per side for the L3 (per-order) order book. `Full` (0) requests
/// the entire book. Numeric rather than string so the handler needs no
/// re-parse.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum L3Depth {
    #[value(name = "0")]
    #[serde(rename = "0")]
    Full,
    #[value(name = "10")]
    #[serde(rename = "10")]
    D10,
    #[value(name = "25")]
    #[serde(rename = "25")]
    D25,
    #[value(name = "100")]
    #[serde(rename = "100")]
    D100,
    #[value(name = "250")]
    #[serde(rename = "250")]
    D250,
    #[value(name = "1000")]
    #[serde(rename = "1000")]
    D1000,
}

impl L3Depth {
    pub fn as_u64(self) -> u64 {
        match self {
            Self::Full => 0,
            Self::D10 => 10,
            Self::D25 => 25,
            Self::D100 => 100,
            Self::D250 => 250,
            Self::D1000 => 1000,
        }
    }
}

/// File format for export reports.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum ReportFormat {
    #[value(name = "CSV")]
    #[serde(rename = "CSV")]
    Csv,
    #[value(name = "TSV")]
    #[serde(rename = "TSV")]
    Tsv,
}

impl CliWire for ReportFormat {}

/// Trade type filter for trades-history. Kraken's tokens contain spaces; the
/// kebab-case aliases exist purely for shell ergonomics.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum TradeType {
    #[value(name = "all")]
    All,
    #[value(name = "any position", alias = "any-position")]
    #[serde(rename = "any position")]
    AnyPosition,
    #[value(name = "closed position", alias = "closed-position")]
    #[serde(rename = "closed position")]
    ClosedPosition,
    #[value(name = "closing position", alias = "closing-position")]
    #[serde(rename = "closing position")]
    ClosingPosition,
    #[value(name = "no position", alias = "no-position")]
    #[serde(rename = "no position")]
    NoPosition,
}

impl CliWire for TradeType {}

/// Contract class filter for futures trading-instruments.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum ContractType {
    #[value(name = "futures_inverse")]
    #[serde(rename = "futures_inverse")]
    FuturesInverse,
    #[value(name = "futures_vanilla")]
    #[serde(rename = "futures_vanilla")]
    FuturesVanilla,
    #[value(name = "flexible_futures")]
    #[serde(rename = "flexible_futures")]
    FlexibleFutures,
}

impl CliWire for ContractType {}

/// Unit for a trailing stop's max deviation.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum TrailingUnit {
    #[value(name = "percent")]
    Percent,
    #[value(name = "quote_currency")]
    #[serde(rename = "quote_currency")]
    QuoteCurrency,
}

impl CliWire for TrailingUnit {}

/// Futures order type. The clap token table is the single CLI/paper vocabulary
/// — what a user types, what the JSON output prints, and what a tool call
/// sends; the REST `orderType` token differs.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum FuturesOrderType {
    #[value(name = "market")]
    Market,
    #[value(name = "limit")]
    Limit,
    #[value(name = "post")]
    Post,
    #[value(name = "stop")]
    Stop,
    #[value(name = "take-profit")]
    TakeProfit,
    #[value(name = "ioc")]
    Ioc,
    #[value(name = "trailing-stop")]
    TrailingStop,
    #[value(name = "fok")]
    Fok,
}

impl CliWire for FuturesOrderType {}

impl std::fmt::Display for FuturesOrderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_wire())
    }
}

/// Which price a futures trigger watches.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum TriggerSignal {
    #[value(name = "mark")]
    Mark,
    #[value(name = "index")]
    Index,
    #[value(name = "last")]
    Last,
}

impl CliWire for TriggerSignal {}

impl std::fmt::Display for TriggerSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_wire())
    }
}

/// A `--to` destination for a recorded stream: a durable backend, or a live
/// stdout echo.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum SinkTarget {
    /// The file-based DuckDB tape (native columnar format; independent of
    /// `-o`).
    Duckdb,
    /// The dependency-free JSONL files (always JSON lines; independent of
    /// `-o`).
    Jsonl,
    /// A live echo of the frames to stdout, rendered per `-o`/`--output`.
    Stdout,
}

/// The session a command reads when the caller names none: the newest.
///
/// Named once here because the clap default and the serde default must be the
/// same value — clap fills it on the command line, serde fills it for a tool
/// call, and a divergence would silently read a different session.
pub(crate) fn latest_session() -> String {
    "latest".to_owned()
}

/// The quote currency a simulated account is denominated in by default.
pub(crate) fn usd() -> String {
    "USD".to_owned()
}

/// Replay speed when the caller names none: wall-clock.
pub(crate) fn real_time() -> f64 {
    1.0
}

/// The order type a futures command assumes when the caller names none.
///
/// Shared by the live and paper futures leaves: the same default declared twice
/// is a default that can drift.
pub(crate) fn limit_order() -> FuturesOrderType {
    FuturesOrderType::Limit
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant's serde name must equal its clap token.
    ///
    /// Walks `value_variants()` rather than a written list, so a variant added
    /// later is covered without touching this test.
    fn clap_and_serde_agree<T: clap::ValueEnum + serde::Serialize>() {
        for variant in T::value_variants() {
            let token = variant
                .to_possible_value()
                .expect("these enums declare no skipped variants")
                .get_name()
                .to_owned();
            assert_eq!(
                serde_json::to_value(variant).expect("serialize"),
                serde_json::Value::String(token.clone()),
                "'{token}' is what a user types and what the venue understands."
            );
        }
    }

    /// Regression: a blanket kebab-case rename made proposals carry
    /// `tokenized-asset` for `tokenized_asset`, `d100` for `100`, and
    /// `any-position` for `any position` — none of which the CLI accepts, so a
    /// policy engine would have been matching on values that cannot occur.
    ///
    /// Only checkable now that one type carries both derives; while the clap
    /// tokens and the wire names lived apart, agreement was a convention.
    #[test]
    fn every_value_enum_proposes_the_token_the_cli_accepts() {
        clap_and_serde_agree::<AssetClass>();
        clap_and_serde_agree::<RebaseMultiplier>();
        clap_and_serde_agree::<Closetime>();
        clap_and_serde_agree::<ReportType>();
        clap_and_serde_agree::<GroupedDepth>();
        clap_and_serde_agree::<Grouping>();
        clap_and_serde_agree::<L3Depth>();
        clap_and_serde_agree::<ReportFormat>();
        clap_and_serde_agree::<TradeType>();
        clap_and_serde_agree::<ContractType>();
        clap_and_serde_agree::<TrailingUnit>();
        clap_and_serde_agree::<FuturesOrderType>();
        clap_and_serde_agree::<TriggerSignal>();
        clap_and_serde_agree::<SinkTarget>();
    }

    #[test]
    fn a_value_enum_decodes_from_its_wire_name() {
        let decoded: AssetClass =
            serde_json::from_str(r#""tokenized_asset""#).expect("the CLI's own token must decode");
        assert_eq!(decoded, AssetClass::TokenizedAsset);
        serde_json::from_str::<AssetClass>(r#""tokenized-asset""#)
            .expect_err("a value the CLI never accepts must not decode");
    }
}

//! Typed enums (and shared `#[command(flatten)]` arg groups) for CLI arguments.
//!
//! Each enum deliberately unifies what were divergent per-command allowlists and
//! unconstrained strings, so one definition feeds clap validation, `--help`, and
//! the wire param. That cuts both ways: formerly free-form values now fail at
//! clap parse (exit 2), while the shared set is a superset of some old
//! per-command allowlists, so a few values previously rejected locally are now
//! validated by the server instead. The allowed sets are mirrored in
//! `agents/tool-catalog.json`.

/// Wire form of a CLI value enum whose protocol string is exactly its clap
/// token. Enums whose wire form diverges from the CLI token (futures order
/// types) implement their own mapping instead.
///
/// Implementors keep an explicit `#[value(name = ...)]` on every variant —
/// even where it matches clap's derived default — so a Rust variant rename
/// can never silently change the protocol string.
pub(crate) trait CliWire: clap::ValueEnum {
    #[expect(
        clippy::expect_used,
        reason = "to_possible_value is None only for #[value(skip)] variants; CliWire enums have none"
    )]
    fn as_wire(&self) -> String {
        self.to_possible_value()
            .expect("CLI value enums declare no skipped variants")
            .get_name()
            .to_owned()
    }
}

/// Asset class filter shared by market, account, and funding commands.
///
/// Unifies what were previously divergent per-command allowlists
/// (`[tokenized_asset, forex]`, `[tokenized_asset]`, and several unconstrained
/// fields) into one set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum AssetClass {
    #[value(name = "currency")]
    Currency,
    #[value(name = "tokenized_asset")]
    TokenizedAsset,
    #[value(name = "forex")]
    Forex,
}

impl CliWire for AssetClass {}

/// Shared `rebase_multiplier` argument, flattened into the many commands that
/// accept it so the flag is declared (and applied to request params) once.
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct RebaseOpts {
    /// Rebase multiplier for xstocks data (rebased or base).
    #[arg(long)]
    pub(crate) rebase_multiplier: Option<RebaseMultiplier>,
}

impl RebaseOpts {
    /// Insert the `rebase_multiplier` request param when set.
    pub(crate) fn apply(&self, params: &mut std::collections::HashMap<String, String>) {
        if let Some(rm) = self.rebase_multiplier {
            params.insert("rebase_multiplier".into(), rm.as_wire());
        }
    }
}

/// Shared offset-style pagination (`start`/`end`/`offset`/`without_count`) for
/// closed-orders, trades-history, and ledgers.
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct Pagination {
    /// Starting unix timestamp or ID (exclusive).
    #[arg(long)]
    pub(crate) start: Option<String>,
    /// Ending unix timestamp or ID (inclusive).
    #[arg(long)]
    pub(crate) end: Option<String>,
    /// Result offset for pagination.
    #[arg(long)]
    pub(crate) offset: Option<u64>,
    /// Omit count from result (faster for large histories).
    #[arg(long)]
    pub(crate) without_count: bool,
}

impl Pagination {
    /// Any window or offset requested — venue-only semantics the paper
    /// journal cannot honour, so mode routing refuses instead of ignoring.
    pub(crate) fn is_bounded(&self) -> bool {
        self.start.is_some() || self.end.is_some() || self.offset.is_some() || self.without_count
    }

    /// Insert the pagination request params that are set. `offset` maps to the
    /// Kraken `ofs` param; `without_count` to `without_count=true`.
    pub(crate) fn apply(&self, params: &mut std::collections::HashMap<String, String>) {
        if let Some(s) = &self.start {
            params.insert("start".into(), s.clone());
        }
        if let Some(e) = &self.end {
            params.insert("end".into(), e.clone());
        }
        if let Some(o) = self.offset {
            params.insert("ofs".into(), o.to_string());
        }
        if self.without_count {
            params.insert("without_count".into(), "true".into());
        }
    }
}

/// Rebase multiplier for xstocks data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum RebaseMultiplier {
    #[value(name = "rebased")]
    Rebased,
    #[value(name = "base")]
    Base,
}

impl CliWire for RebaseMultiplier {}

/// Which timestamp to use when filtering closed orders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Closetime {
    #[value(name = "open")]
    Open,
    #[value(name = "close")]
    Close,
    #[value(name = "both")]
    Both,
}

impl CliWire for Closetime {}

/// Type of data to export in a report request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum ReportType {
    #[value(name = "trades")]
    Trades,
    #[value(name = "ledgers")]
    Ledgers,
}

impl CliWire for ReportType {}

/// Price levels per side for the grouped order book.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum GroupedDepth {
    #[value(name = "10")]
    D10,
    #[value(name = "25")]
    D25,
    #[value(name = "100")]
    D100,
    #[value(name = "250")]
    D250,
    #[value(name = "1000")]
    D1000,
}

impl CliWire for GroupedDepth {}

/// Tick grouping within each price level of the grouped order book.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Grouping {
    #[value(name = "1")]
    G1,
    #[value(name = "5")]
    G5,
    #[value(name = "10")]
    G10,
    #[value(name = "25")]
    G25,
    #[value(name = "50")]
    G50,
    #[value(name = "100")]
    G100,
    #[value(name = "250")]
    G250,
    #[value(name = "500")]
    G500,
    #[value(name = "1000")]
    G1000,
}

impl CliWire for Grouping {}

/// Price levels per side for the L3 (per-order) order book. `Full` (0) requests
/// the entire book. Numeric rather than string so the handler needs no re-parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum L3Depth {
    #[value(name = "0")]
    Full,
    #[value(name = "10")]
    D10,
    #[value(name = "25")]
    D25,
    #[value(name = "100")]
    D100,
    #[value(name = "250")]
    D250,
    #[value(name = "1000")]
    D1000,
}

impl L3Depth {
    pub(crate) fn as_u64(self) -> u64 {
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum ReportFormat {
    #[value(name = "CSV")]
    Csv,
    #[value(name = "TSV")]
    Tsv,
}

impl CliWire for ReportFormat {}

/// Trade type filter for trades-history. Kraken's tokens contain spaces; the
/// kebab-case aliases exist purely for shell ergonomics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum TradeType {
    #[value(name = "all")]
    All,
    #[value(name = "any position", alias = "any-position")]
    AnyPosition,
    #[value(name = "closed position", alias = "closed-position")]
    ClosedPosition,
    #[value(name = "closing position", alias = "closing-position")]
    ClosingPosition,
    #[value(name = "no position", alias = "no-position")]
    NoPosition,
}

impl CliWire for TradeType {}

/// Contract class filter for futures trading-instruments.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum ContractType {
    #[value(name = "futures_inverse")]
    FuturesInverse,
    #[value(name = "futures_vanilla")]
    FuturesVanilla,
    #[value(name = "flexible_futures")]
    FlexibleFutures,
}

impl CliWire for ContractType {}

/// Unit for a trailing stop's max deviation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum TrailingUnit {
    #[value(name = "percent")]
    Percent,
    #[value(name = "quote_currency")]
    QuoteCurrency,
}

impl CliWire for TrailingUnit {}

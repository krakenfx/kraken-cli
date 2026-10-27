//! Argument values → REST request values.
//!
//! Free functions rather than `From` impls: both sides are foreign to this
//! crate, so the orphan rule forbids the trait. Every match is exhaustive, so a
//! variant added on either side without the other stops compiling here.

use kraken_contract::{futures as futures_arg, value_types as arg};
use kraken_rest::{futures, spot};

pub(crate) fn asset_class(class: arg::AssetClass) -> spot::AssetClass {
    match class {
        arg::AssetClass::Currency => spot::AssetClass::Currency,
        arg::AssetClass::TokenizedAsset => spot::AssetClass::TokenizedAsset,
        arg::AssetClass::Forex => spot::AssetClass::Forex,
    }
}

pub(crate) fn rebase_multiplier(multiplier: arg::RebaseMultiplier) -> spot::RebaseMultiplier {
    match multiplier {
        arg::RebaseMultiplier::Rebased => spot::RebaseMultiplier::Rebased,
        arg::RebaseMultiplier::Base => spot::RebaseMultiplier::Base,
    }
}

pub(crate) fn rebase(opts: &arg::RebaseOpts) -> Option<spot::RebaseMultiplier> {
    opts.rebase_multiplier.map(rebase_multiplier)
}

pub(crate) fn window(page: arg::Pagination) -> spot::account::Window {
    let arg::Pagination {
        start,
        end,
        offset,
        without_count,
    } = page;
    spot::account::Window {
        start: start.map(Into::into),
        end: end.map(Into::into),
        ofs: offset,
        without_count,
    }
}

pub(crate) fn close_time(time: arg::Closetime) -> spot::account::CloseTime {
    use spot::account::CloseTime;
    match time {
        arg::Closetime::Open => CloseTime::Open,
        arg::Closetime::Close => CloseTime::Close,
        arg::Closetime::Both => CloseTime::Both,
    }
}

pub(crate) fn trade_filter(filter: arg::TradeType) -> spot::account::TradeFilter {
    use spot::account::TradeFilter;
    match filter {
        arg::TradeType::All => TradeFilter::All,
        arg::TradeType::AnyPosition => TradeFilter::AnyPosition,
        arg::TradeType::ClosedPosition => TradeFilter::ClosedPosition,
        arg::TradeType::ClosingPosition => TradeFilter::ClosingPosition,
        arg::TradeType::NoPosition => TradeFilter::NoPosition,
    }
}

pub(crate) fn l3_depth(depth: arg::L3Depth) -> u32 {
    match depth {
        arg::L3Depth::Full => 0,
        arg::L3Depth::D10 => 10,
        arg::L3Depth::D25 => 25,
        arg::L3Depth::D100 => 100,
        arg::L3Depth::D250 => 250,
        arg::L3Depth::D1000 => 1000,
    }
}

pub(crate) fn export_kind(report: arg::ReportType) -> spot::account::ExportReportKind {
    use spot::account::ExportReportKind;
    match report {
        arg::ReportType::Trades => ExportReportKind::Trades,
        arg::ReportType::Ledgers => ExportReportKind::Ledgers,
    }
}

pub(crate) fn export_format(format: arg::ReportFormat) -> spot::account::ExportFormat {
    use spot::account::ExportFormat;
    match format {
        arg::ReportFormat::Csv => ExportFormat::Csv,
        arg::ReportFormat::Tsv => ExportFormat::Tsv,
    }
}

/// A Ticker request for these pairs; none asks for the class's whole catalog.
pub(crate) fn ticker_query(
    pairs: impl IntoIterator<Item = impl AsRef<str>>,
    asset_class: Option<arg::AssetClass>,
) -> spot::market::TickerQuery {
    spot::market::TickerQuery {
        pair: pairs
            .into_iter()
            .map(|pair| kraken_rest::Pair::from(pair.as_ref()))
            .collect(),
        asset_class: asset_class.map(self::asset_class),
    }
}

/// A venue timestamp as an instant; `None` when it is not a time or is out
/// of range.
pub(crate) fn instant(time: &kraken_rest::UnixTime) -> Option<chrono::DateTime<chrono::Utc>> {
    let (seconds, nanos) = time.parts()?;
    chrono::DateTime::from_timestamp(seconds, nanos)
}

/// A money amount from the command line, refused before it reaches the venue
/// when it is not a decimal.
pub(crate) fn amount(flag: &str, raw: &str) -> crate::errors::Result<rust_decimal::Decimal> {
    raw.parse().map_err(|_| {
        crate::errors::KrakenError::Validation(format!("{flag} {raw:?} is not a decimal amount"))
    })
}

/// A command-line token decoded as the venue spells it; `valid` lists the
/// accepted spellings for the refusal.
pub(crate) fn token<T: serde::de::DeserializeOwned>(
    field: &str,
    raw: &str,
    valid: &str,
) -> crate::errors::Result<T> {
    T::deserialize(serde::de::value::StrDeserializer::<serde::de::value::Error>::new(raw)).map_err(
        |_| {
            crate::errors::KrakenError::Validation(format!("{field} {raw:?} is not one of {valid}"))
        },
    )
}

/// An asset class is an open vocabulary: an unknown token passes through.
pub(crate) fn open_asset_class(raw: &str) -> spot::AssetClass {
    serde::Deserialize::deserialize(
        serde::de::value::StrDeserializer::<serde::de::value::Error>::new(raw),
    )
    .unwrap_or_else(|_: serde::de::value::Error| spot::AssetClass::Unrecognized(raw.to_owned()))
}

pub(crate) fn optional_amount(
    flag: &str,
    raw: Option<&str>,
) -> crate::errors::Result<Option<rust_decimal::Decimal>> {
    raw.map(|raw| amount(flag, raw)).transpose()
}

/// An export window bound, which Kraken takes as whole Unix seconds.
pub(crate) fn unix_seconds(flag: &str, raw: &str) -> crate::errors::Result<i64> {
    raw.parse().map_err(|_| {
        crate::errors::KrakenError::Validation(format!(
            "{flag} {raw:?} is not a Unix timestamp in whole seconds"
        ))
    })
}

/// `pairs --info` is free text on the command line; only Kraken's four slices
/// are a request.
pub(crate) fn pair_info(info: &str) -> crate::errors::Result<spot::market::PairInfo> {
    use spot::market::PairInfo;
    match info {
        "info" => Ok(PairInfo::Info),
        "leverage" => Ok(PairInfo::Leverage),
        "fees" => Ok(PairInfo::Fees),
        "margin" => Ok(PairInfo::Margin),
        other => Err(crate::errors::KrakenError::Validation(format!(
            "--info {other:?} is not one of info, leverage, fees, margin"
        ))),
    }
}

pub(crate) fn grouped_depth(depth: arg::GroupedDepth) -> u32 {
    match depth {
        arg::GroupedDepth::D10 => 10,
        arg::GroupedDepth::D25 => 25,
        arg::GroupedDepth::D100 => 100,
        arg::GroupedDepth::D250 => 250,
        arg::GroupedDepth::D1000 => 1000,
    }
}

pub(crate) fn grouping(grouping: arg::Grouping) -> u32 {
    match grouping {
        arg::Grouping::G1 => 1,
        arg::Grouping::G5 => 5,
        arg::Grouping::G10 => 10,
        arg::Grouping::G25 => 25,
        arg::Grouping::G50 => 50,
        arg::Grouping::G100 => 100,
        arg::Grouping::G250 => 250,
        arg::Grouping::G500 => 500,
        arg::Grouping::G1000 => 1000,
    }
}

/// A value bound for a Futures request path, refused when it could reach
/// another path.
pub(crate) fn segment(
    what: &'static str,
    value: String,
) -> crate::errors::Result<futures::PathSegment> {
    futures::PathSegment::new(what, value)
        .map_err(|e| crate::errors::KrakenError::Validation(e.to_string()))
}

pub(crate) fn instrument_type(contract: arg::ContractType) -> futures::market::InstrumentType {
    match contract {
        arg::ContractType::FuturesInverse => futures::market::InstrumentType::FuturesInverse,
        arg::ContractType::FuturesVanilla => futures::market::InstrumentType::FuturesVanilla,
        arg::ContractType::FlexibleFutures => futures::market::InstrumentType::FlexibleFutures,
    }
}

pub(crate) fn trigger_signal(signal: arg::TriggerSignal) -> futures::trading::TriggerSignal {
    match signal {
        arg::TriggerSignal::Mark => futures::trading::TriggerSignal::Mark,
        arg::TriggerSignal::Index => futures::trading::TriggerSignal::Index,
        arg::TriggerSignal::Last => futures::trading::TriggerSignal::Last,
    }
}

pub(crate) fn deviation_unit(unit: arg::TrailingUnit) -> futures::trading::DeviationUnit {
    match unit {
        arg::TrailingUnit::Percent => futures::trading::DeviationUnit::Percent,
        arg::TrailingUnit::QuoteCurrency => futures::trading::DeviationUnit::QuoteCurrency,
    }
}

pub(crate) fn flag(value: futures_arg::BoolValue) -> bool {
    match value {
        futures_arg::BoolValue::True => true,
        futures_arg::BoolValue::False => false,
    }
}

pub(crate) fn history_window(
    window: futures_arg::HistoryWindow,
) -> futures::history::HistoryWindow {
    let futures_arg::HistoryWindow {
        since,
        before,
        sort,
    } = window;
    futures::history::HistoryWindow {
        since,
        before,
        sort,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arg::CliWire as _;

    /// A filter reaches the venue spelled exactly as the CLI documents it.
    #[test]
    fn argument_and_request_values_share_a_wire_form() {
        for class in [
            arg::AssetClass::Currency,
            arg::AssetClass::TokenizedAsset,
            arg::AssetClass::Forex,
        ] {
            assert_eq!(
                serde_json::to_value(asset_class(class)).expect("serialize"),
                class.as_wire()
            );
        }
        for multiplier in [arg::RebaseMultiplier::Rebased, arg::RebaseMultiplier::Base] {
            assert_eq!(
                serde_json::to_value(rebase_multiplier(multiplier)).expect("serialize"),
                multiplier.as_wire()
            );
        }
        use clap::ValueEnum as _;
        for depth in arg::GroupedDepth::value_variants() {
            assert_eq!(grouped_depth(*depth).to_string(), depth.as_wire());
        }
        for step in arg::Grouping::value_variants() {
            assert_eq!(grouping(*step).to_string(), step.as_wire());
        }
    }
}

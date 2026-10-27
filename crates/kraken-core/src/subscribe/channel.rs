//! The channel catalogue and its validated wire parameters
//! (<https://docs.kraken.com/api/docs/websocket-v2>).
//!
//! [`SubscribableChannel`] is what a client can `subscribe` to (the CLI
//! surface); [`Channel`] is the full incoming-frame discriminant —
//! subscribables plus the server-pushed system feeds. The [`BookDepth`] /
//! [`OhlcInterval`] / [`Level3Depth`] newtypes are validated at construction,
//! so a held value is always wire-valid.

use crate::{
    endpoint::Endpoint,
    error::{Error, Result},
};
use serde::Serialize;
use serde_with::{DeserializeFromStr, SerializeDisplay};
use std::str::FromStr;
use strum::{Display, EnumIter, EnumString};

/// A channel a client can subscribe to, by its v2 wire name or a common alias
/// (`trades`, `l3`). The server-pushed feeds (`status`, `heartbeat`) cannot be
/// subscribed to, so they are not accepted.
// `Channel` is the full wire set; `Display`/`FromStr` use the wire names.
// serde's lowercase names coincide with the strum wire names above, so the
// serialized form is the v2 channel name (`trade`, `level3`), not the variant.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Display,
    EnumIter,
    EnumString,
    serde::Serialize,
    serde::Deserialize,
)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
pub enum SubscribableChannel {
    Ticker,
    Book,
    Ohlc,
    #[strum(to_string = "trade", serialize = "trades")]
    #[cfg_attr(feature = "clap", value(alias = "trades"))]
    Trade,
    #[strum(to_string = "instrument", serialize = "instruments")]
    #[cfg_attr(feature = "clap", value(alias = "instruments"))]
    Instrument,
    #[strum(to_string = "executions", serialize = "execution")]
    #[cfg_attr(feature = "clap", value(alias = "execution"))]
    Executions,
    #[strum(to_string = "balances", serialize = "balance")]
    #[cfg_attr(feature = "clap", value(alias = "balance"))]
    Balances,
    #[strum(to_string = "level3", serialize = "l3")]
    #[cfg_attr(feature = "clap", value(name = "level3", alias = "l3"))]
    Level3,
}

impl SubscribableChannel {
    /// Which socket endpoint serves this channel.
    pub const fn endpoint(self) -> Endpoint {
        match self {
            Self::Ticker | Self::Trade | Self::Book | Self::Ohlc | Self::Instrument => {
                Endpoint::Public
            }
            Self::Executions | Self::Balances => Endpoint::Auth,
            Self::Level3 => Endpoint::L3,
        }
    }
}

/// Widen a [`SubscribableChannel`] to the full [`Channel`] catalogue. Total and
/// trivial: every subscribable channel *is* a channel, carried directly in
/// [`Channel::Subscribable`]. The reverse is partial — the system feeds have no
/// subscribable form.
impl From<SubscribableChannel> for Channel {
    fn from(channel: SubscribableChannel) -> Self {
        Self::Subscribable(channel)
    }
}

/// The **full** set of Kraken v2 `channel` values, the discriminant for
/// *incoming* frames: every [`SubscribableChannel`] (carried by
/// [`Channel::Subscribable`]) plus the server-pushed system feeds
/// [`Channel::Status`] and [`Channel::Heartbeat`]. Subscribe planning uses
/// [`SubscribableChannel`] directly. Deliberately strict: a name outside
/// this catalogue fails the frame, so a new server channel surfaces as a logged
/// error to fix, never as silently reinterpreted data.
///
/// `Channel` *embeds* [`SubscribableChannel`] rather than re-listing its
/// variants, so "every subscribable channel is a channel" is a type-level fact
/// and the two can never drift. The flat v2 wire-name round-trip is all
/// derived: `Display` (`#[strum(transparent)]` forwards the subscribable
/// variant to [`SubscribableChannel`]; the system feeds use their lowercased
/// names), and serde via [`SerializeDisplay`]/[`DeserializeFromStr`] over that
/// `Display` and the [`FromStr`] below. Only [`FromStr`] is hand-written — it
/// routes the system feeds, which no derive expresses.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Display, SerializeDisplay, DeserializeFromStr,
)]
#[strum(serialize_all = "lowercase")]
pub enum Channel {
    /// A channel a client can subscribe to — the bare channel identity, whose
    /// wire name (and so its serde form) comes straight from
    /// [`SubscribableChannel`].
    #[strum(transparent)]
    Subscribable(SubscribableChannel),
    /// Server-pushed trading-engine status, generated automatically on a
    /// successful connection. Carries data but is **not subscribable**.
    Status,
    /// Server-pushed connection heartbeat. **Not subscribable** and
    /// payload-less.
    Heartbeat,
}

impl FromStr for Channel {
    type Err = strum::ParseError;

    /// Parse a v2 wire `channel` name. The system feeds are matched first
    /// (case-insensitively, mirroring [`SubscribableChannel`]); every other
    /// name delegates to [`SubscribableChannel`], keeping a single source
    /// of truth for the subscribable names.
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("status") {
            Ok(Self::Status)
        } else if s.eq_ignore_ascii_case("heartbeat") {
            Ok(Self::Heartbeat)
        } else {
            s.parse().map(Self::Subscribable)
        }
    }
}

// Allowed values per the `book`, `ohlc`, and `level3` subscribe schemas
// (docs.kraken.com/api/docs/websocket-v2/{book,ohlc,level3}).
const VALID_BOOK_DEPTHS: &[u32] = &[10, 25, 100, 500, 1000];
const VALID_OHLC_INTERVALS: &[u32] = &[1, 5, 15, 30, 60, 240, 1440, 10080, 21600];
const VALID_L3_DEPTHS: &[u32] = &[10, 100, 1000];

/// The "not one of the allowed values" error shared by the depth/interval
/// newtypes.
fn invalid_one_of(value: impl std::fmt::Display, valid: &[u32], label: &str) -> Error {
    Error::Validation(format!(
        "invalid {label} '{value}' (valid: {})",
        valid
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Parse `s` as a `u32`, mapping a non-numeric input onto the shared allow-list
/// error.
fn parse_u32(s: &str, valid: &[u32], label: &str) -> Result<u32> {
    s.parse().map_err(|_| invalid_one_of(s, valid, label))
}

/// Order-book depth for the `book` channel: one of {10, 25, 100, 500, 1000}.
/// Its validating [`TryFrom<u32>`] is the canonical constructor ([`FromStr`]
/// delegates), and it serializes transparently as the wire integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct BookDepth(u32);

/// Book depth when the caller gives none.
const DEFAULT_BOOK_DEPTH: u32 = 10;

impl BookDepth {
    /// The wire depth value.
    pub const fn depth(self) -> u32 {
        self.0
    }
}

impl Default for BookDepth {
    fn default() -> Self {
        Self(DEFAULT_BOOK_DEPTH)
    }
}

impl TryFrom<u32> for BookDepth {
    type Error = Error;
    fn try_from(depth: u32) -> Result<Self> {
        VALID_BOOK_DEPTHS
            .contains(&depth)
            .then_some(Self(depth))
            .ok_or_else(|| invalid_one_of(depth, VALID_BOOK_DEPTHS, "book depth"))
    }
}

impl std::str::FromStr for BookDepth {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        parse_u32(s, VALID_BOOK_DEPTHS, "book depth")?.try_into()
    }
}

impl std::fmt::Display for BookDepth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Candle interval in minutes for the `ohlc` channel: one of the fixed Kraken
/// set. Its validating [`TryFrom<u32>`] is the canonical constructor
/// ([`FromStr`] delegates).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct OhlcInterval(u32);

/// Candle interval (minutes) when the caller gives none.
const DEFAULT_OHLC_INTERVAL: u32 = 1;

impl OhlcInterval {
    /// The wire interval in minutes.
    pub const fn minutes(self) -> u32 {
        self.0
    }
}

impl Default for OhlcInterval {
    fn default() -> Self {
        Self(DEFAULT_OHLC_INTERVAL)
    }
}

impl TryFrom<u32> for OhlcInterval {
    type Error = Error;
    fn try_from(minutes: u32) -> Result<Self> {
        VALID_OHLC_INTERVALS
            .contains(&minutes)
            .then_some(Self(minutes))
            .ok_or_else(|| invalid_one_of(minutes, VALID_OHLC_INTERVALS, "OHLC interval"))
    }
}

impl std::str::FromStr for OhlcInterval {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        parse_u32(s, VALID_OHLC_INTERVALS, "OHLC interval")?.try_into()
    }
}

impl std::fmt::Display for OhlcInterval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Price levels per side for the `level3` channel: one of {10, 100, 1000}.
/// Optional on the wire (absent means the server default), so unlike
/// [`BookDepth`] it has no `Default`. Its validating [`TryFrom<u32>`] is the
/// canonical constructor ([`FromStr`] delegates).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct Level3Depth(u32);

impl Level3Depth {
    /// The wire depth value.
    pub const fn depth(self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for Level3Depth {
    type Error = Error;
    fn try_from(depth: u32) -> Result<Self> {
        VALID_L3_DEPTHS
            .contains(&depth)
            .then_some(Self(depth))
            .ok_or_else(|| invalid_one_of(depth, VALID_L3_DEPTHS, "level3 depth"))
    }
}

impl std::str::FromStr for Level3Depth {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        parse_u32(s, VALID_L3_DEPTHS, "level3 depth")?.try_into()
    }
}

impl std::fmt::Display for Level3Depth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<BookDepth> for u32 {
    fn from(depth: BookDepth) -> Self {
        depth.0
    }
}

impl From<OhlcInterval> for u32 {
    fn from(depth: OhlcInterval) -> Self {
        depth.0
    }
}

impl From<Level3Depth> for u32 {
    fn from(depth: Level3Depth) -> Self {
        depth.0
    }
}

/// The schema for a validated wire integer: the same allow-list its
/// `TryFrom<u32>` enforces, so a tool schema can never offer a value the
/// canonical constructor would reject. Written by hand because
/// `#[serde(try_from = "u32")]` hides the newtype from the derive.
#[cfg(feature = "schema")]
fn allow_list_schema(valid: &[u32]) -> schemars::Schema {
    schemars::json_schema!({
        "type": "integer",
        "enum": valid,
    })
}

/// Generates a [`schemars::JsonSchema`] impl over one of the allow-lists above.
#[cfg(feature = "schema")]
macro_rules! allow_list_json_schema {
    ($newtype:ident from $valid:ident) => {
        impl schemars::JsonSchema for $newtype {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                stringify!($newtype).into()
            }

            /// Small enough that a `$defs` indirection costs more than it saves.
            fn inline_schema() -> bool {
                true
            }

            fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
                allow_list_schema($valid)
            }
        }
    };
}

#[cfg(feature = "schema")]
allow_list_json_schema!(BookDepth from VALID_BOOK_DEPTHS);
#[cfg(feature = "schema")]
allow_list_json_schema!(OhlcInterval from VALID_OHLC_INTERVALS);
#[cfg(feature = "schema")]
allow_list_json_schema!(Level3Depth from VALID_L3_DEPTHS);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "clap")]
    fn subscribable_cli_values_parse_aliases_case_insensitively() {
        use clap::ValueEnum;
        let parse = |s| <SubscribableChannel as ValueEnum>::from_str(s, true);
        assert_eq!(parse("trades").unwrap(), SubscribableChannel::Trade);
        assert_eq!(
            parse("EXECUTIONS").unwrap(),
            SubscribableChannel::Executions
        );
        assert_eq!(parse("l3").unwrap(), SubscribableChannel::Level3);
        assert!(parse("nope").is_err());
    }

    #[test]
    #[cfg(feature = "clap")]
    fn system_feeds_are_not_subscribable_cli_values() {
        use clap::ValueEnum;
        let parse = |s| <SubscribableChannel as ValueEnum>::from_str(s, true);
        assert!(parse("status").is_err());
        assert!(parse("heartbeat").is_err());
    }

    #[test]
    fn channel_parses_every_wire_value_including_system_feeds() {
        assert_eq!(
            "trade".parse::<Channel>().unwrap(),
            Channel::Subscribable(SubscribableChannel::Trade)
        );
        assert_eq!("status".parse::<Channel>().unwrap(), Channel::Status);
        assert_eq!("heartbeat".parse::<Channel>().unwrap(), Channel::Heartbeat);
        assert!("nope".parse::<Channel>().is_err());
    }

    #[test]
    fn subscribable_endpoint_partitions_sockets() {
        assert_eq!(SubscribableChannel::Ticker.endpoint(), Endpoint::Public);
        assert_eq!(SubscribableChannel::Executions.endpoint(), Endpoint::Auth);
        assert_eq!(SubscribableChannel::Level3.endpoint(), Endpoint::L3);
    }

    #[test]
    fn subscribable_widens_to_the_matching_channel() {
        for (sub, name) in [
            (SubscribableChannel::Ticker, "ticker"),
            (SubscribableChannel::Trade, "trade"),
            (SubscribableChannel::Level3, "level3"),
            (SubscribableChannel::Executions, "executions"),
        ] {
            let channel: Channel = sub.into();
            assert_eq!(channel.to_string(), name);
            assert_eq!(sub.to_string(), name);
        }
    }

    #[test]
    fn book_depth_validates_its_allowed_set() {
        assert_eq!(BookDepth::default().0, 10);
        assert!("25".parse::<BookDepth>().is_ok());
        assert!("7".parse::<BookDepth>().is_err());
    }

    #[test]
    fn ohlc_interval_and_l3_depth_validate_their_allowed_sets() {
        assert!("60".parse::<OhlcInterval>().is_ok());
        assert!("2".parse::<OhlcInterval>().is_err());
        assert!("100".parse::<Level3Depth>().is_ok());
        assert!("25".parse::<Level3Depth>().is_err());
    }

    #[test]
    fn book_depth_try_from_accepts_exactly_the_allowed_set() {
        for &depth in VALID_BOOK_DEPTHS {
            assert_eq!(BookDepth::try_from(depth).unwrap().depth(), depth);
        }
        assert!(BookDepth::try_from(0).is_err());
        assert!(BookDepth::try_from(7).is_err());
        assert_eq!(BookDepth::default().to_string(), "10");
    }

    #[test]
    fn ohlc_interval_try_from_accepts_exactly_the_allowed_set() {
        for &minutes in VALID_OHLC_INTERVALS {
            assert_eq!(OhlcInterval::try_from(minutes).unwrap().minutes(), minutes);
        }
        assert!(OhlcInterval::try_from(0).is_err());
        assert!(OhlcInterval::try_from(2).is_err());
        assert_eq!(OhlcInterval::default().to_string(), "1");
    }

    #[test]
    fn level3_depth_try_from_accepts_exactly_the_allowed_set() {
        for &depth in VALID_L3_DEPTHS {
            assert_eq!(Level3Depth::try_from(depth).unwrap().depth(), depth);
        }
        assert!(Level3Depth::try_from(0).is_err());
        assert!(Level3Depth::try_from(25).is_err());
        assert_eq!(Level3Depth::try_from(1000).unwrap().to_string(), "1000");
    }
}

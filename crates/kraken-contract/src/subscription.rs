//! The shared subscribe surface of the multiplexed streaming commands
//! (`record`, `session start` and `streamd start`).

use kraken_core::{BookDepth, EventTrigger, OhlcInterval, Users};

/// Per-channel subscribe options. Each applies only to its own channel and is
/// ignored unless that channel is requested (e.g. `event_trigger` without
/// `ticker`).
// The union of every channel's options; each field is read only by its `route`
// arm. `Default` reproduces clap's `default_value`s (Kraken's WS v2 defaults),
// so tests build the common case and the always-sent depth and interval match
// omitting them.
#[derive(
    Debug, Clone, Default, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(default)]
pub struct SubscriptionOptions {
    /// Book depth for the `book` channel (10, 25, 100, 500, 1000).
    #[arg(long, default_value = "10")]
    pub book_depth: BookDepth,
    /// Candle interval in minutes for the `ohlc` channel
    /// (1, 5, 15, 30, 60, 240, 1440, 10080, 21600).
    #[arg(long, default_value = "1")]
    pub ohlc_interval: OhlcInterval,
    /// `ticker` event trigger: `trades` (default) or `bbo`. `bbo` fires on
    /// every best-bid/offer change, not just trades — far busier on quiet
    /// venues.
    #[arg(long)]
    pub event_trigger: Option<EventTrigger>,
    /// Request an initial snapshot on subscribe, for the channels that carry
    /// one (`ticker`, `trade`, `book`, `ohlc`, `instrument`, `balances`);
    /// unset uses the server default. (`executions` snapshots via
    /// `--snap-trades`/`--snap-orders`.)
    #[arg(long)]
    pub snapshot: Option<bool>,
    /// `executions`: include last 50 trade fills in snapshot.
    #[arg(long)]
    pub snap_trades: Option<bool>,
    /// `executions`: include open orders in snapshot (default: true).
    #[arg(long)]
    pub snap_orders: Option<bool>,
    /// `executions`: stream all status transitions (default: true). When false,
    /// only open/close.
    #[arg(long)]
    pub order_status: Option<bool>,
    /// `executions`/`balances`: display xstocks in terms of underlying equity
    /// (default: true).
    #[arg(long)]
    pub rebased: Option<bool>,
    /// `executions`: include the rate-limit counter in the stream.
    #[arg(long)]
    pub ratecounter: Option<bool>,
    /// `executions`/`balances`: stream events for master and subaccounts (pass
    /// "all").
    #[arg(long)]
    pub users: Option<Users>,
}

/// CLI `--help` footer shared by `streamd`/`record`, surfacing the
/// [`SubscriptionOptions`] trade-off to the user where the flags are set: the
/// flag set is the union over channels, so a channel-specific option is ignored
/// unless its channel is requested.
pub const CHANNEL_OPTIONS_NOTE: &str =
    "Channel-specific options (--event-trigger, --book-depth, --ohlc-interval, --snap-trades, \
     --snap-orders, --order-status, --ratecounter, --rebased, --users) apply only to their \
     channel and are ignored unless that channel is passed to --channels.";

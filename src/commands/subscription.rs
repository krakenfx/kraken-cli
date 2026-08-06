//! The shared subscribe surface of the multiplexed streaming commands (`streamd`,
//! `record`, `run start`): one flat flag set ([`SubscriptionOptions`]) planned by
//! [`plan_subscriptions`] into the typed [`WsSubscription`]s the
//! [`stream::run`](crate::stream::run) driver consumes.
//!
//! `kraken ws` does not route through here — it exposes a subcommand per channel and
//! builds its single subscription directly.

use std::hash::Hash;

use itertools::Itertools;
use kraken_core::{
    BookDepth, EventTrigger, OhlcInterval, SubscribableChannel, Users, WsSubscription,
};

use crate::errors::{KrakenError, Result};

/// CLI `--help` footer shared by `streamd`/`record`, surfacing the [`SubscriptionOptions`]
/// trade-off to the user where the flags are set: the flag set is the union over channels,
/// so a channel-specific option is ignored unless its channel is requested.
pub(crate) const CHANNEL_OPTIONS_NOTE: &str = "Channel-specific options (--event-trigger, --book-depth, --ohlc-interval, --snap-trades, \
     --snap-orders, --order-status, --ratecounter, --rebased, --users) apply only to their \
     channel and are ignored unless that channel is passed to --channels.";

/// Per-channel subscribe options shared by the `streamd`/`record` planner — the
/// streaming-side counterpart to the per-channel flags `kraken ws` exposes. Both
/// commands `#[command(flatten)]` it, so their tuning surface stays identical.
///
/// Because one command multiplexes many channels, this is the *union* of every
/// channel's options. Each field is read only by the [`route`] arm it belongs to, so
/// setting one for a channel you didn't request is silently ignored (e.g.
/// `--event-trigger` without `ticker`). `kraken ws` sidesteps this with a subcommand
/// per channel; the multiplexed planner trades that for a flat flag set.
///
/// `Default` reproduces clap's `default_value`s, which are themselves Kraken's WS v2
/// defaults (book depth 10, OHLC interval 1; `None` → server default for the rest), so
/// tests build the common case in one call. `book_depth`/`ohlc_interval` are always sent
/// but carry those server-default values, so the effect matches omitting them.
#[derive(Debug, Clone, Default, clap::Args)]
pub(crate) struct SubscriptionOptions {
    /// Book depth for the `book` channel (10, 25, 100, 500, 1000).
    #[arg(long, default_value = "10")]
    pub(crate) book_depth: BookDepth,
    /// Candle interval in minutes for the `ohlc` channel
    /// (1, 5, 15, 30, 60, 240, 1440, 10080, 21600).
    #[arg(long, default_value = "1")]
    pub(crate) ohlc_interval: OhlcInterval,
    /// `ticker` event trigger: `trades` (default) or `bbo`. `bbo` fires on every
    /// best-bid/offer change, not just trades — far busier on quiet venues.
    #[arg(long)]
    pub(crate) event_trigger: Option<EventTrigger>,
    /// Request an initial snapshot on subscribe, for the channels that carry one
    /// (`ticker`, `trade`, `book`, `ohlc`, `instrument`, `balances`); `None` uses the
    /// server default. (`executions` snapshots via `--snap-trades`/`--snap-orders`.)
    #[arg(long)]
    pub(crate) snapshot: Option<bool>,
    /// `executions`: include last 50 trade fills in snapshot.
    #[arg(long)]
    pub(crate) snap_trades: Option<bool>,
    /// `executions`: include open orders in snapshot (default: true).
    #[arg(long)]
    pub(crate) snap_orders: Option<bool>,
    /// `executions`: stream all status transitions (default: true). When false, only open/close.
    #[arg(long)]
    pub(crate) order_status: Option<bool>,
    /// `executions`/`balances`: display xstocks in terms of underlying equity (default: true).
    #[arg(long)]
    pub(crate) rebased: Option<bool>,
    /// `executions`: include the rate-limit counter in the stream.
    #[arg(long)]
    pub(crate) ratecounter: Option<bool>,
    /// `executions`/`balances`: stream events for master and subaccounts (pass "all").
    #[arg(long)]
    pub(crate) users: Option<Users>,
}

pub(crate) fn plan_subscriptions(
    symbols: &[String],
    channels: &[SubscribableChannel],
    options: &SubscriptionOptions,
) -> Result<Vec<WsSubscription>> {
    let symbols = normalize_symbols(symbols);
    let channels = dedupe_warning(channels, "channel");
    let requested_none = channels.is_empty();

    let mut subscriptions = Vec::new();
    for channel in channels {
        if let Some(params) = route(channel, &symbols, options)? {
            subscriptions.push(params);
        }
    }

    if subscriptions.is_empty() {
        // Nothing-requested and everything-skipped are different mistakes: naming level3 to
        // a user who never mentioned it sends their repair down the wrong path.
        return Err(KrakenError::Validation(if requested_none {
            "at least one --channels entry is required (ticker, trade, book, ohlc, ...)".into()
        } else {
            "no supported channels specified (level3 is not yet supported by the multi-channel stream)".into()
        }));
    }
    Ok(subscriptions)
}

fn dedupe_warning<T>(items: &[T], label: &str) -> Vec<T>
where
    T: Clone + Eq + Hash + std::fmt::Display,
{
    for dup in items.iter().duplicates() {
        tracing::warn!(label, %dup, "duplicate ignored");
    }
    items.iter().unique().cloned().collect()
}

fn normalize_symbols(raw: &[String]) -> Vec<String> {
    let trimmed: Vec<&str> = raw
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    dedupe_warning(&trimmed, "symbol")
        .into_iter()
        .map(str::to_owned)
        .collect()
}

fn route(
    channel: SubscribableChannel,
    symbols: &[String],
    options: &SubscriptionOptions,
) -> Result<Option<WsSubscription>> {
    let with_symbols = || -> Result<Vec<String>> {
        if symbols.is_empty() {
            return Err(KrakenError::Validation(format!(
                "channel '{channel}' requires at least one --symbols entry"
            )));
        }
        Ok(symbols.to_vec())
    };

    // Destructured without `..`: a newly added option won't compile until it's routed
    // to a channel here, rather than being silently dropped.
    let SubscriptionOptions {
        book_depth,
        ohlc_interval,
        event_trigger,
        snapshot,
        snap_trades,
        snap_orders,
        order_status,
        rebased,
        ratecounter,
        users,
    } = options;

    Ok(Some(match channel {
        SubscribableChannel::Ticker => WsSubscription::Ticker {
            symbol: with_symbols()?,
            event_trigger: *event_trigger,
            snapshot: *snapshot,
        },
        SubscribableChannel::Trade => WsSubscription::Trade {
            symbol: with_symbols()?,
            snapshot: *snapshot,
        },
        SubscribableChannel::Book => WsSubscription::Book {
            symbol: with_symbols()?,
            depth: *book_depth,
            snapshot: *snapshot,
        },
        SubscribableChannel::Ohlc => WsSubscription::Ohlc {
            symbol: with_symbols()?,
            interval: *ohlc_interval,
            snapshot: *snapshot,
        },
        SubscribableChannel::Instrument => WsSubscription::Instrument {
            symbol: symbols.to_vec(),
            snapshot: *snapshot,
        },
        SubscribableChannel::Executions => WsSubscription::Executions {
            snap_trades: *snap_trades,
            snap_orders: *snap_orders,
            order_status: *order_status,
            rebased: *rebased,
            ratecounter: *ratecounter,
            users: *users,
        },
        SubscribableChannel::Balances => WsSubscription::Balances {
            snapshot: *snapshot,
            rebased: *rebased,
            users: *users,
        },
        SubscribableChannel::Level3 => {
            tracing::warn!(
                "channel 'level3' is not supported by the multi-channel stream yet; use `kraken ws level3` — skipping"
            );
            return Ok(None);
        }
    }))
}

//! The shared subscribe surface of the multiplexed streaming commands
//! (`streamd`, `record`, `run start`): one flat flag set
//! ([`SubscriptionOptions`]) planned by [`plan_subscriptions`] into the typed
//! [`WsSubscription`]s the [`stream::run`](crate::stream::run) driver consumes.
//!
//! `kraken ws` does not route through here — it exposes a subcommand per
//! channel and builds its single subscription directly.

use crate::errors::{KrakenError, Result};
use itertools::Itertools;
use kraken_contract::subscription::SubscriptionOptions;
use kraken_core::{SubscribableChannel, WsSubscription};
use std::hash::Hash;

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
        // Nothing-requested and everything-skipped are different mistakes: naming
        // level3 to a user who never mentioned it sends their repair down the
        // wrong path.
        return Err(KrakenError::Validation(if requested_none {
            "at least one --channels entry is required (ticker, trade, book, ohlc, ...)".into()
        } else {
            "no supported channels specified (level3 is not yet supported by the multi-channel \
             stream)"
                .into()
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

    // Destructured without `..`: a newly added option won't compile until it's
    // routed to a channel here, rather than being silently dropped.
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
                "channel 'level3' is not supported by the multi-channel stream yet; use `kraken \
                 ws level3` — skipping"
            );
            return Ok(None);
        }
    }))
}

//! `kraken streamd`: one long-lived process that follows many channels across
//! many symbols over a single public socket, plus a second socket for the
//! authenticated executions/balances feed.
//!
//! [`execute`] plans the flat flag surface into typed subscriptions
//! ([`plan_subscriptions`]) — so a bad channel or missing symbol fails before
//! any socket opens — then streams them through the shared
//! [`stream::run`](crate::stream::run) driver into the
//! [`StdoutSink`].

use crate::{
    cli::AppContext, commands::subscription::plan_subscriptions, errors::Result,
    sink::stdout::StdoutSink,
};
use kraken_contract::streamd::{Start, StreamdCommand};

/// Run `kraken streamd`: plan the subscriptions, then stream until shutdown. A
/// market channel given without symbols is rejected as `validation`; an
/// unsupported channel (`level3`) is warned about and skipped, and only if
/// *every* requested channel was skipped — leaving nothing to stream — is that
/// a hard error. Socket drops are retried by the engine indefinitely (paced
/// backoff); the session ends on shutdown, a fatal sink error, or every
/// subscription being rejected.
pub(crate) async fn execute(cmd: &StreamdCommand, ctx: &AppContext) -> Result<()> {
    let StreamdCommand::Start(Start {
        symbols,
        channels,
        options,
    }) = cmd;
    let subs = plan_subscriptions(symbols, channels, options)?;
    crate::stream::run(subs, None, None, StdoutSink::new(ctx.format), None, ctx).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::KrakenError;
    use kraken_contract::subscription::SubscriptionOptions;
    use kraken_core::{SubscribableChannel, WsSubscription, endpoint::Endpoint};

    /// Plan a command's subscriptions the way [`execute`] does, without a
    /// socket.
    fn plan(cmd: &StreamdCommand) -> Result<Vec<WsSubscription>> {
        let StreamdCommand::Start(Start {
            symbols,
            channels,
            options,
        }) = cmd;
        plan_subscriptions(symbols, channels, options)
    }

    fn subscriptions(cmd: &StreamdCommand) -> Vec<WsSubscription> {
        plan(cmd).expect("expected a streaming plan")
    }

    /// How many of `subs` ride a given endpoint.
    fn endpoint_count(subs: &[WsSubscription], endpoint: Endpoint) -> usize {
        subs.iter().filter(|s| s.endpoint() == endpoint).count()
    }

    fn start(symbols: &[&str], channels: &[SubscribableChannel]) -> StreamdCommand {
        StreamdCommand::Start(Start {
            symbols: symbols.iter().map(|s| (*s).to_string()).collect(),
            channels: channels.to_vec(),
            options: SubscriptionOptions::default(),
        })
    }

    #[test]
    fn plan_partitions_public_and_private_channels() {
        let subs = subscriptions(&start(
            &["BTC/USD", "ETH/USD"],
            &[
                SubscribableChannel::Ticker,
                SubscribableChannel::Trade,
                SubscribableChannel::Book,
                SubscribableChannel::Executions,
            ],
        ));
        assert_eq!(endpoint_count(&subs, Endpoint::Public), 3);
        assert_eq!(endpoint_count(&subs, Endpoint::Auth), 1);
    }

    #[test]
    fn plan_dedupes_repeated_symbols_preserving_order() {
        let subs = subscriptions(&start(
            &["BTC/USD", "BTC/USD", "ETH/USD"],
            &[SubscribableChannel::Ticker],
        ));
        assert_eq!(subs[0].symbols(), ["BTC/USD", "ETH/USD"]);
    }

    #[test]
    fn empty_channels_are_rejected() {
        let err = plan(&start(&["BTC/USD"], &[])).unwrap_err();
        assert!(matches!(err, KrakenError::Validation(_)));
    }

    #[test]
    fn market_channel_without_symbols_is_rejected() {
        let err = plan(&start(&[], &[SubscribableChannel::Ticker])).unwrap_err();
        assert!(matches!(err, KrakenError::Validation(_)));
    }

    #[test]
    fn executions_without_symbols_is_allowed() {
        let subs = subscriptions(&start(&[], &[SubscribableChannel::Executions]));
        assert_eq!(endpoint_count(&subs, Endpoint::Auth), 1);
        assert_eq!(endpoint_count(&subs, Endpoint::Public), 0);
    }

    #[test]
    fn level3_alone_leaves_nothing_to_stream() {
        // level3 is warned about and skipped; with no other channels nothing is left
        // to stream, which is a hard error rather than a silent no-op stream.
        let err = plan(&start(&["BTC/USD"], &[SubscribableChannel::Level3])).unwrap_err();
        assert!(matches!(err, KrakenError::Validation(_)));
    }

    #[test]
    fn level3_is_skipped_not_fatal_alongside_supported_channels() {
        // level3 isn't wired into streamd yet, so it's dropped (with a stderr
        // warning) instead of aborting the whole stream; the rest survive.
        let subs = subscriptions(&start(
            &["BTC/USD"],
            &[SubscribableChannel::Ticker, SubscribableChannel::Level3],
        ));
        assert_eq!(endpoint_count(&subs, Endpoint::Public), 1);
        assert_eq!(endpoint_count(&subs, Endpoint::Auth), 0);
    }
}

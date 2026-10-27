//! The streaming client — the stream-side sibling of
//! [`RequestClient`](crate::RequestClient).
//!
//! [`StreamClient::connect`] plans one connection per [`Endpoint`], mints each
//! private endpoint's token once (failing fast on a bad key), spawns one
//! self-reconnecting actor per endpoint, and returns a control handle plus the
//! [`EventStream`] the actors fan into. Shutdown is drop-triggered from either
//! end: dropping the handle cancels every actor; dropping the stream fails
//! their next send and they wind down.

use super::{
    backoff::ReconnectPolicy,
    config::WsConfig,
    connection::{Event, Stream, StreamConnection},
};
use kraken_core::{
    WsSubscription,
    endpoint::Endpoint,
    error::{Error, Result},
};
use std::num::NonZeroU64;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_util::sync::{CancellationToken, DropGuard};

/// Which endpoints to open for a set of subscriptions, given the credential
/// state.
struct EndpointPlan {
    /// `(endpoint, its subscriptions)` per endpoint to open, in first-seen
    /// order.
    open: Vec<(Endpoint, Vec<WsSubscription>)>,
    /// Subscriptions dropped because no credentials are configured.
    skipped: Vec<WsSubscription>,
}

/// Group `subs` by endpoint, applying the no-credentials policy. Pure, so the
/// partition/skip policy is testable without a socket or token.
fn plan_endpoints(subs: Vec<WsSubscription>, have_token: bool) -> EndpointPlan {
    let mut open: Vec<(Endpoint, Vec<WsSubscription>)> = Vec::new();
    let mut skipped: Vec<WsSubscription> = Vec::new();
    for sub in subs {
        if sub.requires_token() && !have_token {
            skipped.push(sub);
            continue;
        }
        let endpoint = sub.endpoint();
        match open.iter_mut().find(|(e, _)| *e == endpoint) {
            Some((_, group)) => group.push(sub),
            None => open.push((endpoint, vec![sub])),
        }
    }
    EndpointPlan { open, skipped }
}

/// A handle to the streaming connections for one session. Dropping it shuts
/// every connection down; the event data flows through the separate
/// [`EventStream`].
#[must_use = "dropping the StreamClient handle shuts every connection down"]
#[derive(Debug)]
pub struct StreamClient {
    /// Cancels every spawned actor when the handle is dropped.
    _guard: DropGuard,
    skipped_for_credentials: usize,
}

/// The decoded [`Event`]s from every connection actor, fanned into one
/// sequence. It buffers from the moment the actors spawn, so no early event is
/// lost before the caller starts draining; dropping it signals every actor to
/// wind down.
pub type EventStream = UnboundedReceiverStream<Event>;

impl StreamClient {
    /// Open the streaming connections for `subs`, returning the control handle
    /// and the [`EventStream`]. Private endpoints are skipped (with a
    /// warning) when no credentials are configured; each surviving private
    /// endpoint's token is minted up front so a bad key fails now rather
    /// than in a forever-reconnecting actor. `req_id` is the caller's
    /// `--req-id`, echoed on the subscribe ack.
    ///
    /// # Errors
    /// - [`Error::NoTokenSource`]: every requested channel is private and no
    ///   credentials are configured (fail closed).
    /// - [`Error::TokenRejected`] / [`Error::MintFailed`]: the up-front token
    ///   mint failed.
    pub async fn connect(
        subs: Vec<WsSubscription>,
        req_id: Option<NonZeroU64>,
        cfg: &WsConfig,
        policy: ReconnectPolicy,
    ) -> Result<(Self, EventStream)> {
        let token_cache = cfg.new_token_cache();
        let plan = plan_endpoints(subs, token_cache.is_some());
        for sub in &plan.skipped {
            tracing::warn!(
                channel = %sub.channel(),
                endpoint = %sub.endpoint(),
                "skipping authenticated subscription: credentials required but none configured"
            );
        }
        if plan.open.is_empty() {
            // Fail closed: an auth error, not a silent empty capture. Empty `subs` is
            // guarded upstream, so the `None` arm is defensive.
            return match plan.skipped.first() {
                Some(sub) => Err(Error::NoTokenSource(sub.endpoint())),
                None => Err(Error::Validation("no subscriptions to stream".into())),
            };
        }
        let skipped_for_credentials = plan.skipped.len();

        // Up-front mint (the cache bounds it): a bad key or wedged REST endpoint fails
        // the command now rather than in a forever-reconnecting actor.
        if let Some(cache) = token_cache
            .as_ref()
            .filter(|_| plan.open.iter().any(|(e, _)| e.requires_token()))
        {
            cache.get().await?;
        }

        // Unbounded fan-in so a burst never back-pressures an actor's read loop; the
        // receiver buffers events emitted before the caller drains.
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let shutdown = CancellationToken::new();
        for (endpoint, endpoint_subs) in plan.open {
            let connection = StreamConnection {
                endpoint,
                transport: cfg.transport(endpoint),
                token_cache: token_cache.clone().filter(|_| endpoint.requires_token()),
                events_tx: events_tx.clone(),
                shutdown: shutdown.child_token(),
                mode: Stream {
                    policy,
                    subs: endpoint_subs,
                    req_id,
                },
            };
            tokio::spawn(connection.run());
        }
        // Only the actors' clones remain, so the stream ends when the last actor stops.
        drop(events_tx);
        Ok((
            Self {
                _guard: shutdown.drop_guard(),
                skipped_for_credentials,
            },
            EventStream::new(events_rx),
        ))
    }

    /// How many requested subscriptions the planner dropped for missing
    /// credentials — reported so the caller never re-derives the skip
    /// policy.
    pub fn skipped_for_credentials(&self) -> usize {
        self.skipped_for_credentials
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::{BoxFuture, Minted, TokenSource};
    use std::sync::Arc;

    fn ticker() -> WsSubscription {
        WsSubscription::Ticker {
            symbol: vec!["BTC/USD".into()],
            event_trigger: None,
            snapshot: None,
        }
    }

    fn balances() -> WsSubscription {
        WsSubscription::Balances {
            snapshot: None,
            rebased: None,
            users: None,
        }
    }

    #[test]
    fn plan_keeps_public_and_skips_private_without_credentials() {
        let plan = plan_endpoints(vec![ticker(), balances()], false);
        assert_eq!(plan.open.len(), 1);
        assert_eq!(plan.open[0].0, Endpoint::Public);
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].endpoint(), Endpoint::Auth);
    }

    #[test]
    fn plan_opens_private_endpoints_when_credentials_are_present() {
        let plan = plan_endpoints(vec![ticker(), balances()], true);
        let opened: Vec<Endpoint> = plan.open.iter().map(|(e, _)| *e).collect();
        assert_eq!(opened, vec![Endpoint::Public, Endpoint::Auth]);
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn plan_groups_subscriptions_that_share_an_endpoint() {
        let plan = plan_endpoints(vec![ticker(), ticker()], false);
        assert_eq!(plan.open.len(), 1);
        assert_eq!(plan.open[0].1.len(), 2);
    }

    /// A token source whose mint always fails, for the bad-key path.
    struct FailingToken;

    impl TokenSource for FailingToken {
        fn fetch(&self) -> BoxFuture<Result<Minted>> {
            Box::pin(async { Err(Error::TokenRejected("EAPI:Invalid key".into())) })
        }
    }

    #[tokio::test]
    async fn connect_fails_fast_when_a_private_token_mint_fails() {
        let cfg = WsConfig {
            token_source: Some(Arc::new(FailingToken)),
            ..WsConfig::default()
        };
        let result =
            StreamClient::connect(vec![balances()], None, &cfg, ReconnectPolicy::default()).await;
        assert!(matches!(result, Err(Error::TokenRejected(_))));
    }

    #[tokio::test]
    async fn connect_fails_closed_for_an_all_private_unauthenticated_stream() {
        let cfg = WsConfig::default();
        let result =
            StreamClient::connect(vec![balances()], None, &cfg, ReconnectPolicy::default()).await;
        assert!(matches!(result, Err(Error::NoTokenSource(Endpoint::Auth))));
    }
}

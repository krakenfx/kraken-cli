//! WebSocket authentication tokens: private frames carry a short-lived,
//! connection-scoped `token` inside `params`. Retrieval sits behind [`TokenSource`]
//! (implemented by the application, keeping this crate REST-free); [`TokenCache`]
//! caches the current value and single-flights concurrent fetches.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use kraken_core::error::{Error, Result};
use tokio::sync::Mutex;

/// Budget for one token mint, so a wedged REST endpoint fails the command instead of
/// stalling it.
const TOKEN_TIMEOUT: Duration = Duration::from_secs(10);

/// Cache freshness when the source states no lifetime (Kraken invalidates an unused
/// token ~15 minutes after minting).
const FALLBACK_TTL: Duration = Duration::from_secs(10 * 60);

/// Shaved off a server-stated lifetime: slack between serving a token and the dial
/// that uses it.
const EXPIRY_MARGIN: Duration = Duration::from_secs(60);

/// Cap on any cached lifetime: `fresh_until` is `now + ttl`, and a nonsensical
/// venue-stated `expires_in` must clamp here instead of overflowing that Add.
const MAX_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// The object-safe return type for [`TokenSource::fetch`], so the client can hold an
/// `Arc<dyn TokenSource>` without being generic over it.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// A freshly minted token, with the server-stated lifetime when the source knows it
/// (Kraken reports `expires` alongside the token).
pub struct Minted {
    pub token: String,
    /// `None`: the cache falls back to a conservative client-side lifetime.
    pub expires_in: Option<Duration>,
}

/// Hand-written: the token is a live credential and must never reach a log via
/// `{:?}`.
impl std::fmt::Debug for Minted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Minted")
            .field("token", &"[redacted]")
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

impl From<String> for Minted {
    fn from(token: String) -> Self {
        Self {
            token,
            expires_in: None,
        }
    }
}

impl From<&str> for Minted {
    fn from(token: &str) -> Self {
        Self::from(token.to_string())
    }
}

/// Supplies authentication tokens (Kraken: the REST `GetWebSocketsToken` call).
/// Object-safe so the client stays non-generic; the live impl is the application wiring's.
pub trait TokenSource: Send + Sync {
    /// Fetch a fresh token: on first private use and on every reconnect. A rejected
    /// mint (a credentials problem) must map to `Error::TokenRejected`; any other
    /// failure to `Error::MintFailed`, carrying the original error whole.
    fn fetch(&self) -> BoxFuture<Result<Minted>>;
}

/// A cached token, servable until `fresh_until` — an absolute `tokio::time::Instant`,
/// so freshness is testable under paused virtual time.
struct CachedToken {
    token: String,
    fresh_until: tokio::time::Instant,
}

impl CachedToken {
    fn is_fresh(&self) -> bool {
        tokio::time::Instant::now() < self.fresh_until
    }
}

/// Caches the current token — served only while fresh — and single-flights concurrent
/// fetches: the fetch runs while the async lock is held, so a cold-cache burst makes
/// one upstream request.
pub(crate) struct TokenCache {
    source: Arc<dyn TokenSource>,
    cached: Mutex<Option<CachedToken>>,
}

impl TokenCache {
    /// Wrap a [`TokenSource`], starting with an empty cache.
    pub(crate) fn new(source: Arc<dyn TokenSource>) -> Self {
        Self {
            source,
            cached: Mutex::new(None),
        }
    }

    /// The cached token, fetching one if the cache is empty or stale. The mint is
    /// bounded by [`TOKEN_TIMEOUT`].
    pub(crate) async fn get(&self) -> Result<String> {
        let mut slot = self.cached.lock().await;
        if let Some(cached) = slot.as_ref()
            && cached.is_fresh()
        {
            return Ok(cached.token.clone());
        }
        self.fetch_bounded(&mut slot).await
    }

    /// Force a new fetch and return *this* mint (a later `get()` could observe a
    /// sibling's concurrent refresh). A failed fetch leaves the cache unchanged.
    pub(crate) async fn refresh(&self) -> Result<String> {
        let mut slot = self.cached.lock().await;
        self.fetch_bounded(&mut slot).await
    }

    /// Drop the cached token: the venue rejected it, so it is dead whatever its age.
    /// May discard a sibling's concurrent fresh mint — one extra mint on the next
    /// `get()`, never a wrong token.
    pub(crate) async fn invalidate(&self) {
        *self.cached.lock().await = None;
    }

    /// Mint within the [`TOKEN_TIMEOUT`] budget and cache the result. Runs under the
    /// single-flight lock its caller holds, so a wedged fetch releases the lock at the
    /// bound instead of never.
    async fn fetch_bounded(&self, slot: &mut Option<CachedToken>) -> Result<String> {
        let minted = tokio::time::timeout(TOKEN_TIMEOUT, self.source.fetch())
            .await
            .map_err(|_| Error::MintTimeout(TOKEN_TIMEOUT))??;
        let ttl = minted
            .expires_in
            .map_or(FALLBACK_TTL, |lifetime| {
                lifetime.saturating_sub(EXPIRY_MARGIN)
            })
            .min(MAX_TTL);
        *slot = Some(CachedToken {
            token: minted.token.clone(),
            fresh_until: tokio::time::Instant::now() + ttl,
        });
        Ok(minted.token)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use kraken_core::error::Error;

    use super::*;

    /// A [`TokenSource`] whose fetch never resolves, for the mint-timeout path.
    struct StalledSource;

    impl TokenSource for StalledSource {
        fn fetch(&self) -> BoxFuture<Result<Minted>> {
            Box::pin(std::future::pending())
        }
    }

    /// A [`TokenSource`] that counts fetches and yields a distinct token each call
    /// (`tok-{n}`). The internal yield gives a concurrent caller a chance to observe a cold
    /// cache if the lock were ever released across the fetch, so the single-flight test can
    /// catch that regression; `fail` drives the refresh error path.
    #[derive(Default)]
    struct CountingSource {
        calls: AtomicUsize,
        fail: AtomicBool,
    }

    impl TokenSource for CountingSource {
        fn fetch(&self) -> BoxFuture<Result<Minted>> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            let fail = self.fail.load(Ordering::SeqCst);
            Box::pin(async move {
                tokio::task::yield_now().await;
                if fail {
                    Err(Error::TokenRejected("mint failed".into()))
                } else {
                    Ok(format!("tok-{n}").into())
                }
            })
        }
    }

    /// Yields `tok-{n}` with a server-stated lifetime.
    struct ExpiringSource {
        calls: AtomicUsize,
        expires_in: Duration,
    }

    impl TokenSource for ExpiringSource {
        fn fetch(&self) -> BoxFuture<Result<Minted>> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            let expires_in = self.expires_in;
            Box::pin(async move {
                Ok(Minted {
                    token: format!("tok-{n}"),
                    expires_in: Some(expires_in),
                })
            })
        }
    }

    #[tokio::test]
    async fn get_caches_after_the_first_fetch() {
        let src = Arc::new(CountingSource::default());
        let cache = TokenCache::new(src.clone());
        assert_eq!(cache.get().await.unwrap(), "tok-0");
        assert_eq!(cache.get().await.unwrap(), "tok-0");
        assert_eq!(
            src.calls.load(Ordering::SeqCst),
            1,
            "second get is served from cache"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_cold_get_makes_a_single_fetch() {
        let src = Arc::new(CountingSource::default());
        let cache = Arc::new(TokenCache::new(src.clone()));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let cache = cache.clone();
                tokio::spawn(async move { cache.get().await })
            })
            .collect();
        for handle in handles {
            assert_eq!(handle.await.unwrap().unwrap(), "tok-0");
        }
        assert_eq!(src.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn refresh_replaces_the_cached_token() {
        let src = Arc::new(CountingSource::default());
        let cache = TokenCache::new(src.clone());
        assert_eq!(cache.get().await.unwrap(), "tok-0");
        assert_eq!(
            cache.refresh().await.unwrap(),
            "tok-1",
            "refresh returns the token it just minted"
        );
        assert_eq!(
            cache.get().await.unwrap(),
            "tok-1",
            "get serves the refreshed value"
        );
        assert_eq!(
            src.calls.load(Ordering::SeqCst),
            2,
            "get after refresh does not refetch"
        );
    }

    #[tokio::test]
    async fn a_failed_refresh_leaves_the_cached_token_intact() {
        let src = Arc::new(CountingSource::default());
        let cache = TokenCache::new(src.clone());
        assert_eq!(cache.get().await.unwrap(), "tok-0");
        src.fail.store(true, Ordering::SeqCst);
        assert!(cache.refresh().await.is_err());
        src.fail.store(false, Ordering::SeqCst);
        assert_eq!(
            cache.get().await.unwrap(),
            "tok-0",
            "the cache survived the failed refresh"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_stale_cached_token_is_re_minted_not_served() {
        // A token unused past its venue lifetime is dead on arrival at a fresh
        // socket, and the one-shot path never re-mints — so the cache must never
        // serve it in the first place.
        let src = Arc::new(CountingSource::default());
        let cache = TokenCache::new(src.clone());
        assert_eq!(cache.get().await.unwrap(), "tok-0");
        tokio::time::advance(FALLBACK_TTL + Duration::from_secs(1)).await;
        assert_eq!(cache.get().await.unwrap(), "tok-1");
        assert_eq!(src.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn invalidate_forces_the_next_get_to_re_mint() {
        let src = Arc::new(CountingSource::default());
        let cache = TokenCache::new(src.clone());
        assert_eq!(cache.get().await.unwrap(), "tok-0");
        cache.invalidate().await;
        assert_eq!(
            cache.get().await.unwrap(),
            "tok-1",
            "a rejected token is never re-served"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_server_stated_lifetime_overrides_the_fallback() {
        let src = Arc::new(ExpiringSource {
            calls: AtomicUsize::new(0),
            expires_in: Duration::from_secs(120),
        });
        let cache = TokenCache::new(src.clone());
        assert_eq!(cache.get().await.unwrap(), "tok-0");
        tokio::time::advance(Duration::from_secs(59)).await;
        assert_eq!(
            cache.get().await.unwrap(),
            "tok-0",
            "fresh inside expires - margin"
        );
        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(
            cache.get().await.unwrap(),
            "tok-1",
            "stale past expires - margin"
        );
    }

    #[tokio::test]
    async fn an_absurd_server_lifetime_is_clamped_not_a_panic() {
        // `fresh_until` is `now + ttl`: unclamped, a nonsensical venue `expires_in`
        // overflows the Instant's range and panics.
        let src = Arc::new(ExpiringSource {
            calls: AtomicUsize::new(0),
            expires_in: Duration::MAX,
        });
        let cache = TokenCache::new(src);
        assert_eq!(cache.get().await.unwrap(), "tok-0");
    }

    #[tokio::test(start_paused = true)]
    async fn a_wedged_mint_fails_at_the_budget_instead_of_stalling() {
        let cache = TokenCache::new(Arc::new(StalledSource));
        let err = cache.get().await.unwrap_err();
        assert!(matches!(err, Error::MintTimeout(_)));
    }

    /// The bound covers only the fetch, not the lock wait: a waiter queued behind a
    /// wedged holder still errors once its own fetch hits the budget, rather than
    /// blocking on the shared cache forever.
    #[tokio::test(start_paused = true)]
    async fn a_waiter_behind_a_wedged_mint_is_bounded_too() {
        let cache = Arc::new(TokenCache::new(Arc::new(StalledSource)));
        let holder = tokio::spawn({
            let cache = cache.clone();
            async move { cache.get().await }
        });
        let waiter = tokio::spawn({
            let cache = cache.clone();
            async move { cache.get().await }
        });
        assert!(holder.await.unwrap().is_err());
        assert!(waiter.await.unwrap().is_err());
    }
}

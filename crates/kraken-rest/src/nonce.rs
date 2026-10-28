use crate::RestError;
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static LAST: AtomicU64 = AtomicU64::new(0);

/// A strictly increasing nonce in nanoseconds. Shared process-wide because Spot
/// and Futures reject a nonce at or below the key's last one, and milliseconds
/// collide under concurrent requests.
pub(crate) fn next() -> Result<u64, RestError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| RestError::Signing("system clock is before Unix epoch".into()))?;
    let now = u64::try_from(elapsed.as_nanos())
        .map_err(|_| RestError::Signing("system clock is past the nonce range".into()))?;
    let previous = LAST
        .try_update(Ordering::SeqCst, Ordering::SeqCst, |last| {
            Some(now.max(last.saturating_add(1)))
        })
        .unwrap_or_else(|last| last);
    Ok(now.max(previous.saturating_add(1)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn nonces_strictly_increase(count in 2usize..64) {
            let nonces: Vec<u64> = (0..count).map(|_| next().unwrap()).collect();
            prop_assert!(nonces.windows(2).all(|w| w[0] < w[1]));
        }
    }
}

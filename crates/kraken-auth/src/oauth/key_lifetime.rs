use crate::error::{Error, Result};
use chrono::{DateTime, TimeDelta, Utc};
use std::time::Duration;

/// How long a created key lives. Kraken enforces no ceiling, so this type does:
/// every key that this crate creates expires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyLifetime(TimeDelta);

impl KeyLifetime {
    pub const MAX_DAYS: i64 = 90;
    /// A key that expires within a minute is a typo, not a choice.
    const MIN: TimeDelta = TimeDelta::minutes(1);
    const MAX: TimeDelta = TimeDelta::days(Self::MAX_DAYS);

    pub fn expires_at(self, from: DateTime<Utc>) -> DateTime<Utc> {
        from + self.0
    }
}

/// The longest life, so a key outlives no more sign-ins than it must.
impl Default for KeyLifetime {
    fn default() -> Self {
        Self(Self::MAX)
    }
}

impl TryFrom<Duration> for KeyLifetime {
    type Error = Error;

    fn try_from(duration: Duration) -> Result<Self> {
        TimeDelta::from_std(duration)
            .ok()
            .filter(|lifetime| (Self::MIN..=Self::MAX).contains(lifetime))
            .map(Self)
            .ok_or(Error::LifetimeOutOfRange)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    #[test]
    fn the_bounds_are_inclusive() {
        assert!(KeyLifetime::try_from(secs(60)).is_ok());
        assert!(KeyLifetime::try_from(secs(90 * 24 * 3600)).is_ok());
    }

    #[test]
    fn a_lifetime_outside_the_bounds_is_refused() {
        for duration in [secs(0), secs(59), secs(90 * 24 * 3600 + 1), Duration::MAX] {
            assert!(
                matches!(
                    KeyLifetime::try_from(duration),
                    Err(Error::LifetimeOutOfRange)
                ),
                "{duration:?}"
            );
        }
    }

    #[test]
    fn the_default_is_the_longest_lifetime() {
        let now = Utc::now();
        assert_eq!(
            KeyLifetime::default().expires_at(now),
            now + TimeDelta::days(KeyLifetime::MAX_DAYS)
        );
    }
}

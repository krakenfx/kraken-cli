//! Reconnect pacing: exponential backoff with jitter (no lockstep thundering herd),
//! plus a stable-session reset so only a genuinely flapping socket escalates.
//! Deliberately no attempt cap — streaming retries until the operator interrupts.
//! Pure of I/O: it computes *how long*; the actor owns the interruptible sleep.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Multiplier applied to the delay after each consecutive failed attempt.
const BACKOFF_FACTOR: u32 = 2;

/// Tunable reconnect schedule. `Copy` so the [`StreamClient`](crate::StreamClient) can stamp one into
/// every connection it spawns.
#[derive(Debug, Clone, Copy)]
pub struct ReconnectPolicy {
    /// Delay before the first reconnect, and the base the schedule escalates from.
    pub initial: Duration,
    /// Upper bound on a single backoff delay (before jitter is added).
    pub max: Duration,
    /// Upper bound on the random jitter added to each delay.
    pub jitter_max: Duration,
    /// A session that stayed connected at least this long resets the schedule to `initial`.
    pub stable_after: Duration,
}

impl ReconnectPolicy {
    /// Whether a session of this length counts as healthy — the one stability notion
    /// shared by the backoff reset and the connection's rejection-incident tracking.
    pub(crate) fn is_stable(&self, session: Duration) -> bool {
        session >= self.stable_after
    }
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial: Duration::from_millis(250),
            max: Duration::from_secs(30),
            jitter_max: Duration::from_millis(750),
            stable_after: Duration::from_secs(60),
        }
    }
}

/// Mutable reconnect state for one connection. Not `Clone` — each actor owns exactly one.
pub(crate) struct Backoff {
    policy: ReconnectPolicy,
    /// The next delay's escalating base (before jitter): reset to `initial` after a stable
    /// session, multiplied by [`BACKOFF_FACTOR`] after each attempt.
    current: Duration,
}

impl Backoff {
    /// A fresh controller primed at `policy.initial`.
    ///
    /// A base above the cap means the caller wants that pacing: the effective `max` is
    /// raised to `initial` here — `ReconnectPolicy`'s pub fields make a validating
    /// constructor bypassable, so the invariant lives at the one funnel every policy
    /// passes through — rather than `next_delay` silently clamping `initial` back down.
    pub(crate) fn new(policy: ReconnectPolicy) -> Self {
        let policy = ReconnectPolicy {
            max: policy.max.max(policy.initial),
            ..policy
        };
        Self {
            policy,
            current: policy.initial,
        }
    }

    /// Record the just-ended session's duration. A session that lasted at least
    /// `stable_after` is healthy, so the schedule restarts from `initial`; a short one
    /// keeps the escalated delay, so a flapping socket backs off progressively.
    pub(crate) fn note_session(&mut self, session: Duration) {
        if self.policy.is_stable(session) {
            self.current = self.policy.initial;
        }
    }

    /// The delay before the next reconnect attempt, advancing the controller: escalates the
    /// base for the following attempt. `wait = min(base, max) + jitter`, computed with
    /// saturating arithmetic throughout so it can never overflow.
    pub(crate) fn next_delay(&mut self) -> Duration {
        let backoff = self
            .current
            .min(self.policy.max)
            .saturating_add(jitter(self.policy.jitter_max));

        // Escalate the base for the next attempt, capped at `max`.
        self.current = self
            .current
            .saturating_mul(BACKOFF_FACTOR)
            .min(self.policy.max);

        backoff
    }
}

/// A pseudo-random jitter in `[0, max]`, derived dependency-free from the clock's
/// sub-second nanos (the same approach the futures WS engine uses). De-correlates
/// reconnects across many clients without pulling in an RNG crate.
fn jitter(max: Duration) -> Duration {
    if max.is_zero() {
        return Duration::ZERO;
    }
    let max_ms = u64::try_from(max.as_millis()).unwrap_or(u64::MAX).max(1);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos()))
        .unwrap_or(0);
    // Saturating: at a (nonsensical) u64::MAX-ms jitter cap, `+ 1` would overflow —
    // a debug panic or a release remainder-by-zero.
    Duration::from_millis(nanos % max_ms.saturating_add(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A policy with no jitter, so a test sees the bare escalation/reset behaviour
    /// deterministically.
    fn deterministic(initial: Duration, max: Duration) -> ReconnectPolicy {
        ReconnectPolicy {
            initial,
            max,
            jitter_max: Duration::ZERO,
            stable_after: Duration::from_secs(60),
        }
    }

    #[test]
    fn escalates_by_factor_and_caps_at_max() {
        let mut b = Backoff::new(deterministic(
            Duration::from_secs(1),
            Duration::from_secs(8),
        ));
        let delays: Vec<Duration> = (0..5).map(|_| b.next_delay()).collect();
        assert_eq!(
            delays,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(8), // capped
            ]
        );
    }

    #[test]
    fn an_initial_above_the_cap_is_not_clamped_down() {
        let mut b = Backoff::new(deterministic(
            Duration::from_secs(60),
            Duration::from_secs(30),
        ));
        assert_eq!(b.next_delay(), Duration::from_secs(60));
        assert_eq!(b.next_delay(), Duration::from_secs(60), "raised cap holds");
    }

    #[test]
    fn a_stable_session_resets_the_schedule() {
        let mut b = Backoff::new(deterministic(
            Duration::from_secs(1),
            Duration::from_secs(8),
        ));
        b.next_delay(); // 1s
        b.next_delay(); // 2s — schedule now escalated
        b.note_session(Duration::from_secs(60));
        assert_eq!(b.next_delay(), Duration::from_secs(1));
    }

    #[test]
    fn a_short_session_keeps_escalating() {
        let mut b = Backoff::new(deterministic(
            Duration::from_secs(1),
            Duration::from_secs(8),
        ));
        b.next_delay(); // 1s
        b.note_session(Duration::from_secs(1));
        assert_eq!(b.next_delay(), Duration::from_secs(2));
    }

    #[test]
    fn jitter_never_exceeds_its_bound() {
        let max = Duration::from_millis(750);
        for _ in 0..1000 {
            assert!(jitter(max) <= max);
        }
        assert_eq!(jitter(Duration::ZERO), Duration::ZERO);
    }
}

#[cfg(test)]
mod delay_properties {
    use super::*;

    proptest::proptest! {
        /// Every delay the schedule ever yields stays inside the policy envelope:
        /// `max(initial, max) + jitter_max` — under escalation, resets, and any
        /// step count.
        #[test]
        fn every_delay_stays_within_the_policy_envelope(
            initial_ms in 0u64..5_000,
            max_ms in 0u64..60_000,
            jitter_ms in 0u64..2_000,
            steps in 1usize..40,
        ) {
            let policy = ReconnectPolicy {
                initial: Duration::from_millis(initial_ms),
                max: Duration::from_millis(max_ms),
                jitter_max: Duration::from_millis(jitter_ms),
                stable_after: Duration::from_secs(60),
            };
            let cap = Duration::from_millis(initial_ms.max(max_ms) + jitter_ms);
            let mut backoff = Backoff::new(policy);
            for _ in 0..steps {
                let delay = backoff.next_delay();
                proptest::prop_assert!(
                    delay <= cap,
                    "delay {delay:?} escaped the envelope {cap:?}"
                );
            }
        }
    }
}

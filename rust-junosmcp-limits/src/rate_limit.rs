//! Per-authenticated-token request-rate limiting.

use crate::config::LimitsConfig;
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

const TOKEN_SCALE: u128 = 1_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RateDecision {
    Allowed,
    Limited { retry_after_secs: u64 },
}

#[derive(Debug)]
struct Bucket {
    available_units: u128,
    last_refill: Instant,
}

impl Bucket {
    fn full(burst: u64, now: Instant) -> Self {
        Self {
            available_units: capacity_units(burst),
            last_refill: now,
        }
    }

    fn check(&mut self, now: Instant, rate: u64, burst: u64) -> RateDecision {
        if let Some(elapsed) = now.checked_duration_since(self.last_refill) {
            self.available_units = self
                .available_units
                .saturating_add(refill_units(elapsed, rate))
                .min(capacity_units(burst));
            self.last_refill = now;
        }

        if self.available_units >= TOKEN_SCALE {
            self.available_units -= TOKEN_SCALE;
            return RateDecision::Allowed;
        }

        let deficit_units = TOKEN_SCALE - self.available_units;
        let wait_ns = deficit_units.div_ceil(u128::from(rate));
        let retry_secs = wait_ns.div_ceil(TOKEN_SCALE).max(1);
        RateDecision::Limited {
            retry_after_secs: u64::try_from(retry_secs).unwrap_or(u64::MAX),
        }
    }
}

#[derive(Clone)]
struct TokenRateLimitState {
    buckets: Arc<DashMap<String, Bucket>>,
    rate_per_second: u64,
    burst: u64,
}

impl TokenRateLimitState {
    fn new(config: &LimitsConfig) -> Self {
        debug_assert!(config.token_rate_limit_enabled());
        Self {
            buckets: Arc::new(DashMap::new()),
            rate_per_second: config.max_requests_per_second_per_token,
            burst: config.max_request_burst_per_token,
        }
    }

    fn check_at(&self, token: &str, now: Instant) -> RateDecision {
        let mut bucket = self
            .buckets
            .entry(token.to_owned())
            .or_insert_with(|| Bucket::full(self.burst, now));
        bucket.check(now, self.rate_per_second, self.burst)
    }
}

fn capacity_units(burst: u64) -> u128 {
    u128::from(burst).saturating_mul(TOKEN_SCALE)
}

fn refill_units(elapsed: Duration, rate: u64) -> u128 {
    elapsed.as_nanos().saturating_mul(u128::from(rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(rate: u64, burst: u64) -> TokenRateLimitState {
        TokenRateLimitState::new(&LimitsConfig {
            max_requests_per_second_per_token: rate,
            max_request_burst_per_token: burst,
            ..Default::default()
        })
    }

    #[test]
    fn fresh_bucket_admits_exact_burst_then_limits() {
        let state = state(2, 3);
        let now = Instant::now();
        assert_eq!(state.check_at("alice", now), RateDecision::Allowed);
        assert_eq!(state.check_at("alice", now), RateDecision::Allowed);
        assert_eq!(state.check_at("alice", now), RateDecision::Allowed);
        assert_eq!(
            state.check_at("alice", now),
            RateDecision::Limited {
                retry_after_secs: 1
            }
        );
    }

    #[test]
    fn partial_refill_reaches_exact_token_boundary() {
        let state = state(2, 1);
        let start = Instant::now();
        assert_eq!(state.check_at("alice", start), RateDecision::Allowed);
        assert_eq!(
            state.check_at("alice", start + Duration::from_millis(250)),
            RateDecision::Limited {
                retry_after_secs: 1
            }
        );
        assert_eq!(
            state.check_at("alice", start + Duration::from_millis(500)),
            RateDecision::Allowed
        );
    }

    #[test]
    fn long_idle_refill_is_capped_at_burst() {
        let state = state(4, 2);
        let start = Instant::now();
        assert_eq!(state.check_at("alice", start), RateDecision::Allowed);
        assert_eq!(state.check_at("alice", start), RateDecision::Allowed);
        let later = start + Duration::from_secs(60);
        assert_eq!(state.check_at("alice", later), RateDecision::Allowed);
        assert_eq!(state.check_at("alice", later), RateDecision::Allowed);
        assert_eq!(
            state.check_at("alice", later),
            RateDecision::Limited {
                retry_after_secs: 1
            }
        );
    }

    #[test]
    fn token_names_are_isolated() {
        let state = state(1, 1);
        let now = Instant::now();
        assert_eq!(state.check_at("alice", now), RateDecision::Allowed);
        assert!(matches!(
            state.check_at("alice", now),
            RateDecision::Limited { .. }
        ));
        assert_eq!(state.check_at("bob", now), RateDecision::Allowed);
    }

    #[test]
    fn concurrent_checks_admit_exactly_the_burst() {
        const BURST: usize = 8;
        let state = Arc::new(state(1, BURST as u64));
        let barrier = Arc::new(std::sync::Barrier::new(BURST * 2));
        let now = Instant::now();
        let admitted = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..BURST * 2)
                .map(|_| {
                    let state = state.clone();
                    let barrier = barrier.clone();
                    scope.spawn(move || {
                        barrier.wait();
                        state.check_at("alice", now) == RateDecision::Allowed
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .filter(|admitted| *admitted)
                .count()
        });
        assert_eq!(admitted, BURST);
    }

    #[test]
    fn refill_arithmetic_saturates() {
        assert_eq!(refill_units(Duration::MAX, u64::MAX), u128::MAX);
    }

    #[test]
    fn earlier_instant_does_not_move_refill_clock_backward() {
        let state = state(2, 1);
        let start = Instant::now();
        assert_eq!(state.check_at("alice", start), RateDecision::Allowed);
        assert!(matches!(
            state.check_at("alice", start + Duration::from_millis(250)),
            RateDecision::Limited { .. }
        ));
        assert!(matches!(
            state.check_at("alice", start),
            RateDecision::Limited { .. }
        ));
        assert_eq!(
            state.check_at("alice", start + Duration::from_millis(500)),
            RateDecision::Allowed
        );
    }
}

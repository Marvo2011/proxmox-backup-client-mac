use std::time::{Duration, Instant};

use anyhow::{Error, bail};

/// Rate limiter interface.
pub trait RateLimit {
    /// Update rate and bucket size
    ///
    /// The next refill applies the new rate to all elapsed time not yet accounted for, including
    /// any fractional-token remainder.
    fn update_rate(&mut self, rate: u64, bucket_size: u64);

    /// Returns the overall traffic (since started)
    fn traffic(&self) -> u64;

    /// Register traffic, returning a proposed delay to reach the
    /// expected rate.
    fn register_traffic(&mut self, current_time: Instant, data_len: u64) -> Duration;
}

/// Like [`RateLimit`], but does not require self to be mutable.
///
/// This is useful for types providing internal mutability (Mutex).
pub trait ShareableRateLimit: Send + Sync {
    /// Update rate and bucket size, with the same semantics as [`RateLimit::update_rate`].
    fn update_rate(&self, rate: u64, bucket_size: u64);

    /// Returns the overall traffic (since started)
    fn traffic(&self) -> u64;

    /// Register traffic, returning a proposed delay to reach the
    /// expected rate.
    fn register_traffic(&self, current_time: Instant, data_len: u64) -> Duration;
}

/// IMPORTANT: We use this struct in shared memory, so please do not
/// change/modify the layout (do not add fields)
#[derive(Clone)]
#[repr(C)]
struct TbfState {
    traffic: u64, // overall traffic
    last_update: Instant,
    consumed_tokens: u64,
}

impl TbfState {
    const NO_DELAY: Duration = Duration::from_millis(0);

    fn refill_bucket(&mut self, rate: u64, current_time: Instant) {
        let time_diff = match current_time.checked_duration_since(self.last_update) {
            Some(duration) => duration.as_nanos(),
            None => return,
        };

        let allowed_traffic = time_diff.saturating_mul(u128::from(rate)) / 1_000_000_000;

        if allowed_traffic == 0 {
            // Keep `last_update` so that the elapsed time is accounted for on a later refill.
            // Advancing it here would truncate the refill to zero on every call, which stalls the
            // bucket indefinitely whenever updates come in faster than the rate grants a token.
            return;
        }

        if allowed_traffic >= u128::from(self.consumed_tokens) {
            self.consumed_tokens = 0;
            self.last_update = current_time;
            return;
        }

        let allowed_traffic = allowed_traffic as u64; // less than consumed_tokens, so this fits
        self.consumed_tokens -= allowed_traffic;

        // Only account for the time that was actually turned into tokens, the remainder is left
        // for the next refill, otherwise the truncated division would drop tokens on every call.
        // Round the consumed time up. Rounding it down hands the fraction of a nanosecond that
        // is left over to the next call, and repeating that at a fine granularity adds up to
        // more than the configured rate. Erring the other way costs at most a nanosecond of
        // refill per call and can never grant more than the rate allows.
        let used_time = u128::from(allowed_traffic)
            .saturating_mul(1_000_000_000)
            .div_ceil(u128::from(rate))
            .min(time_diff);
        let used_time = Duration::from_nanos(u64::try_from(used_time).unwrap_or(u64::MAX));
        self.last_update = self
            .last_update
            .checked_add(used_time)
            .unwrap_or(current_time);
    }

    fn register_traffic(
        &mut self,
        rate: u64,
        bucket_size: u64,
        current_time: Instant,
        data_len: u64,
    ) -> Duration {
        self.refill_bucket(rate, current_time);

        self.traffic = self.traffic.saturating_add(data_len);
        self.consumed_tokens = self.consumed_tokens.saturating_add(data_len);

        if self.consumed_tokens <= bucket_size {
            return Self::NO_DELAY;
        }
        if rate == 0 {
            return Self::NO_DELAY;
        }
        Duration::from_nanos(
            (self.consumed_tokens - bucket_size).saturating_mul(1_000_000_000) / rate,
        )
    }
}

/// Token bucket based rate limiter
///
/// IMPORTANT: We use this struct in shared memory, so please do not
/// change/modify the layout (do not add fields)
#[repr(C)]
pub struct RateLimiter {
    rate: u64,        // tokens/second
    bucket_size: u64, // TBF bucket size
    state: TbfState,
}

// Processes sharing a mapping may run different versions of this crate, so guard the offsets and
// not just the overall size: reordering the fields would keep the size but break every mapping.
const _: () = {
    use std::mem::offset_of;

    assert!(offset_of!(RateLimiter, rate) == 0);
    assert!(offset_of!(RateLimiter, bucket_size) == size_of::<u64>());
    assert!(offset_of!(RateLimiter, state) == 2 * size_of::<u64>());

    assert!(offset_of!(TbfState, traffic) == 0);
    assert!(offset_of!(TbfState, last_update) == size_of::<u64>());
    assert!(offset_of!(TbfState, consumed_tokens) == size_of::<u64>() + size_of::<Instant>());
    assert!(size_of::<TbfState>() == 2 * size_of::<u64>() + size_of::<Instant>());
};

impl RateLimiter {
    /// Creates a new instance, using [Instant::now] as start time.
    pub fn new(rate: u64, bucket_size: u64) -> Self {
        let start_time = Instant::now();
        Self::with_start_time(rate, bucket_size, start_time)
    }

    /// Creates a new instance with specified `rate`, `bucket_size` and `start_time`.
    pub fn with_start_time(rate: u64, bucket_size: u64, start_time: Instant) -> Self {
        Self {
            rate,
            bucket_size,
            state: TbfState {
                traffic: 0,
                last_update: start_time,
                // start with empty bucket (all tokens consumed)
                consumed_tokens: bucket_size,
            },
        }
    }
}

impl RateLimit for RateLimiter {
    fn update_rate(&mut self, rate: u64, bucket_size: u64) {
        self.rate = rate;

        if bucket_size < self.bucket_size && self.state.consumed_tokens > bucket_size {
            self.state.consumed_tokens = bucket_size; // start again
        }

        self.bucket_size = bucket_size;
    }

    fn traffic(&self) -> u64 {
        self.state.traffic
    }

    fn register_traffic(&mut self, current_time: Instant, data_len: u64) -> Duration {
        self.state
            .register_traffic(self.rate, self.bucket_size, current_time, data_len)
    }
}

impl<R: RateLimit + Send> ShareableRateLimit for std::sync::Mutex<R> {
    fn update_rate(&self, rate: u64, bucket_size: u64) {
        self.lock().unwrap().update_rate(rate, bucket_size);
    }

    fn traffic(&self) -> u64 {
        self.lock().unwrap().traffic()
    }

    fn register_traffic(&self, current_time: Instant, data_len: u64) -> Duration {
        self.lock()
            .unwrap()
            .register_traffic(current_time, data_len)
    }
}

/// Array of rate limiters.
///
/// A group of rate limiters with same configuration.
pub struct RateLimiterVec {
    rate: u64,        // tokens/second
    bucket_size: u64, // TBF bucket size
    state: Vec<TbfState>,
}

impl RateLimiterVec {
    /// Creates a new instance, using [Instant::now] as start time.
    pub fn new(group_size: usize, rate: u64, bucket_size: u64) -> Self {
        let start_time = Instant::now();
        Self::with_start_time(group_size, rate, bucket_size, start_time)
    }

    /// Creates a new instance with specified `rate`, `bucket_size` and `start_time`.
    pub fn with_start_time(
        group_size: usize,
        rate: u64,
        bucket_size: u64,
        start_time: Instant,
    ) -> Self {
        let state = TbfState {
            traffic: 0,
            last_update: start_time,
            // start with empty bucket (all tokens consumed)
            consumed_tokens: bucket_size,
        };
        Self {
            rate,
            bucket_size,
            state: vec![state; group_size],
        }
    }

    #[allow(clippy::len_without_is_empty)]
    /// Return the number of TBF entries (group_size)
    pub fn len(&self) -> usize {
        self.state.len()
    }

    /// Traffic for the specified index
    pub fn traffic(&self, index: usize) -> Result<u64, Error> {
        if index >= self.state.len() {
            bail!("RateLimiterVec::traffic - index out of range");
        }
        Ok(self.state[index].traffic)
    }

    /// Register traffic at the specified index
    pub fn register_traffic(
        &mut self,
        index: usize,
        current_time: Instant,
        data_len: u64,
    ) -> Result<Duration, Error> {
        if index >= self.state.len() {
            bail!("RateLimiterVec::register_traffic - index out of range");
        }

        Ok(self.state[index].register_traffic(self.rate, self.bucket_size, current_time, data_len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    const MILLI: Duration = Duration::from_millis(1);

    #[test]
    fn empty_bucket_delays_until_tokens_are_available() {
        let start = Instant::now();
        // 1000 tokens/s, no burst allowance
        let mut limiter = RateLimiter::with_start_time(1000, 0, start);

        assert_eq!(limiter.register_traffic(start, 500), SECOND / 2);
        assert_eq!(limiter.traffic(), 500);
    }

    #[test]
    fn burst_up_to_the_bucket_size_is_not_delayed() {
        let start = Instant::now();
        let mut limiter = RateLimiter::with_start_time(1000, 1000, start);

        // the bucket starts out empty, so it first has to fill up
        assert_eq!(
            limiter.register_traffic(start + SECOND, 1000),
            TbfState::NO_DELAY
        );
        // .. and is empty again afterwards
        assert_eq!(limiter.register_traffic(start + SECOND, 1000), SECOND);
    }

    #[test]
    fn refills_at_rates_below_one_token_per_update() {
        let start = Instant::now();
        // 100 tokens/s means a single 1ms update interval is worth only a tenth of a token
        let mut limiter = RateLimiter::with_start_time(100, 0, start);

        assert_eq!(limiter.register_traffic(start, 100), SECOND);

        let mut now = start;
        for _ in 0..1000 {
            now += MILLI;
            limiter.register_traffic(now, 0);
        }

        // one second at 100 tokens/s refills exactly the 100 consumed tokens
        assert_eq!(limiter.register_traffic(now, 0), TbfState::NO_DELAY);
    }

    #[test]
    fn refill_keeps_the_sub_token_remainder() {
        let start = Instant::now();
        // 100 tokens/s, so a single 1ms step is worth only a tenth of a token
        let mut limiter = RateLimiter::with_start_time(100, 0, start);

        assert_eq!(limiter.register_traffic(start, 10), SECOND / 10);

        let mut now = start;
        for _ in 0..10 {
            now += MILLI;
            limiter.register_traffic(now, 0);
        }

        // the ten steps add up to exactly one token, so one of the ten outstanding tokens is
        // paid off, which only happens if the sub-token remainders were not dropped
        assert_eq!(limiter.register_traffic(now, 0), SECOND * 9 / 100);
    }

    #[test]
    fn a_high_rate_does_not_grant_the_same_time_twice() {
        let start = Instant::now();
        // more than one token per nanosecond, so converting a granted token back into time
        // rounds down to nothing
        let mut limiter = RateLimiter::with_start_time(1_500_000_000, 0, start);

        limiter.register_traffic(start, 10);

        // repeating the very same instant must not pay off the outstanding debt
        let now = start + Duration::from_nanos(1);
        for _ in 0..10 {
            limiter.register_traffic(now, 0);
            assert_eq!(limiter.state.consumed_tokens, 9);
        }
    }

    #[test]
    fn refilling_one_nanosecond_at_a_time_stays_within_the_rate() {
        let start = Instant::now();
        // 0.6 tokens per nanosecond, so no single step is worth a whole token
        let mut limiter = RateLimiter::with_start_time(600_000_000, 0, start);

        assert_eq!(
            limiter.register_traffic(start, 100),
            Duration::from_nanos(166)
        );

        let mut now = start;
        for _ in 0..10 {
            now += Duration::from_nanos(1);
            limiter.register_traffic(now, 0);
        }

        // Ten nanoseconds are worth six tokens. Check the debt directly, since the returned
        // delay truncates fractional nanoseconds and can hide small accounting errors.
        assert!(limiter.state.consumed_tokens >= 94);
        assert!(limiter.state.consumed_tokens < 100);
        let left = limiter.register_traffic(now, 0);
        assert!(
            left >= Duration::from_nanos(156),
            "granted too much: {left:?}"
        );
        assert!(
            left < Duration::from_nanos(166),
            "granted nothing: {left:?}"
        );
    }

    #[test]
    fn a_rate_of_zero_never_delays() {
        let start = Instant::now();
        let mut limiter = RateLimiter::with_start_time(0, 0, start);

        assert_eq!(
            limiter.register_traffic(start, u64::MAX),
            TbfState::NO_DELAY
        );
        // the refill runs with a rate of zero here, it must not try to divide by it
        assert_eq!(
            limiter.register_traffic(start + SECOND, 1),
            TbfState::NO_DELAY
        );
    }

    #[test]
    fn time_going_backwards_does_not_refill() {
        let start = Instant::now() + SECOND;
        let mut limiter = RateLimiter::with_start_time(1000, 0, start);

        assert_eq!(limiter.register_traffic(start, 1000), SECOND);
        assert_eq!(limiter.register_traffic(start - SECOND, 0), SECOND);
    }

    #[test]
    fn traffic_accounting_is_not_affected_by_the_bucket() {
        let start = Instant::now();
        let mut limiter = RateLimiter::with_start_time(1000, 10_000, start);

        limiter.register_traffic(start, 1000);
        limiter.register_traffic(start + SECOND, 2000);
        assert_eq!(limiter.traffic(), 3000);
    }

    #[test]
    fn shrinking_the_bucket_clamps_the_consumed_tokens() {
        let start = Instant::now();
        let mut limiter = RateLimiter::with_start_time(1000, 10_000, start);

        // fill the bucket, then consume all of it plus 1000 tokens
        limiter.register_traffic(start + SECOND * 10, 11_000);
        limiter.update_rate(1000, 100);

        // the outstanding tokens got clamped to the new bucket size, no delay is left over
        assert_eq!(
            limiter.register_traffic(start + SECOND * 10, 0),
            TbfState::NO_DELAY
        );
    }

    #[test]
    fn vec_rejects_out_of_range_indices() {
        let start = Instant::now();
        let mut limiter = RateLimiterVec::with_start_time(2, 1000, 0, start);

        assert_eq!(limiter.len(), 2);
        assert!(limiter.traffic(2).is_err());
        assert!(limiter.register_traffic(2, start, 1).is_err());
        assert!(limiter.register_traffic(1, start, 1).is_ok());
        assert_eq!(limiter.traffic(0).unwrap(), 0);
        assert_eq!(limiter.traffic(1).unwrap(), 1);
    }

    #[test]
    fn counters_saturate_instead_of_wrapping() {
        let start = Instant::now();
        let mut limiter = RateLimiter::with_start_time(1000, 0, start);

        let delay = limiter.register_traffic(start, u64::MAX);
        // Wrapping would erase both the recorded traffic and the outstanding debt.
        assert_eq!(limiter.register_traffic(start, 1), delay);
        assert_eq!(limiter.traffic(), u64::MAX);
    }
}

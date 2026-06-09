use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// Circuit breaker states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation — requests pass through.
    Closed,
    /// Too many failures — requests are rejected.
    Open,
    /// Testing recovery — a single request is allowed through.
    HalfOpen,
}

/// A circuit breaker that tracks consecutive failures and temporarily disables
/// execution when a threshold is exceeded.
///
/// State transitions:
/// - `Closed` → `Open`: when `consecutive_failures >= failure_threshold`
/// - `Open` → `HalfOpen`: after `open_duration_ms` has elapsed
/// - `HalfOpen` → `Closed`: on success
/// - `HalfOpen` → `Open`: on failure (resets timer)
///
/// Designed to be held via `Arc<CircuitBreaker>` and referenced via
/// `Weak<CircuitBreaker>` from executors, so the breaker can be dropped
/// independently without leaking memory.
#[derive(Debug)]
pub struct CircuitBreaker {
    failure_threshold: usize,
    open_duration_ms: u64,
    state: Mutex<CircuitState>,
    consecutive_failures: AtomicUsize,
    opened_at: Mutex<Option<Instant>>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker.
    ///
    /// - `failure_threshold`: number of consecutive failures before opening.
    /// - `open_duration_ms`: how long (in milliseconds) to stay open before
    ///   transitioning to half-open.
    pub fn new(failure_threshold: usize, open_duration_ms: u64) -> Self {
        Self {
            failure_threshold,
            open_duration_ms,
            state: Mutex::new(CircuitState::Closed),
            consecutive_failures: AtomicUsize::new(0),
            opened_at: Mutex::new(None),
        }
    }

    /// Record a successful operation.
    ///
    /// Resets the consecutive failure counter and transitions to `Closed`.
    pub fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::SeqCst);
        let mut state = self.state.lock().unwrap();
        *state = CircuitState::Closed;
        *self.opened_at.lock().unwrap() = None;
    }

    /// Record a failed operation.
    ///
    /// Increments the failure counter. If the threshold is reached,
    /// transitions to `Open` and records the timestamp.
    pub fn record_failure(&self) {
        let count = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
        if count >= self.failure_threshold {
            let mut state = self.state.lock().unwrap();
            *state = CircuitState::Open;
            *self.opened_at.lock().unwrap() = Some(Instant::now());
        }
    }

    /// Check whether the circuit breaker allows execution.
    ///
    /// Returns `true` if the circuit is `Closed` or has transitioned from
    /// `Open` to `HalfOpen` (enough time has passed). Returns `false` if
    /// the circuit is still `Open`.
    ///
    /// When returning `true` in the `HalfOpen` state, the caller should
    /// proceed with a single trial request and call `record_success` or
    /// `record_failure` accordingly.
    pub fn is_available(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        match *state {
            CircuitState::Closed => true,
            CircuitState::HalfOpen => true,
            CircuitState::Open => {
                // Check if enough time has passed to transition to half-open
                let opened_at = self.opened_at.lock().unwrap();
                if let Some(at) = *opened_at
                    && at.elapsed().as_millis() >= self.open_duration_ms as u128
                {
                    *state = CircuitState::HalfOpen;
                    return true;
                }
                false
            }
        }
    }

    /// Get the current state.
    pub fn state(&self) -> CircuitState {
        *self.state.lock().unwrap()
    }

    /// Get the current consecutive failure count.
    pub fn consecutive_failures(&self) -> usize {
        self.consecutive_failures.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn new_breaker_is_closed() {
        let cb = CircuitBreaker::new(3, 1000);
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.is_available());
        assert_eq!(cb.consecutive_failures(), 0);
    }

    #[test]
    fn stays_closed_below_threshold() {
        let cb = CircuitBreaker::new(3, 1000);
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.is_available());
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.is_available());
    }

    #[test]
    fn opens_at_threshold() {
        let cb = CircuitBreaker::new(3, 1000);
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.is_available());
    }

    #[test]
    fn success_resets_counter() {
        let cb = CircuitBreaker::new(3, 1000);
        cb.record_failure();
        cb.record_failure();
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.consecutive_failures(), 0);
    }

    #[test]
    fn transitions_to_half_open_after_duration() {
        let cb = CircuitBreaker::new(2, 50); // 50ms open duration
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.is_available());

        thread::sleep(Duration::from_millis(60));
        assert!(cb.is_available());
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[test]
    fn half_open_success_closes() {
        let cb = CircuitBreaker::new(2, 50);
        cb.record_failure();
        cb.record_failure();
        thread::sleep(Duration::from_millis(60));
        assert!(cb.is_available()); // transitions to HalfOpen
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn half_open_failure_reopens() {
        let cb = CircuitBreaker::new(2, 50);
        cb.record_failure();
        cb.record_failure();
        thread::sleep(Duration::from_millis(60));
        assert!(cb.is_available()); // HalfOpen
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.is_available());
    }

    #[test]
    fn weak_reference_allows_cleanup() {
        let strong = Arc::new(CircuitBreaker::new(3, 1000));
        let weak = Arc::downgrade(&strong);
        assert!(weak.upgrade().is_some());

        drop(strong);
        assert!(weak.upgrade().is_none());
    }
}

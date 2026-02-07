use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

pub struct CircuitBreaker {
    state: RwLock<CircuitState>,
    failure_count: AtomicU32,
    failure_threshold: u32,
    recovery_timeout: Duration,
    last_failure_time: RwLock<Option<Instant>>,
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u32, recovery_timeout: Duration) -> Self {
        Self {
            state: RwLock::new(CircuitState::Closed),
            failure_count: AtomicU32::new(0),
            failure_threshold,
            recovery_timeout,
            last_failure_time: RwLock::new(None),
        }
    }

    pub async fn state(&self) -> CircuitState {
        let current = *self.state.read().await;
        if current == CircuitState::Open {
            if let Some(last_failure) = *self.last_failure_time.read().await {
                if last_failure.elapsed() >= self.recovery_timeout {
                    return CircuitState::HalfOpen;
                }
            }
        }
        current
    }

    pub async fn allow_request(&self) -> bool {
        match self.state().await {
            CircuitState::Closed => true,
            CircuitState::HalfOpen => true,
            CircuitState::Open => false,
        }
    }

    pub async fn record_success(&self) {
        self.failure_count.store(0, Ordering::Relaxed);
        let mut state = self.state.write().await;
        *state = CircuitState::Closed;
    }

    pub async fn record_failure(&self) {
        let count = self.failure_count.fetch_add(1, Ordering::Relaxed) + 1;
        *self.last_failure_time.write().await = Some(Instant::now());

        if count >= self.failure_threshold {
            let mut state = self.state.write().await;
            *state = CircuitState::Open;
            tracing::warn!(
                failures = count,
                threshold = self.failure_threshold,
                "Circuit breaker opened"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn starts_closed() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(30));
        assert_eq!(cb.state().await, CircuitState::Closed);
        assert!(cb.allow_request().await);
    }

    #[tokio::test]
    async fn opens_after_threshold() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(30));

        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Closed);

        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Closed);

        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);
        assert!(!cb.allow_request().await);
    }

    #[tokio::test]
    async fn resets_on_success() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(30));

        cb.record_failure().await;
        cb.record_failure().await;
        cb.record_success().await;

        assert_eq!(cb.state().await, CircuitState::Closed);
        assert!(cb.allow_request().await);
    }

    #[tokio::test]
    async fn transitions_to_half_open() {
        let cb = CircuitBreaker::new(2, Duration::from_millis(10));

        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(cb.state().await, CircuitState::HalfOpen);
        assert!(cb.allow_request().await);
    }

    #[tokio::test]
    async fn half_open_to_closed_on_success() {
        let cb = CircuitBreaker::new(2, Duration::from_millis(10));

        cb.record_failure().await;
        cb.record_failure().await;

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(cb.state().await, CircuitState::HalfOpen);

        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn half_open_to_open_on_failure() {
        let cb = CircuitBreaker::new(2, Duration::from_millis(10));

        cb.record_failure().await;
        cb.record_failure().await;

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(cb.state().await, CircuitState::HalfOpen);

        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);
    }
}

//! Circuit Breaker pattern for gRPC backend resilience
//!
//! The Circuit Breaker prevents cascading failures by "breaking" the circuit when a
//! backend service is unhealthy. This stops sending requests to failing services,
//! giving them time to recover.
//!
//! ## States
//!
//! - **Closed**: Normal operation, requests flow through
//! - **Open**: Service unhealthy, requests fail fast without calling backend
//! - **Half-Open**: Testing recovery, limited requests allowed through
//!
//! ## How It Works
//!
//! 1. Circuit starts **Closed** - all requests go through
//! 2. After `failure_threshold` consecutive failures → Circuit **Opens**
//! 3. After `recovery_timeout` → Circuit becomes **Half-Open**
//! 4. If next request succeeds → Circuit **Closes**
//! 5. If next request fails → Circuit **Opens** again
//!
//! ## Example
//!
//! ```rust,no_run
//! use grpc_graphql_gateway::{Gateway, CircuitBreakerConfig};
//! use std::time::Duration;
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let gateway = Gateway::builder()
//!     .with_circuit_breaker(CircuitBreakerConfig {
//!         failure_threshold: 5,
//!         recovery_timeout: Duration::from_secs(30),
//!         half_open_max_requests: 3,
//!     })
//!     // ... other configuration
//! #   ;
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// Circuit breaker state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation - requests flow through
    Closed,
    /// Service unhealthy - requests fail fast
    Open,
    /// Testing recovery - limited requests allowed
    HalfOpen,
}

impl std::fmt::Display for CircuitState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CircuitState::Closed => write!(f, "Closed"),
            CircuitState::Open => write!(f, "Open"),
            CircuitState::HalfOpen => write!(f, "HalfOpen"),
        }
    }
}

/// Configuration for the Circuit Breaker
///
/// # Example
///
/// ```rust
/// use grpc_graphql_gateway::CircuitBreakerConfig;
/// use std::time::Duration;
///
/// let config = CircuitBreakerConfig {
///     failure_threshold: 5,                      // Open after 5 failures
///     recovery_timeout: Duration::from_secs(30), // Try recovery after 30s
///     half_open_max_requests: 3,                 // Allow 3 test requests
/// };
/// ```
#[derive(Clone, Debug)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the circuit
    pub failure_threshold: u32,
    /// Time to wait before transitioning from Open to Half-Open
    pub recovery_timeout: Duration,
    /// Maximum requests allowed in Half-Open state before deciding
    pub half_open_max_requests: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            recovery_timeout: Duration::from_secs(30),
            half_open_max_requests: 3,
        }
    }
}

/// Circuit breaker for a single service
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    /// Current state
    state: RwLock<CircuitState>,
    /// Consecutive failure count
    failure_count: AtomicU32,
    /// Timestamp when circuit opened (as millis since UNIX_EPOCH for atomicity)
    opened_at: AtomicU64,
    /// Requests allowed through in half-open state
    half_open_requests: AtomicU32,
    /// Successful requests in half-open state
    half_open_successes: AtomicU32,
    /// Service name for logging
    service_name: String,
}

impl CircuitBreaker {
    /// Create a new circuit breaker for a service
    pub fn new(service_name: impl Into<String>, config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: RwLock::new(CircuitState::Closed),
            failure_count: AtomicU32::new(0),
            opened_at: AtomicU64::new(0),
            half_open_requests: AtomicU32::new(0),
            half_open_successes: AtomicU32::new(0),
            service_name: service_name.into(),
        }
    }

    /// Get current circuit state
    pub fn state(&self) -> CircuitState {
        self.maybe_transition_to_half_open();
        *self.state.read().unwrap()
    }

    /// Check if a request should be allowed through
    ///
    /// Returns `Ok(())` if the request can proceed, `Err` if circuit is open.
    pub fn allow_request(&self) -> Result<(), CircuitBreakerError> {
        self.maybe_transition_to_half_open();

        let state = *self.state.read().unwrap();
        match state {
            CircuitState::Closed => Ok(()),
            CircuitState::Open => {
                tracing::debug!(
                    service = %self.service_name,
                    "Circuit breaker OPEN - rejecting request"
                );
                Err(CircuitBreakerError::CircuitOpen {
                    service: self.service_name.clone(),
                    retry_after: self.time_until_half_open(),
                })
            }
            CircuitState::HalfOpen => {
                let current = self.half_open_requests.fetch_add(1, Ordering::SeqCst);
                if current < self.config.half_open_max_requests {
                    tracing::debug!(
                        service = %self.service_name,
                        request = current + 1,
                        max = self.config.half_open_max_requests,
                        "Circuit breaker HALF-OPEN - allowing test request"
                    );
                    Ok(())
                } else {
                    tracing::debug!(
                        service = %self.service_name,
                        "Circuit breaker HALF-OPEN - max test requests reached"
                    );
                    Err(CircuitBreakerError::CircuitOpen {
                        service: self.service_name.clone(),
                        retry_after: Some(Duration::from_secs(1)),
                    })
                }
            }
        }
    }

    /// Record a successful request
    pub fn record_success(&self) {
        let state = *self.state.read().unwrap();
        match state {
            CircuitState::Closed => {
                // Reset failure count on success
                self.failure_count.store(0, Ordering::SeqCst);
            }
            CircuitState::HalfOpen => {
                let successes = self.half_open_successes.fetch_add(1, Ordering::SeqCst) + 1;
                if successes >= self.config.half_open_max_requests {
                    // Enough successes - close the circuit
                    self.close();
                    tracing::info!(
                        service = %self.service_name,
                        "Circuit breaker CLOSED - service recovered"
                    );
                }
            }
            CircuitState::Open => {
                // Shouldn't happen, but reset if it does
                self.failure_count.store(0, Ordering::SeqCst);
            }
        }
    }

    /// Record a failed request
    pub fn record_failure(&self) {
        let state = *self.state.read().unwrap();
        match state {
            CircuitState::Closed => {
                let failures = self.failure_count.fetch_add(1, Ordering::SeqCst) + 1;
                if failures >= self.config.failure_threshold {
                    self.open();
                    tracing::warn!(
                        service = %self.service_name,
                        failures = failures,
                        "Circuit breaker OPENED - too many failures"
                    );
                }
            }
            CircuitState::HalfOpen => {
                // Any failure in half-open reopens the circuit
                self.open();
                tracing::warn!(
                    service = %self.service_name,
                    "Circuit breaker REOPENED - test request failed"
                );
            }
            CircuitState::Open => {
                // Already open, nothing to do
            }
        }
    }

    /// Get the service name
    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    /// Get failure count
    pub fn failure_count(&self) -> u32 {
        self.failure_count.load(Ordering::SeqCst)
    }

    /// Transition to Open state
    fn open(&self) {
        let mut state = self.state.write().unwrap();
        *state = CircuitState::Open;
        // Store timestamp for recovery timeout tracking
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.opened_at.store(now, Ordering::SeqCst);
        self.half_open_requests.store(0, Ordering::SeqCst);
        self.half_open_successes.store(0, Ordering::SeqCst);
    }

    /// Transition to Closed state
    fn close(&self) {
        let mut state = self.state.write().unwrap();
        *state = CircuitState::Closed;
        self.failure_count.store(0, Ordering::SeqCst);
        self.half_open_requests.store(0, Ordering::SeqCst);
        self.half_open_successes.store(0, Ordering::SeqCst);
    }

    /// Check if we should transition from Open to Half-Open
    fn maybe_transition_to_half_open(&self) {
        let state = *self.state.read().unwrap();
        if state != CircuitState::Open {
            return;
        }

        let opened_at = self.opened_at.load(Ordering::SeqCst);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let elapsed = Duration::from_millis(now.saturating_sub(opened_at));
        if elapsed >= self.config.recovery_timeout {
            let mut state = self.state.write().unwrap();
            if *state == CircuitState::Open {
                *state = CircuitState::HalfOpen;
                self.half_open_requests.store(0, Ordering::SeqCst);
                self.half_open_successes.store(0, Ordering::SeqCst);
                tracing::info!(
                    service = %self.service_name,
                    "Circuit breaker HALF-OPEN - testing recovery"
                );
            }
        }
    }

    /// Time until circuit transitions to half-open
    fn time_until_half_open(&self) -> Option<Duration> {
        let state = *self.state.read().unwrap();
        if state != CircuitState::Open {
            return None;
        }

        let opened_at = self.opened_at.load(Ordering::SeqCst);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let elapsed = Duration::from_millis(now.saturating_sub(opened_at));
        self.config.recovery_timeout.checked_sub(elapsed)
    }

    /// Force reset the circuit breaker to closed state
    pub fn reset(&self) {
        self.close();
        tracing::info!(
            service = %self.service_name,
            "Circuit breaker manually RESET"
        );
    }
}

impl Clone for CircuitBreaker {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            state: RwLock::new(*self.state.read().unwrap()),
            failure_count: AtomicU32::new(self.failure_count.load(Ordering::SeqCst)),
            opened_at: AtomicU64::new(self.opened_at.load(Ordering::SeqCst)),
            half_open_requests: AtomicU32::new(self.half_open_requests.load(Ordering::SeqCst)),
            half_open_successes: AtomicU32::new(self.half_open_successes.load(Ordering::SeqCst)),
            service_name: self.service_name.clone(),
        }
    }
}

impl std::fmt::Debug for CircuitBreaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CircuitBreaker")
            .field("service", &self.service_name)
            .field("state", &self.state())
            .field("failure_count", &self.failure_count())
            .finish()
    }
}

/// Registry of circuit breakers for all services
pub struct CircuitBreakerRegistry {
    config: CircuitBreakerConfig,
    breakers: RwLock<HashMap<String, Arc<CircuitBreaker>>>,
}

impl CircuitBreakerRegistry {
    /// Create a new registry with the given configuration
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            breakers: RwLock::new(HashMap::new()),
        }
    }

    /// Get or create a circuit breaker for a service
    pub fn get_or_create(&self, service_name: &str) -> Arc<CircuitBreaker> {
        // Fast path: check if exists
        if let Some(breaker) = self.breakers.read().unwrap().get(service_name) {
            return breaker.clone();
        }

        // Slow path: create new breaker
        let mut breakers = self.breakers.write().unwrap();
        breakers
            .entry(service_name.to_string())
            .or_insert_with(|| {
                Arc::new(CircuitBreaker::new(service_name, self.config.clone()))
            })
            .clone()
    }

    /// Get a circuit breaker for a service (if exists)
    pub fn get(&self, service_name: &str) -> Option<Arc<CircuitBreaker>> {
        self.breakers.read().unwrap().get(service_name).cloned()
    }

    /// Get all circuit breakers
    pub fn all(&self) -> Vec<Arc<CircuitBreaker>> {
        self.breakers.read().unwrap().values().cloned().collect()
    }

    /// Get status of all circuit breakers
    pub fn status(&self) -> HashMap<String, CircuitState> {
        self.breakers
            .read()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.state()))
            .collect()
    }

    /// Reset all circuit breakers
    pub fn reset_all(&self) {
        for breaker in self.breakers.read().unwrap().values() {
            breaker.reset();
        }
    }
}

impl Clone for CircuitBreakerRegistry {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            breakers: RwLock::new(self.breakers.read().unwrap().clone()),
        }
    }
}

impl std::fmt::Debug for CircuitBreakerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CircuitBreakerRegistry")
            .field("config", &self.config)
            .field("services", &self.breakers.read().unwrap().keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Shared circuit breaker registry
pub type SharedCircuitBreakerRegistry = Arc<CircuitBreakerRegistry>;

/// Create a new shared circuit breaker registry
pub fn create_circuit_breaker_registry(config: CircuitBreakerConfig) -> SharedCircuitBreakerRegistry {
    Arc::new(CircuitBreakerRegistry::new(config))
}

/// Error types for circuit breaker
#[derive(Debug, Clone, thiserror::Error)]
pub enum CircuitBreakerError {
    /// Circuit is open - service is unavailable
    #[error("Circuit breaker open for service '{service}'. Retry after {retry_after:?}")]
    CircuitOpen {
        service: String,
        retry_after: Option<Duration>,
    },
}

impl CircuitBreakerError {
    /// Convert to GraphQL error extensions
    pub fn to_extensions(&self) -> HashMap<String, serde_json::Value> {
        let mut extensions = HashMap::new();
        match self {
            CircuitBreakerError::CircuitOpen { service, retry_after } => {
                extensions.insert("code".to_string(), serde_json::json!("SERVICE_UNAVAILABLE"));
                extensions.insert("service".to_string(), serde_json::json!(service));
                if let Some(retry) = retry_after {
                    extensions.insert("retryAfter".to_string(), serde_json::json!(retry.as_secs()));
                }
            }
        }
        extensions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_starts_closed() {
        let cb = CircuitBreaker::new("test", CircuitBreakerConfig::default());
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.allow_request().is_ok());
    }

    #[test]
    fn test_circuit_opens_after_failures() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            recovery_timeout: Duration::from_secs(30),
            half_open_max_requests: 1,
        };
        let cb = CircuitBreaker::new("test", config);

        // Record failures
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        cb.record_failure(); // This should open it
        assert_eq!(cb.state(), CircuitState::Open);

        // Requests should be rejected
        assert!(cb.allow_request().is_err());
    }

    #[test]
    fn test_success_resets_failure_count() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.failure_count(), 2);

        cb.record_success();
        assert_eq!(cb.failure_count(), 0);
    }

    #[test]
    fn test_manual_reset() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            ..Default::default()
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure(); // Opens circuit
        assert_eq!(cb.state(), CircuitState::Open);

        cb.reset();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.allow_request().is_ok());
    }

    #[test]
    fn test_half_open_transition() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_millis(10),
            half_open_max_requests: 1,
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure(); // Opens circuit
        assert_eq!(cb.state(), CircuitState::Open);

        // Wait for recovery timeout
        std::thread::sleep(Duration::from_millis(20));

        // Should transition to half-open
        assert_eq!(cb.state(), CircuitState::HalfOpen);
        assert!(cb.allow_request().is_ok());
    }

    #[test]
    fn test_half_open_success_closes() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_millis(10),
            half_open_max_requests: 1,
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure(); // Opens
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        cb.record_success(); // Should close
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_half_open_failure_reopens() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_millis(10),
            half_open_max_requests: 1,
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure(); // Opens
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        cb.record_failure(); // Should reopen
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_registry() {
        let registry = CircuitBreakerRegistry::new(CircuitBreakerConfig::default());

        let cb1 = registry.get_or_create("service1");
        let cb2 = registry.get_or_create("service2");
        let cb1_again = registry.get_or_create("service1");

        // Same service should return same breaker
        assert!(Arc::ptr_eq(&cb1, &cb1_again));
        assert!(!Arc::ptr_eq(&cb1, &cb2));

        assert_eq!(registry.all().len(), 2);
    }

    #[test]
    fn test_registry_status() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            ..Default::default()
        };
        let registry = CircuitBreakerRegistry::new(config);

        let _cb1 = registry.get_or_create("healthy");
        let cb2 = registry.get_or_create("unhealthy");

        cb2.record_failure(); // Opens circuit

        let status = registry.status();
        assert_eq!(status.get("healthy"), Some(&CircuitState::Closed));
        assert_eq!(status.get("unhealthy"), Some(&CircuitState::Open));
    }

    #[test]
    fn test_error_extensions() {
        let err = CircuitBreakerError::CircuitOpen {
            service: "test".to_string(),
            retry_after: Some(Duration::from_secs(30)),
        };

        let ext = err.to_extensions();
        assert_eq!(ext.get("code"), Some(&serde_json::json!("SERVICE_UNAVAILABLE")));
        assert_eq!(ext.get("service"), Some(&serde_json::json!("test")));
        assert_eq!(ext.get("retryAfter"), Some(&serde_json::json!(30)));
    }
}

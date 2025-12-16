//! Request Collapsing for gRPC Calls
//!
//! This module provides request collapsing/deduplication for GraphQL queries
//! that hit the same gRPC method multiple times. This optimization reduces
//! gRPC round-trips by:
//!
//! 1. **Deduplication**: Identical requests to the same gRPC method share a single call
//! 2. **Concurrent Batching**: Non-identical requests to the same method execute concurrently
//!
//! # Example
//!
//! When a GraphQL query contains multiple fields calling the same gRPC method:
//!
//! ```graphql
//! query {
//!   user1: getUser(id: "1") { name }
//!   user2: getUser(id: "2") { name }
//!   user3: getUser(id: "1") { name }  # Duplicate of user1
//! }
//! ```
//!
//! Without collapsing: 3 gRPC calls
//! With collapsing: 2 gRPC calls (user1 and user3 share the same response)
//!
//! # Configuration
//!
//! ```rust,no_run
//! use grpc_graphql_gateway::{Gateway, RequestCollapsingConfig};
//! use std::time::Duration;
//!
//! # fn example() {
//! let gateway = Gateway::builder()
//!     .with_request_collapsing(RequestCollapsingConfig::default())
//!     .build();
//! # }
//! ```

use async_graphql::Value as GqlValue;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::RwLock;  // SECURITY: Non-poisoning locks
use std::time::{Duration, Instant};
use tokio::sync::{watch, Mutex as TokioMutex};

/// Configuration for request collapsing.
///
/// Controls how duplicate and similar gRPC requests are coalesced
/// to reduce backend load.
#[derive(Debug, Clone)]
pub struct RequestCollapsingConfig {
    /// Maximum time to wait for in-flight requests before making a new call.
    /// Default: 50ms
    pub coalesce_window: Duration,

    /// Maximum number of waiters for a single in-flight request.
    /// If exceeded, a new request is made.
    /// Default: 100
    pub max_waiters: usize,

    /// Whether to enable request collapsing.
    /// Default: true
    pub enabled: bool,

    /// Maximum size of the in-flight request cache.
    /// Default: 10000
    pub max_cache_size: usize,
}

impl Default for RequestCollapsingConfig {
    fn default() -> Self {
        Self {
            coalesce_window: Duration::from_millis(50),
            max_waiters: 100,
            enabled: true,
            max_cache_size: 10000,
        }
    }
}

impl RequestCollapsingConfig {
    /// Create a new configuration with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the coalesce window duration.
    ///
    /// Requests arriving within this window will wait for in-flight
    /// duplicate requests to complete.
    pub fn coalesce_window(mut self, duration: Duration) -> Self {
        self.coalesce_window = duration;
        self
    }

    /// Set the maximum number of waiters per in-flight request.
    pub fn max_waiters(mut self, max: usize) -> Self {
        self.max_waiters = max;
        self
    }

    /// Enable or disable request collapsing.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set the maximum cache size for in-flight requests.
    pub fn max_cache_size(mut self, size: usize) -> Self {
        self.max_cache_size = size;
        self
    }

    /// Create a configuration optimized for high-throughput scenarios.
    pub fn high_throughput() -> Self {
        Self {
            coalesce_window: Duration::from_millis(100),
            max_waiters: 500,
            enabled: true,
            max_cache_size: 50000,
        }
    }

    /// Create a configuration optimized for low-latency scenarios.
    pub fn low_latency() -> Self {
        Self {
            coalesce_window: Duration::from_millis(10),
            max_waiters: 50,
            enabled: true,
            max_cache_size: 5000,
        }
    }

    /// Disable request collapsing entirely.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }
}

/// Key for identifying unique gRPC requests.
///
/// Uses a SHA-256 hash of the service name, method path, and serialized request.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct RequestKey {
    hash: String,
}

impl RequestKey {
    /// Create a new request key from request components.
    pub fn new(service_name: &str, grpc_path: &str, request_bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(service_name.as_bytes());
        hasher.update(b":");
        hasher.update(grpc_path.as_bytes());
        hasher.update(b":");
        hasher.update(request_bytes);
        let hash = hex::encode(hasher.finalize());
        Self { hash }
    }

    /// Get the hash string.
    pub fn hash(&self) -> &str {
        &self.hash
    }
}

/// Result of a collapsed request lookup.
#[derive(Debug)]
pub enum CollapseResult {
    /// This is the leader - execute the request and broadcast the result.
    Leader(RequestBroadcaster),

    /// Wait for the leader's result.
    Follower(RequestReceiver),

    /// Collapsing is disabled or not applicable.
    Passthrough,
}

/// Broadcaster for request results.
///
/// The leader uses this to send the result to all followers.
#[derive(Debug)]
pub struct RequestBroadcaster {
    key: RequestKey,
    sender: watch::Sender<Option<Arc<Result<GqlValue, String>>>>,
    registry: Arc<RequestCollapsingRegistry>,
}

impl RequestBroadcaster {
    /// Broadcast the result to all waiting followers.
    pub fn broadcast(self, result: Result<GqlValue, String>) {
        let result = Arc::new(result);
        // Ignore errors - no receivers is fine
        let _ = self.sender.send(Some(result));
        // Clean up the in-flight entry
        self.registry.remove(&self.key);
    }

    /// Get the request key.
    pub fn key(&self) -> &RequestKey {
        &self.key
    }
}

/// Receiver for request results.
///
/// Followers use this to wait for the leader's result.
#[derive(Debug)]
pub struct RequestReceiver {
    receiver: watch::Receiver<Option<Arc<Result<GqlValue, String>>>>,
}

impl RequestReceiver {
    /// Wait for the result from the leader.
    pub async fn recv(mut self) -> Result<GqlValue, String> {
        loop {
            // Check current value
            if let Some(result) = &*self.receiver.borrow() {
                return (**result).clone();
            }
            
            // Wait for update
            if self.receiver.changed().await.is_err() {
                return Err("Request leader dropped without sending result".to_string());
            }
        }
    }
}

/// In-flight request entry.
struct InFlightRequest {
    sender: watch::Sender<Option<Arc<Result<GqlValue, String>>>>,
    started_at: Instant,
    waiter_count: usize,
}

/// Registry for tracking in-flight requests.
///
/// This is the core data structure for request collapsing.
/// It tracks which requests are currently in-flight and allows
/// new requests to wait for existing ones.
pub struct RequestCollapsingRegistry {
    config: RequestCollapsingConfig,
    in_flight: RwLock<HashMap<RequestKey, Arc<TokioMutex<InFlightRequest>>>>,
}

impl std::fmt::Debug for RequestCollapsingRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let in_flight_count = self.in_flight.read().len();
        f.debug_struct("RequestCollapsingRegistry")
            .field("config", &self.config)
            .field("in_flight_count", &in_flight_count)
            .finish()
    }
}

impl RequestCollapsingRegistry {
    /// Create a new registry with the given configuration.
    pub fn new(config: RequestCollapsingConfig) -> Self {
        Self {
            config,
            in_flight: RwLock::new(HashMap::new()),
        }
    }

    /// Try to collapse a request.
    ///
    /// Returns:
    /// - `CollapseResult::Leader` if this is the first request (execute and broadcast)
    /// - `CollapseResult::Follower` if there's an in-flight request (wait for result)
    /// - `CollapseResult::Passthrough` if collapsing is disabled
    pub async fn try_collapse(&self, key: RequestKey) -> CollapseResult {
        if !self.config.enabled {
            return CollapseResult::Passthrough;
        }

        // Check for existing in-flight request
        // SECURITY: Clone the Arc so we can drop the RwLock before awaiting.
        // Holding a blocking lock across an await point causes deadlocks/freezes.
        let entry_opt = {
            let in_flight = self.in_flight.read();
            in_flight.get(&key).cloned()
        };

        if let Some(entry) = entry_opt {
            let mut guard = entry.lock().await;

            // Check if request is still within coalesce window
            if guard.started_at.elapsed() < self.config.coalesce_window
                && guard.waiter_count < self.config.max_waiters
            {
                guard.waiter_count += 1;
                let receiver = guard.sender.subscribe();
                return CollapseResult::Follower(RequestReceiver { receiver });
            }
            // Request is too old or has too many waiters, fall through to create new
        }

        // Become the leader for this request
        let (sender, _) = watch::channel(None);
        let entry = Arc::new(TokioMutex::new(InFlightRequest {
            sender: sender.clone(),
            started_at: Instant::now(),
            waiter_count: 0,
        }));

        {
            let mut in_flight = self.in_flight.write();

            // Check cache size and evict old entries if needed
            if in_flight.len() >= self.config.max_cache_size {
                self.evict_stale_entries(&mut in_flight);
            }

            in_flight.insert(key.clone(), entry);
        }

        CollapseResult::Leader(RequestBroadcaster {
            key,
            sender,
            registry: Arc::new(Self::new(self.config.clone())),
        })
    }

    /// Remove an in-flight request entry.
    fn remove(&self, key: &RequestKey) {
        let mut in_flight = self.in_flight.write();
        in_flight.remove(key);
    }

    /// Evict stale entries from the in-flight cache.
    fn evict_stale_entries(&self, in_flight: &mut HashMap<RequestKey, Arc<TokioMutex<InFlightRequest>>>) {
        let stale_threshold = self.config.coalesce_window * 10;
        let now = Instant::now();

        // Collect keys to remove (can't await while holding the lock, so we use try_lock)
        let keys_to_remove: Vec<RequestKey> = in_flight
            .iter()
            .filter_map(|(key, entry)| {
                if let Ok(guard) = entry.try_lock() {
                    if now.duration_since(guard.started_at) > stale_threshold {
                        return Some(key.clone());
                    }
                }
                None
            })
            .collect();

        for key in keys_to_remove {
            in_flight.remove(&key);
        }
    }

    /// Get statistics about the registry.
    pub fn stats(&self) -> CollapsingStats {
        let in_flight = self.in_flight.read();
        CollapsingStats {
            in_flight_count: in_flight.len(),
            max_cache_size: self.config.max_cache_size,
            enabled: self.config.enabled,
        }
    }
}

/// Shared reference to the request collapsing registry.
pub type SharedRequestCollapsingRegistry = Arc<RequestCollapsingRegistry>;

/// Create a shared request collapsing registry.
pub fn create_request_collapsing_registry(config: RequestCollapsingConfig) -> SharedRequestCollapsingRegistry {
    Arc::new(RequestCollapsingRegistry::new(config))
}

/// Statistics about request collapsing.
#[derive(Debug, Clone)]
pub struct CollapsingStats {
    /// Number of currently in-flight requests.
    pub in_flight_count: usize,

    /// Maximum cache size configuration.
    pub max_cache_size: usize,

    /// Whether collapsing is enabled.
    pub enabled: bool,
}

/// Metrics for request collapsing.
///
/// Tracks how effective request collapsing has been.
#[derive(Debug, Default)]
pub struct CollapsingMetrics {
    /// Total number of requests processed.
    pub total_requests: std::sync::atomic::AtomicU64,

    /// Number of requests that became leaders.
    pub leader_requests: std::sync::atomic::AtomicU64,

    /// Number of requests that became followers (collapsed).
    pub collapsed_requests: std::sync::atomic::AtomicU64,

    /// Number of passthrough requests (collapsing disabled).
    pub passthrough_requests: std::sync::atomic::AtomicU64,
}

impl CollapsingMetrics {
    /// Create new metrics.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a leader request.
    pub fn record_leader(&self) {
        use std::sync::atomic::Ordering;
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.leader_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a collapsed (follower) request.
    pub fn record_collapsed(&self) {
        use std::sync::atomic::Ordering;
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.collapsed_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a passthrough request.
    pub fn record_passthrough(&self) {
        use std::sync::atomic::Ordering;
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.passthrough_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Get the collapse ratio (0.0 to 1.0).
    ///
    /// Higher is better - means more requests were deduplicated.
    pub fn collapse_ratio(&self) -> f64 {
        use std::sync::atomic::Ordering;
        let total = self.total_requests.load(Ordering::Relaxed);
        let collapsed = self.collapsed_requests.load(Ordering::Relaxed);
        if total == 0 {
            0.0
        } else {
            collapsed as f64 / total as f64
        }
    }

    /// Get a snapshot of the metrics.
    pub fn snapshot(&self) -> CollapsingMetricsSnapshot {
        use std::sync::atomic::Ordering;
        CollapsingMetricsSnapshot {
            total_requests: self.total_requests.load(Ordering::Relaxed),
            leader_requests: self.leader_requests.load(Ordering::Relaxed),
            collapsed_requests: self.collapsed_requests.load(Ordering::Relaxed),
            passthrough_requests: self.passthrough_requests.load(Ordering::Relaxed),
            collapse_ratio: self.collapse_ratio(),
        }
    }
}

/// Snapshot of collapsing metrics.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CollapsingMetricsSnapshot {
    /// Total number of requests processed.
    pub total_requests: u64,

    /// Number of requests that became leaders.
    pub leader_requests: u64,

    /// Number of requests that became followers (collapsed).
    pub collapsed_requests: u64,

    /// Number of passthrough requests.
    pub passthrough_requests: u64,

    /// Collapse ratio (0.0 to 1.0).
    pub collapse_ratio: f64,
}

/// Shared reference to collapsing metrics.
pub type SharedCollapsingMetrics = Arc<CollapsingMetrics>;

/// Create shared collapsing metrics.
pub fn create_collapsing_metrics() -> SharedCollapsingMetrics {
    Arc::new(CollapsingMetrics::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::sleep;

    #[test]
    fn test_request_key_creation() {
        let key1 = RequestKey::new("service", "/path", b"request1");
        let key2 = RequestKey::new("service", "/path", b"request1");
        let key3 = RequestKey::new("service", "/path", b"request2");

        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_config_defaults() {
        let config = RequestCollapsingConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_waiters, 100);
        assert_eq!(config.max_cache_size, 10000);
    }

    #[test]
    fn test_config_presets() {
        let high_throughput = RequestCollapsingConfig::high_throughput();
        assert!(high_throughput.coalesce_window > Duration::from_millis(50));

        let low_latency = RequestCollapsingConfig::low_latency();
        assert!(low_latency.coalesce_window < Duration::from_millis(50));

        let disabled = RequestCollapsingConfig::disabled();
        assert!(!disabled.enabled);
    }

    #[tokio::test]
    async fn test_leader_selection() {
        let config = RequestCollapsingConfig::default();
        let registry = RequestCollapsingRegistry::new(config);

        let key = RequestKey::new("service", "/path", b"request");
        let result = registry.try_collapse(key).await;

        match result {
            CollapseResult::Leader(_) => {}
            _ => panic!("Expected leader result"),
        }
    }

    #[tokio::test]
    async fn test_follower_selection() {
        let config = RequestCollapsingConfig::default();
        let registry = Arc::new(RequestCollapsingRegistry::new(config));

        let key = RequestKey::new("service", "/path", b"request");

        // First request becomes leader
        let result1 = registry.try_collapse(key.clone()).await;
        let broadcaster = match result1 {
            CollapseResult::Leader(b) => b,
            _ => panic!("Expected leader result"),
        };

        // Second request with same key becomes follower
        let result2 = registry.try_collapse(key).await;
        match result2 {
            CollapseResult::Follower(_) => {}
            _ => panic!("Expected follower result"),
        }

        // Clean up
        broadcaster.broadcast(Ok(GqlValue::Null));
    }

    #[tokio::test]
    async fn test_disabled_collapsing() {
        let config = RequestCollapsingConfig::disabled();
        let registry = RequestCollapsingRegistry::new(config);

        let key = RequestKey::new("service", "/path", b"request");
        let result = registry.try_collapse(key).await;

        match result {
            CollapseResult::Passthrough => {}
            _ => panic!("Expected passthrough result"),
        }
    }

    #[tokio::test]
    async fn test_result_broadcasting() {
        let config = RequestCollapsingConfig::default();
        let registry = Arc::new(RequestCollapsingRegistry::new(config));

        let key = RequestKey::new("service", "/path", b"request");

        // Leader
        let result1 = registry.try_collapse(key.clone()).await;
        let broadcaster = match result1 {
            CollapseResult::Leader(b) => b,
            _ => panic!("Expected leader result"),
        };

        // Follower
        let result2 = registry.try_collapse(key).await;
        let receiver = match result2 {
            CollapseResult::Follower(r) => r,
            _ => panic!("Expected follower result"),
        };

        // Broadcast result
        let expected = GqlValue::String("test".to_string());
        broadcaster.broadcast(Ok(expected.clone()));

        // Follower receives result
        let received = receiver.recv().await.unwrap();
        assert_eq!(received, expected);
    }

    #[tokio::test]
    async fn test_expired_requests_create_new_leader() {
        let config = RequestCollapsingConfig::new()
            .coalesce_window(Duration::from_millis(10));
        let registry = Arc::new(RequestCollapsingRegistry::new(config));

        let key = RequestKey::new("service", "/path", b"request");

        // First request becomes leader
        let _result1 = registry.try_collapse(key.clone()).await;

        // Wait for coalesce window to expire
        sleep(Duration::from_millis(20)).await;

        // Second request should become a new leader (old one expired)
        let result2 = registry.try_collapse(key).await;
        match result2 {
            CollapseResult::Leader(_) => {}
            _ => panic!("Expected new leader after expiry"),
        }
    }

    #[test]
    fn test_metrics() {
        let metrics = CollapsingMetrics::new();

        metrics.record_leader();
        metrics.record_collapsed();
        metrics.record_collapsed();
        metrics.record_passthrough();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.total_requests, 4);
        assert_eq!(snapshot.leader_requests, 1);
        assert_eq!(snapshot.collapsed_requests, 2);
        assert_eq!(snapshot.passthrough_requests, 1);
        assert!((snapshot.collapse_ratio - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_stats() {
        let config = RequestCollapsingConfig::default();
        let registry = RequestCollapsingRegistry::new(config);

        let stats = registry.stats();
        assert_eq!(stats.in_flight_count, 0);
        assert!(stats.enabled);
    }
}

#[cfg(test)]
mod proptest_checks {
    use super::*;
    use proptest::prelude::*;
    use tokio::runtime::Runtime;

    proptest! {
        #[test]
        fn fuzz_collapsing_mixed_traffic(
            requests in proptest::collection::vec(("[a-c]", 0..5u64), 10..50)
        ) {
            let rt = Runtime::new().unwrap();
            rt.block_on(async {
                let config = RequestCollapsingConfig::new()
                    .coalesce_window(Duration::from_millis(20)); // Short window
                let registry = Arc::new(RequestCollapsingRegistry::new(config));
                
                let mut handles = vec![];
                
                for (suffix, delay_ms) in requests {
                    let registry = registry.clone();
                    let key = RequestKey::new("svc", &format!("path_{}", suffix), b"data");
                    
                    // Simulate random arrival
                    // We can't await sleep here easily without delaying loop.
                    // But we can spawn and sleep inside.
                    handles.push(tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        
                        match registry.try_collapse(key).await {
                           CollapseResult::Leader(broadcaster) => {
                               // Simulate execution work
                               tokio::time::sleep(Duration::from_millis(10)).await;
                               broadcaster.broadcast(Ok(async_graphql::Value::String("result".to_string())));
                               "leader"
                           }
                           CollapseResult::Follower(receiver) => {
                               let res = receiver.recv().await;
                               assert!(res.is_ok());
                               "follower"
                           }
                           CollapseResult::Passthrough => "passthrough"
                        }
                    }));
                }
                
                for h in handles {
                     let _ = h.await;
                }
            });
        }
    }
}

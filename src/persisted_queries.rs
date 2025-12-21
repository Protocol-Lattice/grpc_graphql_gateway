//! Automatic Persisted Queries (APQ) support
//!
//! APQ is a technique to reduce bandwidth by sending query hashes instead of full query strings.
//! When a client sends a persisted query hash, the gateway looks it up in its cache. If found,
//! the cached query is executed. If not found, the client resends with both hash and query,
//! which the gateway caches for future use.
//!
//! ## How It Works
//!
//! 1. **First Request**: Client sends hash only → Gateway returns "PersistedQueryNotFound"
//! 2. **Second Request**: Client sends hash + query → Gateway caches and executes
//! 3. **Subsequent Requests**: Client sends hash only → Gateway uses cached query
//!
//! ## Protocol
//!
//! APQ uses the `extensions.persistedQuery` field in GraphQL requests:
//!
//! ```json
//! {
//!   "query": null,
//!   "extensions": {
//!     "persistedQuery": {
//!       "version": 1,
//!       "sha256Hash": "ecf4edb46db40b5132295c0291d62fb65d6759a9eedfa4d5d612dd5ec54a6b38"
//!     }
//!   }
//! }
//! ```
//!
//! ## Example
//!
//! ```rust,no_run
//! use grpc_graphql_gateway::{Gateway, PersistedQueryConfig};
//! use std::time::Duration;
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let gateway = Gateway::builder()
//!     .with_persisted_queries(PersistedQueryConfig {
//!         cache_size: 1000,
//!         ttl: Some(Duration::from_secs(3600)),
//!     })
//!     // ... other configuration
//! #   ;
//! # Ok(())
//! # }
//! ```

use parking_lot::RwLock; // SECURITY: Non-poisoning locks
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Configuration for Automatic Persisted Queries
///
/// # Example
///
/// ```rust
/// use grpc_graphql_gateway::PersistedQueryConfig;
/// use std::time::Duration;
///
/// let config = PersistedQueryConfig {
///     cache_size: 1000,           // Store up to 1000 queries
///     ttl: Some(Duration::from_secs(3600)),  // Expire after 1 hour
/// };
/// ```
#[derive(Clone, Debug)]
pub struct PersistedQueryConfig {
    /// Maximum number of queries to cache
    pub cache_size: usize,
    /// Time-to-live for cached queries (None = forever)
    pub ttl: Option<Duration>,
}

impl Default for PersistedQueryConfig {
    fn default() -> Self {
        Self {
            cache_size: 1000,
            ttl: None, // No expiration by default
        }
    }
}

/// Persisted query extension in GraphQL request
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedQueryExtension {
    /// APQ version (currently always 1)
    pub version: u32,
    /// SHA-256 hash of the query
    pub sha256_hash: String,
}

/// Entry in the persisted query cache
#[derive(Clone)]
struct CacheEntry {
    query: String,
    created_at: Instant,
}

/// Store for persisted queries with LRU-like eviction
///
/// Thread-safe cache that stores query strings by their SHA-256 hash.
pub struct PersistedQueryStore {
    config: PersistedQueryConfig,
    cache: RwLock<HashMap<String, CacheEntry>>,
    /// Order of insertion for LRU eviction
    insertion_order: RwLock<Vec<String>>,
}

impl PersistedQueryStore {
    /// Create a new persisted query store with the given configuration
    pub fn new(config: PersistedQueryConfig) -> Self {
        Self {
            config,
            cache: RwLock::new(HashMap::with_capacity(1000)),
            insertion_order: RwLock::new(Vec::with_capacity(1000)),
        }
    }

    /// Compute SHA-256 hash of a query string
    pub fn hash_query(query: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(query.as_bytes());
        let result = hasher.finalize();
        hex::encode(result)
    }

    /// Get a cached query by its hash
    ///
    /// Returns `None` if not found or if TTL has expired.
    pub fn get(&self, hash: &str) -> Option<String> {
        let cache = self.cache.read();
        let entry = cache.get(hash)?;

        // Check TTL expiration
        if let Some(ttl) = self.config.ttl {
            if entry.created_at.elapsed() > ttl {
                drop(cache);
                // Expired - remove it (best effort, don't block)
                let mut cache_write = self.cache.write();
                cache_write.remove(hash);
                return None;
            }
        }

        Some(entry.query.clone())
    }

    /// Store a query with its hash
    ///
    /// Validates that the provided hash matches the query's actual hash.
    /// Returns `Ok(())` if stored successfully, `Err` if hash mismatch.
    pub fn put(&self, hash: &str, query: &str) -> Result<(), PersistedQueryError> {
        // Validate hash
        let computed_hash = Self::hash_query(query);
        if computed_hash != hash {
            return Err(PersistedQueryError::HashMismatch {
                provided: hash.to_string(),
                computed: computed_hash,
            });
        }

        // Evict if at capacity
        self.evict_if_needed();

        // Store the query
        {
            let mut cache = self.cache.write();
            cache.insert(
                hash.to_string(),
                CacheEntry {
                    query: query.to_string(),
                    created_at: Instant::now(),
                },
            );
        }

        {
            let mut order = self.insertion_order.write();
            // Remove if already present (move to end)
            order.retain(|h| h != hash);
            order.push(hash.to_string());
        }

        Ok(())
    }

    /// Check if a query exists in the cache
    pub fn contains(&self, hash: &str) -> bool {
        self.get(hash).is_some()
    }

    /// Get the current number of cached queries
    pub fn len(&self) -> usize {
        self.cache.read().len()
    }

    /// Check if the cache is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clear all cached queries
    pub fn clear(&self) {
        self.cache.write().clear();
        self.insertion_order.write().clear();
    }

    /// Evict oldest entries if cache is at capacity
    fn evict_if_needed(&self) {
        let current_len = self.len();
        if current_len >= self.config.cache_size {
            let to_remove = current_len - self.config.cache_size + 1;

            let hashes_to_remove: Vec<String> = {
                let mut order = self.insertion_order.write();
                let drain_count = to_remove.min(order.len());
                order.drain(..drain_count).collect()
            };

            let mut cache = self.cache.write();
            for hash in hashes_to_remove {
                cache.remove(&hash);
            }
        }
    }
}

impl Clone for PersistedQueryStore {
    fn clone(&self) -> Self {
        let cache = self.cache.read().clone();
        let order = self.insertion_order.read().clone();

        Self {
            config: self.config.clone(),
            cache: RwLock::new(cache),
            insertion_order: RwLock::new(order),
        }
    }
}

impl std::fmt::Debug for PersistedQueryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersistedQueryStore")
            .field("config", &self.config)
            .field("cached_queries", &self.len())
            .finish()
    }
}

/// Error types for persisted queries
#[derive(Debug, Clone, thiserror::Error)]
pub enum PersistedQueryError {
    /// Query not found in cache
    #[error("PersistedQueryNotFound")]
    NotFound,

    /// Hash of provided query doesn't match the provided hash
    #[error("Provided APQ hash does not match query. Provided: {provided}, Computed: {computed}")]
    HashMismatch { provided: String, computed: String },

    /// Invalid APQ version
    #[error("PersistedQueryNotSupported: version {0} is not supported, use version 1")]
    UnsupportedVersion(u32),
}

impl PersistedQueryError {
    /// Convert to GraphQL error extensions
    pub fn to_extensions(&self) -> HashMap<String, serde_json::Value> {
        let mut extensions = HashMap::new();
        let code = match self {
            PersistedQueryError::NotFound => "PERSISTED_QUERY_NOT_FOUND",
            PersistedQueryError::HashMismatch { .. } => "PERSISTED_QUERY_HASH_MISMATCH",
            PersistedQueryError::UnsupportedVersion(_) => "PERSISTED_QUERY_NOT_SUPPORTED",
        };
        extensions.insert("code".to_string(), serde_json::json!(code));
        extensions
    }
}

/// Process a GraphQL request with APQ support
///
/// This function handles the APQ protocol:
/// - If no APQ extension, returns the query as-is
/// - If APQ extension with cached query, returns cached query
/// - If APQ extension without cache hit, returns error for retry
/// - If APQ extension with query, caches and returns query
pub fn process_apq_request(
    store: &PersistedQueryStore,
    query: Option<&str>,
    extensions: Option<&serde_json::Value>,
) -> Result<Option<String>, PersistedQueryError> {
    // Extract APQ extension if present
    let apq_ext = extensions
        .and_then(|ext| ext.get("persistedQuery"))
        .and_then(|pq| serde_json::from_value::<PersistedQueryExtension>(pq.clone()).ok());

    let Some(apq) = apq_ext else {
        // No APQ extension - return query as-is
        return Ok(query.map(String::from));
    };

    // Validate version
    if apq.version != 1 {
        return Err(PersistedQueryError::UnsupportedVersion(apq.version));
    }

    // Check if query is in cache
    if let Some(cached_query) = store.get(&apq.sha256_hash) {
        tracing::debug!(hash = %apq.sha256_hash, "APQ cache hit");
        return Ok(Some(cached_query));
    }

    // Query not in cache
    match query {
        Some(q) if !q.is_empty() => {
            // Query provided - cache it
            store.put(&apq.sha256_hash, q)?;
            tracing::debug!(hash = %apq.sha256_hash, "APQ cached new query");
            Ok(Some(q.to_string()))
        }
        _ => {
            // No query - tell client to retry with query
            tracing::debug!(hash = %apq.sha256_hash, "APQ cache miss");
            Err(PersistedQueryError::NotFound)
        }
    }
}

/// Thread-safe wrapper for use in async contexts
pub type SharedPersistedQueryStore = Arc<PersistedQueryStore>;

/// Create a new shared persisted query store
pub fn create_apq_store(config: PersistedQueryConfig) -> SharedPersistedQueryStore {
    Arc::new(PersistedQueryStore::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_query() {
        let query = "{ hello }";
        let hash = PersistedQueryStore::hash_query(query);

        // SHA-256 produces a 64-character hex string
        assert_eq!(hash.len(), 64);

        // Same query should produce same hash
        assert_eq!(hash, PersistedQueryStore::hash_query(query));

        // Different query should produce different hash
        assert_ne!(hash, PersistedQueryStore::hash_query("{ world }"));
    }

    #[test]
    fn test_store_and_retrieve() {
        let store = PersistedQueryStore::new(PersistedQueryConfig::default());
        let query = "{ users { id name } }";
        let hash = PersistedQueryStore::hash_query(query);

        // Initially not in cache
        assert!(store.get(&hash).is_none());
        assert!(!store.contains(&hash));

        // Store it
        store.put(&hash, query).unwrap();

        // Now it should be found
        assert_eq!(store.get(&hash), Some(query.to_string()));
        assert!(store.contains(&hash));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn test_hash_mismatch() {
        let store = PersistedQueryStore::new(PersistedQueryConfig::default());
        let query = "{ users { id } }";
        let wrong_hash = "deadbeef".repeat(8); // 64 chars

        let result = store.put(&wrong_hash, query);
        assert!(matches!(
            result,
            Err(PersistedQueryError::HashMismatch { .. })
        ));
    }

    #[test]
    fn test_lru_eviction() {
        let config = PersistedQueryConfig {
            cache_size: 3,
            ttl: None,
        };
        let store = PersistedQueryStore::new(config);

        // Add 4 queries (exceeds capacity of 3)
        for i in 0..4 {
            let query = format!("{{ query{} }}", i);
            let hash = PersistedQueryStore::hash_query(&query);
            store.put(&hash, &query).unwrap();
        }

        // First query should be evicted
        let first_hash = PersistedQueryStore::hash_query("{ query0 }");
        assert!(store.get(&first_hash).is_none());

        // Last 3 should still be there
        for i in 1..4 {
            let query = format!("{{ query{} }}", i);
            let hash = PersistedQueryStore::hash_query(&query);
            assert!(store.contains(&hash), "query{} should still be cached", i);
        }
    }

    #[test]
    fn test_ttl_expiration() {
        let config = PersistedQueryConfig {
            cache_size: 100,
            ttl: Some(Duration::from_millis(10)),
        };
        let store = PersistedQueryStore::new(config);

        let query = "{ expiring }";
        let hash = PersistedQueryStore::hash_query(query);
        store.put(&hash, query).unwrap();

        // Should be found immediately
        assert!(store.get(&hash).is_some());

        // Wait for TTL to expire
        std::thread::sleep(Duration::from_millis(20));

        // Should no longer be found
        assert!(store.get(&hash).is_none());
    }

    #[test]
    fn test_process_apq_request_no_extension() {
        let store = PersistedQueryStore::new(PersistedQueryConfig::default());
        let query = "{ hello }";

        let result = process_apq_request(&store, Some(query), None);
        assert_eq!(result.unwrap(), Some(query.to_string()));
    }

    #[test]
    fn test_process_apq_request_cache_miss() {
        let store = PersistedQueryStore::new(PersistedQueryConfig::default());
        let hash = "a".repeat(64);

        let extensions = serde_json::json!({
            "persistedQuery": {
                "version": 1,
                "sha256Hash": hash
            }
        });

        let result = process_apq_request(&store, None, Some(&extensions));
        assert!(matches!(result, Err(PersistedQueryError::NotFound)));
    }

    #[test]
    fn test_process_apq_request_cache_and_retrieve() {
        let store = PersistedQueryStore::new(PersistedQueryConfig::default());
        let query = "{ users { id } }";
        let hash = PersistedQueryStore::hash_query(query);

        let extensions = serde_json::json!({
            "persistedQuery": {
                "version": 1,
                "sha256Hash": hash
            }
        });

        // First request with query - should cache
        let result = process_apq_request(&store, Some(query), Some(&extensions));
        assert_eq!(result.unwrap(), Some(query.to_string()));

        // Second request without query - should retrieve from cache
        let result = process_apq_request(&store, None, Some(&extensions));
        assert_eq!(result.unwrap(), Some(query.to_string()));
    }

    #[test]
    fn test_unsupported_version() {
        let store = PersistedQueryStore::new(PersistedQueryConfig::default());

        let extensions = serde_json::json!({
            "persistedQuery": {
                "version": 2,
                "sha256Hash": "a".repeat(64)
            }
        });

        let result = process_apq_request(&store, None, Some(&extensions));
        assert!(matches!(
            result,
            Err(PersistedQueryError::UnsupportedVersion(2))
        ));
    }

    #[test]
    fn test_clear_cache() {
        let store = PersistedQueryStore::new(PersistedQueryConfig::default());
        let query = "{ test }";
        let hash = PersistedQueryStore::hash_query(query);

        store.put(&hash, query).unwrap();
        assert_eq!(store.len(), 1);

        store.clear();
        assert_eq!(store.len(), 0);
        assert!(store.is_empty());
    }

    #[test]
    fn test_concurrent_cache_operations() {
        use std::sync::Arc;
        use std::thread;

        let store = Arc::new(PersistedQueryStore::new(PersistedQueryConfig {
            cache_size: 100,
            ttl: None,
        }));

        let mut handles = vec![];

        // Spawn multiple threads storing queries
        for i in 0..10 {
            let store_clone = store.clone();
            let handle = thread::spawn(move || {
                let query = format!("{{ query{} }}", i);
                let hash = PersistedQueryStore::hash_query(&query);
                store_clone.put(&hash, &query).unwrap();
                store_clone.get(&hash)
            });
            handles.push(handle);
        }

        // All operations should succeed
        for handle in handles {
            let result = handle.join().unwrap();
            assert!(result.is_some());
        }

        assert!(store.len() <= 100);
    }

    #[test]
    fn test_clone_independence() {
        let store1 = PersistedQueryStore::new(PersistedQueryConfig::default());
        let q = "{ clone }";
        let h = PersistedQueryStore::hash_query(q);
        
        store1.put(&h, q).unwrap();
        
        let store2 = store1.clone();
        
        // verify clone has data
        assert!(store2.contains(&h));
        
        // modify store1
        let q2 = "{ extra }";
        let h2 = PersistedQueryStore::hash_query(q2);
        store1.put(&h2, q2).unwrap();
        
        // store2 should NOT have it (independent)
        assert!(!store2.contains(&h2));
        assert!(store1.contains(&h2));
    }

    #[test]
    fn test_persisted_query_error_extensions() {
         let err = PersistedQueryError::NotFound;
         let ext = err.to_extensions();
         assert_eq!(ext.get("code").unwrap(), "PERSISTED_QUERY_NOT_FOUND");
         
         let err = PersistedQueryError::UnsupportedVersion(99);
         let ext = err.to_extensions();
         assert_eq!(ext.get("code").unwrap(), "PERSISTED_QUERY_NOT_SUPPORTED");
    }

    #[test]
    fn test_debug_impls() {
        let config = PersistedQueryConfig::default();
        let store = PersistedQueryStore::new(config.clone());
        
        let debug_str = format!("{:?}", store);
        assert!(debug_str.contains("PersistedQueryStore"));
        assert!(debug_str.contains("cached_queries"));
        
        let debug_cfg = format!("{:?}", config);
        assert!(debug_cfg.contains("PersistedQueryConfig"));
    }
    
    #[test]
    fn test_persisted_query_config_creation() {
        let config = PersistedQueryConfig {
            cache_size: 50,
            ttl: Some(Duration::from_secs(60)),
        };
        assert_eq!(config.cache_size, 50);
        assert_eq!(config.ttl, Some(Duration::from_secs(60)));
    }

    #[test]
    fn test_json_serialization() {
       let ext = PersistedQueryExtension {
           version: 1,
           sha256_hash: "abc".to_string(),
       };
       let json = serde_json::to_string(&ext).unwrap();
       assert!(json.contains("sha256Hash"));
       assert!(json.contains("version"));
    }
    
    #[test]
    fn test_store_is_empty() {
        let store = PersistedQueryStore::new(PersistedQueryConfig::default());
        assert!(store.is_empty());
        
        let query = "{ a }";
        let hash = PersistedQueryStore::hash_query(query);
        store.put(&hash, query).unwrap();
        assert!(!store.is_empty());
    }
    
    #[test]
    fn test_eviction_updates_lru_order() {
         let config = PersistedQueryConfig { cache_size: 2, ttl: None };
         let store = PersistedQueryStore::new(config);
         
         let q1 = "query { a }";
         let q2 = "query { b }";
         let q3 = "query { c }";
         
         let h1 = PersistedQueryStore::hash_query(q1);
         let h2 = PersistedQueryStore::hash_query(q2);
         let h3 = PersistedQueryStore::hash_query(q3);
         
         store.put(&h1, q1).unwrap();
         store.put(&h2, q2).unwrap();
         
         // re-insert h1 -> moves to back
         store.put(&h1, q1).unwrap();
         
         // insert h3 -> h2 should be evicted (since h1 was just refreshed)
         store.put(&h3, q3).unwrap();
         
         assert!(store.contains(&h1));
         assert!(store.contains(&h3));
         assert!(!store.contains(&h2));
    }
}

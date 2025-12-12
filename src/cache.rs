//! Response Caching for GraphQL queries
//!
//! This module provides an LRU-based response cache with support for:
//! - TTL-based expiration
//! - Mutation-triggered invalidation
//! - Stale-while-revalidate for improved latency
//! - Type-based invalidation
//!
//! ## How It Works
//!
//! 1. **Cache Key**: Generated from normalized query + variables + operation name
//! 2. **Cache Hit**: Return cached response immediately
//! 3. **Cache Miss**: Execute query, cache response, return response
//! 4. **Invalidation**: Mutations automatically invalidate related cache entries
//!
//! ## Example
//!
//! ```rust,no_run
//! use grpc_graphql_gateway::{Gateway, CacheConfig};
//! use std::time::Duration;
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let gateway = Gateway::builder()
//!     .with_response_cache(CacheConfig {
//!         max_size: 10_000,
//!         default_ttl: Duration::from_secs(60),
//!         stale_while_revalidate: Some(Duration::from_secs(30)),
//!         invalidate_on_mutation: true,
//!     })
//!     // ... other configuration
//! #   ;
//! # Ok(())
//! # }
//! ```

use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// Configuration for response caching
///
/// # Example
///
/// ```rust
/// use grpc_graphql_gateway::CacheConfig;
/// use std::time::Duration;
///
/// let config = CacheConfig {
///     max_size: 10_000,                           // Store up to 10k responses
///     default_ttl: Duration::from_secs(60),       // Expire after 1 minute
///     stale_while_revalidate: Some(Duration::from_secs(30)), // Serve stale for 30s
///     invalidate_on_mutation: true,               // Auto-invalidate on mutations
/// };
/// ```
#[derive(Clone, Debug)]
pub struct CacheConfig {
    /// Maximum number of cached responses
    pub max_size: usize,
    /// Default TTL for cached responses
    pub default_ttl: Duration,
    /// Time to serve stale content while revalidating in background
    /// If None, stale content is never served
    pub stale_while_revalidate: Option<Duration>,
    /// Automatically invalidate cache entries when mutations are executed
    pub invalidate_on_mutation: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_size: 10_000,
            default_ttl: Duration::from_secs(60),
            stale_while_revalidate: None,
            invalidate_on_mutation: true,
        }
    }
}

/// A cached GraphQL response
#[derive(Clone, Debug)]
pub struct CachedResponse {
    /// The JSON response data
    pub data: serde_json::Value,
    /// When the entry was created
    pub created_at: Instant,
    /// TTL for this specific entry
    pub ttl: Duration,
    /// GraphQL types referenced in this response (for invalidation)
    pub referenced_types: HashSet<String>,
    /// Entity IDs referenced in this response (for fine-grained invalidation)
    pub referenced_entities: HashSet<String>,
}

impl CachedResponse {
    /// Check if this entry is expired
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.ttl
    }

    /// Check if this entry is stale but within revalidation window
    pub fn is_stale_but_usable(&self, stale_window: Duration) -> bool {
        let elapsed = self.created_at.elapsed();
        elapsed > self.ttl && elapsed <= self.ttl + stale_window
    }
}

/// Result of a cache lookup
#[derive(Debug)]
pub enum CacheLookupResult {
    /// Fresh cache hit - can be served directly
    Hit(CachedResponse),
    /// Stale hit - serve but revalidate in background
    Stale(CachedResponse),
    /// Cache miss - need to execute query
    Miss,
}

/// Thread-safe response cache with LRU eviction
///
/// The cache stores GraphQL responses keyed by a hash of the query, variables,
/// and operation name. It supports TTL expiration and LRU eviction when at capacity.
pub struct ResponseCache {
    config: CacheConfig,
    /// Main cache storage: cache_key -> cached response
    cache: RwLock<HashMap<String, CachedResponse>>,
    /// Insertion order for LRU eviction
    insertion_order: RwLock<Vec<String>>,
    /// Type index: type_name -> set of cache keys that reference this type
    type_index: RwLock<HashMap<String, HashSet<String>>>,
    /// Entity index: entity_key (Type#id) -> set of cache keys that reference this entity
    entity_index: RwLock<HashMap<String, HashSet<String>>>,
}

impl ResponseCache {
    /// Create a new response cache with the given configuration
    pub fn new(config: CacheConfig) -> Self {
        Self {
            config,
            cache: RwLock::new(HashMap::with_capacity(10_000)),
            insertion_order: RwLock::new(Vec::with_capacity(10_000)),
            type_index: RwLock::new(HashMap::new()),
            entity_index: RwLock::new(HashMap::new()),
        }
    }

    /// Generate a cache key from query components
    ///
    /// The key is a SHA-256 hash of:
    /// - Normalized query string (whitespace-normalized)
    /// - Sorted variables JSON
    /// - Operation name (if provided)
    pub fn generate_cache_key(
        query: &str,
        variables: Option<&serde_json::Value>,
        operation_name: Option<&str>,
    ) -> String {
        let mut hasher = Sha256::new();

        // Normalize query: remove extra whitespace
        let normalized_query: String = query
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        hasher.update(normalized_query.as_bytes());

        // Add sorted variables if present
        if let Some(vars) = variables {
            if !vars.is_null() {
                // Sort object keys for consistent hashing
                let sorted_vars = sort_json_value(vars);
                if let Ok(vars_str) = serde_json::to_string(&sorted_vars) {
                    hasher.update(vars_str.as_bytes());
                }
            }
        }

        // Add operation name if present
        if let Some(op_name) = operation_name {
            hasher.update(op_name.as_bytes());
        }

        let result = hasher.finalize();
        hex::encode(result)
    }

    /// Get a cached response by cache key
    ///
    /// Returns `CacheLookupResult` indicating hit, stale, or miss.
    pub fn get(&self, cache_key: &str) -> CacheLookupResult {
        let cache = match self.cache.read() {
            Ok(c) => c,
            Err(_) => return CacheLookupResult::Miss,
        };

        let Some(entry) = cache.get(cache_key) else {
            return CacheLookupResult::Miss;
        };

        // Check if fresh
        if !entry.is_expired() {
            tracing::debug!(cache_key = %cache_key, "Response cache hit");
            return CacheLookupResult::Hit(entry.clone());
        }

        // Check if stale but usable
        if let Some(stale_window) = self.config.stale_while_revalidate {
            if entry.is_stale_but_usable(stale_window) {
                tracing::debug!(cache_key = %cache_key, "Response cache stale hit (revalidating)");
                return CacheLookupResult::Stale(entry.clone());
            }
        }

        // Expired and past stale window
        tracing::debug!(cache_key = %cache_key, "Response cache miss (expired)");
        CacheLookupResult::Miss
    }

    /// Store a response in the cache
    ///
    /// This will:
    /// 1. Evict old entries if at capacity
    /// 2. Store the response
    /// 3. Update type and entity indexes for invalidation
    pub fn put(
        &self,
        cache_key: String,
        response: serde_json::Value,
        referenced_types: HashSet<String>,
        referenced_entities: HashSet<String>,
    ) {
        // Evict if needed
        self.evict_if_needed();

        let entry = CachedResponse {
            data: response,
            created_at: Instant::now(),
            ttl: self.config.default_ttl,
            referenced_types: referenced_types.clone(),
            referenced_entities: referenced_entities.clone(),
        };

        // Store in main cache
        if let Ok(mut cache) = self.cache.write() {
            cache.insert(cache_key.clone(), entry);
        }

        // Update insertion order
        if let Ok(mut order) = self.insertion_order.write() {
            order.retain(|k| k != &cache_key);
            order.push(cache_key.clone());
        }

        // Update type index
        if let Ok(mut type_idx) = self.type_index.write() {
            for type_name in &referenced_types {
                type_idx
                    .entry(type_name.clone())
                    .or_insert_with(HashSet::new)
                    .insert(cache_key.clone());
            }
        }

        // Update entity index
        if let Ok(mut entity_idx) = self.entity_index.write() {
            for entity_key in &referenced_entities {
                entity_idx
                    .entry(entity_key.clone())
                    .or_insert_with(HashSet::new)
                    .insert(cache_key.clone());
            }
        }

        tracing::debug!(cache_key = %cache_key, "Response cached");
    }

    /// Invalidate all cache entries that reference a specific type
    ///
    /// Call this when a mutation modifies data of a certain type.
    ///
    /// # Example
    ///
    /// After `updateUser` mutation, call `invalidate_by_type("User")`
    pub fn invalidate_by_type(&self, type_name: &str) -> usize {
        let cache_keys = {
            let type_idx = match self.type_index.read() {
                Ok(idx) => idx,
                Err(_) => return 0,
            };
            type_idx.get(type_name).cloned().unwrap_or_default()
        };

        let count = cache_keys.len();
        if count > 0 {
            self.remove_entries(&cache_keys);
            tracing::debug!(type_name = %type_name, count = count, "Invalidated cache by type");
        }
        count
    }

    /// Invalidate all cache entries that reference a specific entity
    ///
    /// The entity key format is "TypeName#id", e.g., "User#123"
    ///
    /// # Example
    ///
    /// After `updateUser(id: "123")`, call `invalidate_by_entity("User#123")`
    pub fn invalidate_by_entity(&self, entity_key: &str) -> usize {
        let cache_keys = {
            let entity_idx = match self.entity_index.read() {
                Ok(idx) => idx,
                Err(_) => return 0,
            };
            entity_idx.get(entity_key).cloned().unwrap_or_default()
        };

        let count = cache_keys.len();
        if count > 0 {
            self.remove_entries(&cache_keys);
            tracing::debug!(entity_key = %entity_key, count = count, "Invalidated cache by entity");
        }
        count
    }

    /// Invalidate cache entries based on a mutation result
    ///
    /// This extracts type and entity information from the mutation response
    /// and invalidates matching cache entries.
    pub fn invalidate_for_mutation(&self, mutation_response: &serde_json::Value) -> usize {
        if !self.config.invalidate_on_mutation {
            return 0;
        }

        let mut total_invalidated = 0;

        // Extract types from the mutation response
        let types = extract_types_from_response(mutation_response);
        for type_name in types {
            total_invalidated += self.invalidate_by_type(&type_name);
        }

        // Extract entity keys (Type#id patterns)
        let entities = extract_entities_from_response(mutation_response);
        for entity_key in entities {
            total_invalidated += self.invalidate_by_entity(&entity_key);
        }

        if total_invalidated > 0 {
            tracing::info!(count = total_invalidated, "Cache invalidated after mutation");
        }

        total_invalidated
    }

    /// Clear all cached responses
    pub fn clear(&self) {
        if let Ok(mut cache) = self.cache.write() {
            cache.clear();
        }
        if let Ok(mut order) = self.insertion_order.write() {
            order.clear();
        }
        if let Ok(mut type_idx) = self.type_index.write() {
            type_idx.clear();
        }
        if let Ok(mut entity_idx) = self.entity_index.write() {
            entity_idx.clear();
        }
        tracing::debug!("Response cache cleared");
    }

    /// Get the current number of cached responses
    pub fn len(&self) -> usize {
        self.cache.read().map(|c| c.len()).unwrap_or(0)
    }

    /// Check if the cache is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            size: self.len(),
            max_size: self.config.max_size,
            type_index_size: self.type_index.read().map(|t| t.len()).unwrap_or(0),
            entity_index_size: self.entity_index.read().map(|e| e.len()).unwrap_or(0),
        }
    }

    fn evict_if_needed(&self) {
        let current_len = self.len();
        if current_len >= self.config.max_size {
            let to_remove = current_len - self.config.max_size + 1;

            if let Ok(mut order) = self.insertion_order.write() {
                let drain_count = to_remove.min(order.len());
                let keys_to_remove: Vec<String> = order.drain(..drain_count).collect();
                self.remove_entries_internal(&keys_to_remove);
            }
        }
    }

    fn remove_entries(&self, cache_keys: &HashSet<String>) {
        let keys_vec: Vec<String> = cache_keys.iter().cloned().collect();
        self.remove_entries_internal(&keys_vec);

        // Update insertion order
        if let Ok(mut order) = self.insertion_order.write() {
            order.retain(|k| !cache_keys.contains(k));
        }
    }

    fn remove_entries_internal(&self, cache_keys: &[String]) {
        if let Ok(mut cache) = self.cache.write() {
            for key in cache_keys {
                if let Some(entry) = cache.remove(key) {
                    // Clean up type index
                    if let Ok(mut type_idx) = self.type_index.write() {
                        for type_name in &entry.referenced_types {
                            if let Some(keys) = type_idx.get_mut(type_name) {
                                keys.remove(key);
                                if keys.is_empty() {
                                    type_idx.remove(type_name);
                                }
                            }
                        }
                    }
                    // Clean up entity index
                    if let Ok(mut entity_idx) = self.entity_index.write() {
                        for entity_key in &entry.referenced_entities {
                            if let Some(keys) = entity_idx.get_mut(entity_key) {
                                keys.remove(key);
                                if keys.is_empty() {
                                    entity_idx.remove(entity_key);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

impl Clone for ResponseCache {
    fn clone(&self) -> Self {
        let cache = self.cache.read().map(|c| c.clone()).unwrap_or_default();
        let order = self
            .insertion_order
            .read()
            .map(|o| o.clone())
            .unwrap_or_default();
        let type_idx = self
            .type_index
            .read()
            .map(|t| t.clone())
            .unwrap_or_default();
        let entity_idx = self
            .entity_index
            .read()
            .map(|e| e.clone())
            .unwrap_or_default();

        Self {
            config: self.config.clone(),
            cache: RwLock::new(cache),
            insertion_order: RwLock::new(order),
            type_index: RwLock::new(type_idx),
            entity_index: RwLock::new(entity_idx),
        }
    }
}

impl std::fmt::Debug for ResponseCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponseCache")
            .field("config", &self.config)
            .field("cached_responses", &self.len())
            .finish()
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Current number of cached responses
    pub size: usize,
    /// Maximum cache size
    pub max_size: usize,
    /// Number of types being tracked for invalidation
    pub type_index_size: usize,
    /// Number of entities being tracked for invalidation
    pub entity_index_size: usize,
}

/// Thread-safe wrapper for use in async contexts
pub type SharedResponseCache = Arc<ResponseCache>;

/// Create a new shared response cache
pub fn create_response_cache(config: CacheConfig) -> SharedResponseCache {
    Arc::new(ResponseCache::new(config))
}

/// Check if an operation is a mutation (should not be cached and triggers invalidation)
pub fn is_mutation(query: &str) -> bool {
    let query_trimmed = query.trim();
    query_trimmed.starts_with("mutation")
        || query_trimmed.contains("mutation ")
        || query_trimmed.contains("mutation{")
}

/// Extract type names from a GraphQL __typename fields in response
fn extract_types_from_response(response: &serde_json::Value) -> HashSet<String> {
    let mut types = HashSet::new();
    extract_types_recursive(response, &mut types);
    types
}

fn extract_types_recursive(value: &serde_json::Value, types: &mut HashSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            // Look for __typename
            if let Some(serde_json::Value::String(type_name)) = map.get("__typename") {
                types.insert(type_name.clone());
            }
            // Recurse into all values
            for v in map.values() {
                extract_types_recursive(v, types);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                extract_types_recursive(item, types);
            }
        }
        _ => {}
    }
}

/// Extract entity keys (Type#id) from a GraphQL response
fn extract_entities_from_response(response: &serde_json::Value) -> HashSet<String> {
    let mut entities = HashSet::new();
    extract_entities_recursive(response, &mut entities);
    entities
}

fn extract_entities_recursive(value: &serde_json::Value, entities: &mut HashSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            // Look for __typename + id combination
            let type_name = map.get("__typename").and_then(|t| t.as_str());
            let id = map
                .get("id")
                .and_then(|i| i.as_str())
                .or_else(|| map.get("_id").and_then(|i| i.as_str()));

            if let (Some(tn), Some(id_val)) = (type_name, id) {
                entities.insert(format!("{}#{}", tn, id_val));
            }

            // Recurse into all values
            for v in map.values() {
                extract_entities_recursive(v, entities);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                extract_entities_recursive(item, entities);
            }
        }
        _ => {}
    }
}

/// Sort JSON object keys recursively for consistent hashing
fn sort_json_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut sorted: Vec<_> = map.iter().collect();
            sorted.sort_by_key(|(k, _)| *k);
            let sorted_map: serde_json::Map<String, serde_json::Value> = sorted
                .into_iter()
                .map(|(k, v)| (k.clone(), sort_json_value(v)))
                .collect();
            serde_json::Value::Object(sorted_map)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(sort_json_value).collect())
        }
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_key_generation() {
        let query = "{ users { id name } }";
        let key1 = ResponseCache::generate_cache_key(query, None, None);

        // Same query should produce same key
        assert_eq!(key1, ResponseCache::generate_cache_key(query, None, None));

        // Different query should produce different key
        let key2 = ResponseCache::generate_cache_key("{ products { id } }", None, None);
        assert_ne!(key1, key2);

        // Whitespace normalized
        let key3 = ResponseCache::generate_cache_key("{ users { id   name } }", None, None);
        assert_eq!(key1, key3);
    }

    #[test]
    fn test_cache_key_with_variables() {
        let query = "query GetUser($id: ID!) { user(id: $id) { name } }";
        let vars1 = serde_json::json!({"id": "123"});
        let vars2 = serde_json::json!({"id": "456"});

        let key1 = ResponseCache::generate_cache_key(query, Some(&vars1), None);
        let key2 = ResponseCache::generate_cache_key(query, Some(&vars2), None);

        assert_ne!(key1, key2);
    }

    #[test]
    fn test_cache_put_and_get() {
        let cache = ResponseCache::new(CacheConfig::default());
        let cache_key = "test_key".to_string();
        let response = serde_json::json!({"data": {"user": {"id": "1", "name": "Alice"}}});

        cache.put(
            cache_key.clone(),
            response.clone(),
            HashSet::from(["User".to_string()]),
            HashSet::from(["User#1".to_string()]),
        );

        match cache.get(&cache_key) {
            CacheLookupResult::Hit(entry) => {
                assert_eq!(entry.data, response);
            }
            _ => panic!("Expected cache hit"),
        }
    }

    #[test]
    fn test_cache_miss() {
        let cache = ResponseCache::new(CacheConfig::default());
        match cache.get("nonexistent") {
            CacheLookupResult::Miss => {}
            _ => panic!("Expected cache miss"),
        }
    }

    #[test]
    fn test_ttl_expiration() {
        let config = CacheConfig {
            max_size: 100,
            default_ttl: Duration::from_millis(10),
            stale_while_revalidate: None,
            invalidate_on_mutation: true,
        };
        let cache = ResponseCache::new(config);
        let cache_key = "expiring".to_string();

        cache.put(
            cache_key.clone(),
            serde_json::json!({"test": true}),
            HashSet::new(),
            HashSet::new(),
        );

        // Should be a hit immediately
        assert!(matches!(cache.get(&cache_key), CacheLookupResult::Hit(_)));

        // Wait for TTL
        std::thread::sleep(Duration::from_millis(20));

        // Should be a miss now
        assert!(matches!(cache.get(&cache_key), CacheLookupResult::Miss));
    }

    #[test]
    fn test_stale_while_revalidate() {
        let config = CacheConfig {
            max_size: 100,
            default_ttl: Duration::from_millis(10),
            stale_while_revalidate: Some(Duration::from_millis(50)),
            invalidate_on_mutation: true,
        };
        let cache = ResponseCache::new(config);
        let cache_key = "stale_test".to_string();

        cache.put(
            cache_key.clone(),
            serde_json::json!({"test": true}),
            HashSet::new(),
            HashSet::new(),
        );

        // Wait past TTL but within stale window
        std::thread::sleep(Duration::from_millis(15));

        // Should be a stale hit
        assert!(matches!(cache.get(&cache_key), CacheLookupResult::Stale(_)));

        // Wait past stale window
        std::thread::sleep(Duration::from_millis(50));

        // Should be a miss now
        assert!(matches!(cache.get(&cache_key), CacheLookupResult::Miss));
    }

    #[test]
    fn test_invalidate_by_type() {
        let cache = ResponseCache::new(CacheConfig::default());

        // Add entries with type references
        cache.put(
            "key1".to_string(),
            serde_json::json!({}),
            HashSet::from(["User".to_string()]),
            HashSet::new(),
        );
        cache.put(
            "key2".to_string(),
            serde_json::json!({}),
            HashSet::from(["User".to_string()]),
            HashSet::new(),
        );
        cache.put(
            "key3".to_string(),
            serde_json::json!({}),
            HashSet::from(["Product".to_string()]),
            HashSet::new(),
        );

        assert_eq!(cache.len(), 3);

        // Invalidate User entries
        let invalidated = cache.invalidate_by_type("User");
        assert_eq!(invalidated, 2);
        assert_eq!(cache.len(), 1);

        // Product entry should still exist
        assert!(matches!(cache.get("key3"), CacheLookupResult::Hit(_)));
    }

    #[test]
    fn test_invalidate_by_entity() {
        let cache = ResponseCache::new(CacheConfig::default());

        cache.put(
            "key1".to_string(),
            serde_json::json!({}),
            HashSet::new(),
            HashSet::from(["User#123".to_string()]),
        );
        cache.put(
            "key2".to_string(),
            serde_json::json!({}),
            HashSet::new(),
            HashSet::from(["User#456".to_string()]),
        );

        assert_eq!(cache.len(), 2);

        // Invalidate specific entity
        let invalidated = cache.invalidate_by_entity("User#123");
        assert_eq!(invalidated, 1);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_lru_eviction() {
        let config = CacheConfig {
            max_size: 3,
            default_ttl: Duration::from_secs(60),
            stale_while_revalidate: None,
            invalidate_on_mutation: true,
        };
        let cache = ResponseCache::new(config);

        // Add 4 entries (exceeds capacity)
        for i in 0..4 {
            cache.put(
                format!("key{}", i),
                serde_json::json!({"num": i}),
                HashSet::new(),
                HashSet::new(),
            );
        }

        // First entry should be evicted
        assert!(matches!(cache.get("key0"), CacheLookupResult::Miss));

        // Last 3 should still be there
        assert!(matches!(cache.get("key1"), CacheLookupResult::Hit(_)));
        assert!(matches!(cache.get("key2"), CacheLookupResult::Hit(_)));
        assert!(matches!(cache.get("key3"), CacheLookupResult::Hit(_)));
    }

    #[test]
    fn test_is_mutation() {
        assert!(is_mutation("mutation { createUser { id } }"));
        assert!(is_mutation("mutation CreateUser { createUser { id } }"));
        assert!(is_mutation("  mutation { test }"));
        assert!(!is_mutation("query { users { id } }"));
        assert!(!is_mutation("{ users { id } }"));
    }

    #[test]
    fn test_extract_types() {
        let response = serde_json::json!({
            "data": {
                "user": {
                    "__typename": "User",
                    "id": "1",
                    "posts": [
                        {"__typename": "Post", "id": "10"},
                        {"__typename": "Post", "id": "11"}
                    ]
                }
            }
        });

        let types = extract_types_from_response(&response);
        assert!(types.contains("User"));
        assert!(types.contains("Post"));
        assert_eq!(types.len(), 2);
    }

    #[test]
    fn test_extract_entities() {
        let response = serde_json::json!({
            "data": {
                "user": {
                    "__typename": "User",
                    "id": "123",
                    "friend": {
                        "__typename": "User",
                        "id": "456"
                    }
                }
            }
        });

        let entities = extract_entities_from_response(&response);
        assert!(entities.contains("User#123"));
        assert!(entities.contains("User#456"));
        assert_eq!(entities.len(), 2);
    }

    #[test]
    fn test_clear_cache() {
        let cache = ResponseCache::new(CacheConfig::default());
        cache.put(
            "key1".to_string(),
            serde_json::json!({}),
            HashSet::from(["User".to_string()]),
            HashSet::from(["User#1".to_string()]),
        );

        assert_eq!(cache.len(), 1);
        cache.clear();
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }
}

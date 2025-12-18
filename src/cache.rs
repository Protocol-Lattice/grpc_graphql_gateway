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
//! 1. **Cache Key**: Generated from normalized query + variables + operation name + vary headers
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
//!         ..Default::default()
//!     })
//!     // ... other configuration
//! #   ;
//! # Ok(())
//! # }
//! ```

use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use parking_lot::RwLock;  // SECURITY: Use parking_lot for non-poisoning locks
use std::time::{Duration, SystemTime};
use serde::{Serialize, Deserialize};

/// Cache key prefix for Redis to enable safe deletion without FLUSHDB
const REDIS_CACHE_PREFIX: &str = "gqlcache:";
const REDIS_TYPE_PREFIX: &str = "gqltype:";
const REDIS_ENTITY_PREFIX: &str = "gqlentity:";

/// Configuration for response caching
///
/// # Example
///
/// ```rust
/// use grpc_graphql_gateway::CacheConfig;
/// use std::time::Duration;
///
/// let config = CacheConfig {
///     max_size: 10_000,
///     default_ttl: Duration::from_secs(60),
///     stale_while_revalidate: Some(Duration::from_secs(30)),
///     invalidate_on_mutation: true,
///     redis_url: None, // Use in-memory cache
///     vary_headers: vec!["Authorization".to_string()],
/// };
/// ```
#[derive(Clone, Debug)]
pub struct CacheConfig {
    /// Maximum number of cached responses (for in-memory cache)
    pub max_size: usize,
    /// Default TTL for cached responses
    pub default_ttl: Duration,
    /// Time to serve stale content while revalidating in background
    /// If None, stale content is never served
    pub stale_while_revalidate: Option<Duration>,
    /// Automatically invalidate cache entries when mutations are executed
    pub invalidate_on_mutation: bool,
    /// Optional Redis URL for distributed caching
    /// If provided, the gateway will use Redis instead of in-memory cache
    pub redis_url: Option<String>,
    /// Headers that should be included in the cache key (e.g. "Authorization", "X-User-Id")
    /// Defaults to ["Authorization"]
    pub vary_headers: Vec<String>,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_size: 10_000,
            default_ttl: Duration::from_secs(60),
            stale_while_revalidate: None,
            invalidate_on_mutation: true,
            redis_url: None,
            vary_headers: vec!["Authorization".to_string()],
        }
    }
}

/// A cached GraphQL response
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedResponse {
    /// The JSON response data
    pub data: serde_json::Value,
    /// When the entry was created
    #[serde(with = "serde_millis")]
    pub created_at: SystemTime,
    /// TTL for this specific entry (in seconds)
    pub ttl_secs: u64,
    /// GraphQL types referenced in this response (for invalidation)
    pub referenced_types: HashSet<String>,
    /// Entity IDs referenced in this response (for fine-grained invalidation)
    pub referenced_entities: HashSet<String>,
}

mod serde_millis {
    use std::time::{SystemTime, UNIX_EPOCH};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(time: &SystemTime, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let millis = time
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        serializer.serialize_u64(millis)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<SystemTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        let millis = u64::deserialize(deserializer)?;
        Ok(UNIX_EPOCH + std::time::Duration::from_millis(millis))
    }
}

impl CachedResponse {
    /// Check if this entry is expired
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed().unwrap_or_default() > Duration::from_secs(self.ttl_secs)
    }

    /// Check if this entry is stale but within revalidation window
    pub fn is_stale_but_usable(&self, stale_window: Duration) -> bool {
        let elapsed = self.created_at.elapsed().unwrap_or_default();
        let ttl = Duration::from_secs(self.ttl_secs);
        elapsed > ttl && elapsed <= ttl + stale_window
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

/// Thread-safe response cache with support for Memory and Redis backends
pub struct ResponseCache {
    pub config: CacheConfig,
    backend: CacheBackend,
}

enum CacheBackend {
    Memory {
        /// Main cache storage: cache_key -> cached response
        cache: RwLock<HashMap<String, CachedResponse>>,
        /// Insertion order for LRU eviction
        insertion_order: RwLock<Vec<String>>,
        /// Type index: type_name -> set of cache keys that reference this type
        type_index: RwLock<HashMap<String, HashSet<String>>>,
        /// Entity index: entity_key (Type#id) -> set of cache keys that reference this entity
        entity_index: RwLock<HashMap<String, HashSet<String>>>,
    },
    Redis {
        client: redis::Client,
    },
}

impl ResponseCache {
    /// Create a new response cache with the given configuration
    pub fn new(config: CacheConfig) -> Self {
        let backend = if let Some(url) = &config.redis_url {
            match redis::Client::open(url.as_str()) {
                Ok(client) => CacheBackend::Redis {
                    client,
                },
                Err(e) => {
                    tracing::error!("Failed to create Redis client: {}", e);
                    // Fallback to memory
                    // In a real generic app we might want to panic or result, but keeping signature compatible
                    Self::new_memory()
                }
            }
        } else {
            Self::new_memory()
        };

        Self { config, backend }
    }

    fn new_memory() -> CacheBackend {
        CacheBackend::Memory {
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
        extra_key_components: &[String],
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

        // Add extra key components (e.g. vary headers)
        for component in extra_key_components {
            hasher.update(component.as_bytes());
        }

        let result = hasher.finalize();
        hex::encode(result)
    }

    /// Get a cached response by cache key
    ///
    /// Returns `CacheLookupResult` indicating hit, stale, or miss.
    pub async fn get(&self, cache_key: &str) -> CacheLookupResult {
        match &self.backend {
            CacheBackend::Memory { cache, .. } => {
                let cache = cache.read();
        
                let Some(entry) = cache.get(cache_key) else {
                    return CacheLookupResult::Miss;
                };
        
                // Check if fresh
                if !entry.is_expired() {
                    tracing::debug!(cache_key = %cache_key, "Response cache hit (Memory)");
                    return CacheLookupResult::Hit(entry.clone());
                }
        
                // Check if stale but usable
                if let Some(stale_window) = self.config.stale_while_revalidate {
                    if entry.is_stale_but_usable(stale_window) {
                        tracing::debug!(cache_key = %cache_key, "Response cache stale hit (revalidating) (Memory)");
                        return CacheLookupResult::Stale(entry.clone());
                    }
                }
        
                // Expired and past stale window
                tracing::debug!(cache_key = %cache_key, "Response cache miss (expired) (Memory)");
                CacheLookupResult::Miss
            }
            CacheBackend::Redis { client, .. } => {
                 let mut conn = match client.get_multiplexed_async_connection().await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!("Redis connection error: {}", e);
                        return CacheLookupResult::Miss;
                    }
                };

                let data: Option<String> = match redis::cmd("GET").arg(cache_key).query_async(&mut conn).await {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::error!("Redis GET error: {}", e);
                        return CacheLookupResult::Miss;
                    }
                };

                if let Some(json) = data {
                    if let Ok(entry) = serde_json::from_str::<CachedResponse>(&json) {
                         // Check if fresh
                        if !entry.is_expired() {
                            tracing::debug!(cache_key = %cache_key, "Response cache hit (Redis)");
                            return CacheLookupResult::Hit(entry);
                        }
                        
                         // Check if stale
                        if let Some(stale_window) = self.config.stale_while_revalidate {
                            if entry.is_stale_but_usable(stale_window) {
                                tracing::debug!(cache_key = %cache_key, "Response cache stale hit (revalidating) (Redis)");
                                return CacheLookupResult::Stale(entry);
                            }
                        }
                    }
                }
                
                CacheLookupResult::Miss
            }
        }
    }

    /// Retrieve a specific entity from the cache
    pub async fn get_entity(&self, type_name: &str, id: &str) -> Option<serde_json::Value> {
        let entity_key = format!("entity:{}#{}", type_name, id);
        match self.get(&entity_key).await {
            CacheLookupResult::Hit(entry) => Some(entry.data),
            _ => None,
        }
    }

    /// Store a response in the cache
    pub async fn put(
        &self,
        cache_key: String,
        response: serde_json::Value,
        referenced_types: HashSet<String>,
        referenced_entities: HashSet<String>,
    ) {
        match &self.backend {
            CacheBackend::Memory { cache, insertion_order, type_index, entity_index, .. } => {
                // Evict if needed
                self.evict_if_needed();

                let entry = CachedResponse {
                    data: response,
                    created_at: SystemTime::now(),
                    ttl_secs: self.config.default_ttl.as_secs(),
                    referenced_types: referenced_types.clone(),
                    referenced_entities: referenced_entities.clone(),
                };

                // Store in main cache
                {
                    let mut c = cache.write();
                    c.insert(cache_key.clone(), entry);
                }

                // Update insertion order
                {
                    let mut order = insertion_order.write();
                    order.retain(|k| k != &cache_key);
                    order.push(cache_key.clone());
                }

                // Update type index
                {
                    let mut type_idx = type_index.write();
                    for type_name in &referenced_types {
                        type_idx
                            .entry(type_name.clone())
                            .or_insert_with(HashSet::new)
                            .insert(cache_key.clone());
                    }
                }

                // Update entity index
                {
                    let mut entity_idx = entity_index.write();
                    for entity_key in &referenced_entities {
                        entity_idx
                            .entry(entity_key.clone())
                            .or_insert_with(HashSet::new)
                            .insert(cache_key.clone());
                    }
                }
                 tracing::debug!(cache_key = %cache_key, "Response cached (Memory)");
            }
            CacheBackend::Redis { client, .. } => {
                let mut conn = match client.get_multiplexed_async_connection().await {
                   Ok(c) => c,
                   Err(e) => {
                       tracing::error!("Redis connection error: {}", e);
                       return;
                   }
               };
               
               let entry = CachedResponse {
                   data: response,
                   created_at: SystemTime::now(),
                   ttl_secs: self.config.default_ttl.as_secs(),
                   referenced_types: referenced_types.clone(),
                   referenced_entities: referenced_entities.clone(),
               };
               
               let json = match serde_json::to_string(&entry) {
                   Ok(j) => j,
                   Err(e) => {
                       tracing::error!("Serialization error: {}", e);
                       return;
                   }
               };
               
               // Pipeline the commands
               let mut pipe = redis::pipe();
               pipe.atomic();
               
               // SETEX cache_key ttl json
               pipe.set_ex(&cache_key, json, self.config.default_ttl.as_secs());
               
               // Add to type indexes
               for type_name in &referenced_types {
                   pipe.sadd(format!("type:{}", type_name), &cache_key);
               }
               
               // Add to entity indexes
               for entity_key in &referenced_entities {
                   pipe.sadd(format!("entity:{}", entity_key), &cache_key);
               }
               
               if let Err(e) = pipe.query_async::<()>(&mut conn).await {
                    tracing::error!("Redis PUT error: {}", e);
               } else {
                    tracing::debug!(cache_key = %cache_key, "Response cached (Redis)");
               }
            }
        }
    }

    /// Explicitly cache individual entities found in a response
    /// This allows different queries to share the same cached "nodes"
    pub async fn put_all_entities(
        &self,
        response: &serde_json::Value,
        ttl: Option<Duration>,
    ) {
        let entities = extract_entities_with_data(response);
        let ttl_secs = ttl.map(|t| t.as_secs()).unwrap_or(self.config.default_ttl.as_secs());

        for (entity_key, data) in entities {
            match &self.backend {
                CacheBackend::Memory { cache, .. } => {
                    let entry = CachedResponse {
                        data: data.clone(),
                        created_at: SystemTime::now(),
                        ttl_secs,
                        referenced_types: HashSet::new(),
                        referenced_entities: HashSet::new(),
                    };
                    cache.write().insert(format!("entity:{}", entity_key), entry);
                }
                CacheBackend::Redis { client } => {
                    let mut conn = match client.get_multiplexed_async_connection().await {
                        Ok(c) => c,
                        Err(_) => continue,
                    };
                    if let Ok(json) = serde_json::to_string(&data) {
                        let _: () = redis::cmd("SETEX")
                            .arg(format!("entity:{}", entity_key))
                            .arg(ttl_secs)
                            .arg(json)
                            .query_async(&mut conn).await.unwrap_or(());
                    }
                }
            }
        }
    }

    /// Invalidate all cache entries that reference a specific type
    pub async fn invalidate_by_type(&self, type_name: &str) -> usize {
        match &self.backend {
            CacheBackend::Memory { type_index, .. } => {
                let cache_keys = {
                    let type_idx = type_index.read();
                    type_idx.get(type_name).cloned().unwrap_or_default()
                };

                let count = cache_keys.len();
                if count > 0 {
                    self.remove_entries(&cache_keys);
                    tracing::debug!(type_name = %type_name, count = count, "Invalidated cache by type (Memory)");
                }
                count
            }
             CacheBackend::Redis { client, .. } => {
                let mut conn = match client.get_multiplexed_async_connection().await {
                    Ok(c) => c,
                    Err(_) => return 0,
                };
                
                let index_key = format!("type:{}", type_name);
                let keys: Vec<String> = match redis::cmd("SMEMBERS").arg(&index_key).query_async(&mut conn).await {
                    Ok(k) => k,
                    Err(_) => return 0,
                };
                
                if keys.is_empty() {
                    return 0;
                }
                
                let mut pipe = redis::pipe();
                pipe.atomic();
                for key in &keys {
                    pipe.del(key);
                }
                pipe.del(&index_key);
                
                let _: () = pipe.query_async(&mut conn).await.unwrap_or(());
                 tracing::debug!(type_name = %type_name, count = keys.len(), "Invalidated cache by type (Redis)");
                keys.len()
             }
        }
    }

    /// Invalidate all cache entries that reference a specific entity
    pub async fn invalidate_by_entity(&self, entity_key: &str) -> usize {
         match &self.backend {
             CacheBackend::Memory { entity_index, .. } => {
                let cache_keys = {
                    let entity_idx = entity_index.read();
                    entity_idx.get(entity_key).cloned().unwrap_or_default()
                };

                let count = cache_keys.len();
                if count > 0 {
                    self.remove_entries(&cache_keys);
                    tracing::debug!(entity_key = %entity_key, count = count, "Invalidated cache by entity (Memory)");
                }
                count
             }
             CacheBackend::Redis { client, .. } => {
                let mut conn = match client.get_multiplexed_async_connection().await {
                    Ok(c) => c,
                    Err(_) => return 0,
                };
                
                let index_key = format!("entity:{}", entity_key);
                let keys: Vec<String> = match redis::cmd("SMEMBERS").arg(&index_key).query_async(&mut conn).await {
                    Ok(k) => k,
                    Err(_) => return 0,
                };
                
                if keys.is_empty() {
                    return 0;
                }
                
                let mut pipe = redis::pipe();
                pipe.atomic();
                for key in &keys {
                    pipe.del(key);
                }
                pipe.del(&index_key);
                
                let _: () = pipe.query_async(&mut conn).await.unwrap_or(());
                 tracing::debug!(entity_key = %entity_key, count = keys.len(), "Invalidated cache by entity (Redis)");
                keys.len()
             }
         }
    }

    /// Invalidate cache entries based on a mutation result
    pub async fn invalidate_for_mutation(&self, mutation_response: &serde_json::Value) -> usize {
        if !self.config.invalidate_on_mutation {
            return 0;
        }

        let mut total_invalidated = 0;

        // Extract types from the mutation response
        let types = extract_types_from_response(mutation_response);
        for type_name in types {
            total_invalidated += self.invalidate_by_type(&type_name).await;
        }

        // Extract entity keys (Type#id patterns)
        let entities = extract_entities_from_response(mutation_response);
        for entity_key in entities {
            total_invalidated += self.invalidate_by_entity(&entity_key).await;
        }

        if total_invalidated > 0 {
            tracing::info!(count = total_invalidated, "Cache invalidated after mutation");
        }

        total_invalidated
    }

    /// Clear all cached responses
    ///
    /// # Security
    ///
    /// For Redis, this uses SCAN with key prefixes instead of FLUSHDB
    /// to avoid destroying data from other applications sharing the same Redis instance.
    pub async fn clear(&self) {
        match &self.backend {
            CacheBackend::Memory { cache, insertion_order, type_index, entity_index, .. } => {
                cache.write().clear();
                insertion_order.write().clear();
                type_index.write().clear();
                entity_index.write().clear();
                tracing::debug!("Response cache cleared (Memory)");
            }
            CacheBackend::Redis { client, .. } => {
                let mut conn = match client.get_multiplexed_async_connection().await {
                    Ok(c) => c,
                    Err(_) => return,
                };
                
                // SECURITY: Use SCAN with prefix instead of FLUSHDB
                // This protects other applications sharing the same Redis instance
                let mut cursor: u64 = 0;
                let mut total_deleted = 0;
                
                loop {
                    // Scan for cache keys with our prefix
                    let (next_cursor, keys): (u64, Vec<String>) = match redis::cmd("SCAN")
                        .arg(cursor)
                        .arg("MATCH")
                        .arg(format!("{}*", REDIS_CACHE_PREFIX))
                        .arg("COUNT")
                        .arg(100)
                        .query_async(&mut conn)
                        .await
                    {
                        Ok(result) => result,
                        Err(e) => {
                            tracing::error!("Redis SCAN error: {}", e);
                            break;
                        }
                    };
                    
                    if !keys.is_empty() {
                        let _: () = redis::cmd("DEL")
                            .arg(&keys)
                            .query_async(&mut conn)
                            .await
                            .unwrap_or(());
                        total_deleted += keys.len();
                    }
                    
                    cursor = next_cursor;
                    if cursor == 0 {
                        break;
                    }
                }
                
                // Also clean up type and entity indexes
                for prefix in [REDIS_TYPE_PREFIX, REDIS_ENTITY_PREFIX] {
                    cursor = 0;
                    loop {
                        let (next_cursor, keys): (u64, Vec<String>) = match redis::cmd("SCAN")
                            .arg(cursor)
                            .arg("MATCH")
                            .arg(format!("{}*", prefix))
                            .arg("COUNT")
                            .arg(100)
                            .query_async(&mut conn)
                            .await
                        {
                            Ok(result) => result,
                            Err(_) => break,
                        };
                        
                        if !keys.is_empty() {
                            let _: () = redis::cmd("DEL")
                                .arg(&keys)
                                .query_async(&mut conn)
                                .await
                                .unwrap_or(());
                            total_deleted += keys.len();
                        }
                        
                        cursor = next_cursor;
                        if cursor == 0 {
                            break;
                        }
                    }
                }
                
                tracing::debug!("Response cache cleared - {} keys deleted (Redis SCAN)", total_deleted);
            }
        }
    }

    /// Get the current number of cached responses
    pub fn len(&self) -> usize {
        match &self.backend {
             CacheBackend::Memory { cache, .. } => {
                cache.read().len()
             }
             CacheBackend::Redis { .. } => 0, // Not easily available without SCAN, return 0
        }
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
            type_index_size: match &self.backend {
                 CacheBackend::Memory { type_index, .. } => type_index.read().len(),
                 _ => 0,
            },
            entity_index_size: match &self.backend {
                 CacheBackend::Memory { entity_index, .. } => entity_index.read().len(),
                 _ => 0,
            },
        }
    }

    fn evict_if_needed(&self) {
        match &self.backend {
            CacheBackend::Memory { .. } => {
                let current_len = self.len();
                if current_len >= self.config.max_size {
                    // Logic continues below...
                    let to_remove = current_len - self.config.max_size + 1;
                    if let CacheBackend::Memory { insertion_order, .. } = &self.backend {
                        let mut order = insertion_order.write();
                        let drain_count = to_remove.min(order.len());
                        let keys_to_remove: Vec<String> = order.drain(..drain_count).collect();
                        drop(order); // Release lock before calling remove_entries_internal
                        self.remove_entries_internal(&keys_to_remove);
                    }
                }
            }
            _ => {} // Redis handles eviction (maxmemory policy)
        }
    }

    fn remove_entries(&self, cache_keys: &HashSet<String>) {
         match &self.backend {
            CacheBackend::Memory { insertion_order, .. } => {
                let keys_vec: Vec<String> = cache_keys.iter().cloned().collect();
                self.remove_entries_internal(&keys_vec);
        
                // Update insertion order
                let mut order = insertion_order.write();
                order.retain(|k| !cache_keys.contains(k));
            }
             _ => {} // Handled elsewhere for Redis
         }
    }

    fn remove_entries_internal(&self, cache_keys: &[String]) {
        match &self.backend {
             CacheBackend::Memory { cache, type_index, entity_index, .. } => {
                let mut c = cache.write();
                for key in cache_keys {
                    if let Some(entry) = c.remove(key) {
                        // Clean up type index
                        let mut type_idx = type_index.write();
                        for type_name in &entry.referenced_types {
                            if let Some(keys) = type_idx.get_mut(type_name) {
                                keys.remove(key);
                                if keys.is_empty() {
                                    type_idx.remove(type_name);
                                }
                            }
                        }
                        drop(type_idx);
                        
                        // Clean up entity index
                        let mut entity_idx = entity_index.write();
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
             _ => {}
        }
    }
}

impl Clone for ResponseCache {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            backend: match &self.backend {
                CacheBackend::Memory { cache, insertion_order, type_index, entity_index } => {
                     let cache = cache.read().clone();
                    let order = insertion_order.read().clone();
                    let type_idx = type_index.read().clone();
                    let entity_idx = entity_index.read().clone();
                    CacheBackend::Memory {
                        cache: RwLock::new(cache),
                        insertion_order: RwLock::new(order),
                        type_index: RwLock::new(type_idx),
                        entity_index: RwLock::new(entity_idx),
                    }
                }
                CacheBackend::Redis { client } => CacheBackend::Redis {
                    client: client.clone(),
                }
            }
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

/// Extract entity keys (Type#id) and their corresponding data from a response
fn extract_entities_with_data(response: &serde_json::Value) -> Vec<(String, serde_json::Value)> {
    let mut entities = Vec::new();
    extract_entities_data_recursive(response, &mut entities);
    entities
}

fn extract_entities_data_recursive(value: &serde_json::Value, entities: &mut Vec<(String, serde_json::Value)>) {
    match value {
        serde_json::Value::Object(map) => {
            let type_name = map.get("__typename").and_then(|t| t.as_str());
            let id = map
                .get("id")
                .and_then(|i| i.as_str())
                .or_else(|| map.get("_id").and_then(|i| i.as_str()));

            if let (Some(tn), Some(id_val)) = (type_name, id) {
                entities.push((format!("{}#{}", tn, id_val), value.clone()));
            }

            for v in map.values() {
                extract_entities_data_recursive(v, entities);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                extract_entities_data_recursive(item, entities);
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
        let key1 = ResponseCache::generate_cache_key(query, None, None, &[]);

        // Same query should produce same key
        assert_eq!(key1, ResponseCache::generate_cache_key(query, None, None, &[]));

        // Different query should produce different key
        let key2 = ResponseCache::generate_cache_key("{ products { id } }", None, None, &[]);
        assert_ne!(key1, key2);

        // Whitespace normalized
        let key3 = ResponseCache::generate_cache_key("{ users { id   name } }", None, None, &[]);
        assert_eq!(key1, key3);
    }

    #[test]
    fn test_cache_key_with_variables() {
        let query = "query GetUser($id: ID!) { user(id: $id) { name } }";
        let vars1 = serde_json::json!({"id": "123"});
        let vars2 = serde_json::json!({"id": "456"});

        let key1 = ResponseCache::generate_cache_key(query, Some(&vars1), None, &[]);
        let key2 = ResponseCache::generate_cache_key(query, Some(&vars2), None, &[]);

        assert_ne!(key1, key2);
    }

    #[test]
    fn test_cache_key_with_vary_headers() {
        let query = "{ users { id name } }";
        
        // Simulating headers: "HeaderName:HeaderValue"
        let headers1 = vec!["Authorization:Bearer TokenA".to_string()];
        let headers2 = vec!["Authorization:Bearer TokenB".to_string()];
        
        let key1 = ResponseCache::generate_cache_key(query, None, None, &headers1);
        let key2 = ResponseCache::generate_cache_key(query, None, None, &headers2);
        let key3 = ResponseCache::generate_cache_key(query, None, None, &[]);

        // Different auth tokens should produce different keys
        assert_ne!(key1, key2);
        
        // Auth token vs no auth token should produce different keys
        assert_ne!(key1, key3);
        
        // Same auth token should produce same key
        let key1_again = ResponseCache::generate_cache_key(query, None, None, &headers1);
        assert_eq!(key1, key1_again);
    }

    #[tokio::test]
    async fn test_cache_put_and_get() {
        let cache = ResponseCache::new(CacheConfig::default());
        let cache_key = "test_key".to_string();
        let response = serde_json::json!({"data": {"user": {"id": "1", "name": "Alice"}}});

        cache.put(
            cache_key.clone(),
            response.clone(),
            HashSet::from(["User".to_string()]),
            HashSet::from(["User#1".to_string()]),
        ).await;

        match cache.get(&cache_key).await {
            CacheLookupResult::Hit(entry) => {
                assert_eq!(entry.data, response);
            }
            _ => panic!("Expected cache hit"),
        }
    }

    #[tokio::test]
    async fn test_cache_miss() {
        let cache = ResponseCache::new(CacheConfig::default());
        match cache.get("nonexistent").await {
            CacheLookupResult::Miss => {}
            _ => panic!("Expected cache miss"),
        }
    }

    #[tokio::test]
    async fn test_ttl_expiration() {
        let config = CacheConfig {
            max_size: 100,
            default_ttl: Duration::from_secs(1),
            stale_while_revalidate: None,
            invalidate_on_mutation: true,
            redis_url: None,
            vary_headers: vec![],
        };
        let cache = ResponseCache::new(config);
        let cache_key = "expiring".to_string();

        cache.put(
            cache_key.clone(),
            serde_json::json!({"test": true}),
            HashSet::new(),
            HashSet::new(),
        ).await;

        // Should be a hit immediately
        assert!(matches!(cache.get(&cache_key).await, CacheLookupResult::Hit(_)));

        // Wait for TTL
        std::thread::sleep(Duration::from_millis(1100));

        // Should be a miss now
        assert!(matches!(cache.get(&cache_key).await, CacheLookupResult::Miss));
    }

    #[tokio::test]
    async fn test_stale_while_revalidate() {
        let config = CacheConfig {
            max_size: 100,
            default_ttl: Duration::from_millis(10),
            stale_while_revalidate: Some(Duration::from_millis(50)),
            invalidate_on_mutation: true,
            redis_url: None,
            vary_headers: vec![],
        };
        let cache = ResponseCache::new(config);
        let cache_key = "stale_test".to_string();

        cache.put(
            cache_key.clone(),
            serde_json::json!({"test": true}),
            HashSet::new(),
            HashSet::new(),
        ).await;

        // Wait past TTL but within stale window
        std::thread::sleep(Duration::from_millis(15));

        // Should be a stale hit
        assert!(matches!(cache.get(&cache_key).await, CacheLookupResult::Stale(_)));

        // Wait past stale window
        std::thread::sleep(Duration::from_millis(50));

        // Should be a miss now
        assert!(matches!(cache.get(&cache_key).await, CacheLookupResult::Miss));
    }

    #[tokio::test]
    async fn test_invalidate_by_type() {
        let cache = ResponseCache::new(CacheConfig::default());

        // Add entries with type references
        cache.put(
            "key1".to_string(),
            serde_json::json!({}),
            HashSet::from(["User".to_string()]),
            HashSet::new(),
        ).await;
        cache.put(
            "key2".to_string(),
            serde_json::json!({}),
            HashSet::from(["User".to_string()]),
            HashSet::new(),
        ).await;
        cache.put(
            "key3".to_string(),
            serde_json::json!({}),
            HashSet::from(["Product".to_string()]),
            HashSet::new(),
        ).await;

        assert_eq!(cache.len(), 3);

        // Invalidate User entries
        let invalidated = cache.invalidate_by_type("User").await;
        assert_eq!(invalidated, 2);
        assert_eq!(cache.len(), 1);

        // Product entry should still exist
        assert!(matches!(cache.get("key3").await, CacheLookupResult::Hit(_)));
    }

    #[tokio::test]
    async fn test_invalidate_by_entity() {
        let cache = ResponseCache::new(CacheConfig::default());

        cache.put(
            "key1".to_string(),
            serde_json::json!({}),
            HashSet::new(),
            HashSet::from(["User#123".to_string()]),
        ).await;
        cache.put(
            "key2".to_string(),
            serde_json::json!({}),
            HashSet::new(),
            HashSet::from(["User#456".to_string()]),
        ).await;

        assert_eq!(cache.len(), 2);

        // Invalidate specific entity
        let invalidated = cache.invalidate_by_entity("User#123").await;
        assert_eq!(invalidated, 1);
        assert_eq!(cache.len(), 1);
    }

    #[tokio::test]
    async fn test_lru_eviction() {
        let config = CacheConfig {
            max_size: 3,
            default_ttl: Duration::from_secs(60),
            stale_while_revalidate: None,
            invalidate_on_mutation: true,
            redis_url: None,
            vary_headers: vec![],
        };
        let cache = ResponseCache::new(config);

        // Add 4 entries (exceeds capacity)
        for i in 0..4 {
            cache.put(
                format!("key{}", i),
                serde_json::json!({"num": i}),
                HashSet::new(),
                HashSet::new(),
            ).await;
        }

        // First entry should be evicted
        assert!(matches!(cache.get("key0").await, CacheLookupResult::Miss));

        // Last 3 should still be there
        assert!(matches!(cache.get("key1").await, CacheLookupResult::Hit(_)));
        assert!(matches!(cache.get("key2").await, CacheLookupResult::Hit(_)));
        assert!(matches!(cache.get("key3").await, CacheLookupResult::Hit(_)));
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

    #[tokio::test]
    async fn test_clear_cache() {
        let cache = ResponseCache::new(CacheConfig::default());
        cache.put(
            "key1".to_string(),
            serde_json::json!({}),
            HashSet::from(["User".to_string()]),
            HashSet::from(["User#1".to_string()]),
        ).await;

        assert_eq!(cache.len(), 1);
        cache.clear().await;
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    #[tokio::test]
    async fn test_put_all_entities() {
        let cache = ResponseCache::new(CacheConfig::default());
        let response = serde_json::json!({
            "data": {
                "user": {
                    "__typename": "User",
                    "id": "123",
                    "name": "Alice",
                    "friend": {
                        "__typename": "User",
                        "id": "456",
                        "name": "Bob"
                    }
                }
            }
        });

        cache.put_all_entities(&response, None).await;

        // Verify "Alice" is cached by field/ID
        let alice = cache.get_entity("User", "123").await.expect("Alice should be cached");
        assert_eq!(alice["name"], "Alice");

        // Verify nested "Bob" is also cached by field/ID
        let bob = cache.get_entity("User", "456").await.expect("Bob should be cached");
        assert_eq!(bob["name"], "Bob");
    }
}

//! Live Query / Reactive Subscriptions support for GraphQL.
//!
//! This module enables automatic data updates when underlying data changes.
//! When a client sends a query with the `@live` directive, the gateway will:
//!
//! 1. Execute the query immediately and return the result
//! 2. Track the query and its data dependencies
//! 3. Re-execute and push updates when any dependency is invalidated
//! 4. Throttle updates to prevent flooding clients
//!
//! # Example
//!
//! ```graphql
//! query @live {
//!   user(id: "1") {
//!     name
//!     status
//!   }
//! }
//! ```
//!
//! # Configuration
//!
//! Live queries are configured in proto files using the `graphql.live_query` extension:
//!
//! ```protobuf
//! service UserService {
//!    rpc GetUser(GetUserRequest) returns (User) {
//!      option (graphql.schema) = {
//!        type: QUERY
//!        name: "user"
//!      };
//!      option (graphql.live_query) = {
//!        enabled: true
//!        throttle_ms: 100
//!        triggers: ["User.update", "User.delete"]
//!      };
//!    }
//! }
//! ```

use ahash::AHashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info};

/// Configuration for live query behavior.
#[derive(Debug, Clone)]
pub struct LiveQueryConfig {
    /// Global throttle interval (default: 100ms)
    pub default_throttle_ms: u32,
    /// Maximum live queries per WebSocket connection (default: 10)
    pub max_per_connection: u32,
    /// Default TTL in seconds (0 = infinite)
    pub default_ttl_seconds: u32,
    /// Maximum total live queries across all connections (default: 10000)
    pub max_total: usize,
    /// Enable hash-based change detection (default: true)
    pub enable_hash_diff: bool,
    /// Channel buffer size for invalidation events (default: 1000)
    pub invalidation_buffer_size: usize,
}

impl Default for LiveQueryConfig {
    fn default() -> Self {
        Self {
            default_throttle_ms: 100,
            max_per_connection: 10,
            default_ttl_seconds: 0,
            max_total: 10000,
            enable_hash_diff: true,
            invalidation_buffer_size: 1000,
        }
    }
}

impl LiveQueryConfig {
    /// Create a strict configuration for production
    pub fn production() -> Self {
        Self {
            default_throttle_ms: 200,
            max_per_connection: 5,
            default_ttl_seconds: 3600, // 1 hour
            max_total: 5000,
            enable_hash_diff: true,
            invalidation_buffer_size: 500,
        }
    }

    /// Create a permissive configuration for development
    pub fn development() -> Self {
        Self {
            default_throttle_ms: 50,
            max_per_connection: 50,
            default_ttl_seconds: 0,
            max_total: 50000,
            enable_hash_diff: true,
            invalidation_buffer_size: 2000,
        }
    }
}

/// Strategy for detecting when to push live query updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LiveQueryStrategy {
    /// Push updates when invalidation triggers fire (recommended)
    Invalidation,
    /// Poll the underlying data source at regular intervals
    Polling,
    /// Compare response hashes to detect changes
    HashDiff,
}

impl Default for LiveQueryStrategy {
    fn default() -> Self {
        Self::Invalidation
    }
}

impl std::str::FromStr for LiveQueryStrategy {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "INVALIDATION" => Ok(Self::Invalidation),
            "POLLING" => Ok(Self::Polling),
            "HASH_DIFF" | "HASHDIFF" => Ok(Self::HashDiff),
            _ => Err(()),
        }
    }
}

/// An invalidation event that triggers live query updates.
#[derive(Debug, Clone)]
pub struct InvalidationEvent {
    /// Type name (e.g., "User", "Product")
    pub type_name: String,
    /// Action (e.g., "create", "update", "delete", "*")
    pub action: String,
    /// Optional entity ID for targeted invalidation
    pub entity_id: Option<String>,
    /// Timestamp of the event
    pub timestamp: Instant,
}

impl InvalidationEvent {
    /// Create a new invalidation event
    pub fn new(type_name: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            type_name: type_name.into(),
            action: action.into(),
            entity_id: None,
            timestamp: Instant::now(),
        }
    }

    /// Create an invalidation event for a specific entity
    pub fn for_entity(
        type_name: impl Into<String>,
        action: impl Into<String>,
        entity_id: impl Into<String>,
    ) -> Self {
        Self {
            type_name: type_name.into(),
            action: action.into(),
            entity_id: Some(entity_id.into()),
            timestamp: Instant::now(),
        }
    }

    /// Check if this event matches a trigger pattern
    pub fn matches(&self, trigger: &str) -> bool {
        let parts: Vec<&str> = trigger.split('.').collect();
        if parts.len() != 2 {
            return false;
        }

        let (trigger_type, trigger_action) = (parts[0], parts[1]);

        // Type must match exactly or be wildcard
        let type_matches = trigger_type == "*" || trigger_type == self.type_name;

        // Action must match exactly or be wildcard
        let action_matches = trigger_action == "*" || trigger_action == self.action;

        type_matches && action_matches
    }
}

/// Information about an active live query subscription.
#[derive(Debug, Clone)]
pub struct ActiveLiveQuery {
    /// Unique subscription ID
    pub id: String,
    /// The GraphQL operation name
    pub operation_name: String,
    /// The full query string
    pub query: String,
    /// Query variables
    pub variables: Option<serde_json::Value>,
    /// Invalidation triggers this query responds to
    pub triggers: Vec<String>,
    /// Throttle interval
    pub throttle_ms: u32,
    /// TTL (0 = infinite)
    pub ttl_seconds: u32,
    /// Strategy for updates
    pub strategy: LiveQueryStrategy,
    /// Poll interval for POLLING strategy
    pub poll_interval_ms: u32,
    /// Last result hash (for HASH_DIFF strategy)
    pub last_hash: Option<String>,
    /// Last update time
    pub last_update: Instant,
    /// Creation time
    pub created_at: Instant,
    /// Client connection ID
    pub connection_id: String,
}

impl ActiveLiveQuery {
    /// Check if this query should be updated based on an invalidation event
    pub fn should_update(&self, event: &InvalidationEvent) -> bool {
        // Check if any trigger matches the event
        self.triggers.iter().any(|trigger| event.matches(trigger))
    }

    /// Check if the throttle period has passed
    pub fn throttle_elapsed(&self) -> bool {
        self.last_update.elapsed() >= Duration::from_millis(self.throttle_ms as u64)
    }

    /// Check if the TTL has expired
    pub fn is_expired(&self) -> bool {
        if self.ttl_seconds == 0 {
            return false;
        }
        self.created_at.elapsed() >= Duration::from_secs(self.ttl_seconds as u64)
    }

    /// Generate a cache key for this query
    pub fn cache_key(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.query.as_bytes());
        if let Some(vars) = &self.variables {
            hasher.update(vars.to_string().as_bytes());
        }
        format!("{:x}", hasher.finalize())
    }
}

/// Message sent to live query subscribers
#[derive(Debug, Clone, Serialize)]
pub struct LiveQueryUpdate {
    /// Subscription ID
    pub id: String,
    /// The updated data
    pub data: serde_json::Value,
    /// Whether this is the initial result
    pub is_initial: bool,
    /// Revision number (increments with each update)
    pub revision: u64,
}

/// Statistics about live query usage
#[derive(Debug, Clone, Default)]
pub struct LiveQueryStats {
    /// Total active live queries
    pub active_count: usize,
    /// Total updates pushed
    pub total_updates: u64,
    /// Total invalidation events processed
    pub total_invalidations: u64,
    /// Queries expired due to TTL
    pub expired_count: u64,
    /// Queries cancelled by client
    pub cancelled_count: u64,
}

/// The main live query store that manages all active subscriptions.
pub struct LiveQueryStore {
    config: LiveQueryConfig,
    /// Active live queries indexed by subscription ID
    queries: RwLock<AHashMap<String, ActiveLiveQuery>>,
    /// Index: trigger pattern -> set of subscription IDs
    trigger_index: RwLock<AHashMap<String, HashSet<String>>>,
    /// Index: connection ID -> set of subscription IDs
    connection_index: RwLock<AHashMap<String, HashSet<String>>>,
    /// Channels to send updates to subscribers
    update_senders: RwLock<AHashMap<String, mpsc::Sender<LiveQueryUpdate>>>,
    /// Broadcast channel for invalidation events
    invalidation_tx: broadcast::Sender<InvalidationEvent>,
    /// Statistics
    stats: LiveQueryStats,
    /// Atomic counters
    update_counter: AtomicU64,
    invalidation_counter: AtomicU64,
}

impl LiveQueryStore {
    /// Create a new live query store with default configuration
    pub fn new() -> Self {
        Self::with_config(LiveQueryConfig::default())
    }

    /// Create a new live query store with custom configuration
    pub fn with_config(config: LiveQueryConfig) -> Self {
        let (invalidation_tx, _) = broadcast::channel(config.invalidation_buffer_size);
        Self {
            config,
            queries: RwLock::new(AHashMap::new()),
            trigger_index: RwLock::new(AHashMap::new()),
            connection_index: RwLock::new(AHashMap::new()),
            update_senders: RwLock::new(AHashMap::new()),
            invalidation_tx,
            stats: LiveQueryStats::default(),
            update_counter: AtomicU64::new(0),
            invalidation_counter: AtomicU64::new(0),
        }
    }

    /// Register a new live query subscription
    pub fn register(
        &self,
        query: ActiveLiveQuery,
        sender: mpsc::Sender<LiveQueryUpdate>,
    ) -> Result<(), LiveQueryError> {
        let queries = self.queries.read();
        
        // Check global limit
        if queries.len() >= self.config.max_total {
            return Err(LiveQueryError::TooManyQueries {
                current: queries.len(),
                max: self.config.max_total,
            });
        }
        drop(queries);

        // Check per-connection limit
        let connection_index = self.connection_index.read();
        if let Some(conn_queries) = connection_index.get(&query.connection_id) {
            if conn_queries.len() >= self.config.max_per_connection as usize {
                return Err(LiveQueryError::TooManyQueriesPerConnection {
                    current: conn_queries.len(),
                    max: self.config.max_per_connection as usize,
                });
            }
        }
        drop(connection_index);

        let id = query.id.clone();
        let connection_id = query.connection_id.clone();
        let triggers = query.triggers.clone();

        // Register query
        self.queries.write().insert(id.clone(), query);

        // Update trigger index
        {
            let mut trigger_index = self.trigger_index.write();
            for trigger in &triggers {
                trigger_index
                    .entry(trigger.clone())
                    .or_default()
                    .insert(id.clone());
            }
        }

        // Update connection index
        {
            let mut connection_index = self.connection_index.write();
            connection_index
                .entry(connection_id)
                .or_default()
                .insert(id.clone());
        }

        // Store the sender
        self.update_senders.write().insert(id.clone(), sender);

        debug!(subscription_id = %id, "Registered live query");
        Ok(())
    }

    /// Unregister a live query subscription
    pub fn unregister(&self, subscription_id: &str) -> Option<ActiveLiveQuery> {
        let query = self.queries.write().remove(subscription_id)?;

        // Remove from trigger index
        {
            let mut trigger_index = self.trigger_index.write();
            for trigger in &query.triggers {
                if let Some(ids) = trigger_index.get_mut(trigger) {
                    ids.remove(subscription_id);
                    if ids.is_empty() {
                        trigger_index.remove(trigger);
                    }
                }
            }
        }

        // Remove from connection index
        {
            let mut connection_index = self.connection_index.write();
            if let Some(ids) = connection_index.get_mut(&query.connection_id) {
                ids.remove(subscription_id);
                if ids.is_empty() {
                    connection_index.remove(&query.connection_id);
                }
            }
        }

        // Remove sender
        self.update_senders.write().remove(subscription_id);

        debug!(subscription_id = %subscription_id, "Unregistered live query");
        Some(query)
    }

    /// Unregister all live queries for a connection
    pub fn unregister_connection(&self, connection_id: &str) -> Vec<ActiveLiveQuery> {
        let ids: Vec<String> = {
            let connection_index = self.connection_index.read();
            connection_index
                .get(connection_id)
                .map(|ids| ids.iter().cloned().collect())
                .unwrap_or_default()
        };

        ids.iter()
            .filter_map(|id| self.unregister(id))
            .collect()
    }

    /// Broadcast an invalidation event
    pub fn invalidate(&self, event: InvalidationEvent) -> usize {
        self.invalidation_counter.fetch_add(1, Ordering::Relaxed);

        // Find affected subscriptions
        let mut affected = HashSet::new();
        
        // Check exact trigger matches
        let trigger_pattern = format!("{}.{}", event.type_name, event.action);
        let wildcard_type = format!("*.{}", event.action);
        let wildcard_action = format!("{}.*", event.type_name);
        let wildcard_all = "*.*".to_string();

        let trigger_index = self.trigger_index.read();
        for pattern in [trigger_pattern, wildcard_type, wildcard_action, wildcard_all] {
            if let Some(ids) = trigger_index.get(&pattern) {
                affected.extend(ids.iter().cloned());
            }
        }
        drop(trigger_index);

        let affected_count = affected.len();
        debug!(
            type_name = %event.type_name,
            action = %event.action,
            affected_count = affected_count,
            "Broadcasting invalidation event"
        );

        // Broadcast the event
        let _ = self.invalidation_tx.send(event);

        affected_count
    }

    /// Send an update to a specific subscription
    pub async fn send_update(
        &self,
        subscription_id: &str,
        data: serde_json::Value,
        is_initial: bool,
    ) -> Result<(), LiveQueryError> {
        // Get sender
        let sender = {
            let senders = self.update_senders.read();
            senders.get(subscription_id).cloned()
        };

        let sender = sender.ok_or_else(|| LiveQueryError::SubscriptionNotFound {
            id: subscription_id.to_string(),
        })?;

        // Update the query's last_update time
        {
            let mut queries = self.queries.write();
            if let Some(query) = queries.get_mut(subscription_id) {
                query.last_update = Instant::now();
            }
        }

        let revision = self.update_counter.fetch_add(1, Ordering::Relaxed);

        let update = LiveQueryUpdate {
            id: subscription_id.to_string(),
            data,
            is_initial,
            revision,
        };

        sender
            .send(update)
            .await
            .map_err(|_| LiveQueryError::ChannelClosed)
    }

    /// Subscribe to invalidation events
    pub fn subscribe_invalidations(&self) -> broadcast::Receiver<InvalidationEvent> {
        self.invalidation_tx.subscribe()
    }

    /// Get a live query by ID
    pub fn get(&self, subscription_id: &str) -> Option<ActiveLiveQuery> {
        self.queries.read().get(subscription_id).cloned()
    }

    /// Get all live queries for a connection
    pub fn get_for_connection(&self, connection_id: &str) -> Vec<ActiveLiveQuery> {
        let connection_index = self.connection_index.read();
        let ids = connection_index.get(connection_id);
        
        if let Some(ids) = ids {
            let queries = self.queries.read();
            ids.iter()
                .filter_map(|id| queries.get(id).cloned())
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Check if a query operation supports live mode
    pub fn is_live_enabled(&self, operation_name: &str, live_query_configs: &AHashMap<String, LiveQueryOperationConfig>) -> bool {
        live_query_configs.contains_key(operation_name)
    }

    /// Get configuration for a live query operation
    pub fn get_operation_config<'a>(
        &self,
        operation_name: &str,
        live_query_configs: &'a AHashMap<String, LiveQueryOperationConfig>,
    ) -> Option<&'a LiveQueryOperationConfig> {
        live_query_configs.get(operation_name)
    }

    /// Get current statistics
    pub fn stats(&self) -> LiveQueryStats {
        let active_count = self.queries.read().len();
        LiveQueryStats {
            active_count,
            total_updates: self.update_counter.load(Ordering::Relaxed),
            total_invalidations: self.invalidation_counter.load(Ordering::Relaxed),
            ..Default::default()
        }
    }

    /// Prune expired queries
    pub fn prune_expired(&self) -> Vec<String> {
        let expired_ids: Vec<String> = {
            let queries = self.queries.read();
            queries
                .iter()
                .filter(|(_, q)| q.is_expired())
                .map(|(id, _)| id.clone())
                .collect()
        };

        for id in &expired_ids {
            self.unregister(id);
        }

        if !expired_ids.is_empty() {
            info!(count = expired_ids.len(), "Pruned expired live queries");
        }

        expired_ids
    }
}

impl Default for LiveQueryStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe shared live query store
pub type SharedLiveQueryStore = Arc<LiveQueryStore>;

lazy_static::lazy_static! {
    /// Global live query store singleton
    /// All WebSocket connections and mutations share this store
    static ref GLOBAL_LIVE_QUERY_STORE: SharedLiveQueryStore = Arc::new(LiveQueryStore::new());
}

/// Get the global live query store
/// 
/// This returns a reference to the singleton store that is shared across
/// all WebSocket connections and can be used by mutations to trigger invalidations.
pub fn global_live_query_store() -> SharedLiveQueryStore {
    GLOBAL_LIVE_QUERY_STORE.clone()
}

/// Create a new shared live query store
pub fn create_live_query_store() -> SharedLiveQueryStore {
    global_live_query_store()
}

/// Create a shared live query store with custom configuration
/// Note: This creates a NEW store, not the global one
pub fn create_live_query_store_with_config(config: LiveQueryConfig) -> SharedLiveQueryStore {
    Arc::new(LiveQueryStore::with_config(config))
}

/// Configuration info for a live query operation (generated from proto)
#[derive(Debug, Clone)]
pub struct LiveQueryConfigInfo {
    /// GraphQL operation name
    pub operation_name: &'static str,
    /// Throttle interval in milliseconds
    pub throttle_ms: u32,
    /// Invalidation triggers
    pub triggers: &'static [&'static str],
    /// Maximum connections per client
    pub max_connections: u32,
    /// TTL in seconds (0 = infinite)
    pub ttl_seconds: u32,
    /// Strategy name
    pub strategy: &'static str,
    /// Poll interval for POLLING strategy
    pub poll_interval_ms: u32,
    /// Entity dependencies
    pub depends_on: &'static [&'static str],
}

/// Runtime configuration for a live query operation (loaded from proto)
#[derive(Debug, Clone)]
pub struct LiveQueryOperationConfig {
    /// GraphQL operation name
    pub operation_name: String,
    /// Enabled status
    pub enabled: bool,
    /// Throttle interval in milliseconds
    pub throttle_ms: u32,
    /// Invalidation triggers
    pub triggers: Vec<String>,
    /// Maximum connections per client
    pub max_connections: u32,
    /// TTL in seconds (0 = infinite)
    pub ttl_seconds: u32,
    /// Strategy
    pub strategy: LiveQueryStrategy,
    /// Poll interval for POLLING strategy
    pub poll_interval_ms: u32,
    /// Entity dependencies
    pub depends_on: Vec<String>,
}

/// Errors that can occur during live query operations
#[derive(Debug, thiserror::Error)]
pub enum LiveQueryError {
    #[error("Too many live queries ({current}/{max})")]
    TooManyQueries { current: usize, max: usize },

    #[error("Too many live queries per connection ({current}/{max})")]
    TooManyQueriesPerConnection { current: usize, max: usize },

    #[error("Subscription not found: {id}")]
    SubscriptionNotFound { id: String },

    #[error("Channel closed")]
    ChannelClosed,

    #[error("Operation not supported for live queries: {operation}")]
    NotSupported { operation: String },

    #[error("Query execution failed: {message}")]
    ExecutionFailed { message: String },
}

/// Parse `@live` directive from a GraphQL query
// Parse @live directive from a GraphQL query
// Parse @live directive from a GraphQL query
pub fn has_live_directive(query: &str) -> bool {
    let normalized = query
        .lines()
        .map(|line| {
            if let Some(idx) = line.find('#') {
                &line[..idx]
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    // Basic detection of @live directive
    // Matches @live followed by whitespace, parenthesis (args), or brace (selection set)
    let custom_patterns = ["@live ", "@live\n", "@live\t", "@live(", "@live{"];
    for pattern in custom_patterns {
        if normalized.contains(pattern) {
            return true;
        }
    }
    // Check for @live at end of string (e.g. "query @live")
    if normalized.trim_end().ends_with("@live") {
        return true;
    }
    
    false
}

/// Strip the `@live` directive from a GraphQL query.
/// 
/// This is necessary because async-graphql doesn't support custom client directives
/// without explicit registration. We detect the directive, store its presence, then
/// strip it before executing the query.
/// 
/// # Examples
/// 
/// ```
/// use grpc_graphql_gateway::live_query::strip_live_directive;
/// 
/// let query = "query @live { user { name } }";
/// let stripped = strip_live_directive(query);
/// assert_eq!(stripped, "query  { user { name } }");
/// ```
pub fn strip_live_directive(query: &str) -> String {
    // Simple regex-like replacement for @live directive
    // Handles common patterns: @live, @live(), @live(throttle: 100)
    let mut result = query.to_string();
    
    // Pattern 1: @live with arguments - @live(...)
    if let Some(start) = result.find("@live(") {
        // Find matching closing paren
        let after_open = start + 6; // length of "@live("
        let mut depth = 1;
        let mut end = after_open;
        for (i, ch) in result[after_open..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = after_open + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        result = format!("{}{}", &result[..start], &result[end..]);
    } else {
        // Pattern 2: Simple @live without arguments
        result = result.replace("@live", "");
    }
    
    result
}

/// Generate a unique subscription ID
pub fn generate_subscription_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalidation_event_matching() {
        let event = InvalidationEvent::new("User", "update");

        assert!(event.matches("User.update"));
        assert!(event.matches("User.*"));
        assert!(event.matches("*.update"));
        assert!(event.matches("*.*"));

        assert!(!event.matches("Product.update"));
        assert!(!event.matches("User.delete"));
        assert!(!event.matches("User"));
    }

    #[test]
    fn test_has_live_directive() {
        assert!(has_live_directive("query @live { user { name } }"));
        assert!(has_live_directive("query GetUser @live { user(id: 1) { name } }"));
        assert!(has_live_directive("query @live\n{ user { name } }"));
        
        assert!(!has_live_directive("query { user { name } }"));
        assert!(!has_live_directive("# @live\nquery { user { name } }"));
    }

    #[test]
    fn test_live_query_store() {
        let store = LiveQueryStore::new();
        let (tx, _rx) = mpsc::channel(10);

        let query = ActiveLiveQuery {
            id: "sub-1".to_string(),
            operation_name: "user".to_string(),
            query: "query @live { user { name } }".to_string(),
            variables: None,
            triggers: vec!["User.update".to_string(), "User.delete".to_string()],
            throttle_ms: 100,
            ttl_seconds: 0,
            strategy: LiveQueryStrategy::Invalidation,
            poll_interval_ms: 0,
            last_hash: None,
            last_update: Instant::now(),
            created_at: Instant::now(),
            connection_id: "conn-1".to_string(),
        };

        // Register
        store.register(query, tx).unwrap();
        assert_eq!(store.stats().active_count, 1);

        // Invalidate
        let event = InvalidationEvent::new("User", "update");
        let affected = store.invalidate(event);
        assert_eq!(affected, 1);

        // Unregister
        let removed = store.unregister("sub-1");
        assert!(removed.is_some());
    }
}

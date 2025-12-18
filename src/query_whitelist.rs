//! Query Whitelisting for production security
//!
//! This module provides query whitelisting (also known as "Stored Operations" or "Persisted Queries")
//! to restrict which GraphQL queries can be executed. This is a critical security feature for
//! public-facing GraphQL APIs.
//!
//! # Features
//!
//! - **Hash-based validation**: Queries identified by SHA-256 hash
//! - **ID-based validation**: Queries identified by custom operation IDs
//! - **Multiple modes**: Enforce, Warn, or Disabled
//! - **Flexible loading**: From files, inline maps, or runtime registration
//! - **APQ compatible**: Works alongside Automatic Persisted Queries
//!
//! # Example
//!
//! ```rust,no_run
//! use grpc_graphql_gateway::{Gateway, QueryWhitelistConfig, WhitelistMode};
//! use std::collections::HashMap;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut allowed_queries = HashMap::new();
//! allowed_queries.insert(
//!     "getUserById".to_string(),
//!     "query getUserById($id: ID!) { user(id: $id) { id name email } }".to_string()
//! );
//!
//! let gateway = Gateway::builder()
//!     .with_descriptor_set_bytes(&[])
//!     .with_query_whitelist(QueryWhitelistConfig {
//!         mode: WhitelistMode::Enforce,
//!         allowed_queries,
//!         allow_introspection: true,
//!     })
//!     .build()?;
//! # Ok(())
//! # }
//! ```

use parking_lot::RwLock; // SECURITY: Non-poisoning locks
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;

/// Mode for query whitelist enforcement
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WhitelistMode {
    /// Enforce whitelist - reject non-whitelisted queries with error
    Enforce,
    /// Warn mode - log warnings but allow non-whitelisted queries
    Warn,
    /// Disabled - whitelist is not checked (default behavior)
    Disabled,
}

/// Configuration for query whitelisting
#[derive(Debug, Clone)]
pub struct QueryWhitelistConfig {
    /// Enforcement mode
    pub mode: WhitelistMode,
    /// Map of operation ID/name -> query string
    /// The key can be a custom operation ID or the query hash
    pub allowed_queries: HashMap<String, String>,
    /// Whether to allow introspection queries even in Enforce mode
    pub allow_introspection: bool,
}

impl Default for QueryWhitelistConfig {
    fn default() -> Self {
        Self {
            mode: WhitelistMode::Disabled,
            allowed_queries: HashMap::new(),
            allow_introspection: true,
        }
    }
}

impl QueryWhitelistConfig {
    /// Create a new disabled whitelist config
    pub fn disabled() -> Self {
        Self {
            mode: WhitelistMode::Disabled,
            ..Default::default()
        }
    }

    /// Create a new enforcing whitelist config
    pub fn enforce() -> Self {
        Self {
            mode: WhitelistMode::Enforce,
            ..Default::default()
        }
    }

    /// Create a new warning whitelist config
    pub fn warn() -> Self {
        Self {
            mode: WhitelistMode::Warn,
            ..Default::default()
        }
    }

    /// Load allowed queries from a JSON file
    ///
    /// Expected format:
    /// ```json
    /// {
    ///   "getUserById": "query getUserById($id: ID!) { user(id: $id) { id name } }",
    ///   "listProducts": "query { products { id name price } }"
    /// }
    /// ```
    pub fn from_json_file<P: AsRef<Path>>(
        path: P,
        mode: WhitelistMode,
    ) -> Result<Self, QueryWhitelistError> {
        let content = std::fs::read_to_string(path.as_ref())
            .map_err(|e| QueryWhitelistError::FileLoadError(e.to_string()))?;

        let allowed_queries: HashMap<String, String> = serde_json::from_str(&content)
            .map_err(|e| QueryWhitelistError::ParseError(e.to_string()))?;

        Ok(Self {
            mode,
            allowed_queries,
            allow_introspection: true,
        })
    }

    /// Load allowed queries from a YAML file
    ///
    /// Note: Requires the `yaml` feature to be enabled in Cargo.toml and
    /// the `serde_yaml` dependency to be added.
    #[cfg(feature = "yaml")]
    pub fn from_yaml_file<P: AsRef<Path>>(
        path: P,
        mode: WhitelistMode,
    ) -> Result<Self, QueryWhitelistError> {
        let content = std::fs::read_to_string(path.as_ref())
            .map_err(|e| QueryWhitelistError::FileLoadError(e.to_string()))?;

        let allowed_queries: HashMap<String, String> = serde_yaml::from_str(&content)
            .map_err(|e| QueryWhitelistError::ParseError(e.to_string()))?;

        Ok(Self {
            mode,
            allowed_queries,
            allow_introspection: true,
        })
    }

    /// Set enforcement mode
    pub fn with_mode(mut self, mode: WhitelistMode) -> Self {
        self.mode = mode;
        self
    }

    /// Set whether to allow introspection queries
    pub fn with_introspection(mut self, allow: bool) -> Self {
        self.allow_introspection = allow;
        self
    }

    /// Add a single allowed query
    pub fn add_query(mut self, id: String, query: String) -> Self {
        self.allowed_queries.insert(id, query);
        self
    }

    /// Add multiple queries
    pub fn add_queries(mut self, queries: HashMap<String, String>) -> Self {
        self.allowed_queries.extend(queries);
        self
    }
}

/// Errors related to query whitelisting
#[derive(Debug, Error)]
pub enum QueryWhitelistError {
    #[error("Query not in whitelist: {0}")]
    QueryNotWhitelisted(String),

    #[error("Failed to load whitelist file: {0}")]
    FileLoadError(String),

    #[error("Failed to parse whitelist file: {0}")]
    ParseError(String),

    #[error("Query validation failed: {0}")]
    ValidationError(String),
}

/// Query whitelist validator
///
/// This struct is responsible for validating incoming GraphQL queries against
/// a whitelist of allowed operations.
#[derive(Clone)]
pub struct QueryWhitelist {
    config: QueryWhitelistConfig,
    /// Hash lookup: SHA-256 hash -> query string
    query_by_hash: Arc<RwLock<HashMap<String, String>>>,
    /// ID lookup: operation ID/name -> query string
    query_by_id: Arc<RwLock<HashMap<String, String>>>,
    /// Reverse lookup: query hash -> operation IDs
    ids_by_hash: Arc<RwLock<HashMap<String, HashSet<String>>>>,
}

impl QueryWhitelist {
    /// Create a new query whitelist from config
    pub fn new(config: QueryWhitelistConfig) -> Self {
        let mut query_by_hash = HashMap::new();
        let mut query_by_id = HashMap::new();
        let mut ids_by_hash: HashMap<String, HashSet<String>> = HashMap::new();

        // Build indexes
        for (id, query) in &config.allowed_queries {
            // SECURITY: Normalize query before hashing to ensure consistent matching
            // regardless of client formatting
            let hash = Self::hash_query(query);

            // Store by hash
            query_by_hash.insert(hash.clone(), query.clone());

            // Store by ID
            query_by_id.insert(id.clone(), query.clone());

            // Track which IDs map to this hash
            ids_by_hash
                .entry(hash)
                .or_default()
                .insert(id.clone());
        }

        Self {
            config,
            query_by_hash: Arc::new(RwLock::new(query_by_hash)),
            query_by_id: Arc::new(RwLock::new(query_by_id)),
            ids_by_hash: Arc::new(RwLock::new(ids_by_hash)),
        }
    }

    /// Hash a query string using SHA-256 after normalization
    pub fn hash_query(query: &str) -> String {
        let normalized = Self::normalize_query(query);
        let mut hasher = Sha256::new();
        hasher.update(normalized.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Normalize query to handle whitespace and comments consistently
    ///
    /// This implementation uses semantic normalization:
    /// - Drops comments
    /// - Collapses whitespace
    /// - Removes whitespace around punctuators (e.g. `( arg )` -> `(arg)`)
    fn normalize_query(query: &str) -> String {
        let mut normalized = String::with_capacity(query.len());
        let mut in_string = false;
        let mut in_comment = false;
        let mut pending_space = false;

        let chars: Vec<char> = query.chars().collect();
        let mut i = 0;

        // GraphQL Punctuators + Common separators relative to syntax
        // ! $ ( ) ... : = @ [ ] { | }
        let is_punctuator = |c: char| "!$():=@[]{|}".contains(c);

        while i < chars.len() {
            let c = chars[i];

            if in_comment {
                if c == '\n' || c == '\r' {
                    in_comment = false;
                    // Treated as whitespace
                    pending_space = true;
                }
                i += 1;
                continue;
            }

            if in_string {
                normalized.push(c);
                if c == '"' {
                    // Check for escaped quote
                    let mut backslashes = 0;
                    let mut j = i;
                    while j > 0 {
                        j -= 1;
                        if chars[j] == '\\' {
                            backslashes += 1;
                        } else {
                            break;
                        }
                    }
                    if backslashes % 2 == 0 {
                        in_string = false;
                    }
                }
                i += 1;
                continue;
            }

            if c == '"' {
                in_string = true;
                if pending_space {
                    if let Some(last) = normalized.chars().last() {
                        // Space before string if previous char wasn't a punctuator (e.g. `name: "val"` internal space logic)
                        // Wait, `name:"val"`. `:` is punct. `n:"`. No space.
                        // `directive @"val"`. `@` is punct. No space.
                        // `msg "hello"`. `msg` is alpha. `"` is alpha? No.
                        // String quote is not punctuator in list above.
                        // So we check.
                        if !is_punctuator(last) {
                            normalized.push(' ');
                        }
                    }
                }
                normalized.push(c);
                pending_space = false;
                i += 1;
                continue;
            }

            if c == '#' {
                in_comment = true;
                i += 1;
                continue;
            }

            if c.is_whitespace() || c == ',' {
                pending_space = true;
                i += 1;
                continue;
            }

            // Valid token char
            if pending_space {
                if let Some(last) = normalized.chars().last() {
                    // Only insert space if neither is punctuator
                    if !is_punctuator(last) && !is_punctuator(c) {
                        normalized.push(' ');
                    }
                }
            }
            normalized.push(c);
            pending_space = false;
            i += 1;
        }

        normalized
    }

    /// Check if the whitelist is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.mode != WhitelistMode::Disabled
    }

    /// Get the enforcement mode
    pub fn mode(&self) -> WhitelistMode {
        self.config.mode
    }

    /// Validate a query against the whitelist
    ///
    /// Returns Ok(()) if the query is allowed, or an error if it's not whitelisted.
    pub fn validate_query(
        &self,
        query: &str,
        operation_id: Option<&str>,
    ) -> Result<(), QueryWhitelistError> {
        // Disabled mode - always allow
        if self.config.mode == WhitelistMode::Disabled {
            return Ok(());
        }

        // Check if this is an introspection query
        if self.config.allow_introspection && self.is_introspection_query(query) {
            tracing::debug!("Allowing introspection query");
            return Ok(());
        }

        // Try to validate by operation ID first if provided
        if let Some(id) = operation_id {
            if self.is_allowed_by_id(id) {
                tracing::debug!("Query allowed by operation ID: {}", id);
                return Ok(());
            }
        }

        // Validate by query hash
        let hash = Self::hash_query(query);
        if self.is_allowed_by_hash(&hash) {
            tracing::debug!("Query allowed by hash: {}", hash);
            return Ok(());
        }

        // Query not whitelisted
        let error_msg = operation_id
            .map(|id| format!("Operation '{}' (hash: {})", id, &hash[..16]))
            .unwrap_or_else(|| format!("Query hash: {}", &hash[..16]));

        match self.config.mode {
            WhitelistMode::Enforce => {
                tracing::warn!("Query rejected by whitelist: {}", error_msg);
                Err(QueryWhitelistError::QueryNotWhitelisted(error_msg))
            }
            WhitelistMode::Warn => {
                tracing::warn!(
                    "Query not in whitelist (allowed in Warn mode): {}",
                    error_msg
                );
                Ok(())
            }
            WhitelistMode::Disabled => Ok(()),
        }
    }

    /// Check if a query is allowed by its hash
    fn is_allowed_by_hash(&self, hash: &str) -> bool {
        self.query_by_hash.read().contains_key(hash)
    }

    /// Check if a query is allowed by its operation ID
    fn is_allowed_by_id(&self, id: &str) -> bool {
        self.query_by_id.read().contains_key(id)
    }

    /// Check if a query is an introspection query
    fn is_introspection_query(&self, query: &str) -> bool {
        let normalized = query.trim().to_lowercase();
        normalized.contains("__schema")
            || normalized.contains("__type")
            || normalized.contains("introspectionquery")
    }

    /// Get a whitelisted query by operation ID
    pub fn get_query_by_id(&self, id: &str) -> Option<String> {
        self.query_by_id.read().get(id).cloned()
    }

    /// Get a whitelisted query by hash
    pub fn get_query_by_hash(&self, hash: &str) -> Option<String> {
        self.query_by_hash.read().get(hash).cloned()
    }

    /// Register a new allowed query at runtime (useful for dynamic whitelists)
    pub fn register_query(&self, id: String, query: String) {
        let hash = Self::hash_query(&query);

        // Add to hash lookup
        self.query_by_hash
            .write()
            .insert(hash.clone(), query.clone());

        // Add to ID lookup
        self.query_by_id.write().insert(id.clone(), query);

        // Add to reverse lookup
        self.ids_by_hash
            .write()
            .entry(hash)
            .or_default()
            .insert(id);
    }

    /// Remove a query from the whitelist
    pub fn remove_query(&self, id: &str) -> bool {
        let mut query_by_id = self.query_by_id.write();

        if let Some(query) = query_by_id.remove(id) {
            let hash = Self::hash_query(&query);

            // Update reverse lookup
            let mut ids_by_hash = self.ids_by_hash.write();
            if let Some(ids) = ids_by_hash.get_mut(&hash) {
                ids.remove(id);

                // If no more IDs reference this hash, remove the hash entry
                if ids.is_empty() {
                    ids_by_hash.remove(&hash);
                    self.query_by_hash.write().remove(&hash);
                }
            }

            true
        } else {
            false
        }
    }

    /// Get statistics about the whitelist
    pub fn stats(&self) -> WhitelistStats {
        WhitelistStats {
            mode: self.config.mode,
            total_queries: self.query_by_id.read().len(),
            total_hashes: self.query_by_hash.read().len(),
            allow_introspection: self.config.allow_introspection,
        }
    }
}

/// Statistics about the query whitelist
#[derive(Debug, Clone, Serialize)]
pub struct WhitelistStats {
    pub mode: WhitelistMode,
    pub total_queries: usize,
    pub total_hashes: usize,
    pub allow_introspection: bool,
}

/// Shared query whitelist (thread-safe)
pub type SharedQueryWhitelist = Arc<QueryWhitelist>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_hashing() {
        let query1 = "query { user(id: 1) { name } }";
        let query2 = "query { user(id: 1) { name } }"; // Same
        let query3 = "query { user(id: 2) { name } }"; // Different

        let hash1 = QueryWhitelist::hash_query(query1);
        let hash2 = QueryWhitelist::hash_query(query2);
        let hash3 = QueryWhitelist::hash_query(query3);

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
        assert_eq!(hash1.len(), 64); // SHA-256 produces 64 hex chars
    }

    #[test]
    fn test_whitelist_enforce_mode() {
        let config = QueryWhitelistConfig {
            mode: WhitelistMode::Enforce,
            allowed_queries: vec![(
                "getUser".to_string(),
                "query { user { id name } }".to_string(),
            )]
            .into_iter()
            .collect(),
            allow_introspection: false,
        };

        let whitelist = QueryWhitelist::new(config);

        // Allowed query
        assert!(whitelist
            .validate_query("query { user { id name } }", None)
            .is_ok());

        // Allowed by operation ID
        assert!(whitelist
            .validate_query("query { user { id name } }", Some("getUser"))
            .is_ok());

        // Not allowed
        assert!(whitelist
            .validate_query("query { posts { title } }", None)
            .is_err());
    }

    #[test]
    fn test_whitelist_warn_mode() {
        let config = QueryWhitelistConfig {
            mode: WhitelistMode::Warn,
            allowed_queries: HashMap::new(),
            allow_introspection: false,
        };

        let whitelist = QueryWhitelist::new(config);

        // Any query should pass in Warn mode
        assert!(whitelist
            .validate_query("query { unknown { field } }", None)
            .is_ok());
    }

    #[test]
    fn test_whitelist_disabled() {
        let config = QueryWhitelistConfig::disabled();
        let whitelist = QueryWhitelist::new(config);

        // Any query should pass when disabled
        assert!(whitelist.validate_query("query { anything }", None).is_ok());
    }

    #[test]
    fn test_introspection_allowed() {
        let config = QueryWhitelistConfig {
            mode: WhitelistMode::Enforce,
            allowed_queries: HashMap::new(),
            allow_introspection: true,
        };

        let whitelist = QueryWhitelist::new(config);

        // Introspection queries should be allowed
        assert!(whitelist
            .validate_query("query { __schema { types { name } } }", None)
            .is_ok());
        assert!(whitelist
            .validate_query("query { __type(name: \"User\") { fields { name } } }", None)
            .is_ok());

        // Regular queries should still be blocked
        assert!(whitelist
            .validate_query("query { user { id } }", None)
            .is_err());
    }

    #[test]
    fn test_runtime_registration() {
        let config = QueryWhitelistConfig::enforce();
        let whitelist = QueryWhitelist::new(config);

        // Initially not allowed
        let query = "query { newQuery { field } }";
        assert!(whitelist.validate_query(query, None).is_err());

        // Register the query
        whitelist.register_query("newQuery".to_string(), query.to_string());

        // Now it should be allowed
        assert!(whitelist.validate_query(query, None).is_ok());
        assert!(whitelist.validate_query(query, Some("newQuery")).is_ok());
    }

    #[test]
    fn test_remove_query() {
        let mut queries = HashMap::new();
        queries.insert("getUser".to_string(), "query { user { id } }".to_string());

        let config = QueryWhitelistConfig {
            mode: WhitelistMode::Enforce,
            allowed_queries: queries,
            allow_introspection: false,
        };

        let whitelist = QueryWhitelist::new(config);

        // Initially allowed
        assert!(whitelist
            .validate_query("query { user { id } }", Some("getUser"))
            .is_ok());

        // Remove it
        assert!(whitelist.remove_query("getUser"));

        // Now should be blocked
        assert!(whitelist
            .validate_query("query { user { id } }", Some("getUser"))
            .is_err());

        // Removing again should return false
        assert!(!whitelist.remove_query("getUser"));
    }

    #[test]
    fn test_stats() {
        let mut queries = HashMap::new();
        queries.insert("q1".to_string(), "query { a }".to_string());
        queries.insert("q2".to_string(), "query { b }".to_string());

        let config = QueryWhitelistConfig {
            mode: WhitelistMode::Enforce,
            allowed_queries: queries,
            allow_introspection: true,
        };

        let whitelist = QueryWhitelist::new(config);
        let stats = whitelist.stats();

        assert_eq!(stats.mode, WhitelistMode::Enforce);
        assert_eq!(stats.total_queries, 2);
        assert_eq!(stats.total_hashes, 2);
        assert!(stats.allow_introspection);
    }
}

#[cfg(test)]
mod proptest_checks {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        // 1. Normalization should never panic on random strings
        #[test]
        fn doesnt_crash(s in "\\PC*") {
            let _ = QueryWhitelist::normalize_query(&s);
        }

        // 2. Idempotency: normalize(normalize(s)) == normalize(s)
        #[test]
        fn is_idempotent(s in "\\PC*") {
            let n1 = QueryWhitelist::normalize_query(&s);
            let n2 = QueryWhitelist::normalize_query(&n1);
            assert_eq!(n1, n2);
        }

        // 3. Stability: Adding comments should not change normalized output
        #[test]
        fn ignores_comments(s in "[a-zA-Z0-9]+") {
            let with_comment = format!("{} # comment", s);
            let n1 = QueryWhitelist::normalize_query(&s);
            let n2 = QueryWhitelist::normalize_query(&with_comment);
            // Normalization trims input, but if `s` is "A", n1="A".
            // "A # comment" -> "A ".
            // So we compare trimmed.
            assert_eq!(n1.trim(), n2.trim());
        }

        // 4. Stability: Extra whitespace should collapse
        #[test]
        fn collapses_whitespace(s in "[a-zA-Z0-9]+") {
            let spaced = format!("  {}   \n  ", s);
            let n1 = QueryWhitelist::normalize_query(&s);
            let n2 = QueryWhitelist::normalize_query(&spaced);
            assert_eq!(n1.trim(), n2.trim());
        }
    }
}

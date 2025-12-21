//! Header Propagation Support
//!
//! This module enables automatic forwarding of HTTP headers from incoming GraphQL
//! requests to outgoing gRPC metadata. This is essential for:
//!
//! - **Authentication**: Forward `Authorization` headers to backend services
//! - **Distributed Tracing**: Propagate trace IDs (`X-Request-ID`, `traceparent`)
//! - **Tenant Context**: Pass tenant/organization identifiers
//! - **Localization**: Forward `Accept-Language` for i18n
//!
//! # Security
//!
//! Header propagation uses an **allowlist** approach - only explicitly configured
//! headers are forwarded. This prevents accidental leakage of sensitive headers
//! like `Cookie`, `Host`, or internal proxy headers.
//!
//! # Example
//!
//! ```rust,no_run
//! use grpc_graphql_gateway::{Gateway, HeaderPropagationConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let gateway = Gateway::builder()
//!     .with_header_propagation(HeaderPropagationConfig::new()
//!         .propagate("authorization")
//!         .propagate("x-request-id")
//!         .propagate("x-tenant-id")
//!         .propagate_with_prefix("x-custom-"))
//!     // ... other configuration
//!     .build()?;
//! # Ok(())
//! # }
//! ```
//!
//! # How It Works
//!
//! 1. Client sends GraphQL request with headers (e.g., `Authorization: Bearer token`)
//! 2. Gateway extracts headers matching the propagation config
//! 3. Headers are converted to gRPC metadata
//! 4. gRPC request includes metadata for backend authentication/context

use axum::http::HeaderMap;
use std::collections::HashSet;
use tonic::metadata::{Ascii, MetadataKey, MetadataMap, MetadataValue};

/// Configuration for header propagation from GraphQL to gRPC.
///
/// # Example
///
/// ```rust
/// use grpc_graphql_gateway::HeaderPropagationConfig;
///
/// let config = HeaderPropagationConfig::new()
///     .propagate("authorization")
///     .propagate("x-request-id")
///     .propagate_with_prefix("x-custom-");
/// ```
#[derive(Debug, Clone, Default)]
pub struct HeaderPropagationConfig {
    /// Exact header names to propagate (lowercase)
    headers: HashSet<String>,

    /// Header prefixes to propagate (lowercase)
    prefixes: Vec<String>,

    /// Whether to propagate all headers (not recommended for security)
    propagate_all: bool,

    /// Headers to explicitly exclude (even if matching prefix or propagate_all)
    exclude: HashSet<String>,
}

impl HeaderPropagationConfig {
    /// Create a new empty header propagation config.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a config that propagates common authentication and tracing headers.
    ///
    /// Includes:
    /// - `authorization` - Bearer tokens, API keys
    /// - `x-request-id` - Request correlation
    /// - `x-correlation-id` - Distributed tracing
    /// - `traceparent` - W3C Trace Context
    /// - `tracestate` - W3C Trace Context state
    /// - `x-b3-*` - Zipkin B3 propagation headers
    ///
    /// # Example
    ///
    /// ```rust
    /// use grpc_graphql_gateway::HeaderPropagationConfig;
    ///
    /// let config = HeaderPropagationConfig::common();
    /// ```
    pub fn common() -> Self {
        Self::new()
            .propagate("authorization")
            .propagate("x-request-id")
            .propagate("x-correlation-id")
            .propagate("traceparent")
            .propagate("tracestate")
            .propagate_with_prefix("x-b3-")
    }

    /// Add a specific header to propagate (case-insensitive).
    ///
    /// # Example
    ///
    /// ```rust
    /// use grpc_graphql_gateway::HeaderPropagationConfig;
    ///
    /// let config = HeaderPropagationConfig::new()
    ///     .propagate("authorization")
    ///     .propagate("X-Tenant-ID"); // Will be normalized to lowercase
    /// ```
    pub fn propagate(mut self, header: impl Into<String>) -> Self {
        self.headers.insert(header.into().to_lowercase());
        self
    }

    /// Add multiple headers to propagate.
    pub fn propagate_many<I, S>(mut self, headers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for header in headers {
            self.headers.insert(header.into().to_lowercase());
        }
        self
    }

    /// Propagate all headers matching a prefix (case-insensitive).
    ///
    /// # Example
    ///
    /// ```rust
    /// use grpc_graphql_gateway::HeaderPropagationConfig;
    ///
    /// let config = HeaderPropagationConfig::new()
    ///     .propagate_with_prefix("x-custom-")  // Matches x-custom-foo, x-custom-bar, etc.
    ///     .propagate_with_prefix("x-tenant-");
    /// ```
    pub fn propagate_with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefixes.push(prefix.into().to_lowercase());
        self
    }

    /// Propagate all headers (use with caution).
    ///
    /// **Warning**: This may expose sensitive headers. Consider using `exclude()`
    /// to block specific headers like `Cookie`, `Host`, etc.
    ///
    /// # Example
    ///
    /// ```rust
    /// use grpc_graphql_gateway::HeaderPropagationConfig;
    ///
    /// let config = HeaderPropagationConfig::new()
    ///     .propagate_all_headers()
    ///     .exclude("cookie")
    ///     .exclude("host");
    /// ```
    pub fn propagate_all_headers(mut self) -> Self {
        self.propagate_all = true;
        self
    }

    /// Exclude a specific header from propagation.
    ///
    /// Useful when using `propagate_all_headers()` or broad prefixes.
    pub fn exclude(mut self, header: impl Into<String>) -> Self {
        self.exclude.insert(header.into().to_lowercase());
        self
    }

    /// Check if a header should be propagated.
    pub fn should_propagate(&self, header_name: &str) -> bool {
        let lower = header_name.to_lowercase();

        // Check exclusions first
        if self.exclude.contains(&lower) {
            return false;
        }

        // Check exact match
        if self.headers.contains(&lower) {
            return true;
        }

        // Check prefix match
        for prefix in &self.prefixes {
            if lower.starts_with(prefix) {
                return true;
            }
        }

        // Check propagate all
        self.propagate_all
    }

    /// Check if any propagation rules are configured.
    pub fn is_enabled(&self) -> bool {
        !self.headers.is_empty() || !self.prefixes.is_empty() || self.propagate_all
    }

    /// Extract headers from an HTTP HeaderMap and convert to gRPC MetadataMap.
    ///
    /// Only headers matching the propagation config are included.
    pub fn extract_metadata(&self, headers: &HeaderMap) -> MetadataMap {
        let mut metadata = MetadataMap::new();

        if !self.is_enabled() {
            return metadata;
        }

        for (name, value) in headers.iter() {
            let header_name = name.as_str();

            if !self.should_propagate(header_name) {
                continue;
            }

            // Convert header value to string
            let Ok(value_str) = value.to_str() else {
                continue;
            };

            // Try to create gRPC metadata key/value
            // gRPC metadata keys must be lowercase ASCII
            let Ok(key) = MetadataKey::<Ascii>::from_bytes(header_name.to_lowercase().as_bytes())
            else {
                tracing::debug!("Skipping header '{}': invalid metadata key", header_name);
                continue;
            };

            let Ok(value) = MetadataValue::try_from(value_str) else {
                tracing::debug!("Skipping header '{}': invalid metadata value", header_name);
                continue;
            };

            metadata.insert(key, value);
        }

        metadata
    }
}

/// Merge metadata from header propagation into a tonic Request.
pub fn apply_metadata_to_request<T>(
    mut request: tonic::Request<T>,
    metadata: MetadataMap,
) -> tonic::Request<T> {
    for key_value in metadata.iter() {
        match key_value {
            tonic::metadata::KeyAndValueRef::Ascii(key, value) => {
                request.metadata_mut().insert(key, value.clone());
            }
            tonic::metadata::KeyAndValueRef::Binary(key, value) => {
                request.metadata_mut().insert_bin(key, value.clone());
            }
        }
    }
    request
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn test_propagate_exact_header() {
        let config = HeaderPropagationConfig::new()
            .propagate("authorization")
            .propagate("x-request-id");

        assert!(config.should_propagate("authorization"));
        assert!(config.should_propagate("Authorization")); // Case insensitive
        assert!(config.should_propagate("x-request-id"));
        assert!(!config.should_propagate("cookie"));
    }

    #[test]
    fn test_propagate_case_insensitive() {
        let config = HeaderPropagationConfig::new()
            .propagate("Authorization")
            .propagate("X-Request-ID");

        // All these should work due to case normalization
        assert!(config.should_propagate("authorization"));
        assert!(config.should_propagate("AUTHORIZATION"));
        assert!(config.should_propagate("x-request-id"));
        assert!(config.should_propagate("X-REQUEST-ID"));
    }

    #[test]
    fn test_propagate_prefix() {
        let config = HeaderPropagationConfig::new().propagate_with_prefix("x-custom-");

        assert!(config.should_propagate("x-custom-foo"));
        assert!(config.should_propagate("x-custom-bar"));
        assert!(config.should_propagate("X-Custom-Baz")); // Case insensitive
        assert!(!config.should_propagate("x-other-header"));
        assert!(!config.should_propagate("x-custom")); // No trailing dash
    }

    #[test]
    fn test_propagate_multiple_prefixes() {
        let config = HeaderPropagationConfig::new()
            .propagate_with_prefix("x-custom-")
            .propagate_with_prefix("x-tenant-")
            .propagate_with_prefix("x-trace-");

        assert!(config.should_propagate("x-custom-id"));
        assert!(config.should_propagate("x-tenant-org"));
        assert!(config.should_propagate("x-trace-id"));
        assert!(!config.should_propagate("x-other-id"));
    }

    #[test]
    fn test_propagate_all_with_exclusions() {
        let config = HeaderPropagationConfig::new()
            .propagate_all_headers()
            .exclude("cookie")
            .exclude("host");

        assert!(config.should_propagate("authorization"));
        assert!(config.should_propagate("x-custom-header"));
        assert!(!config.should_propagate("cookie"));
        assert!(!config.should_propagate("host"));
        assert!(!config.should_propagate("Cookie")); // Case insensitive
        assert!(!config.should_propagate("HOST"));
    }

    #[test]
    fn test_exclusions_override_exact_match() {
        let config = HeaderPropagationConfig::new()
            .propagate("authorization")
            .exclude("authorization");

        // Exclusion should win
        assert!(!config.should_propagate("authorization"));
    }

    #[test]
    fn test_exclusions_override_prefix() {
        let config = HeaderPropagationConfig::new()
            .propagate_with_prefix("x-custom-")
            .exclude("x-custom-secret");

        assert!(config.should_propagate("x-custom-public"));
        assert!(!config.should_propagate("x-custom-secret"));
    }

    #[test]
    fn test_common_config() {
        let config = HeaderPropagationConfig::common();

        assert!(config.should_propagate("authorization"));
        assert!(config.should_propagate("x-request-id"));
        assert!(config.should_propagate("x-correlation-id"));
        assert!(config.should_propagate("traceparent"));
        assert!(config.should_propagate("tracestate"));
        assert!(config.should_propagate("x-b3-traceid"));
        assert!(config.should_propagate("x-b3-spanid"));
        assert!(!config.should_propagate("cookie"));
        assert!(!config.should_propagate("host"));
    }

    #[test]
    fn test_propagate_many() {
        let headers = vec!["auth", "x-id", "x-trace"];
        let config = HeaderPropagationConfig::new().propagate_many(headers);

        assert!(config.should_propagate("auth"));
        assert!(config.should_propagate("x-id"));
        assert!(config.should_propagate("x-trace"));
        assert!(!config.should_propagate("other"));
    }

    #[test]
    fn test_propagate_many_case_insensitive() {
        let headers = vec!["Authorization", "X-Request-ID"];
        let config = HeaderPropagationConfig::new().propagate_many(headers);

        assert!(config.should_propagate("authorization"));
        assert!(config.should_propagate("x-request-id"));
    }

    #[test]
    fn test_extract_metadata() {
        let config = HeaderPropagationConfig::new()
            .propagate("authorization")
            .propagate("x-request-id");

        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer token123".parse().unwrap());
        headers.insert("x-request-id", "req-456".parse().unwrap());
        headers.insert("cookie", "session=abc".parse().unwrap());

        let metadata = config.extract_metadata(&headers);

        assert_eq!(
            metadata.get("authorization").map(|v| v.to_str().unwrap()),
            Some("Bearer token123")
        );
        assert_eq!(
            metadata.get("x-request-id").map(|v| v.to_str().unwrap()),
            Some("req-456")
        );
        assert!(metadata.get("cookie").is_none());
    }

    #[test]
    fn test_extract_metadata_empty_config() {
        let config = HeaderPropagationConfig::new();

        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer token".parse().unwrap());

        let metadata = config.extract_metadata(&headers);
        assert_eq!(metadata.len(), 0);
    }

    #[test]
    fn test_extract_metadata_with_prefix() {
        let config = HeaderPropagationConfig::new().propagate_with_prefix("x-custom-");

        let mut headers = HeaderMap::new();
        headers.insert("x-custom-foo", "value1".parse().unwrap());
        headers.insert("x-custom-bar", "value2".parse().unwrap());
        headers.insert("x-other", "value3".parse().unwrap());

        let metadata = config.extract_metadata(&headers);

        assert_eq!(
            metadata.get("x-custom-foo").map(|v| v.to_str().unwrap()),
            Some("value1")
        );
        assert_eq!(
            metadata.get("x-custom-bar").map(|v| v.to_str().unwrap()),
            Some("value2")
        );
        assert!(metadata.get("x-other").is_none());
    }

    #[test]
    fn test_extract_metadata_invalid_header_value() {
        let config = HeaderPropagationConfig::new().propagate("test-header");

        let mut headers = HeaderMap::new();
        // Create invalid UTF-8 in header value
        headers.insert(
            "test-header",
            HeaderValue::from_bytes(&[0xFF, 0xFE]).unwrap(),
        );

        let metadata = config.extract_metadata(&headers);
        // Should skip invalid values
        assert!(metadata.get("test-header").is_none());
    }

    #[test]
    fn test_extract_metadata_all_headers() {
        let config = HeaderPropagationConfig::new().propagate_all_headers();

        let mut headers = HeaderMap::new();
        headers.insert("authorization", "token".parse().unwrap());
        headers.insert("x-custom", "value".parse().unwrap());

        let metadata = config.extract_metadata(&headers);

        assert!(metadata.get("authorization").is_some());
        assert!(metadata.get("x-custom").is_some());
    }

    #[test]
    fn test_is_enabled() {
        let empty = HeaderPropagationConfig::new();
        assert!(!empty.is_enabled());

        let with_header = HeaderPropagationConfig::new().propagate("auth");
        assert!(with_header.is_enabled());

        let with_prefix = HeaderPropagationConfig::new().propagate_with_prefix("x-");
        assert!(with_prefix.is_enabled());

        let with_all = HeaderPropagationConfig::new().propagate_all_headers();
        assert!(with_all.is_enabled());
    }

    #[test]
    fn test_is_enabled_with_exclusions_only() {
        // Exclusions alone don't enable propagation
        let config = HeaderPropagationConfig::new().exclude("cookie");
        assert!(!config.is_enabled());
    }

    #[test]
    fn test_apply_metadata_to_request() {
        let mut metadata = MetadataMap::new();
        metadata.insert(
            MetadataKey::from_static("authorization"),
            "Bearer token".parse().unwrap(),
        );
        metadata.insert(
            MetadataKey::from_static("x-request-id"),
            "req-123".parse().unwrap(),
        );

        let request = tonic::Request::new(());
        let modified = apply_metadata_to_request(request, metadata);

        assert_eq!(
            modified
                .metadata()
                .get("authorization")
                .map(|v| v.to_str().unwrap()),
            Some("Bearer token")
        );
        assert_eq!(
            modified
                .metadata()
                .get("x-request-id")
                .map(|v| v.to_str().unwrap()),
            Some("req-123")
        );
    }

    #[test]
    fn test_apply_metadata_empty() {
        let metadata = MetadataMap::new();
        let request = tonic::Request::new(());
        let modified = apply_metadata_to_request(request, metadata);

        assert_eq!(modified.metadata().len(), 0);
    }

    #[test]
    fn test_config_default() {
        let config = HeaderPropagationConfig::default();
        assert!(!config.is_enabled());
        assert!(!config.should_propagate("anything"));
    }

    #[test]
    fn test_config_clone() {
        let original = HeaderPropagationConfig::new()
            .propagate("auth")
            .propagate_with_prefix("x-");

        let cloned = original.clone();

        assert!(cloned.should_propagate("auth"));
        assert!(cloned.should_propagate("x-custom"));
    }

    #[test]
    fn test_config_debug() {
        let config = HeaderPropagationConfig::new().propagate("test");
        let debug_str = format!("{:?}", config);

        assert!(debug_str.contains("HeaderPropagationConfig"));
    }

    #[test]
    fn test_empty_header_name() {
        let config = HeaderPropagationConfig::new().propagate("");

        // Empty header names should be normalized but won't match anything useful
        assert!(!config.should_propagate("some-header"));
    }

    #[test]
    fn test_special_characters_in_prefix() {
        let config = HeaderPropagationConfig::new().propagate_with_prefix("x-app_");

        assert!(config.should_propagate("x-app_id"));
        assert!(config.should_propagate("x-app_version"));
        assert!(!config.should_propagate("x-other"));
    }

    #[test]
    fn test_unicode_header_names() {
        let config = HeaderPropagationConfig::new().propagate("x-test");

        let mut headers = HeaderMap::new();
        headers.insert("x-test", "value".parse().unwrap());

        let metadata = config.extract_metadata(&headers);
        assert!(metadata.get("x-test").is_some());
    }

    #[test]
    fn test_multiple_values_same_header() {
        let config = HeaderPropagationConfig::new().propagate("x-multi");

        let mut headers = HeaderMap::new();
        headers.insert("x-multi", "value1".parse().unwrap());
        headers.append("x-multi", "value2".parse().unwrap());

        let metadata = config.extract_metadata(&headers);
        // Should only get the first value due to how metadata extraction works
        assert!(metadata.get("x-multi").is_some());
    }

    #[test]
    fn test_header_with_spaces() {
        let config = HeaderPropagationConfig::new().propagate("x-test");

        let mut headers = HeaderMap::new();
        headers.insert("x-test", "value with spaces".parse().unwrap());

        let metadata = config.extract_metadata(&headers);
        assert_eq!(
            metadata.get("x-test").map(|v| v.to_str().unwrap()),
            Some("value with spaces")
        );
    }

    #[test]
    fn test_propagate_all_excludes_multiple() {
        let config = HeaderPropagationConfig::new()
            .propagate_all_headers()
            .exclude("cookie")
            .exclude("host")
            .exclude("connection")
            .exclude("upgrade");

        assert!(config.should_propagate("authorization"));
        assert!(!config.should_propagate("cookie"));
        assert!(!config.should_propagate("host"));
        assert!(!config.should_propagate("connection"));
        assert!(!config.should_propagate("upgrade"));
    }

    #[test]
    fn test_long_header_value() {
        let config = HeaderPropagationConfig::new().propagate("x-long");

        let long_value = "a".repeat(1000);
        let mut headers = HeaderMap::new();
        headers.insert("x-long", long_value.parse().unwrap());

        let metadata = config.extract_metadata(&headers);
        assert!(metadata.get("x-long").is_some());
    }

    #[test]
    fn test_binary_metadata() {
        let mut metadata = MetadataMap::new();
        
        // Binary metadata keys end with "-bin"
        let binary_key = MetadataKey::from_bytes(b"data-bin").unwrap();
        let binary_value = tonic::metadata::MetadataValue::from_bytes(&[1, 2, 3, 4]);
        metadata.insert_bin(binary_key, binary_value);

        let request = tonic::Request::new(());
        let modified = apply_metadata_to_request(request, metadata);

        assert!(modified.metadata().get_bin("data-bin").is_some());
    }

    #[test]
    fn test_mixed_ascii_binary_metadata() {
        let mut metadata = MetadataMap::new();
        metadata.insert(
            MetadataKey::from_static("text-key"),
            "value".parse().unwrap(),
        );
        metadata.insert_bin(
            MetadataKey::from_bytes(b"binary-bin").unwrap(),
            tonic::metadata::MetadataValue::from_bytes(&[1, 2, 3]),
        );

        let request = tonic::Request::new(());
        let modified = apply_metadata_to_request(request, metadata);

        assert!(modified.metadata().get("text-key").is_some());
        assert!(modified.metadata().get_bin("binary-bin").is_some());
    }
}

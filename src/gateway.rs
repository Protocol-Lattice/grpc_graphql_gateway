//! Gateway builder and main orchestration

use crate::compression::CompressionConfig;
use crate::error::{GraphQLError, Result};
use crate::grpc_client::{GrpcClient, GrpcClientPool};
use crate::headers::HeaderPropagationConfig;
use crate::middleware::Middleware;
use crate::plugin::{Plugin, PluginRegistry};
use crate::request_collapsing::RequestCollapsingConfig;
use crate::rest_connector::{RestConnector, RestConnectorRegistry};
use crate::runtime::ServeMux;
use crate::schema::{DynamicSchema, SchemaBuilder};
use crate::shutdown::{run_with_graceful_shutdown, ShutdownConfig};
use axum::Router;
use std::path::Path;
use std::sync::Arc;

/// Main Gateway struct - entry point for the library
///
/// The `Gateway` orchestrates the GraphQL schema, gRPC clients, and HTTP/WebSocket server.
/// It is created via the [`GatewayBuilder`].
///
/// # Example
///
/// ```rust,no_run
/// use grpc_graphql_gateway::{Gateway, GrpcClient};
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let gateway = Gateway::builder()
///     // ... configuration ...
///     .build()?;
///
/// gateway.serve("0.0.0.0:8080").await?;
/// # Ok(())
/// # }
/// ```
pub struct Gateway {
    mux: ServeMux,
    client_pool: GrpcClientPool,
    schema: DynamicSchema,
}

impl Gateway {
    /// Create a new gateway builder
    pub fn builder() -> GatewayBuilder {
        GatewayBuilder::new()
    }

    /// Get the ServeMux
    pub fn mux(&self) -> &ServeMux {
        &self.mux
    }

    /// Access the built GraphQL schema
    pub fn schema(&self) -> &DynamicSchema {
        &self.schema
    }

    /// Get the client pool
    pub fn client_pool(&self) -> &GrpcClientPool {
        &self.client_pool
    }

    /// Convert gateway into Axum router
    pub fn into_router(self) -> Router {
        self.mux.into_router()
    }
}

/// Builder for creating a Gateway
///
/// # Example
///
/// ```rust,no_run
/// use grpc_graphql_gateway::{Gateway, GrpcClient};
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let gateway = Gateway::builder()
///     .with_descriptor_set_file("path/to/descriptor.bin")?
///     .add_grpc_client("my.service", GrpcClient::new("http://localhost:50051").await?)
///     .build()?;
/// # Ok(())
/// # }
/// ```
pub struct GatewayBuilder {
    client_pool: GrpcClientPool,
    schema_builder: SchemaBuilder,
    middlewares: Vec<Arc<dyn Middleware>>,
    error_handler: Option<Arc<dyn Fn(Vec<GraphQLError>) + Send + Sync>>,
    entity_resolver: Option<Arc<dyn crate::federation::EntityResolver>>,
    service_allowlist: Option<std::collections::HashSet<String>>,
    /// Enable health check endpoints
    health_checks_enabled: bool,
    /// Enable metrics endpoint
    metrics_enabled: bool,
    /// Enable OpenTelemetry tracing
    tracing_enabled: bool,
    /// APQ configuration
    apq_config: Option<crate::persisted_queries::PersistedQueryConfig>,
    /// Circuit breaker configuration
    circuit_breaker_config: Option<crate::circuit_breaker::CircuitBreakerConfig>,
    /// Graceful shutdown configuration
    shutdown_config: Option<ShutdownConfig>,
    /// Response cache configuration
    cache_config: Option<crate::cache::CacheConfig>,
    /// Compression configuration
    compression_config: Option<CompressionConfig>,
    /// Header propagation configuration
    header_propagation_config: Option<HeaderPropagationConfig>,
    /// Query whitelist configuration
    query_whitelist_config: Option<crate::query_whitelist::QueryWhitelistConfig>,
    /// REST connector registry
    rest_connectors: RestConnectorRegistry,
    /// Query analytics configuration
    analytics_config: Option<crate::analytics::AnalyticsConfig>,
    /// Request collapsing configuration
    request_collapsing_config: Option<RequestCollapsingConfig>,
    /// High-performance configuration
    high_perf_config: Option<crate::high_performance::HighPerfConfig>,
    /// @defer incremental delivery configuration
    defer_config: Option<crate::defer::DeferConfig>,
    /// Plugin registry
    plugins: PluginRegistry,
}

impl GatewayBuilder {
    /// Create a new gateway builder
    pub fn new() -> Self {
        Self {
            client_pool: GrpcClientPool::new(),
            schema_builder: SchemaBuilder::new(),
            middlewares: Vec::new(),
            error_handler: None,
            entity_resolver: None,
            service_allowlist: None,
            health_checks_enabled: false,
            metrics_enabled: false,
            tracing_enabled: false,
            apq_config: None,
            circuit_breaker_config: None,
            shutdown_config: None,
            cache_config: None,
            compression_config: None,
            header_propagation_config: None,
            query_whitelist_config: None,
            rest_connectors: RestConnectorRegistry::new(),
            analytics_config: None,
            request_collapsing_config: None,
            high_perf_config: None,
            defer_config: None,
            plugins: PluginRegistry::new(),
        }
    }

    /// Add a gRPC client to the pool
    ///
    /// # Arguments
    ///
    /// * `name` - The service name (e.g., "my.package.Service")
    /// * `client` - The `GrpcClient` instance
    pub fn add_grpc_client(self, name: impl Into<String>, client: GrpcClient) -> Self {
        self.client_pool.add(name, client);
        self
    }

    /// Add many gRPC clients in one shot.
    pub fn add_grpc_clients<I>(self, clients: I) -> Self
    where
        I: IntoIterator<Item = (String, GrpcClient)>,
    {
        for (name, client) in clients {
            self.client_pool.add(name, client);
        }
        self
    }

    /// Add middleware
    pub fn add_middleware<M: Middleware + 'static>(mut self, middleware: M) -> Self {
        self.middlewares.push(Arc::new(middleware));
        self
    }

    /// Register a plugin
    pub fn register_plugin<P>(mut self, plugin: P) -> Self
    where
        P: Plugin + 'static,
        P::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.plugins.register(plugin);
        self
    }

    /// Provide the primary protobuf descriptor set (bytes).
    ///
    /// This clears any existing descriptors and sets this as the primary.
    /// Use [`Self::add_descriptor_set_bytes`] to add additional descriptor sets for schema stitching.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::Gateway;
    ///
    /// const DESCRIPTORS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/descriptor.bin"));
    ///
    /// # fn example() {
    /// let gateway = Gateway::builder()
    ///     .with_descriptor_set_bytes(DESCRIPTORS);
    /// # }
    /// ```
    pub fn with_descriptor_set_bytes(mut self, bytes: impl AsRef<[u8]>) -> Self {
        self.schema_builder = self.schema_builder.with_descriptor_set_bytes(bytes);
        self
    }

    /// Add an additional protobuf descriptor set (bytes) for schema stitching.
    ///
    /// Use this to combine multiple protobuf descriptor sets from different
    /// microservices into a unified GraphQL schema.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::Gateway;
    ///
    /// const USERS_DESCRIPTORS: &[u8] = include_bytes!("path/to/users.bin");
    /// const PRODUCTS_DESCRIPTORS: &[u8] = include_bytes!("path/to/products.bin");
    /// const ORDERS_DESCRIPTORS: &[u8] = include_bytes!("path/to/orders.bin");
    ///
    /// # fn example() {
    /// let gateway = Gateway::builder()
    ///     .with_descriptor_set_bytes(USERS_DESCRIPTORS)
    ///     .add_descriptor_set_bytes(PRODUCTS_DESCRIPTORS)
    ///     .add_descriptor_set_bytes(ORDERS_DESCRIPTORS);
    /// # }
    /// ```
    pub fn add_descriptor_set_bytes(mut self, bytes: impl AsRef<[u8]>) -> Self {
        self.schema_builder = self.schema_builder.add_descriptor_set_bytes(bytes);
        self
    }

    /// Provide a custom entity resolver for federation.
    pub fn with_entity_resolver(
        mut self,
        resolver: Arc<dyn crate::federation::EntityResolver>,
    ) -> Self {
        self.entity_resolver = Some(resolver.clone());
        self.schema_builder = self.schema_builder.with_entity_resolver(resolver);
        self
    }

    /// Restrict the schema to the provided gRPC service full names.
    pub fn with_services<I, S>(mut self, services: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let set: std::collections::HashSet<String> = services.into_iter().map(Into::into).collect();
        self.schema_builder = self.schema_builder.with_services(set.clone());
        self.service_allowlist = Some(set);
        self
    }

    /// Enable GraphQL federation features.
    pub fn enable_federation(mut self) -> Self {
        self.schema_builder = self.schema_builder.enable_federation();
        self
    }

    /// Provide the primary protobuf descriptor set file.
    ///
    /// This clears any existing descriptors and sets this as the primary.
    /// Use [`Self::add_descriptor_set_file`] to add additional descriptor sets.
    pub fn with_descriptor_set_file(mut self, path: impl AsRef<Path>) -> Result<Self> {
        self.schema_builder = self.schema_builder.with_descriptor_set_file(path)?;
        Ok(self)
    }

    /// Add an additional protobuf descriptor set file for schema stitching.
    ///
    /// Use this to combine multiple protobuf descriptor sets from different
    /// microservices into a unified GraphQL schema.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::Gateway;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let gateway = Gateway::builder()
    ///     .with_descriptor_set_file("path/to/users.bin")?
    ///     .add_descriptor_set_file("path/to/products.bin")?
    ///     .add_descriptor_set_file("path/to/orders.bin")?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn add_descriptor_set_file(mut self, path: impl AsRef<Path>) -> Result<Self> {
        self.schema_builder = self.schema_builder.add_descriptor_set_file(path)?;
        Ok(self)
    }

    /// Provide a handler to inspect/augment GraphQL errors before they are returned.
    pub fn with_error_handler<F>(mut self, handler: F) -> Self
    where
        F: Fn(Vec<GraphQLError>) + Send + Sync + 'static,
    {
        self.error_handler = Some(Arc::new(handler));
        self
    }

    /// Set the maximum query depth (nesting level) allowed.
    ///
    /// This is a critical DoS protection mechanism that prevents deeply nested queries
    /// from overwhelming your gRPC backends. Queries exceeding this depth will return
    /// an error: "Query is nested too deep".
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::Gateway;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let gateway = Gateway::builder()
    ///     .with_query_depth_limit(10)  // Max 10 levels of nesting
    ///     // ... other configuration
    /// #   ;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Recommended Values
    ///
    /// - **5-10**: Strict, suitable for simple APIs
    /// - **10-15**: Moderate, good for most production use cases
    /// - **15-25**: Lenient, for complex nested schemas
    pub fn with_query_depth_limit(mut self, max_depth: usize) -> Self {
        self.schema_builder = self.schema_builder.with_query_depth_limit(max_depth);
        self
    }

    /// Set the maximum query complexity allowed.
    ///
    /// This is a critical DoS protection mechanism that limits the total "cost" of a query.
    /// Each field in a query adds to the complexity (default: 1 per field).
    /// Queries exceeding this limit will return an error: "Query is too complex".
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::Gateway;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let gateway = Gateway::builder()
    ///     .with_query_complexity_limit(100)  // Max complexity of 100
    ///     // ... other configuration
    /// #   ;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # How Complexity is Calculated
    ///
    /// - Each scalar field adds 1 to complexity
    /// - Nested objects add their fields' complexity
    /// - List fields multiply by the expected count
    ///
    /// # Recommended Values
    ///
    /// - **50-100**: Strict, suitable for public APIs
    /// - **100-500**: Moderate, good for authenticated users
    /// - **500-1000**: Lenient, for internal/trusted clients
    pub fn with_query_complexity_limit(mut self, max_complexity: usize) -> Self {
        self.schema_builder = self
            .schema_builder
            .with_query_complexity_limit(max_complexity);
        self
    }

    /// Enable health check endpoints (`/health` and `/ready`).
    ///
    /// These endpoints are essential for Kubernetes liveness and readiness probes.
    ///
    /// # Endpoints
    ///
    /// - `GET /health` - Liveness probe, returns 200 if server is running
    /// - `GET /ready` - Readiness probe, checks gRPC client configuration
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::Gateway;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let gateway = Gateway::builder()
    ///     .enable_health_checks()
    ///     // ... other configuration
    /// #   ;
    /// # Ok(())
    /// # }
    /// ```
    pub fn enable_health_checks(mut self) -> Self {
        self.health_checks_enabled = true;
        self
    }

    /// Enable Prometheus metrics endpoint (`/metrics`).
    ///
    /// Exposes metrics for monitoring GraphQL request performance and gRPC backend health.
    ///
    /// # Metrics Exposed
    ///
    /// - `graphql_requests_total` - Total GraphQL requests by operation type
    /// - `graphql_request_duration_seconds` - Request latency histogram
    /// - `graphql_errors_total` - Total GraphQL errors
    /// - `grpc_backend_requests_total` - Total gRPC backend calls
    /// - `grpc_backend_duration_seconds` - gRPC backend latency histogram
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::Gateway;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let gateway = Gateway::builder()
    ///     .enable_metrics()
    ///     // ... other configuration
    /// #   ;
    /// # Ok(())
    /// # }
    /// ```
    pub fn enable_metrics(mut self) -> Self {
        self.metrics_enabled = true;
        self
    }

    /// Enable OpenTelemetry distributed tracing.
    ///
    /// Creates spans for GraphQL operations and gRPC backend calls,
    /// enabling end-to-end visibility across your distributed system.
    ///
    /// # Spans Created
    ///
    /// - `graphql.query` / `graphql.mutation` - For GraphQL operations
    /// - `grpc.call` - For gRPC backend calls
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::Gateway;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let gateway = Gateway::builder()
    ///     .enable_tracing()
    ///     // ... other configuration
    /// #   ;
    /// # Ok(())
    /// # }
    /// ```
    pub fn enable_tracing(mut self) -> Self {
        self.tracing_enabled = true;
        self
    }

    /// Disable GraphQL introspection queries.
    ///
    /// This is a security best practice for production environments to prevent
    /// attackers from discovering your schema structure through `__schema` and `__type` queries.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::Gateway;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let gateway = Gateway::builder()
    ///     .disable_introspection()
    ///     // ... other configuration
    /// #   ;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Environment-Based Toggle
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::Gateway;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let is_prod = std::env::var("ENV").map(|e| e == "production").unwrap_or(false);
    ///
    /// let mut builder = Gateway::builder();
    /// if is_prod {
    ///     builder = builder.disable_introspection();
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn disable_introspection(mut self) -> Self {
        self.schema_builder = self.schema_builder.disable_introspection();
        self
    }

    /// Enable Automatic Persisted Queries (APQ).
    ///
    /// APQ reduces bandwidth by allowing clients to send a hash of the query instead
    /// of the full query string. Queries are cached on the server and retrieved by hash.
    ///
    /// # How It Works
    ///
    /// 1. Client sends hash only → Server returns "PersistedQueryNotFound"
    /// 2. Client sends hash + query → Server caches and executes
    /// 3. Subsequent requests send hash only → Server uses cached query
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::{Gateway, PersistedQueryConfig};
    /// use std::time::Duration;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let gateway = Gateway::builder()
    ///     .with_persisted_queries(PersistedQueryConfig {
    ///         cache_size: 1000,
    ///         ttl: Some(Duration::from_secs(3600)),
    ///     })
    ///     // ... other configuration
    /// #   ;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Default Configuration
    ///
    /// Use `PersistedQueryConfig::default()` for sensible defaults:
    /// - `cache_size`: 1000 queries
    /// - `ttl`: None (no expiration)
    pub fn with_persisted_queries(
        mut self,
        config: crate::persisted_queries::PersistedQueryConfig,
    ) -> Self {
        self.apq_config = Some(config);
        self
    }

    /// Enable Circuit Breaker for gRPC backend resilience.
    ///
    /// The Circuit Breaker prevents cascading failures by "breaking" the circuit
    /// when a backend service is unhealthy, giving it time to recover.
    ///
    /// # States
    ///
    /// - **Closed**: Normal operation, requests flow through
    /// - **Open**: Service unhealthy, requests fail fast (returns error immediately)
    /// - **Half-Open**: Testing recovery, limited requests allowed
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::{Gateway, CircuitBreakerConfig};
    /// use std::time::Duration;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let gateway = Gateway::builder()
    ///     .with_circuit_breaker(CircuitBreakerConfig {
    ///         failure_threshold: 5,                      // Open after 5 failures
    ///         recovery_timeout: Duration::from_secs(30), // Try recovery after 30s
    ///         half_open_max_requests: 3,                 // Allow 3 test requests
    ///     })
    ///     // ... other configuration
    /// #   ;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Default Configuration
    ///
    /// Use `CircuitBreakerConfig::default()` for sensible defaults:
    /// - `failure_threshold`: 5 consecutive failures
    /// - `recovery_timeout`: 30 seconds
    /// - `half_open_max_requests`: 3 test requests
    pub fn with_circuit_breaker(
        mut self,
        config: crate::circuit_breaker::CircuitBreakerConfig,
    ) -> Self {
        self.circuit_breaker_config = Some(config);
        self
    }

    /// Enable response caching for GraphQL queries.
    ///
    /// Response caching stores complete GraphQL responses in memory and serves
    /// them directly for subsequent identical requests, dramatically reducing
    /// latency and backend load.
    ///
    /// # Features
    ///
    /// - **LRU Eviction**: Oldest entries are removed when cache is full
    /// - **TTL Expiration**: Entries expire after a configurable duration
    /// - **Stale-While-Revalidate**: Serve stale content while refreshing in background
    /// - **Mutation Invalidation**: Automatically invalidate cache when mutations run
    /// - **Type/Entity Tracking**: Fine-grained invalidation by type or entity ID
    /// - **Redis Support**: Distributed caching by setting `redis_url` in `CacheConfig`
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::{Gateway, CacheConfig};
    /// use std::time::Duration;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let gateway = Gateway::builder()
    ///     .with_response_cache(CacheConfig {
    ///         max_size: 10_000,                              // Max 10k cached responses
    ///         default_ttl: Duration::from_secs(60),          // 1 minute TTL
    ///         stale_while_revalidate: Some(Duration::from_secs(30)), // Serve stale for 30s
    ///         invalidate_on_mutation: true,                  // Auto-invalidate on mutations
    ///         redis_url: Some("redis://127.0.0.1:6379".to_string()), // Use Redis
    ///         vary_headers: vec!["Authorization".to_string()], // Include auth in cache key
    ///     })
    ///     // ... other configuration
    /// #   ;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # What Gets Cached
    ///
    /// - ✅ Queries (GET and POST)
    /// - ❌ Mutations (never cached, trigger invalidation)
    /// - ❌ Subscriptions (streaming, not cacheable)
    ///
    /// # Cache Key
    ///
    /// The cache key is a SHA-256 hash of:
    /// - Normalized query string
    /// - Sorted variables JSON
    /// - Operation name (if provided)
    /// - Vary headers (e.g. Authorization)
    pub fn with_response_cache(mut self, config: crate::cache::CacheConfig) -> Self {
        self.cache_config = Some(config);
        self
    }

    /// Enable high-performance optimizations for 100K+ RPS.
    ///
    /// This enables features like SIMD-accelerated JSON parsing, lock-free
    /// sharded caching, and optimized gRPC connection pool settings.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::{Gateway, HighPerfConfig};
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let gateway = Gateway::builder()
    ///     .with_high_performance(HighPerfConfig::ultra_fast())
    ///     // ... other configuration
    /// #   ;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_high_performance(
        mut self,
        config: crate::high_performance::HighPerfConfig,
    ) -> Self {
        self.high_perf_config = Some(config);
        self
    }

    /// Enable response compression with gzip, brotli, and deflate.
    ///
    /// Response compression reduces bandwidth usage by compressing HTTP response bodies
    /// before sending them to clients. This is particularly beneficial for GraphQL responses
    /// which are typically JSON and compress very well (50-90% size reduction).
    ///
    /// # Supported Algorithms
    ///
    /// - **Brotli** (`br`) - Best compression ratio, preferred for modern browsers
    /// - **Gzip** (`gzip`) - Widely supported, good compression
    /// - **Deflate** (`deflate`) - Legacy support
    /// - **Zstd** (`zstd`) - Modern, fast compression
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::{Gateway, CompressionConfig, CompressionLevel};
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let gateway = Gateway::builder()
    ///     .with_compression(CompressionConfig {
    ///         enabled: true,
    ///         level: CompressionLevel::Default,
    ///         min_size_bytes: 1024,  // Only compress responses > 1KB
    ///         algorithms: vec!["br".into(), "gzip".into()],
    ///     })
    ///     // ... other configuration
    /// #   ;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Preset Configurations
    ///
    /// Use preset configs for common use cases:
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::{Gateway, CompressionConfig};
    ///
    /// // Fast compression for low latency
    /// # fn fast() { let _ =
    /// Gateway::builder().with_compression(CompressionConfig::fast())
    /// # ; }
    ///
    /// // Best compression for bandwidth savings
    /// # fn best() { let _ =
    /// Gateway::builder().with_compression(CompressionConfig::best())
    /// # ; }
    ///
    /// // Default balanced configuration
    /// # fn default() { let _ =
    /// Gateway::builder().with_compression(CompressionConfig::default())
    /// # ; }
    /// ```
    ///
    /// # Performance Considerations
    ///
    /// - **CPU Cost**: Compression uses CPU. Use `CompressionLevel::Fast` for latency-sensitive apps.
    /// - **Min Size**: Set `min_size_bytes` to skip compression for small responses.
    /// - **Caching**: Compressed responses work well with the response cache.
    pub fn with_compression(mut self, config: CompressionConfig) -> Self {
        self.compression_config = Some(config);
        self
    }

    /// Enable header propagation from GraphQL requests to gRPC backends.
    ///
    /// This forwards specified HTTP headers from incoming GraphQL requests
    /// to outgoing gRPC metadata. Essential for authentication, distributed
    /// tracing, and context propagation.
    ///
    /// # Security
    ///
    /// Uses an **allowlist** approach - only explicitly configured headers
    /// are forwarded to prevent accidental leakage of sensitive data.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::{Gateway, HeaderPropagationConfig};
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let gateway = Gateway::builder()
    ///     .with_header_propagation(HeaderPropagationConfig::new()
    ///         .propagate("authorization")
    ///         .propagate("x-request-id")
    ///         .propagate("x-tenant-id"))
    ///     // ... other configuration
    /// #   ;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Common Headers
    ///
    /// Use `HeaderPropagationConfig::common()` for a preset that includes:
    /// - `authorization` - Bearer tokens
    /// - `x-request-id`, `x-correlation-id` - Request tracking
    /// - `traceparent`, `tracestate` - W3C Trace Context
    /// - `x-b3-*` - Zipkin B3 headers
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::{Gateway, HeaderPropagationConfig};
    ///
    /// # fn example() { let _ =
    /// Gateway::builder().with_header_propagation(HeaderPropagationConfig::common())
    /// # ; }
    /// ```
    pub fn with_header_propagation(mut self, config: HeaderPropagationConfig) -> Self {
        self.header_propagation_config = Some(config);
        self
    }

    /// Enable query whitelisting for production security.
    ///
    /// Query whitelisting (also known as "Stored Operations" or "Persisted Operations")
    /// restricts which GraphQL queries can be executed. This is a **critical security feature**
    /// for public-facing GraphQL APIs that prevents malicious or arbitrary queries.
    ///
    /// # Security Benefits
    ///
    /// - **No Arbitrary Queries**: Only pre-approved queries can be executed
    /// - **Reduced Attack Surface**: Prevents schema exploration and DoS attacks
    /// - **Compliance**: Required for PCI-DSS and other security standards
    /// - **Performance**: Known queries can be optimized and monitored
    ///
    /// # Modes
    ///
    /// - `WhitelistMode::Enforce` - Reject non-whitelisted queries (production)
    /// - `WhitelistMode::Warn` - Log warnings but allow all queries (staging)
    /// - `WhitelistMode::Disabled` - No whitelist checking (development)
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::{Gateway, QueryWhitelistConfig, WhitelistMode};
    /// use std::collections::HashMap;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut allowed_queries = HashMap::new();
    /// allowed_queries.insert(
    ///     "getUserById".to_string(),
    ///     "query getUserById($id: ID!) { user(id: $id) { id name email } }".to_string()
    /// );
    /// allowed_queries.insert(
    ///     "listProducts".to_string(),
    ///     "query { products { id name price } }".to_string()
    /// );
    ///
    /// let gateway = Gateway::builder()
    ///     .with_query_whitelist(QueryWhitelistConfig {
    ///         mode: WhitelistMode::Enforce,
    ///         allowed_queries,
    ///         allow_introspection: true,  // Allow introspection in dev/staging
    ///     })
    ///     // ... other configuration
    /// #   ;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Loading from File
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::{Gateway, QueryWhitelistConfig, WhitelistMode};
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let config = QueryWhitelistConfig::from_json_file(
    ///     "allowed_queries.json",
    ///     WhitelistMode::Enforce
    /// )?;
    ///
    /// let gateway = Gateway::builder()
    ///     .with_query_whitelist(config)
    ///     // ... other configuration
    /// #   ;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Builder Pattern
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::{Gateway, QueryWhitelistConfig, WhitelistMode};
    ///
    /// # fn example() { let _ =
    /// Gateway::builder()
    ///     .with_query_whitelist(
    ///         QueryWhitelistConfig::enforce()
    ///             .add_query("getUser".into(), "query { user { id } }".into())
    ///             .add_query("listPosts".into(), "query { posts { title } }".into())
    ///             .with_introspection(false)  // Disable introspection in production
    ///     )
    /// # ; }
    /// ```
    ///
    /// # How Validation Works
    ///
    /// The whitelist validates queries by:
    /// 1. **Operation ID**: If client provides an operation ID (via extensions), validate by ID
    /// 2. **Query Hash**: Calculate SHA-256 hash of query and validate by hash
    /// 3. **Introspection**: Optionally allow `__schema` and `__type` queries
    ///
    /// # Client Integration
    ///
    /// Clients can reference queries by ID using GraphQL extensions:
    ///
    /// ```json
    /// {
    ///   "operationName": "getUserById",
    ///   "variables": {"id": "123"},
    ///   "extensions": {
    ///     "operationId": "getUserById"
    ///   }
    /// }
    /// ```
    ///
    /// # Works with APQ
    ///
    /// Query whitelisting works alongside Automatic Persisted Queries (APQ):
    /// - **APQ**: Caches any query after first use (bandwidth optimization)
    /// - **Whitelist**: Only allows pre-approved queries (security)
    ///
    /// Use both together for maximum security and performance.
    pub fn with_query_whitelist(
        mut self,
        config: crate::query_whitelist::QueryWhitelistConfig,
    ) -> Self {
        self.query_whitelist_config = Some(config);
        self
    }

    /// Add a REST API connector for hybrid gRPC/REST architectures.
    ///
    /// REST connectors allow you to resolve GraphQL fields from REST APIs
    /// alongside gRPC services, enabling gradual migration or integration
    /// with existing REST backends.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::{Gateway, RestConnector, RestEndpoint, HttpMethod};
    /// use std::time::Duration;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let rest_connector = RestConnector::builder()
    ///     .base_url("https://api.example.com")
    ///     .timeout(Duration::from_secs(30))
    ///     .default_header("Accept", "application/json")
    ///     .add_endpoint(RestEndpoint::new("getUser", "/users/{id}")
    ///         .method(HttpMethod::GET)
    ///         .description("Fetch a user by ID"))
    ///     .add_endpoint(RestEndpoint::new("createUser", "/users")
    ///         .method(HttpMethod::POST)
    ///         .body_template(r#"{"name": "{name}", "email": "{email}"}"#))
    ///     .build()?;
    ///
    /// let gateway = Gateway::builder()
    ///     .add_rest_connector("users_api", rest_connector)
    ///     // ... other configuration
    /// #   ;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Multiple Connectors
    ///
    /// You can add multiple REST connectors for different services:
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::{Gateway, RestConnector, RestEndpoint};
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let users_api = RestConnector::builder()
    ///     .base_url("https://users.example.com")
    ///     .add_endpoint(RestEndpoint::new("getUser", "/users/{id}"))
    ///     .build()?;
    ///
    /// let products_api = RestConnector::builder()
    ///     .base_url("https://products.example.com")
    ///     .add_endpoint(RestEndpoint::new("listProducts", "/products"))
    ///     .build()?;
    ///
    /// let gateway = Gateway::builder()
    ///     .add_rest_connector("users", users_api)
    ///     .add_rest_connector("products", products_api)
    ///     // ... other configuration
    /// #   ;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Authentication
    ///
    /// Use interceptors for authentication:
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::{RestConnector, RestEndpoint, BearerAuthInterceptor};
    /// use std::sync::Arc;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let connector = RestConnector::builder()
    ///     .base_url("https://api.example.com")
    ///     .interceptor(Arc::new(BearerAuthInterceptor::new("your-token")))
    ///     .add_endpoint(RestEndpoint::new("getUser", "/users/{id}"))
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Use Cases
    ///
    /// | Scenario | Description |
    /// |----------|-------------|
    /// | Hybrid Architecture | Mix gRPC and REST backends in one GraphQL API |
    /// | Gradual Migration | Migrate from REST to gRPC incrementally |
    /// | Third-Party APIs | Integrate external REST APIs into your schema |
    /// | Legacy Systems | Bridge legacy REST services with modern gRPC |
    pub fn add_rest_connector(mut self, name: impl Into<String>, connector: RestConnector) -> Self {
        self.rest_connectors.register(name, connector);
        self
    }

    /// Get a reference to the REST connector registry.
    pub fn rest_connectors(&self) -> &RestConnectorRegistry {
        &self.rest_connectors
    }

    /// Enable query analytics with the built-in dashboard.
    ///
    /// Query analytics provides comprehensive insights into your GraphQL API usage:
    /// - **Most Used Queries**: Track which queries are most popular
    /// - **Slowest Queries**: Identify performance bottlenecks
    /// - **Error Patterns**: Analyze common errors and their causes
    /// - **Field Usage**: See which fields are requested most often
    ///
    /// # Endpoints
    ///
    /// When enabled, the following endpoints are added:
    /// - `GET /analytics` - Interactive analytics dashboard
    /// - `GET /analytics/api` - JSON API for analytics data
    /// - `POST /analytics/reset` - Reset all analytics data
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::{Gateway, AnalyticsConfig};
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let gateway = Gateway::builder()
    ///     .enable_analytics(AnalyticsConfig::default())
    ///     // ... other configuration
    /// #   ;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Privacy-Conscious Configuration
    ///
    /// For production environments where query text should not be stored:
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::{Gateway, AnalyticsConfig};
    ///
    /// # fn example() { let _ =
    /// Gateway::builder()
    ///     .enable_analytics(AnalyticsConfig::production())
    /// # ; }
    /// ```
    ///
    /// # Preset Configurations
    ///
    /// - `AnalyticsConfig::default()` - Balanced settings for most use cases
    /// - `AnalyticsConfig::production()` - Privacy-focused, shorter retention
    /// - `AnalyticsConfig::development()` - Verbose, longer retention for debugging
    pub fn enable_analytics(mut self, config: crate::analytics::AnalyticsConfig) -> Self {
        self.analytics_config = Some(config);
        self
    }

    /// Enable request collapsing to deduplicate identical gRPC requests.
    ///
    /// Request collapsing detects when multiple GraphQL fields in the same query
    /// would trigger identical gRPC calls and collapses them into a single call,
    /// sharing the response across all requesting fields.
    ///
    /// # How It Works
    ///
    /// When a GraphQL query contains multiple fields calling the same gRPC method
    /// with the same arguments:
    ///
    /// ```graphql
    /// query {
    ///   user1: getUser(id: "1") { name }
    ///   user2: getUser(id: "2") { name }
    ///   user3: getUser(id: "1") { name }  # Duplicate of user1
    /// }
    /// ```
    ///
    /// - **Without collapsing**: 3 gRPC calls
    /// - **With collapsing**: 2 gRPC calls (user1 and user3 share the same response)
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::{Gateway, RequestCollapsingConfig};
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let gateway = Gateway::builder()
    ///     .with_request_collapsing(RequestCollapsingConfig::default())
    ///     // ... other configuration
    /// #   ;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Preset Configurations
    ///
    /// - `RequestCollapsingConfig::default()` - Balanced settings for most use cases
    /// - `RequestCollapsingConfig::high_throughput()` - Longer coalesce window, more waiters
    /// - `RequestCollapsingConfig::low_latency()` - Shorter coalesce window for speed
    /// - `RequestCollapsingConfig::disabled()` - Disable collapsing entirely
    ///
    /// # Configuration Options
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::RequestCollapsingConfig;
    /// use std::time::Duration;
    ///
    /// # fn example() {
    /// let config = RequestCollapsingConfig::new()
    ///     .coalesce_window(Duration::from_millis(100))  // Wait up to 100ms for in-flight
    ///     .max_waiters(200)                             // Max 200 followers per leader
    ///     .max_cache_size(20000);                       // Track up to 20k in-flight requests
    /// # }
    /// ```
    ///
    /// # When to Use
    ///
    /// Request collapsing is most effective when:
    /// - Queries frequently request the same data with aliases
    /// - High-traffic scenarios where concurrent requests are common
    /// - DataLoader is not applicable (e.g., non-entity resolvers)
    ///
    /// # Relationship with Response Caching
    ///
    /// - **Response Cache**: Caches complete responses for TTL duration
    /// - **Request Collapsing**: Deduplicates in-flight requests during execution
    ///
    /// Both features work together: collapsing handles concurrent requests,
    /// while caching handles sequential repeated requests.
    pub fn with_request_collapsing(mut self, config: RequestCollapsingConfig) -> Self {
        self.request_collapsing_config = Some(config);
        self
    }

    /// Enable `@defer` incremental delivery support.
    ///
    /// When enabled, queries containing `@defer` directives will return an
    /// initial payload immediately followed by incremental patches streamed
    /// over a `multipart/mixed` HTTP response.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::{Gateway, DeferConfig};
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let gateway = Gateway::builder()
    ///     .with_defer(DeferConfig::default())
    ///     // ... other configuration
    /// #   ;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Preset Configurations
    ///
    /// - `DeferConfig::default()` — Balanced settings
    /// - `DeferConfig::production()` — Strict limits
    /// - `DeferConfig::development()` — Permissive
    /// - `DeferConfig::disabled()` — Disabled (all fields resolved eagerly)
    pub fn with_defer(mut self, config: crate::defer::DeferConfig) -> Self {
        self.defer_config = Some(config);
        self
    }

    /// Build the gateway
    pub fn build(self) -> Result<Gateway> {
        let mut schema_builder = self.schema_builder;
        if let Some(resolver) = self.entity_resolver {
            schema_builder = schema_builder.with_entity_resolver(resolver);
        }
        if let Some(services) = self.service_allowlist {
            schema_builder = schema_builder.with_services(services);
        }
        if let Some(header_config) = self.header_propagation_config.as_ref() {
            schema_builder = schema_builder.with_header_propagation(header_config.clone());
        }

        // Pass REST connectors to schema builder for GraphQL field generation
        if !self.rest_connectors.is_empty() {
            let mut registry = crate::rest_connector::RestConnectorRegistry::new();
            for (name, connector) in self.rest_connectors.connectors() {
                // Clone the connector into the registry (dereference Arc)
                registry.register(name.clone(), (**connector).clone());
            }
            schema_builder = schema_builder.with_rest_connectors(registry);
        }

        // Plugin Hook: on_schema_build
        // We need to clone plugins or pass a mutable reference. Since build consumes self,
        // we can use the plugins directly but we need to deal with async in a synchronous build method.
        // Wait, build is synchronous. Hooks are async.
        //
        // This is a problem. on_schema_build must be synchronous or build must be async.
        // The Plugin trait defined on_schema_build as async. 
        // 
        // Let's check `GatewayBuilder::build` signature: public fn build(self) -> Result<Gateway>
        // Use block_on to execute the async hook or change the signature?
        // Changing signature is a breaking change.
        //
        // However, schema building is CPU intensive and usually done at startup.
        // Let's use `futures::executor::block_on` or similar if available, or just tokio::task::block_in_place if inside runtime.
        // But `build` might be called outside of runtime context (e.g. tests).
        //
        // Actually, `GatewayBuilder::serve` is async. But `build` is sync.
        // Let's try to run it synchronously for now.
        //
        // Wait, I can't easily change the trait to be sync if it's already async in my definition.
        // Let's make `on_schema_build` synchronous in the trait definition instead?
        //
        // Re-reading `src/plugin.rs`:
        // async fn on_schema_build(&self, _builder: &mut crate::schema::SchemaBuilder) -> crate::error::Result<()> {
        //
        // It is async.
        //
        // I will use `futures::executor::block_on` to run the hooks.
        // We need to add `futures` to dependencies or use `tokio::runtime::Handle::current().block_on(async { ... })`.

        let plugins = self.plugins;
        // Run on_schema_build hook
        // Run on_schema_build hook
        // We use futures::executor::block_on because we are in a synchronous build method.
        // This blocks the current thread, which is acceptable during initialization.
        futures::executor::block_on(plugins.on_schema_build(&mut schema_builder))?;

        let schema = schema_builder.build(&self.client_pool)?;
        let mut mux = ServeMux::new(schema.clone());
        mux.set_plugins(plugins);

        // Add middlewares
        for middleware in self.middlewares {
            mux.add_middleware(middleware);
        }

        if let Some(handler) = self.error_handler {
            mux.set_error_handler_arc(handler);
        }

        // Configure health checks
        if self.health_checks_enabled {
            mux.set_client_pool(self.client_pool.clone());
            mux.enable_health_checks();
        }

        // Configure metrics
        if self.metrics_enabled {
            mux.enable_metrics();
        }

        // Configure APQ
        if let Some(apq_config) = self.apq_config {
            mux.enable_persisted_queries(apq_config);
        }

        // Configure Circuit Breaker
        if let Some(cb_config) = self.circuit_breaker_config {
            mux.enable_circuit_breaker(cb_config);
        }

        // Configure Response Cache
        if let Some(cache_config) = self.cache_config {
            mux.enable_response_cache(cache_config);
        }

        // Configure Compression
        if let Some(compression_config) = self.compression_config {
            mux.enable_compression(compression_config);
        }

        // Configure Query Whitelist
        if let Some(whitelist_config) = self.query_whitelist_config {
            mux.enable_query_whitelist(whitelist_config);
        }

        // Configure Analytics
        if let Some(analytics_config) = self.analytics_config {
            mux.enable_analytics(analytics_config);
        }

        // Configure Request Collapsing
        if let Some(collapsing_config) = self.request_collapsing_config {
            mux.enable_request_collapsing(collapsing_config);
        }

        // Configure High Performance mode
        if let Some(high_perf_config) = self.high_perf_config {
            mux.enable_high_performance(high_perf_config);
        }

        // Configure @defer support
        if let Some(defer_config) = self.defer_config {
            mux.enable_defer(defer_config);
        }

        Ok(Gateway {
            mux,
            client_pool: self.client_pool,
            schema,
        })
    }

    /// Build and start the gateway server
    pub async fn serve(self, addr: impl Into<String>) -> Result<()> {
        let shutdown_config = self.shutdown_config.clone();
        let gateway = self.build()?;
        let addr = addr.into();
        let listener = tokio::net::TcpListener::bind(&addr).await?;

        tracing::info!("Gateway server listening on {}", addr);

        let app = gateway.into_router();

        // Use graceful shutdown if configured
        if let Some(config) = shutdown_config {
            run_with_graceful_shutdown(listener, app, config).await?;
        } else {
            axum::serve(listener, app).await?;
        }

        Ok(())
    }

    /// Enable graceful shutdown with the specified configuration.
    ///
    /// When graceful shutdown is enabled, the server will:
    /// 1. Stop accepting new connections on SIGTERM/SIGINT
    /// 2. Wait for in-flight requests to complete (up to timeout)
    /// 3. Clean up resources and exit
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::{Gateway, ShutdownConfig};
    /// use std::time::Duration;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// Gateway::builder()
    ///     .with_graceful_shutdown(ShutdownConfig {
    ///         timeout: Duration::from_secs(30),
    ///         ..Default::default()
    ///     })
    ///     .serve("0.0.0.0:8080")
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Default Configuration
    ///
    /// Use `ShutdownConfig::default()` for sensible defaults:
    /// - `timeout`: 30 seconds
    /// - `handle_signals`: true (handles SIGTERM/SIGINT)
    /// - `force_shutdown_delay`: 5 seconds
    pub fn with_graceful_shutdown(mut self, config: ShutdownConfig) -> Self {
        self.shutdown_config = Some(config);
        self
    }

    /// Build and start the gateway server with explicit shutdown signal.
    ///
    /// This method allows you to provide a custom shutdown future, giving
    /// you full control over when the server shuts down.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use grpc_graphql_gateway::Gateway;
    /// use tokio::sync::oneshot;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let (tx, rx) = oneshot::channel::<()>();
    ///
    /// // Spawn a task that triggers shutdown after some condition
    /// tokio::spawn(async move {
    ///     tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    ///     let _ = tx.send(());
    /// });
    ///
    /// Gateway::builder()
    ///     // ... configuration ...
    ///     .serve_with_shutdown("0.0.0.0:8080", async { let _ = rx.await; })
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn serve_with_shutdown<F>(
        self,
        addr: impl Into<String>,
        shutdown_signal: F,
    ) -> Result<()>
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let gateway = self.build()?;
        let addr = addr.into();
        let listener = tokio::net::TcpListener::bind(&addr).await?;

        tracing::info!("Gateway server listening on {}", addr);

        let app = gateway.into_router();
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal)
            .await?;

        Ok(())
    }
}

impl Default for GatewayBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use crate::compression::CompressionLevel;

    // Minimal descriptor for testing
    const TEST_DESCRIPTOR: &[u8] = include_bytes!("generated/greeter_descriptor.bin");

    #[tokio::test]
    async fn test_builder_creation() {
        let builder = GatewayBuilder::new();
        let result = builder.build();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_builder_default() {
        let builder1 = GatewayBuilder::new();
        let builder2 = GatewayBuilder::default();
        
        // Both should behave the same
        assert!(builder1.build().is_err());
        assert!(builder2.build().is_err());
    }

    #[tokio::test]
    async fn test_gateway_builder() {
        let builder = Gateway::builder();
        assert!(builder.build().is_err()); // No descriptor set yet
    }

    #[tokio::test]
    async fn test_grpc_client_pool() {
        let pool = GrpcClientPool::new();
        assert_eq!(pool.names().len(), 0);
    }

    #[tokio::test]
    async fn test_builder_with_descriptor_bytes() {
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR);
        
        // Should not error with valid descriptor
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_builder_add_descriptor_bytes() {
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .add_descriptor_set_bytes(TEST_DESCRIPTOR); // Add another
        
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_builder_add_grpc_client() {
        let client = GrpcClient::connect_lazy("http://localhost:50051", true).unwrap();
        
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .add_grpc_client("test.Service", client);
        
        let gateway = builder.build().unwrap();
        assert!(!gateway.client_pool().names().is_empty());
    }

    #[tokio::test]
    async fn test_builder_add_grpc_clients() {
        let client1 = GrpcClient::connect_lazy("http://localhost:50051", true).unwrap();
        let client2 = GrpcClient::connect_lazy("http://localhost:50052", true).unwrap();
        
        let clients = vec![
            ("service1".to_string(), client1),
            ("service2".to_string(), client2),
        ];
        
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .add_grpc_clients(clients);
        
        let gateway = builder.build().unwrap();
        assert!(gateway.client_pool().names().len() >= 2);
    }

    #[tokio::test]
    async fn test_builder_with_services() {
        let services = vec!["example.UserService".to_string()];
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .with_services(services);
        
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_builder_enable_federation() {
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .enable_federation();
        
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_builder_with_compression() {
        let config = CompressionConfig::default();
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .with_compression(config);
        
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_builder_with_compression_fast() {
        let config = CompressionConfig::fast();
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .with_compression(config);
        
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_builder_with_compression_best() {
        let config = CompressionConfig::best();
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .with_compression(config);
        
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_builder_with_header_propagation() {
        let config = HeaderPropagationConfig::new()
            .propagate("authorization")
            .propagate("x-request-id");
        
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .with_header_propagation(config);
        
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_builder_with_header_propagation_common() {
        let config = HeaderPropagationConfig::common();
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .with_header_propagation(config);
        
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_builder_with_query_depth_limit() {
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .with_query_depth_limit(10);
        
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_builder_with_query_complexity_limit() {
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .with_query_complexity_limit(1000);
        
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_builder_enable_health_checks() {
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .enable_health_checks();
        
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_builder_enable_metrics() {
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .enable_metrics();
        
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_builder_with_graceful_shutdown() {
        let config = ShutdownConfig {
            timeout: Duration::from_secs(30),
            ..Default::default()
        };
        
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .with_graceful_shutdown(config);
        
        let result = builder.build();
        assert!(result.is_ok());
    }



    #[tokio::test]
    async fn test_builder_chain_multiple_configs() {
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .with_compression(CompressionConfig::fast())
            .with_header_propagation(HeaderPropagationConfig::common())
            .with_query_depth_limit(15)
            .with_query_complexity_limit(2000)
            .enable_health_checks()
            .enable_metrics();
        
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_gateway_accessors() {
        let gateway = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .build()
            .unwrap();
        
        // Test accessor methods
        let _ = gateway.schema();
        let _ = gateway.client_pool();
        let _ = gateway.mux();
    }

    #[tokio::test]
    async fn test_gateway_into_router() {
        let gateway = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .build()
            .unwrap();
        
        let _router = gateway.into_router();
        // Router created successfully
    }

    #[tokio::test]
    async fn test_builder_empty_descriptor_fails() {
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes([]);
        
        let result = builder.build();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_builder_invalid_descriptor_fails() {
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes([0xFF, 0xFF, 0xFF, 0xFF]);
        
        let result = builder.build();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_builder_with_circuit_breaker() {
        use crate::circuit_breaker::CircuitBreakerConfig;
        
        let config = CircuitBreakerConfig::default();
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .with_circuit_breaker(config);
        
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_builder_with_response_cache() {
        use crate::cache::CacheConfig;
        
        let config = CacheConfig {
            max_size: 1000,
            default_ttl: Duration::from_secs(60),
            ..Default::default()
        };
        
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .with_response_cache(config);
        
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_builder_enable_analytics() {
        use crate::analytics::AnalyticsConfig;
        
        let config = AnalyticsConfig::default();
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .enable_analytics(config);
        
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_builder_with_request_collapsing() {
        use crate::request_collapsing::RequestCollapsingConfig;
        
        let config = RequestCollapsingConfig::default();
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .with_request_collapsing(config);
        
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_builder_with_high_performance() {
        use crate::high_performance::HighPerfConfig;
        
        let config = HighPerfConfig::default();
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .with_high_performance(config);
        
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_builder_with_query_whitelist() {
        let config = crate::query_whitelist::QueryWhitelistConfig::warn();
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .with_query_whitelist(config);
        
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_rest_connectors_empty() {
        let builder = GatewayBuilder::new();
        assert!(builder.rest_connectors().is_empty());
    }

    #[tokio::test]
    async fn test_client_pool_operations() {
        let pool = GrpcClientPool::new();
        
        let client = GrpcClient::connect_lazy("http://localhost:50051", true).unwrap();
        pool.add("service1", client.clone());
        
        assert_eq!(pool.names().len(), 1);
        assert!(pool.get("service1").is_some());
        assert!(pool.get("nonexistent").is_none());
    }

    #[tokio::test]
    async fn test_complete_configuration_stack() {
        // Build a gateway with ALL features enabled
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .with_compression(CompressionConfig::best())
            .with_header_propagation(HeaderPropagationConfig::common())
            .with_query_depth_limit(20)
            .with_query_complexity_limit(5000)
            .enable_health_checks()
            .enable_metrics()
            .with_graceful_shutdown(ShutdownConfig::default())
            .with_circuit_breaker(crate::circuit_breaker::CircuitBreakerConfig::default())
            .with_response_cache(crate::cache::CacheConfig::default())
            .with_high_performance(crate::high_performance::HighPerfConfig::default())
            .with_query_whitelist(crate::query_whitelist::QueryWhitelistConfig::warn())
            .enable_analytics(crate::analytics::AnalyticsConfig::default())
            .with_request_collapsing(RequestCollapsingConfig::default());
        
        let result = builder.build();
        assert!(result.is_ok(), "Full configuration stack should build successfully");
    }

    #[tokio::test]
    async fn test_multiple_descriptor_sets() {
        let builder = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .add_descriptor_set_bytes(TEST_DESCRIPTOR)
            .add_descriptor_set_bytes(TEST_DESCRIPTOR);
        
        let result = builder.build();
        assert!(result.is_ok());
    }



    #[tokio::test]
    async fn test_gateway_schema_access() {
        let gateway = GatewayBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR)
            .build()
            .unwrap();
        
        let _schema = gateway.schema();
        // Schema is accessible
    }

    #[tokio::test]
    async fn test_compression_levels() {
        for level in [
            CompressionLevel::Fast,
            CompressionLevel::Default,
            CompressionLevel::Best,
            CompressionLevel::Custom(5),
        ] {
            let builder = GatewayBuilder::new()
                .with_descriptor_set_bytes(TEST_DESCRIPTOR)
                .with_compression(CompressionConfig::new().with_level(level));
            
            assert!(builder.build().is_ok());
        }
    }


}

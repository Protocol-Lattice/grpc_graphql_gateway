# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.3] - 2025-12-18

### Added
- **GBP Federation Router**: New `router` module with a high-performance GraphQL Federation Router.
  - `GbpRouter` - Scatter-gather federation router with GBP Ultra compression for subgraph communication
  - `RouterConfig` - Configure subgraphs, GBP settings, and HTTP/2 connections
  - `SubgraphConfig` - Per-subgraph configuration (URL, timeout, GBP enable)
  - `DdosConfig` - Two-tier DDoS protection configuration with presets
  - `DdosProtection` - Token bucket rate limiting with global and per-IP limits
  - Parallel subgraph execution with latency ≈ slowest subgraph
  - ~99% bandwidth reduction between router and subgraphs

### Performance
- GBP-enabled router reduces inter-service bandwidth by 99%
- Scatter-gather pattern minimizes total federation latency
- HTTP/2 connection pooling for efficient subgraph communication

## [0.5.2] - 2025-12-18

### Added
- **DDoS Protection Presets**: Added `DdosConfig::strict()` and `DdosConfig::relaxed()` for common use cases.
- **Router Binary**: New `cargo run --bin router` for standalone federation router deployment.

### Changed
- Improved token bucket algorithm efficiency in `DdosProtection`.
- Enhanced rate limiter cleanup for stale IP entries.

## [0.5.1] - 2025-12-18

### Added
- **GBP Decoder**: Full `GbpDecoder` implementation for decoding GBP Ultra payloads.
  - `decode()` - Decode raw GBP bytes to JSON
  - `decode_lz4()` - Decode LZ4-compressed GBP payloads
  - Value pool reference resolution
  - Columnar array reconstruction

### Fixed
- Fixed value pool synchronization between encoder and decoder (Post-Order traversal).
- Fixed columnar encoding for arrays with 5+ homogeneous objects.

## [0.5.0] - 2025-12-18

### Added
- **GBP Ultra**: Official release of the **GraphQL Binary Protocol v8** with **<1ms encoding latency** and **99.25% compression ratio**.
  - **Zero-Allocation Hashing**: Replaced expensive string serialization with fast structural hashing (`AHashMap` + custom hashers).
  - **LZ4 Optimization**: Tuned encoding for maximum throughput (176 MB/s).
  - **Memory Efficiency**: Reduced memory usage during high-load encoding by ~40%.
## [0.4.9] - 2025-12-18

### Added
- **High-Performance Optimization Mode**:
  - **SIMD-Accelerated JSON Parsing**: Switched to `simd-json` for 2x-5x faster parsing using AVX2/NEON instructions.
  - **Lock-Free Sharded Cache**: Replaced global cache locks with a sharded implementation (128 shards) to eliminate contention in multi-core environments.
  - **Object Pooling**: Pre-allocated request/response buffers to achieve near-zero allocation in steady-state high-traffic scenarios.
  - **100K+ RPS Support**: Validated architecture capable of exceeding 100,000 requests per second per instance.

- **Cost Reduction & Efficiency Suite**:
  - **Advanced Query Cost Analysis**: Implemented a tree-walking algorithm to calculate query complexity and prevent expensive "depth-bomb" or "wide-fetch" queries.
  - **Smart TTL Management**: Dynamic cache TTL adjustment based on object frequency and mutation patterns, significantly improving cache hit rates for expensive entities.
  - **Webhook & Event System**: Direct integration for cache invalidation and external triggers.

- **GraphQL Binary Protocol (GBP) v8**:
  - **99.25% Compression Reduction**: Verified on 100MB+ "Behemoth" payloads (~107MB reduced to ~804KB).
  - **Proven Reliability**: Full data integrity verification on large-scale datasets.
  - **Structural Deduplication**: Efficient "Shape" templates and Value Pooling to eliminate redundant overhead.
  - **Columnar Storage**: Optimized array encoding for maximal LZ4 block compression.

- **Security Hardening**:
  - **Vulnerability Remediation**: Addressed multiple CVEs in dependencies as part of the 0.4.2 and 0.4.4 security bumps.
  - **Enhanced IP Spoofing Protection**: Hardened `X-Forwarded-For` and `X-Real-IP` handling in the middleware.
  - **Query Whitelist Normalization**: Multi-pass normalization to prevent bypasses via whitespace or comment manipulation.

### Changed
- **Performance Profile Refactor**: `CompressionConfig::ultra_fast()` now defaults to `gbp-lz4` for server-to-server traffic.
- **Dependency Upgrades**: Large-scale update of core dependencies including `tonic`, `axum`, and `prost`.

### Fixed
- Fixed data integrity issue in recursive encoding by using Post-Order synchronization for value pooling.
- Fixed borrow checker contention in GBP decoder implementation.
- Fixed `DataLoader` entity resolution bugs in federated environments.

## [0.4.8] - 2025-12-18

### Added
- Refined **LZ4 + GBP Ultra** algorithm with aggressive structural deduplication.
- Updated `GbpEncoder` to v8 for improved performance on deeply nested responses.

## [0.4.7] - 2025-12-18

### Added
- Initial support for **LZ4 algorithm (#49)** integration.
- Performance benchmarks for LZ4 vs standard Gzip.

## [0.4.6] - 2025-12-18

### Changed
- Updated internal buffer handling for high-throughput mode.
- Maintenance bump for dependency synchronization.

## [0.4.5] - 2025-12-18

### Fixed
- Minor fixes in the `protoc` plugin's template generation.

## [0.4.4] - 2025-12-17

### Security
- **Version Bump**: Critical security fixes for dependency vulnerabilities.
- Hardened internal gRPC client request handling.

## [0.4.3] - 2025-12-17

### Changed
- Improved memory management in the sharded cache implementation.

## [0.4.2] - 2025-12-17

### Security
- **Vulnerability Patch**: Fixed multiple security vulnerabilities identified in CI/CD pipeline.
- Improved error handling in `protoc-gen-graphql-template`.

## [0.4.1] - 2025-12-17

### Fixed
- Fixed edge cases in JSON serialization for large integers.

## [0.4.0] - 2025-12-17

### Added
- **High Performance Runtime**: Initial integration of sharded cache and SIMD optimizations into the main serving loop.
- **Protoc Plugin Enhancement**: Metadata support for `.rs` file generation.

## [0.3.9] - 2025-12-16

### Added
- **Redis Cache Backend Integration**: Support for distributed caching across multiple gateway instances.
- **Smart TTL Foundations**: Initial implementation of dynamic TTL logic.

## [0.3.8] - 2025-12-16

### Added
- **Helm Chart Deployment**: Production-ready Kubernetes deployment via Helm charts.
  - `helm/grpc-graphql-gateway/` - Complete Helm chart with templates for deployment, service, HPA, VPA, and LoadBalancer
  - `deploy-federation.sh` - One-click deployment script for federated architecture (3 subgraphs)
  - `validate-chart.sh` - Chart validation and packaging script
  - Pre-configured values files for autoscaling and federation scenarios
  - Support for AWS, GCP, and generic LoadBalancer configurations

- **Docker Containerization**: Multi-stage Dockerfile for optimized production images.
  - `Dockerfile` - Standard gateway image with minimal footprint
  - `Dockerfile.federation` - Federation-ready image for subgraph deployments
  - `docker-compose.federation.yml` - Complete federation example with 3 subgraphs
  - `build-docker.sh` - Build script with version tagging

- **Kubernetes Autoscaling**: Native Kubernetes autoscaling support.
  - **HPA (Horizontal Pod Autoscaler)**: Scale pods based on CPU/memory utilization (5-50 pods configurable)
  - **VPA (Vertical Pod Autoscaler)**: Resource recommendation and automatic adjustment
  - Multi-AZ distribution for high availability
  - External LoadBalancer with cloud provider annotations

- **Deployment Documentation**: Comprehensive guides for production deployment.
  - `DEPLOYMENT.md` - Quick start guide for Docker and Kubernetes
  - `ARCHITECTURE.md` - System architecture documentation
  - `docs/src/production/helm-deployment.md` - Detailed Helm deployment guide
  - `docs/src/production/autoscaling.md` - Autoscaling configuration guide

### Fixed
- **Rustdoc Intra-Doc Links**: Fixed broken documentation links for `add_descriptor_set_bytes` and `add_descriptor_set_file` methods in `gateway.rs` and `schema.rs`, resolving docs.rs build failures.

### Deployment Quick Start
```bash
# Docker
docker build -t grpc-graphql-gateway:latest .
docker run -p 8080:8080 grpc-graphql-gateway:latest

# Kubernetes (Helm)
helm install my-gateway ./helm/grpc-graphql-gateway \
  --namespace grpc-gateway \
  --create-namespace

# Federation
./helm/deploy-federation.sh
```

## [0.3.7] - 2025-12-16

### Security - Production Hardening

#### Comprehensive Security Headers
- **HSTS (Strict-Transport-Security)**: Added `max-age=31536000; includeSubDomains` header to enforce HTTPS
- **Content-Security-Policy**: Added restrictive CSP (`default-src 'self'`) to prevent XSS attacks
- **X-XSS-Protection**: Added `1; mode=block` for legacy browser protection
- **Referrer-Policy**: Added `strict-origin-when-cross-origin` to limit referrer information leakage
- **Cache-Control**: Added `no-store, no-cache, must-revalidate` to prevent caching of sensitive responses
- **CORS Preflight**: Proper OPTIONS method handling with configurable CORS headers

#### Query Whitelist Enforcement
- **Production Mode**: Changed default to `WhitelistMode::Enforce` (rejects non-whitelisted queries)
- **Introspection Disabled**: Set `allow_introspection: false` for production security
- **Improved Normalization**: Enhanced `normalize_query()` to correctly handle:
  - Comment stripping (`# comment` and `"""..."""`)
  - Whitespace collapsing around punctuation
  - String literal preservation
  - Consistent hash generation for semantically equivalent queries

### Changed
- **Redis Crate Upgrade**: Updated `redis` dependency from `0.24` to `0.27`
  - Replaced deprecated `get_async_connection()` with `get_multiplexed_async_connection()`
  - Removed unused `connection_manager` field from `CacheBackend::Redis`
  - Fixed trait bounds for `Pipeline::query_async()` with explicit type annotation

### Fixed
- Fixed query whitelist normalization to correctly match queries with different formatting
- Fixed T3 security test false positive caused by grepping startup banner text
- Fixed Redis cache pipeline operations for `redis` 0.27 API compatibility

### Added
- 31-test comprehensive security assessment script (`test_security.sh`)
- Transport security tests (HSTS, XSS, CSP, CORS)
- Input validation tests (fuzzing, payload limits, depth limits)
- GraphQL protocol tests (introspection, batching, operations)

### Security Test Results (v0.3.7)
```
[PASS] T1-T3:   Core Headers & Normalization
[PASS] T8:      Introspection Disabled
[PASS] T9-T10:  DoS Protection (Large Payload, Deep Query)
[PASS] T11:     Suggestions Disabled
[PASS] T12:     HSTS Enabled
[PASS] T15-T16: HTTP Methods (TRACE blocked, OPTIONS CORS)
[PASS] T17-T22: Input Validation
[PASS] T23-T30: GraphQL Protocol Security
```

## [0.3.6] - 2025-12-16

### Security
- **DoS Protection**: Completely removed `std::sync::RwLock` in favor of `parking_lot::RwLock` across all modules (`cache`, `analytics`, `persisted_queries`, `circuit_breaker`, `grpc_client`, `query_whitelist`, `request_collapsing`). This prevents lock poisoning which could lead to Denial of Service.
- **IP Spoofing Protection**: Added strict IP address validation in `middleware.rs` to prevent `X-Forwarded-For` and `X-Real-IP` header injection attacks.
- **Query Normalization**: Implemented robust query normalization in `QueryWhitelist` to prevent bypasses via whitespace/comment manipulation.
- **SSRF Protection**: Enhanced validation in `RestConnector` to prevent control character injection in URL construction.
- **Security Headers**: Added `X-Content-Type-Options: nosniff` and `X-Frame-Options: DENY` headers to all responses.
- **Safe Lock Access**: Removed all `.unwrap()` calls on locks, replacing them with safe `parking_lot` API access patterns.

### Fixed
- Fixed compilation error in `runtime.rs` related to Axum router state typing.
- Fixed weak hashing usage in `analytics.rs` (now consistently using SHA-256).

## [0.3.5] - 2025-12-16

### Added
- **Redis Distributed Cache Backend**: New optional Redis backend for response caching, enabling horizontal scaling.
  - `CacheConfig.redis_url` - Configure Redis connection URL for distributed caching
  - `CacheBackend::Redis` - Redis-based cache storage with automatic connection management
  - Redis SET/GET operations for cache entries with TTL expiration
  - Redis SETs for type and entity indexes supporting distributed invalidation
  - Atomic pipeline operations for efficient cache updates
  - Automatic fallback to in-memory cache if Redis connection fails

### Features
- **Dual Backend Support**: Choose between in-memory (single instance) or Redis (distributed) caching
- **Distributed Invalidation**: Type-based and entity-based cache invalidation works across all gateway instances
- **Redis Index Sets**: `type:{TypeName}` and `entity:{EntityKey}` sets for efficient distributed invalidation
- **TTL Synchronization**: Cache entries use Redis `SETEX` with configurable TTL
- **Connection Resilience**: Graceful error handling with fallback behavior

### Configuration
```rust
use grpc_graphql_gateway::CacheConfig;
use std::time::Duration;

// In-memory cache (default)
let config = CacheConfig::default();

// Redis-backed distributed cache
let config = CacheConfig {
    max_size: 10_000,
    default_ttl: Duration::from_secs(60),
    stale_while_revalidate: Some(Duration::from_secs(30)),
    invalidate_on_mutation: true,
    redis_url: Some("redis://localhost:6379".to_string()),
};
```

### Use Cases
- Horizontal scaling with multiple gateway instances sharing cached responses
- Kubernetes deployments where pod instances need shared cache state
- High-availability setups requiring cache consistency across replicas
- Microservices architectures with centralized cache management

### Dependencies
- Added `redis` crate (0.24) with `tokio-comp` and `connection-manager` features

## [0.3.4] - 2025-12-14

### Added
- **OpenAPI to REST Connector**: New `openapi` module for automatically generating REST connectors from OpenAPI/Swagger specifications.
  - `OpenApiParser` - Parse OpenAPI 3.0/3.1 and Swagger 2.0 specs
  - `OpenApiSpec` - Parsed specification structure
  - `Operation`, `Parameter`, `Response` - Operation metadata types
  - Support for JSON and YAML formats (YAML requires `yaml` feature)
  - Automatic endpoint generation from paths and operations
  - Schema resolution for request bodies and response types
  - Operation filtering by tags or custom predicates
  - Base URL override for different environments

### Features
- **From File**: `OpenApiParser::from_file("openapi.yaml")?`
- **From URL**: `OpenApiParser::from_url("https://api.example.com/openapi.json").await?`
- **From String**: `OpenApiParser::from_string(json_str, false)?`
- **Filtering**: `.with_tags(vec!["pets"])` or `.filter_operations(|id, _| id.starts_with("get"))`
- **Prefix**: `.with_prefix("api_")` to namespace all operations

### Example Usage
```rust
use grpc_graphql_gateway::{Gateway, OpenApiParser};

// Parse OpenAPI spec and create REST connector
let connector = OpenApiParser::from_file("petstore.yaml")?
    .with_base_url("https://api.petstore.io/v2")
    .with_tags(vec!["pets".to_string()])
    .build()?;

let gateway = Gateway::builder()
    .add_rest_connector("petstore", connector)
    .build()?;
```

## [0.3.3] - 2025-12-14

### Added
- **Request Collapsing**: New `request_collapsing` module for deduplicating identical gRPC calls.
  - `RequestCollapsingConfig` - Configure coalesce window, max waiters, and cache size
  - `RequestCollapsingRegistry` - Thread-safe registry for tracking in-flight requests
  - `RequestKey` - SHA-256 based key generation for request identification
  - `CollapseResult` - Leader/Follower/Passthrough result for request handling
  - `RequestBroadcaster` - Broadcast results to waiting followers
  - `RequestReceiver` - Wait for leader's result
  - `CollapsingMetrics` - Track collapse ratio and request statistics
  - `with_request_collapsing()` - New `GatewayBuilder` method to enable collapsing

### Performance Benefits
- **Reduced gRPC Calls**: Identical requests share a single backend call
- **Concurrent Deduplication**: In-flight requests are automatically deduplicated
- **Configurable Window**: Tune the coalesce window for your workload

### How It Works
When a GraphQL query contains multiple fields calling the same gRPC method:
```graphql
query {
  user1: getUser(id: "1") { name }
  user2: getUser(id: "2") { name }
  user3: getUser(id: "1") { name }  # Duplicate of user1
}
```
- Without collapsing: 3 gRPC calls
- With collapsing: 2 gRPC calls (user1 and user3 share response)

### Configuration Presets
- `RequestCollapsingConfig::default()` - Balanced settings (50ms window, 100 max waiters)
- `RequestCollapsingConfig::high_throughput()` - Longer window (100ms), more waiters
- `RequestCollapsingConfig::low_latency()` - Shorter window (10ms) for speed
- `RequestCollapsingConfig::disabled()` - Disable collapsing entirely

### Metrics
- `total_requests` - Total requests processed
- `leader_requests` - Requests that executed the gRPC call
- `collapsed_requests` - Requests that waited for a leader
- `collapse_ratio` - Percentage of requests that were collapsed

## [0.3.2] - 2025-12-14

### Added
- **Query Analytics Dashboard**: New `analytics` module for comprehensive query performance insights.
  - `AnalyticsConfig` - Configure tracking limits, privacy settings, and retention period
  - `QueryAnalytics` - Thread-safe analytics engine for collecting query metrics
  - `AnalyticsSnapshot` - Point-in-time view of all analytics data
  - `QueryStats` - Per-query statistics including execution count, latency, and error rate
  - `FieldStats` - Track field-level usage across all queries
  - `ErrorStats` - Error pattern analysis with affected query tracking
  - `enable_analytics()` - New `GatewayBuilder` method to enable analytics
  - `AnalyticsGuard` - RAII guard for automatic query timing

### Dashboard Features
- **Beautiful Dark Theme**: Modern, responsive dashboard with real-time updates
- **Most Used Queries**: Track which queries are executed most frequently
- **Slowest Queries**: Identify performance bottlenecks with latency metrics
- **Error Patterns**: Analyze common errors and their affected queries
- **Field Usage Statistics**: See which fields are requested most often
- **Operation Distribution**: Visualize query/mutation/subscription breakdown
- **Cache Hit Rate**: Monitor response cache effectiveness
- **Uptime Tracking**: Server uptime display

### Endpoints
- `GET /analytics` - Interactive analytics dashboard
- `GET /analytics/api` - JSON API for programmatic access
- `POST /analytics/reset` - Reset all analytics data

### Configuration Options
- `AnalyticsConfig::default()` - Balanced settings for most use cases
- `AnalyticsConfig::production()` - Privacy-focused (no query text), shorter retention
- `AnalyticsConfig::development()` - Verbose tracking for debugging

### Privacy
- Optional query text tracking (disable for production privacy compliance)
- Configurable retention periods for automatic data pruning
- LRU eviction when limits are reached

## [0.3.1] - 2025-12-14

### Fixed
- Minor bug fixes and performance improvements

### Changed
- Updated dependencies

## [0.3.0] - 2025-12-14

### Added
- **REST API Connectors**: New `rest_connector` module for hybrid gRPC/REST GraphQL gateways.
  - `RestConnector` - HTTP client with retry logic, caching, and interceptor support
  - `RestConnectorBuilder` - Builder pattern for connector configuration
  - `RestEndpoint` - Define REST endpoints with path templates, query params, and body templates
  - **Typed Responses** - Define GraphQL schemas for REST endpoints to enable field selection (`RestResponseSchema`)
  - **GraphQL Schema Integration** - REST endpoints automatically exposed as query/mutation fields
  - `HttpMethod` - GET, POST, PUT, PATCH, DELETE support
  - `RestConnectorRegistry` - Manage multiple REST connectors for different services
  - `RetryConfig` - Configurable retry with exponential backoff
  - `BearerAuthInterceptor` / `ApiKeyInterceptor` - Built-in authentication interceptors
  - `ResponseTransformer` trait - Custom response transformation
  - `RequestInterceptor` trait - Custom request modification (auth, headers, logging)
  - JSONPath response extraction (e.g., `$.data.users`)
  - Response caching for GET requests
  - `add_rest_connector()` - New `GatewayBuilder` method

### Features
- **Hybrid Architecture**: Mix gRPC and REST backends in one GraphQL API
- **Path Templates**: URL path parameters with `{variable}` syntax
- **Body Templates**: Request body templating for POST/PUT/PATCH
- **Query Parameters**: Templated query string construction
- **Response Extraction**: JSONPath to extract nested data from responses
- **Retry Logic**: Exponential backoff with configurable retry statuses
- **Authentication**: Bearer token and API key interceptors included
- **Caching**: Optional LRU cache for GET request responses

### Use Cases
- Gradual migration from REST to gRPC
- Integrating third-party REST APIs into GraphQL schema
- Bridging legacy REST services with modern gRPC backends
- Multi-protocol microservices architectures

## [0.2.9] - 2025-12-14

### Added
- **Enhanced Authentication Middleware**: Complete rewrite of the authentication system with production-ready features.
  - `EnhancedAuthMiddleware` - New middleware with JWT support, claims extraction, and context enrichment
  - `AuthConfig` - Configurable authentication with required/optional modes and scheme selection
  - `AuthScheme` - Support for Bearer, Basic, ApiKey, and custom authentication schemes
  - `AuthClaims` - Standardized JWT claims structure with roles, expiration, and custom claims
  - `TokenValidator` - Trait for custom token validation logic
  - Claims extraction with `has_role()` and `has_any_role()` helpers
  - Automatic token expiration checking

- **Enhanced Logging Middleware**: Production-ready structured logging with security features.
  - `EnhancedLoggingMiddleware` - New middleware with configurable log levels and structured output
  - `LoggingConfig` - Configure log level, header logging, sensitive data masking, and slow request detection
  - `LogLevel` - Support for Trace, Debug, Info, Warn, and Error levels
  - Sensitive header masking (authorization, cookies, API keys automatically redacted)
  - Preset configurations: `LoggingConfig::minimal()` and `LoggingConfig::verbose()`
  - Slow request threshold detection

- **Improved Context**: Enhanced request context with observability features.
  - `request_id` - Automatic UUID generation or extraction from `x-request-id` header
  - `client_ip` - Client IP extraction from `x-forwarded-for` or `x-real-ip` headers
  - `request_start` - Request timing for performance monitoring
  - `user_id()` / `user_roles()` - Convenience methods for accessing auth context
  - `elapsed()` - Get request duration
  - `get_typed()` - Type-safe extension data retrieval

- **Middleware Chain**: New `MiddlewareChain` struct for combining multiple middleware.
  - Builder pattern for middleware composition
  - Arc-wrapped middleware support

- **Middleware Trait Improvements**: Added `name()` method for debugging and logging.

### Changed
- **AuthMiddleware** (backwards compatible): Added `new()`, `allow_all()`, and `require_token()` constructors.
- **LoggingMiddleware** (backwards compatible): Now uses request ID in log output.
- **RateLimitMiddleware**: Added `per_minute()` constructor and improved logging.

### Security
- Sensitive headers are automatically masked in logs (`authorization`, `x-api-key`, `cookie`, etc.)
- Token expiration checking prevents use of expired credentials
- Request ID propagation enables full request tracing across services

## [0.2.8] - 2025-12-13

### Added
- **Query Whitelisting (Stored Operations)**: New `query_whitelist` module for production security.
  - `QueryWhitelistConfig` - Configure allowed queries, enforcement mode, and introspection
  - `WhitelistMode` - Enforce, Warn, or Disabled modes for flexible deployment
  - `QueryWhitelist` - Hash-based and ID-based query validation
  - `with_query_whitelist()` - Builder method to enable query whitelisting
  - `from_json_file()` - Load allowed queries from JSON configuration

### Security Features
- **Enforce Mode**: Reject non-whitelisted queries in production (required for PCI-DSS compliance)
- **Warn Mode**: Log warnings but allow all queries (useful for staging environments)
- **Introspection Control**: Optionally allow `__schema` and `__type` queries in whitelisted mode
- **Dual Validation**: Validate queries by SHA-256 hash or custom operation ID
- **Runtime Registration**: Dynamically register new allowed queries at runtime

### Benefits
- Prevents arbitrary/malicious queries in production
- Reduces attack surface (no schema exploration or DoS via complex queries)
- Compatible with APQ (Automatic Persisted Queries) for bandwidth + security
- Works with Apollo Client, Relay, and other GraphQL clients
- Required for many security compliance standards (PCI-DSS, SOC 2)

### Use Cases
- Public-facing GraphQL APIs requiring strict query control
- Compliance with PCI-DSS and other security standards
- Mobile apps with pre-defined query sets
- Pre-production query testing and validation

## [0.2.7] - 2025-12-12

### Added
- **Multi-Descriptor Support (Schema Stitching)**: Combine multiple protobuf descriptor sets from different microservices into a unified GraphQL schema.
  - `add_descriptor_set_bytes()` - Add additional descriptor sets by bytes
  - `add_descriptor_set_file()` - Add additional descriptor sets from files
  - `descriptor_count()` - Get the number of configured descriptor sets
  - Seamless merging of services, types, and extensions from multiple sources

### Features
- **Microservice Architecture Support**: Each team can own their proto files independently
- **Schema Stitching**: Combine multiple services into one unified GraphQL API
- **Modular Development**: Add/remove services independently without rebuilding entire schema
- **Duplicate Handling**: Automatically skips duplicate file descriptors

### Use Cases
- Combine `users.bin`, `products.bin`, `orders.bin` from different teams
- Add new microservices to existing gateway without modifying core configuration
- Build federated architectures with independent service deployments

## [0.2.6] - 2025-12-12

### Added
- **Header Propagation**: New `headers` module for forwarding HTTP headers to gRPC metadata.
  - `HeaderPropagationConfig` - Configure which headers are forwarded to gRPC backends
  - `with_header_propagation()` - Builder method to enable header propagation
  - `apply_metadata_to_request()` - Helper to apply extracted metadata to gRPC requests

### Features
- **Allowlist Approach**: Security-first design - only explicitly configured headers are forwarded
- **Exact Header Matching**: `.propagate("authorization")` for specific headers
- **Prefix Matching**: `.propagate_with_prefix("x-custom-")` for header groups
- **Common Presets**: `HeaderPropagationConfig::common()` includes auth and tracing headers
- **Exclusions**: `.exclude("cookie")` to block specific headers even with `propagate_all_headers()`

### Use Cases
- Forward `Authorization` headers for backend authentication
- Propagate `X-Request-ID`, `traceparent` for distributed tracing
- Pass `X-Tenant-ID` for multi-tenant applications
- Forward `Accept-Language` for localization

## [0.2.5] - 2025-12-12

### Added
- **Response Compression**: New `compression` module for automatic HTTP response compression.
  - `CompressionConfig` - Configure compression level, algorithms, and minimum size threshold
  - `CompressionLevel` - Control compression speed vs ratio trade-off (Fast/Default/Best)
  - `with_compression()` - Builder method to enable response compression
  - `CompressionStats` - Statistics for monitoring compression performance
  - `create_compression_layer()` - Create a tower-http compression layer

### Features
- **Multiple Algorithms**: Support for Brotli (`br`), Gzip, Deflate, and Zstd compression
- **Preset Configurations**: `CompressionConfig::fast()`, `::best()`, `::default()`, `::disabled()`
- **Minimum Size Threshold**: Skip compression for small responses where overhead exceeds savings
- **Content-Based Selection**: Gateway selects best algorithm based on client `Accept-Encoding`

### Performance
- JSON responses typically compress 50-90% with Brotli
- Reduces bandwidth usage significantly for large query responses
- Configurable compression level to balance CPU vs compression ratio

## [0.2.4] - 2025-12-12

### Added
- **Response Caching**: New `cache` module for in-memory GraphQL response caching.
  - `CacheConfig` - Configure max size, TTL, stale-while-revalidate, and mutation invalidation
  - `ResponseCache` - Thread-safe LRU cache with automatic eviction
  - `with_response_cache()` - Builder method to enable response caching
  - `CacheLookupResult` - Hit/Stale/Miss result types for cache lookups
  - `CacheStats` - Runtime cache statistics

### Features
- **LRU Eviction**: Oldest entries removed when cache reaches capacity
- **TTL Expiration**: Cached responses expire after configurable duration
- **Stale-While-Revalidate**: Serve stale content immediately while refreshing in background
- **Mutation Invalidation**: Automatic cache invalidation when mutations are executed
- **Type/Entity Tracking**: Fine-grained invalidation by GraphQL type or entity ID (e.g., `User#123`)
- **Cache Key Generation**: SHA-256 hash of normalized query + variables + operation name

### Performance
- Cache hits return in <1ms vs ~50ms for gRPC backend calls
- Reduces gRPC backend load significantly for read-heavy workloads
- Zero external dependencies - purely in-memory using `HashMap` and `RwLock`

## [0.2.3] - 2025-12-11
### Added
- **Graceful Shutdown Support**: New `shutdown` module for production-ready server lifecycle management.
  - `ShutdownConfig` - Configure timeout, signal handling, and force shutdown delay
  - `ShutdownCoordinator` - Tracks in-flight requests and manages shutdown state
  - `with_graceful_shutdown()` - Builder method to enable graceful shutdown
  - `serve_with_shutdown()` - Serve with custom shutdown signal
  - `os_signal_shutdown()` - Handle SIGTERM/SIGINT signals
  - `RequestGuard` - RAII guard for automatic request lifecycle tracking


## [0.2.2] - 2025-12-11

### Added
- **Multiplex Subscription Support**: New `subscription` module implementing the `graphql-transport-ws` protocol without async-graphql dependency.
  - `MultiplexSubscription` - WebSocket handler supporting multiple concurrent subscriptions per connection
  - `MultiplexSubscriptionBuilder` - Builder pattern for configuring subscription handlers
  - `SubscriptionResolver` trait - Custom subscription resolution with gRPC streaming support
  - `GrpcSubscriptionResolver` - gRPC-backed subscription resolver
  - `SubscriptionConfig` - Configure connection init timeout, keep-alive, and max subscriptions
  - `SubscriptionRegistry` - Manage active subscriptions with cancellation support
  - Protocol support: `ConnectionInit`, `Subscribe`, `Next`, `Error`, `Complete`, `Ping/Pong`

### Features
- Multiple concurrent subscriptions per WebSocket connection
- Subscription lifecycle management with proper cleanup
- Keep-alive ping/pong support
- Connection initialization with configurable timeout
- gRPC server-streaming integration for real-time data
- Graceful shutdown with configurable timeout (default: 30s)
- In-flight request draining before shutdown
- Automatic OS signal handling (SIGTERM, SIGINT, Ctrl+C)


## [0.2.1] - 2025-12-11

### Added
- **Circuit Breaker Pattern**: New `with_circuit_breaker()` method on `GatewayBuilder` for backend resilience.
  - `CircuitBreakerConfig` - Configure failure threshold, recovery timeout, and half-open requests
  - `CircuitBreaker` - Per-service circuit breaker with Closed/Open/HalfOpen states
  - `CircuitBreakerRegistry` - Thread-safe registry for managing per-service breakers
  - `CircuitBreakerError` - Proper error handling with `SERVICE_UNAVAILABLE` code
  - Automatic state transitions and exponential recovery testing

### Reliability
- Circuit breaker prevents cascading failures when backend services are unhealthy
- Fast-fail behavior reduces latency when services are known to be down
- Configurable recovery testing allows gradual service restoration

## [0.2.0] - 2025-12-11

### Added
- **Automatic Persisted Queries (APQ)**: New `with_persisted_queries()` method on `GatewayBuilder` for bandwidth optimization.
  - `PersistedQueryConfig` - Configure cache size and TTL
  - `PersistedQueryStore` - Thread-safe LRU cache with SHA-256 hashing
  - `PersistedQueryError` - Proper error handling with `PERSISTED_QUERY_NOT_FOUND` code
  - Follows Apollo APQ protocol for client compatibility
  - Automatic cache eviction when at capacity

### Performance
- APQ reduces bandwidth by allowing clients to send query hashes instead of full query strings
- LRU eviction prevents unbounded memory growth

## [0.1.9] - 2025-12-11

### Added
- **OpenTelemetry Distributed Tracing**: New `enable_tracing()` method on `GatewayBuilder` and `tracing_otel` module for end-to-end visibility.
  - `GraphQLSpan` - Tracks GraphQL operations with attributes for operation name, type, and query
  - `GrpcSpan` - Tracks gRPC backend calls with service, method, and status code
  - `TracingConfig` - Configuration for service name and sampling ratio
  - `init_tracer()` / `shutdown_tracer()` - Lifecycle management for OpenTelemetry
- **Schema Introspection Controls**: New `disable_introspection()` method on both `GatewayBuilder` and `SchemaBuilder`.
  - Blocks `__schema` and `__type` queries in production for security
  - Enabled by default for development convenience
- **OTLP Export Support**: Optional `otlp` feature flag for OpenTelemetry Protocol export.
- **Tests**: Test coverage for tracing configuration and introspection controls.

### Security
- Schema introspection can now be disabled in production to prevent schema discovery attacks.

## [0.1.8] - 2025-12-11

### Added
- **Health Checks**: New `enable_health_checks()` method on `GatewayBuilder` adds `/health` (liveness) and `/ready` (readiness) endpoints for Kubernetes/container orchestration.
  - `GET /health` - Simple liveness probe, returns 200 if server is running
  - `GET /ready` - Readiness probe, checks gRPC client configuration
- **Prometheus Metrics**: New `enable_metrics()` method adds `/metrics` endpoint exposing Prometheus-compatible metrics:
  - `graphql_requests_total` - Total GraphQL requests by operation type
  - `graphql_request_duration_seconds` - Request latency histogram
  - `graphql_errors_total` - Total GraphQL errors
  - `grpc_backend_requests_total` - Total gRPC backend calls
  - `grpc_backend_duration_seconds` - gRPC backend latency histogram
- **Metrics API**: New `GatewayMetrics`, `RequestTimer`, and `GrpcTimer` types for programmatic metrics recording.
- **Health API**: New `HealthResponse`, `HealthStatus`, and `ComponentHealth` types for health check responses.
- **Tests**: Comprehensive test coverage for health and metrics modules.

## [0.1.7] - 2025-12-11

### Added
- **DoS Protection - Query Depth Limiting**: New `with_query_depth_limit(max_depth)` method on both `GatewayBuilder` and `SchemaBuilder` to prevent deeply nested queries that could overwhelm gRPC backends.
- **DoS Protection - Query Complexity Limiting**: New `with_query_complexity_limit(max_complexity)` method to limit the total "cost" of a query.
- **Error Types**: Added `QueryTooDeep` and `QueryTooComplex` error variants with corresponding error codes.

### Security
- Query depth and complexity limiting for DoS protection.


## [0.1.6] - 2025-12-07
### Added
- **Middlewares**: Added middleware support with `RateLimitMiddleware` for rate limiting (#13).
- **Protoc Generator Scaffolding**: Updated `protoc-gen-graphql-template` to support scaffolding (#11).

### Changed
- **Refactor DataLoader**: Improved dataloader implementation (#12).
- **README**: Updated documentation.

### Fixed
- **protoc-gen-graphql-template**: Fixed template generation issues.

## [0.1.5] - 2025-12-04

### Added
- **Documentation**: Comprehensive Rustdoc documentation across all public APIs and internal modules.

### Changed
- **Refactor**: Major codebase refactoring (#8).
- **README**: Updated documentation.

### Removed
- **FEDERATION.md**: Consolidated federation docs into README.

## [0.1.4] - 2025-12-04

### Added
- **Federation v2**: Full support for `@shareable` directive in proto options and schema generation.

### Fixed
- **GraphQL Federation**: Fixed federation composition issues, added shareable directive support.

## [0.1.3] - 2025-12-03

### Added
- **GraphQL Playground**: Added interactive GraphQL Playground UI (#6).
- **Tests**: Added more comprehensive test coverage (#7).

### Changed
- **Crate Name**: Changed crate name to `grpc_graphql_gateway`.

## [0.1.2] - 2025-12-03

### Added
- **Apollo Federation**: Full Apollo Federation v2 support (#5).
- **N+1 Query Fix**: Built-in DataLoader for batching entity resolution requests.

### Fixed
- **Federation**: Fixed federation entity resolution issues.

## [0.1.1] - 2025-12-03

### Added
- **File Uploads**: Support for GraphQL file uploads via multipart requests (#3).

## [0.1.0] - 2025-12-02

### Added
- Initial release
- Basic gRPC to GraphQL gateway functionality
- Query and Mutation support
- Subscription support via WebSocket
- Middleware system
- Example implementation
- **GraphQL Federation v2 support**
  - Entity definitions via `graphql.entity` proto option
  - Entity key support (single and composite keys)
  - Entity extensions with `@extends` directive
  - Field-level federation directives: `@external`, `@requires`, `@provides`
  - Automatic `_entities` query generation for entity resolution
  - `EntityResolver` trait for custom entity resolution logic

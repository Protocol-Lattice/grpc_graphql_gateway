# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

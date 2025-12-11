# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

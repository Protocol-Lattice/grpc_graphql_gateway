# Changelog

All notable changes to this project are documented here.

For the full changelog, see the [CHANGELOG.md](https://github.com/Protocol-Lattice/grpc_graphql_gateway/blob/main/CHANGELOG.md) file in the repository.


## Recent Releases

### [0.5.0] - 2025-12-18

**GBP Ultra: The "Speed of Light" Upgrade**

- **GraphQL Binary Protocol v8**: A complete reimagining of the binary layer.
  - **<1ms Latency**: Structural hashing eliminates allocation overhead.
  - **99.25% Compression**: Intelligent deduplication makes JSON payloads vanish.
  - **176+ MB/s**: Throughput that saturates 10Gbps links before maxing CPU.

### [0.4.9] - 2025-12-18 (Cumulative Release: 0.3.9 - 0.4.9)

**Enterprise Performance & Cost Optimization**

- **High-Performance Mode**: SIMD JSON parsing, lock-free sharded caching, and object pooling for 100K+ RPS per instance.
- **Cost Reduction Suite**: Advanced Query Cost Analysis and Smart TTL Management for significant resource savings.
- **GBP (GraphQL Binary Protocol) v8**: Achievement of **99.25% compression reduction** verified on 100MB+ "Behemoth" payloads.
- **Binary Interoperability**: New `GbpDecoder` and `gbp-lz4` encoding for high-speed server-to-server GraphQL communication.
- **Infrastructure Upgrades**: Migrated to latest stable versions of `axum`, `tonic`, and `prost`.

### [0.4.8] - 2025-12-18

**LZ4 + GBP Ultra Refinement**

- Refined the structural deduplication algorithm to achieve higher compression ratios.
- Optimized `GbpEncoder` and `GbpDecoder` for deeper recursion.

### [0.4.7] - 2025-12-18

**LZ4 Block Compression**

- Integrated `lz4` block compression for ultra-fast, low-CPU overhead data transfer.
- New `CompressionConfig::ultra_fast()` preset.

### [0.4.6] - 2025-12-18

**Maintenance Release**

- Internal buffer optimizations and dependency updates.

### [0.4.4] - 2025-12-17

**Security Maintenance**

- Critical dependency updates to address identified security vulnerabilities.
- Hardened gRPC metadata handling.

### [0.4.2] - 2025-12-17

**Vulnerability Patching**

- Fixed multiple security vulnerabilities identified in CI/CD pipeline.
- Improved error handling in `protoc-gen-graphql-template`.

### [0.4.0] - 2025-12-17

**High Performance Foundations**

- Initial support for SIMD-accelerated data processing and sharded caching.
- Enhanced `protoc` plugin capabilities.

### [0.3.9] - 2025-12-16

**Redis & Smart TTL**

- **Redis Backend**: Distributed caching support for horizontal scalability.
- **Smart TTL**: Initial foundation for mutation-aware cache invalidation.

### [0.3.8] - 2025-12-16

**Helm & Kubernetes Deployment**

- Production-ready Helm chart (`helm/grpc-graphql-gateway/`)
- Docker multi-stage builds for optimized images
- HPA (Horizontal Pod Autoscaler) support (5-50 pods)
- VPA (Vertical Pod Autoscaler) resource recommendations
- Federation deployment script (`deploy-federation.sh`)
- Docker Compose for local federation testing
- AWS/GCP LoadBalancer annotations support
- Comprehensive deployment guides (`DEPLOYMENT.md`, `ARCHITECTURE.md`)
- Fixed rustdoc intra-doc links for docs.rs compatibility

### [0.3.7] - 2025-12-16

**Production Security Hardening**

- Comprehensive security headers: HSTS, CSP, X-XSS-Protection, Referrer-Policy
- CORS preflight handling with proper OPTIONS response (204)
- Cache-Control headers to prevent sensitive data caching
- Query whitelist default to `Enforce` mode with introspection disabled
- Improved query normalization for robust hash matching
- Redis crate upgraded from 0.24 to 0.27
- 31-test security assessment script (`test_security.sh`)

### [0.3.6] - 2025-12-16

**Security Fixes**

- Replaced `std::sync::RwLock` with `parking_lot::RwLock` to prevent DoS via lock poisoning
- IP spoofing protection in middleware
- SSRF protection in REST connectors
- Security headers (X-Content-Type-Options, X-Frame-Options)

### [0.3.5] - 2025-12-16

**Redis Distributed Cache Backend**

- `CacheConfig.redis_url` - Configure Redis connection for distributed caching
- Dual backend support: in-memory (single instance) or Redis (distributed)
- Distributed cache invalidation across all gateway instances
- Redis SETs for type and entity indexes (`type:{TypeName}`, `entity:{EntityKey}`)
- TTL synchronization with Redis `SETEX`
- Automatic fallback to in-memory cache on connection failure
- Ideal for Kubernetes deployments and horizontal scaling

### [0.3.4] - 2025-12-14

**OpenAPI to REST Connector**

- `OpenApiParser` - Parse OpenAPI 3.0/3.1 and Swagger 2.0 specs
- Support for JSON and YAML formats
- Automatic endpoint generation from paths and operations
- Operation filtering by tags or custom predicates
- Base URL override for different environments

### [0.3.3] - 2025-12-14

**Request Collapsing**

- `RequestCollapsingConfig` - Configure coalesce window, max waiters, and cache size
- `RequestCollapsingRegistry` - Track in-flight requests for deduplication
- Reduces gRPC calls by sharing responses for identical concurrent requests
- Presets: `default()`, `high_throughput()`, `low_latency()`, `disabled()`
- Metrics tracking: collapse ratio, leader/follower counts

### [0.3.2] - 2025-12-14

**Query Analytics Dashboard**

- Beautiful dark-themed analytics dashboard at `/analytics`
- Most used queries, slowest queries, error patterns tracking
- Field usage statistics and operation distribution
- Cache hit rate monitoring and uptime tracking
- Privacy-focused production mode (no query text storage)
- JSON API at `/analytics/api`

### [0.3.1] - 2025-12-14

**Bug Fixes**

- Minor bug fixes and performance improvements
- Updated dependencies

### [0.3.0] - 2025-12-14

**REST API Connectors**

- `RestConnector` - HTTP client with retry logic, caching, and interceptor support
- `RestEndpoint` - Define REST endpoints with path templates and body templates
- Typed responses with `RestResponseSchema` for GraphQL field selection
- `add_rest_connector()` - New `GatewayBuilder` method
- Built-in interceptors: `BearerAuthInterceptor`, `ApiKeyInterceptor`
- JSONPath response extraction (e.g., `$.data.users`)
- Ideal for hybrid gRPC/REST architectures and gradual migrations

### [0.2.9] - 2025-12-14

**Enhanced Middleware & Auth System**

- `EnhancedAuthMiddleware` - JWT support with claims extraction and context enrichment
- `AuthConfig` - Required/optional modes with Bearer, Basic, ApiKey schemes
- `EnhancedLoggingMiddleware` - Structured logging with sensitive data masking
- `LoggingConfig` - Configurable log levels and slow request detection
- Improved context with `request_id`, `client_ip`, and auth helpers
- `MiddlewareChain` - Combine multiple middleware with builder pattern

### [0.2.8] - 2025-12-13

**Query Whitelisting (Stored Operations)**

- `QueryWhitelistConfig` - Configure allowed queries and enforcement mode
- `WhitelistMode` - Enforce, Warn, or Disabled modes
- Hash-based and ID-based query validation
- Production security for PCI-DSS compliance
- Compatible with APQ and GraphQL clients

### [0.2.7] - 2025-12-12

**Multi-Descriptor Support (Schema Stitching)**

- `add_descriptor_set_bytes()` - Add additional descriptor sets
- `add_descriptor_set_file()` - Add descriptors from files
- Seamless merging of services from multiple sources
- Essential for microservice architectures

### [0.2.6] - 2025-12-12

**Header Propagation**

- `HeaderPropagationConfig` - Configure header forwarding
- Allowlist approach for security
- Support for distributed tracing headers

### [0.2.5] - 2025-12-12

**Response Compression**

- Brotli, Gzip, Deflate, Zstd support
- Configurable compression levels
- Minimum size threshold

### [0.2.4] - 2025-12-12

**Response Caching**

- LRU cache with TTL expiration
- Stale-while-revalidate support
- Mutation-triggered invalidation

### [0.2.3] - 2025-12-11

**Graceful Shutdown**

- Clean server shutdown
- In-flight request draining
- OS signal handling

### [0.2.2] - 2025-12-11

**Multiplex Subscriptions**

- Multiple subscriptions per WebSocket
- `graphql-transport-ws` protocol support

### [0.2.1] - 2025-12-11

**Circuit Breaker**

- Per-service circuit breakers
- Automatic recovery testing
- Cascading failure prevention

### [0.2.0] - 2025-12-11

**Automatic Persisted Queries**

- SHA-256 query hashing
- LRU cache with optional TTL
- Apollo APQ protocol support

### [0.1.x] - Earlier Releases

See the [full changelog](https://github.com/Protocol-Lattice/grpc_graphql_gateway/blob/main/CHANGELOG.md) for earlier versions including:

- Health checks and Prometheus metrics
- OpenTelemetry tracing
- Query depth and complexity limiting
- Apollo Federation v2 support
- File uploads
- Middleware system

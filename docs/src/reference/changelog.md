# Changelog

All notable changes to this project are documented here.

For the full changelog, see the [CHANGELOG.md](https://github.com/Protocol-Lattice/grpc_graphql_gateway/blob/main/CHANGELOG.md) file in the repository.

## Recent Releases

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

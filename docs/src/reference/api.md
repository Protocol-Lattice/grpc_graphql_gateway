# API Documentation

The full Rust API documentation is available on docs.rs:

**[📚 docs.rs/grpc_graphql_gateway](https://docs.rs/grpc_graphql_gateway)**

## Main Types

### Gateway

The main entry point for creating and running the gateway.

```rust
use grpc_graphql_gateway::Gateway;

let gateway = Gateway::builder()
    // ... configuration
    .build()?;
```

### GatewayBuilder

Configuration builder with fluent API.

### GrpcClient

Manages connections to gRPC backend services.

```rust
use grpc_graphql_gateway::GrpcClient;

// Lazy connection (connects on first request)
let client = GrpcClient::builder("http://localhost:50051")
    .connect_lazy()?;

// Immediate connection
let client = GrpcClient::new("http://localhost:50051").await?;
```

### SchemaBuilder

Low-level builder for the dynamic GraphQL schema.

## Module Reference

| Module | Description |
|--------|-------------|
| `gateway` | Main Gateway and GatewayBuilder |
| `schema` | Schema generation from protobuf |
| `grpc_client` | gRPC client management |
| `federation` | Apollo Federation support |
| `middleware` | Request middleware |
| `cache` | Response caching |
| `compression` | Response compression |
| `circuit_breaker` | Circuit breaker pattern |
| `persisted_queries` | APQ support |
| `health` | Health check endpoints |
| `metrics` | Prometheus metrics |
| `tracing_otel` | OpenTelemetry tracing |
| `shutdown` | Graceful shutdown |
| `headers` | Header propagation |

## Re-exported Types

```rust
pub use gateway::{Gateway, GatewayBuilder};
pub use grpc_client::GrpcClient;
pub use schema::SchemaBuilder;
pub use cache::{CacheConfig, ResponseCache};
pub use compression::{CompressionConfig, CompressionLevel};
pub use circuit_breaker::{CircuitBreakerConfig, CircuitBreaker};
pub use persisted_queries::PersistedQueryConfig;
pub use shutdown::ShutdownConfig;
pub use headers::HeaderPropagationConfig;
pub use tracing_otel::TracingConfig;
pub use middleware::{Middleware, Context};
pub use federation::{EntityResolver, EntityResolverMapping, GrpcEntityResolver};
```

## Error Types

```rust
use grpc_graphql_gateway::{Error, Result};

// Main error type
enum Error {
    Schema(String),
    Io(std::io::Error),
    Grpc(tonic::Status),
    // ...
}
```

## Async Traits

When implementing custom resolvers or middleware, you'll use:

```rust
use async_trait::async_trait;

#[async_trait]
impl Middleware for MyMiddleware {
    async fn call(&self, ctx: &mut Context, next: ...) -> Result<()> {
        // ...
    }
}
```

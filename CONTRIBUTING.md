# Contributing to grpc-graphql-gateway-rs

Thank you for considering contributing to the gRPC-GraphQL Gateway! This document will help you get started quickly.

## Table of Contents

- [Getting Started](#getting-started)
- [Architecture Overview](#architecture-overview)
- [Development Workflow](#development-workflow)
- [Coding Guidelines](#coding-guidelines)
- [Testing](#testing)
- [Pull Request Process](#pull-request-process)

---

## Getting Started

### Prerequisites

- **Rust 1.70+** (stable toolchain)
- **protoc** (Protocol Buffers compiler)
- **cargo** (included with Rust)

### Setup

```bash
# Clone the repository
git clone https://github.com/Protocol-Lattice/grpc_graphql_gateway.git
cd grpc_graphql_gateway

# Build the project
cargo build

# Run tests
cargo test

# Run the greeter example
cargo run --bin greeter
```

---

## Architecture Overview

The gateway transforms gRPC services into a unified GraphQL API by dynamically generating schemas from protobuf descriptors.

### High-Level Flow

```
┌─────────────────────────────────────────────────────────────────────┐
│                          Client Request                              │
│                    (HTTP POST / WebSocket)                           │
└─────────────────────────────────────────────────────────────────────┘
                                  │
                                  ▼
┌─────────────────────────────────────────────────────────────────────┐
│                           ServeMux (runtime.rs)                      │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────────┐  │
│  │   Health    │  │   Metrics   │  │   APQ / Circuit Breaker     │  │
│  │  Endpoints  │  │  Endpoint   │  │   Processing                │  │
│  └─────────────┘  └─────────────┘  └─────────────────────────────┘  │
│                          │                                           │
│                          ▼                                           │
│              ┌─────────────────────┐                                 │
│              │    Middleware       │ (Auth, RateLimiting, etc.)      │
│              └─────────────────────┘                                 │
└─────────────────────────────────────────────────────────────────────┘
                                  │
                                  ▼
┌─────────────────────────────────────────────────────────────────────┐
│                     DynamicSchema (schema.rs)                        │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │   async-graphql execution engine                                │ │
│  │   - Query / Mutation / Subscription resolvers                  │ │
│  │   - Field resolvers with gRPC calls                            │ │
│  └────────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────────┘
                                  │
          ┌───────────────────────┼───────────────────────┐
          ▼                       ▼                       ▼
┌─────────────────┐   ┌─────────────────┐   ┌─────────────────────────┐
│  GrpcClientPool │   │  EntityResolver │   │   Federation Support    │
│ (grpc_client.rs)│   │ (federation.rs) │   │   (_entities, _service) │
└─────────────────┘   └─────────────────┘   └─────────────────────────┘
          │                       │
          ▼                       ▼
┌─────────────────────────────────────────────────────────────────────┐
│                         gRPC Backend Services                        │
└─────────────────────────────────────────────────────────────────────┘
```

### Core Modules

| Module | File | Responsibility |
|--------|------|----------------|
| **Gateway** | `gateway.rs` | Entry point & builder pattern. Orchestrates all components. |
| **Schema** | `schema.rs` | Builds `async-graphql` dynamic schema from protobuf descriptors. Handles type mapping, field resolution, and gRPC calls. |
| **Runtime** | `runtime.rs` | HTTP/WebSocket server via Axum. Routes requests, executes middleware, handles APQ and circuit breakers. |
| **gRPC Client** | `grpc_client.rs` | Connection pooling for gRPC backends using Tonic. |
| **Federation** | `federation.rs` | Apollo Federation v2 support. Entity resolution, `@key`, `@shareable` directives. |
| **DataLoader** | `dataloader.rs` | Batches entity resolution requests to prevent N+1 queries. |
| **Middleware** | `middleware.rs` | Extensible middleware system (auth, rate limiting, logging). |
| **Circuit Breaker** | `circuit_breaker.rs` | Resilience pattern for backend health management. |
| **Persisted Queries** | `persisted_queries.rs` | APQ implementation with LRU cache. |
| **Health** | `health.rs` | Kubernetes-compatible health check endpoints. |
| **Metrics** | `metrics.rs` | Prometheus metrics collection and exposure. |
| **Tracing** | `tracing_otel.rs` | OpenTelemetry distributed tracing. |
| **Error** | `error.rs` | Unified error types and handling. |

### Key Types

```rust
// Entry point - use the builder pattern
let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .add_grpc_client("service", client)
    .build()?;

// Core types
Gateway          // Main orchestrator
GatewayBuilder   // Configuration (clients, middleware, options)
ServeMux         // Request router, converted to Axum Router
DynamicSchema    // async-graphql schema wrapper
SchemaBuilder    // Low-level schema construction
GrpcClient       // Individual gRPC connection
GrpcClientPool   // Connection pool for multiple services
```

### Request Lifecycle

1. **HTTP/WebSocket Request** arrives at `ServeMux`
2. **APQ Processing** (if enabled) - lookup/cache query by hash
3. **Circuit Breaker Check** - fast-fail if backend is unhealthy
4. **Middleware Chain** - auth, rate limiting, logging
5. **Schema Execution** - `async-graphql` resolves the query
6. **Field Resolution** - each field maps to a gRPC call
7. **Response** - JSON GraphQL response returned

### Proto → GraphQL Mapping

The gateway reads protobuf descriptors and generates GraphQL schema:

```
proto/graphql.proto          →  Custom options (@graphql.schema, @graphql.entity)
proto/your_service.proto     →  Service definitions with options
       ↓
tonic-build (build.rs)       →  Generates descriptor.bin
       ↓
SchemaBuilder                →  Parses descriptors with prost-reflect
       ↓
DynamicSchema                →  async-graphql dynamic schema
```

---

## Development Workflow

### Directory Structure

```
grpc-graphql-gateway-rs/
├── src/
│   ├── lib.rs                 # Public API exports
│   ├── gateway.rs             # Gateway & GatewayBuilder
│   ├── schema.rs              # Schema generation (largest module)
│   ├── runtime.rs             # HTTP/WS server
│   ├── federation.rs          # Federation support
│   ├── dataloader.rs          # N+1 prevention
│   ├── grpc_client.rs         # gRPC connection pool
│   ├── middleware.rs          # Middleware system
│   ├── circuit_breaker.rs     # Resilience patterns
│   ├── persisted_queries.rs   # APQ
│   ├── health.rs              # Health endpoints
│   ├── metrics.rs             # Prometheus metrics
│   ├── tracing_otel.rs        # OpenTelemetry
│   ├── error.rs               # Error types
│   ├── types.rs               # Shared types
│   ├── bin/                   # CLI tools
│   │   └── protoc-gen-graphql-template.rs
│   └── generated/             # Generated proto code
├── proto/
│   └── graphql.proto          # Custom GraphQL options
├── examples/
│   ├── greeter/               # Basic example
│   ├── federation/            # Federation example
│   └── graphql/               # Generated gateway example
├── build.rs                   # Proto compilation
├── Cargo.toml
└── README.md
```

### Making Changes

1. **Schema Generation** (`schema.rs`) - Most complexity lives here. If you're changing how proto maps to GraphQL, start here.

2. **New Builder Options** - Add to `GatewayBuilder` in `gateway.rs`, wire it through to `ServeMux` or `SchemaBuilder`.

3. **New Middleware** - Implement the `Middleware` trait in `middleware.rs`.

4. **New Health/Metrics** - Add handlers in `health.rs`/`metrics.rs`, wire routing in `runtime.rs`.

5. **Federation Features** - Entity handling in `federation.rs`, batching in `dataloader.rs`.

---

## Coding Guidelines

### Style

- Follow Rust idioms and standard formatting (`cargo fmt`)
- Run `cargo clippy` and fix warnings
- Add rustdoc comments to public APIs
- Use `thiserror` for error types

### API Design

- Use the builder pattern for configuration
- Keep the public API minimal (`lib.rs` exports)
- Prefer `Arc<T>` for shared ownership across async boundaries
- Use `Result<T>` with our custom `Error` type

### Documentation

```rust
/// Short summary of what this does.
///
/// Longer description if needed, with context about when/why
/// you'd use this.
///
/// # Example
///
/// ```rust,no_run
/// // Runnable example
/// ```
pub fn my_function() { }
```

---

## Testing

### Running Tests

```bash
# Run all tests
cargo test

# Run with logging
RUST_LOG=debug cargo test

# Run specific test
cargo test test_federation_config

# Run tests for a specific module
cargo test --lib schema::tests
```

### Test Structure

- Unit tests live in the same file as the code (`#[cfg(test)] mod tests { ... }`)
- Integration tests use the `examples/` binaries
- Tests use pre-generated descriptors from `src/generated/`

### Writing Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const TEST_DESCRIPTOR: &[u8] = include_bytes!("generated/test_descriptor.bin");

    #[test]
    fn test_my_feature() {
        // Arrange
        let builder = SchemaBuilder::new()
            .with_descriptor_set_bytes(TEST_DESCRIPTOR);

        // Act
        let result = builder.build(&pool);

        // Assert
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_async_feature() {
        // Async test body
    }
}
```

---

## Pull Request Process

### Before Submitting

1. **Run the full test suite**: `cargo test`
2. **Format code**: `cargo fmt`
3. **Fix lints**: `cargo clippy`
4. **Update docs** if you changed public APIs
5. **Update CHANGELOG.md** with your changes

### PR Guidelines

- **Title**: Clear, concise description (e.g., "Add rate limiting middleware")
- **Description**: Explain what and why, not just what
- **Small PRs**: Easier to review, faster to merge
- **Tests**: Include tests for new functionality

### Commit Messages

```
feat: add circuit breaker for backend resilience

- Add CircuitBreakerConfig for configuration
- Implement per-service circuit breakers
- Add automatic recovery testing
```

Use conventional commits: `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`

---

## Questions?

- Open an issue for bugs or feature requests
- Check existing issues before creating new ones
- Be respectful and constructive

Thank you for contributing! 🚀

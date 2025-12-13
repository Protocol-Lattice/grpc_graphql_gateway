# gRPC-GraphQL Gateway

**A high-performance Rust gateway that bridges gRPC services to GraphQL with full Apollo Federation v2 support.**

[![Crates.io](https://img.shields.io/crates/v/grpc-graphql-gateway.svg)](https://crates.io/crates/grpc-graphql-gateway)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://github.com/Protocol-Lattice/grpc_graphql_gateway/blob/main/LICENSE)

Transform your gRPC microservices into a unified GraphQL API with zero GraphQL code. This gateway dynamically generates GraphQL schemas from protobuf descriptors and routes requests to your gRPC backends via Tonic, providing a seamless bridge between gRPC and GraphQL ecosystems.

## ✨ Features

### Core Capabilities
- 🚀 **Dynamic Schema Generation** - Automatic GraphQL schema from protobuf descriptors
- ⚡ **Full Operation Support** - Queries, Mutations, and Subscriptions
- 🔌 **WebSocket Subscriptions** - Real-time data via GraphQL subscriptions (`graphql-ws` protocol)
- 📤 **File Uploads** - Multipart form data support for file uploads
- 🎯 **Type Safety** - Leverages Rust's type system for robust schema generation

### Federation & Enterprise
- 🌐 **Apollo Federation v2** - Complete federation support with entity resolution
- 🔄 **Entity Resolution** - Production-ready resolver with DataLoader batching
- 🚫 **No N+1 Queries** - Built-in DataLoader prevents performance issues
- 🔗 **All Federation Directives** - `@key`, `@external`, `@requires`, `@provides`, `@shareable`
- 📊 **Batch Operations** - Efficient entity resolution with automatic batching

### Developer Experience
- 🛠️ **Code Generation** - `protoc-gen-graphql-template` generates starter gateway code
- 🔧 **Middleware Support** - Extensible middleware for auth, logging, and observability
- 📝 **Rich Examples** - Complete working examples for all features
- 🧪 **Well Tested** - Comprehensive test coverage

### Production Ready
- 🏥 **Health Checks** - `/health` and `/ready` endpoints for Kubernetes
- 📊 **Prometheus Metrics** - `/metrics` endpoint with request counts and latencies
- 🔭 **OpenTelemetry Tracing** - Distributed tracing with GraphQL and gRPC spans
- 🛡️ **DoS Protection** - Query depth and complexity limiting
- 🔒 **Introspection Control** - Disable schema introspection in production
- 🔐 **Query Whitelisting** - Restrict to pre-approved queries (PCI-DSS compliant)
- ⚡ **Rate Limiting** - Built-in rate limiting middleware
- 📦 **Automatic Persisted Queries** - Reduce bandwidth with query hash caching
- 🔌 **Circuit Breaker** - Prevent cascading failures
- 🗄️ **Response Caching** - In-memory LRU cache with TTL
- 📋 **Batch Queries** - Execute multiple operations in one request
- 🛑 **Graceful Shutdown** - Clean shutdown with request draining
- 🗜️ **Response Compression** - Automatic gzip/brotli compression
- 🔀 **Header Propagation** - Forward HTTP headers to gRPC backends
- 🧩 **Multi-Descriptor Support** - Combine multiple protobuf descriptors

## Why gRPC-GraphQL Gateway?

If you have existing gRPC microservices and want to expose them via GraphQL without writing GraphQL resolvers manually, this gateway is for you. It:

1. **Reads your protobuf definitions** - Including custom GraphQL annotations
2. **Generates a GraphQL schema automatically** - Types, queries, mutations, subscriptions
3. **Routes requests to your gRPC backends** - With full async/await support
4. **Supports federation** - Build a unified supergraph from multiple services

## Quick Example

```rust
use grpc_graphql_gateway::{Gateway, GrpcClient};

const DESCRIPTORS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/descriptor.bin"));

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let gateway = Gateway::builder()
        .with_descriptor_set_bytes(DESCRIPTORS)
        .add_grpc_client(
            "greeter.Greeter",
            GrpcClient::builder("http://127.0.0.1:50051").connect_lazy()?,
        )
        .build()?;

    gateway.serve("0.0.0.0:8888").await?;
    Ok(())
}
```

**That's it!** Your gateway is now running at:
- GraphQL HTTP: `http://localhost:8888/graphql`
- GraphQL WebSocket: `ws://localhost:8888/graphql/ws`

## Getting Started

Ready to dive in? Start with the [Installation](./getting-started/installation.md) guide.

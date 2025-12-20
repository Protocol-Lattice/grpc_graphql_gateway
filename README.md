# grpc_graphql_gateway

**High-performance Rust gateway bridging gRPC to GraphQL with Apollo Federation v2.**

[![Crates.io](https://img.shields.io/crates/v/grpc_graphql_gateway.svg)](https://crates.io/crates/grpc_graphql_gateway)
[![Docs](https://img.shields.io/badge/docs-online-blue.svg)](https://protocol-lattice.github.io/grpc_graphql_gateway)
[![MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

Transform gRPC microservices into a unified GraphQL API. Zero GraphQL code required.

## 🚀 Performance at a Glance

```
Performance Rankings (Rust):
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
1. grpc_graphql_gateway    ████████████████████ 112,000 req/s 🚀
2. async-graphql (Actix)   █████████░░░░░░░░░░░  45,000 req/s
3. Juniper (Actix)         ████████░░░░░░░░░░░░  39,000 req/s

Data Efficiency Mode (GBP Ultra + Parallel):
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
JSON Payload (1GB)             ████████████████████ 100%
GBP Ultra (9MB)                ▏                    0.9% ⚡ (1,749 MB/s)

Compression by Data Pattern:
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Highly Repetitive (orgs/perms) ▏                    1%  (99% reduction)
Moderately Repetitive (enums)  █████                25% (75% reduction)
Unique/Varied (logs)           ██████████           50% (50% reduction)
```

## ✨ Features

| Category | Capabilities |
|----------|-------------|
| **Core** | Schema generation, Queries/Mutations/Subscriptions, WebSocket, File uploads |
| **Live Queries** | `@live` directive, Real-time updates, Invalidation triggers, WebSocket push |
| **Federation** | Apollo Federation v2, Entity resolution, DataLoader batching, No N+1 |
| **Production** | Health checks, Prometheus, OpenTelemetry, Rate limiting, Circuit breaker |
| **Security** | Query depth/complexity limits, Introspection control, Query whitelisting |
| **Performance** | SIMD JSON, Sharded Cache, Response caching (Redis), APQ, Request collapsing |
| **Connectors** | REST APIs, OpenAPI integration, Multi-descriptor stitching |

## 📦 Quick Start

```toml
[dependencies]
grpc_graphql_gateway = "0.5.5"
tokio = { version = "1", features = ["full"] }
```

```rust
use grpc_graphql_gateway::{Gateway, GrpcClient};

const DESCRIPTORS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/descriptor.bin"));

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    Gateway::builder()
        .with_descriptor_set_bytes(DESCRIPTORS)
        .add_grpc_client("service", GrpcClient::builder("http://localhost:50051").connect_lazy()?)
        .build()?
        .serve("0.0.0.0:8888")
        .await
}
```

**Endpoints:** `http://localhost:8888/graphql` | `ws://localhost:8888/graphql/ws` | `ws://localhost:8888/graphql/live`

## 🌐 Federation

```rust
Gateway::builder()
    .enable_federation()
    .with_entity_resolver(Arc::new(resolver))
    .build()?;
```

## 🔧 Production Config

```rust
Gateway::builder()
    .enable_health_checks()           // /health, /ready
    .enable_metrics()                 // /metrics (Prometheus)
    .enable_tracing()                 // OpenTelemetry
    .with_query_depth_limit(10)       // DoS protection
    .with_query_complexity_limit(100)
    .with_response_cache(CacheConfig::default())
    .with_circuit_breaker(CircuitBreakerConfig::default())
    .build()?;
```

## ⚡ Live Queries

Real-time updates with the `@live` directive:

```graphql
# Connect to ws://localhost:8888/graphql/live
query @live {
  users {
    id
    name
    status
  }
}
```

Configure in your proto:

```protobuf
rpc GetUser(GetUserRequest) returns (User) {
  option (graphql.schema) = { type: QUERY, name: "user" };
  option (graphql.live_query) = {
    enabled: true
    strategy: INVALIDATION
    triggers: ["User.update", "User.delete"]
  };
}
```

Trigger updates from mutations:

```rust
// After mutation, invalidate affected queries
LIVE_QUERY_STORE.invalidate(InvalidationEvent::new("User", "update"));
```

## 📚 Examples

```bash
cargo run --example greeter      # Basic queries/mutations/subscriptions
cargo run --example federation   # 3 federated subgraphs
cargo run --example live_query   # Live queries with @live directive
```

## 📦 Client SDKs

Official high-performance decoders for GBP:

- **TypeScript/JavaScript**: [`@protocol-lattice/graphql-binary-protocol`](https://www.npmjs.com/package/@protocol-lattice/graphql-binary-protocol)

```bash
npm i @protocol-lattice/graphql-binary-protocol
```

```typescript
import { GbpDecoder } from '@protocol-lattice/graphql-binary-protocol';
const decoder = new GbpDecoder();
const decompressed = decoder.decodeLz4(new Uint8Array(data));
```

## 🔗 Links

[📖 Full Documentation](https://protocol-lattice.github.io/grpc_graphql_gateway) • [📦 Crates.io](https://crates.io/crates/grpc_graphql_gateway) • [💻 GitHub](https://github.com/Protocol-Lattice/grpc_graphql_gateway)

---
**Made with ❤️ by Protocol Lattice**

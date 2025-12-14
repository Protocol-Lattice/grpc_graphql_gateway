# grpc_graphql_gateway

**A high-performance Rust gateway that bridges gRPC services to GraphQL with full Apollo Federation v2 support.**

[![Crates.io](https://img.shields.io/crates/v/grpc_graphql_gateway.svg)](https://crates.io/crates/grpc_graphql_gateway)
[![Documentation](https://img.shields.io/badge/docs-online-blue.svg)](https://protocol-lattice.github.io/grpc_graphql_gateway)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

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
- 🏥 **Health Checks** - `/health` and `/ready` endpoints for Kubernetes liveness/readiness probes
- 📊 **Prometheus Metrics** - `/metrics` endpoint with request counts, latencies, and error rates
- 🔭 **OpenTelemetry Tracing** - Distributed tracing with GraphQL and gRPC span tracking
- 📈 **Query Analytics Dashboard** - Built-in dashboard for query insights, performance analysis, and error tracking
- 🛡️ **DoS Protection** - Query depth and complexity limiting to prevent expensive queries
- 🔒 **Introspection Control** - Disable schema introspection in production for security
- 🔐 **Query Whitelisting** - Restrict to pre-approved queries for maximum security (PCI-DSS compliant)
- ⚡ **Rate Limiting** - Built-in rate limiting middleware
- 📦 **Automatic Persisted Queries (APQ)** - Reduce bandwidth with query hash caching
- 🔌 **Circuit Breaker** - Prevent cascading failures with automatic backend health management
- 🗄️ **Response Caching** - In-memory LRU cache with TTL and mutation-triggered invalidation
- � **Request Collapsing** - Deduplicate identical in-flight gRPC calls for reduced backend load
- �📋 **Batch Queries** - Execute multiple GraphQL operations in a single HTTP request
- 🛑 **Graceful Shutdown** - Clean server shutdown with in-flight request draining
- 🗜️ **Response Compression** - Automatic gzip/brotli compression for reduced bandwidth
- 🔀 **Header Propagation** - Forward HTTP headers to gRPC backends for auth and tracing
- 🧩 **Multi-Descriptor Support** - Combine multiple protobuf descriptor sets for schema stitching
- 🌐 **REST API Connectors** - Hybrid gRPC/REST architectures with path templates, retry, and caching

## 🚀 Quick Start

### Installation

```toml
[dependencies]
grpc_graphql_gateway = "0.3.0"
tokio = { version = "1", features = ["full"] }
tonic = "0.12"
```

### Basic Gateway

```rust
use grpc_graphql_gateway::{Gateway, GrpcClient};

const DESCRIPTORS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/graphql_descriptor.bin"));

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

**Your gateway is now running!**
- GraphQL HTTP: `http://localhost:8888/graphql`
- GraphQL WebSocket: `ws://localhost:8888/graphql/ws`

### Generate Descriptors

Add to your `build.rs`:

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = std::env::var("OUT_DIR")?;
    
    tonic_build::configure()
        .build_server(false)
        .build_client(false)
        .file_descriptor_set_path(
            std::path::PathBuf::from(&out_dir).join("graphql_descriptor.bin")
        )
        .compile_protos(&["proto/your_service.proto"], &["proto"])?;
    
    Ok(())
}
```

## 📖 Usage Examples

### Queries, Mutations & Subscriptions

Annotate your proto file with GraphQL directives:

```protobuf
service UserService {
  option (graphql.service) = {
    host: "localhost:50051"
    insecure: true
  };

  // Query
  rpc GetUser(GetUserRequest) returns (User) {
    option (graphql.schema) = {
      type: QUERY
      name: "user"
    };
  }

  // Mutation
  rpc CreateUser(CreateUserRequest) returns (User) {
    option (graphql.schema) = {
      type: MUTATION
      name: "createUser"
      request { name: "input" }
    };
  }

  // Subscription (server streaming)
  rpc WatchUser(WatchUserRequest) returns (stream User) {
    option (graphql.schema) = {
      type: SUBSCRIPTION
      name: "userUpdates"
    };
  }
}
```

**GraphQL operations:**
```graphql
# Query
query {
  user(id: "123") {
    id
    name
    email
  }
}

# Mutation
mutation {
  createUser(input: { name: "Alice", email: "alice@example.com" }) {
    id
    name
  }
}

# Subscription
subscription {
  userUpdates(id: "123") {
    id
    name
    status
  }
}
```

### File Uploads

The gateway automatically supports GraphQL file uploads via multipart requests:

```protobuf
message UploadAvatarRequest {
  string user_id = 1;
  bytes avatar = 2;  // Maps to Upload scalar in GraphQL
}
```

```bash
curl http://localhost:8888/graphql \
  --form 'operations={"query": "mutation($file: Upload!) { uploadAvatar(input:{userId:\"123\", avatar:$file}) { userId size } }", "variables": {"file": null}}' \
  --form 'map={"0": ["variables.file"]}' \
  --form '0=@avatar.png'
```

### Field-Level Control

```protobuf
message User {
  string id = 1 [(graphql.field) = { required: true }];
  string email = 2 [(graphql.field) = { name: "emailAddress" }];
  string internal_id = 3 [(graphql.field) = { omit: true }];
  string password_hash = 4 [(graphql.field) = { omit: true }];
}
```

## 🌐 Apollo Federation v2

Build federated GraphQL architectures with multiple subgraphs.

### Defining Entities

```protobuf
message User {
  option (graphql.entity) = {
    keys: "id"
    resolvable: true
  };
  
  string id = 1 [(graphql.field) = { required: true }];
  string email = 2 [(graphql.field) = { shareable: true }];
  string name = 3 [(graphql.field) = { shareable: true }];
}

message Product {
  option (graphql.entity) = {
    keys: "upc"
    resolvable: true
  };
  
  string upc = 1 [(graphql.field) = { required: true }];
  string name = 2 [(graphql.field) = { shareable: true }];
  int32 price = 3 [(graphql.field) = { shareable: true }];
  User created_by = 4 [(graphql.field) = { 
    name: "createdBy"
    shareable: true 
  }];
}
```

### Entity Resolution with DataLoader

The gateway includes production-ready entity resolution with automatic batching:

```rust
use grpc_graphql_gateway::{
    Gateway, GrpcClient, EntityResolverMapping, GrpcEntityResolver
};

// Configure entity resolver with DataLoader batching
let resolver = GrpcEntityResolver::builder(client_pool)
    .register_entity_resolver(
        "User",
        EntityResolverMapping {
            service_name: "UserService".to_string(),
            method_name: "GetUser".to_string(),
            key_field: "id".to_string(),
        }
    )
    .build();

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .enable_federation()
    .with_entity_resolver(Arc::new(resolver))
    .add_grpc_client("UserService", user_client)
    .serve("0.0.0.0:8891")
    .await?;
```

**Benefits:**
- ✅ **No N+1 Queries** - DataLoader batches concurrent entity requests
- ✅ **Automatic Batching** - Multiple entities resolved in single operation
- ✅ **Production Ready** - Comprehensive error handling and logging

### Extending Entities

```protobuf
message UserReviews {
  option (graphql.entity) = {
    extend: true
    keys: "id"
  };
  
  string id = 1 [(graphql.field) = {
    external: true
    required: true
  }];
  
  repeated Review reviews = 2 [(graphql.field) = {
    requires: "id"
  }];
}
```

### Federation Directives

| Directive | Purpose | Example |
|-----------|---------|---------|
| `@key` | Define entity key fields | `keys: "id"` |
| `@shareable` | Field resolvable from multiple subgraphs | `shareable: true` |
| `@external` | Field defined in another subgraph | `external: true` |
| `@requires` | Fields needed from other subgraphs | `requires: "id email"` |
| `@provides` | Fields this resolver provides | `provides: "id name"` |

### Running with Apollo Router

```bash
# Start your federation subgraphs
cargo run --bin federation

# Compose the supergraph
./examples/federation/compose_supergraph.sh

# Run Apollo Router
router --supergraph examples/federation/supergraph.graphql --dev
```

**Query the federated graph:**
```graphql
query {
  product(upc: "123") {
    upc
    name
    price
    createdBy {
      id
      name
      email  # Resolved from User subgraph!
    }
  }
}
```

## 🔧 Advanced Features

### Middleware

```rust
use grpc_graphql_gateway::middleware::{Middleware, Context};

struct AuthMiddleware;

#[async_trait::async_trait]
impl Middleware for AuthMiddleware {
    async fn call(
        &self,
        ctx: &mut Context,
        next: Box<dyn Fn(&mut Context) -> BoxFuture<'_, Result<()>>>,
    ) -> Result<()> {
        // Validate auth token
        let token = ctx.headers().get("authorization")
            .ok_or_else(|| Error::Unauthorized)?;
        
        // Add user info to context
        ctx.extensions_mut().insert(UserInfo { /* ... */ });
        
        next(ctx).await
    }
}

let gateway = Gateway::builder()
    .add_middleware(AuthMiddleware)
    .build()?;
```

### Multi-Descriptor Support (Schema Stitching)

Combine multiple protobuf descriptor sets from different microservices into a unified GraphQL schema:

```rust
use grpc_graphql_gateway::{Gateway, GrpcClient};

// Load descriptor sets from different microservices
const USERS_DESCRIPTORS: &[u8] = include_bytes!("path/to/users.bin");
const PRODUCTS_DESCRIPTORS: &[u8] = include_bytes!("path/to/products.bin");
const ORDERS_DESCRIPTORS: &[u8] = include_bytes!("path/to/orders.bin");

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let gateway = Gateway::builder()
        // Primary descriptor set
        .with_descriptor_set_bytes(USERS_DESCRIPTORS)
        // Add additional services from other teams
        .add_descriptor_set_bytes(PRODUCTS_DESCRIPTORS)
        .add_descriptor_set_bytes(ORDERS_DESCRIPTORS)
        // Add clients for each service
        .add_grpc_client("users.UserService", GrpcClient::builder("http://users:50051").connect_lazy()?)
        .add_grpc_client("products.ProductService", GrpcClient::builder("http://products:50052").connect_lazy()?)
        .add_grpc_client("orders.OrderService", GrpcClient::builder("http://orders:50053").connect_lazy()?)
        .build()?;

    gateway.serve("0.0.0.0:8888").await?;
    Ok(())
}
```

**File-based loading:**

```rust
let gateway = Gateway::builder()
    .with_descriptor_set_file("path/to/users.bin")?
    .add_descriptor_set_file("path/to/products.bin")?
    .add_descriptor_set_file("path/to/orders.bin")?
    .build()?;
```

**Use Cases:**

| Scenario | Description |
|----------|-------------|
| Microservice Architecture | Each team owns their proto files independently |
| Schema Stitching | Combine multiple services into one unified GraphQL API |
| Modular Development | Add/remove services without rebuilding entire gateway |
| Independent Deployments | Update individual service descriptors without affecting others |

**Notes:**
- Duplicate file descriptors are automatically skipped
- Services, types, and extensions from all descriptor sets are merged
- GraphQL extensions (like `graphql.schema`, `graphql.field`) must be present in at least one descriptor set

### REST API Connectors

Build hybrid architectures that combine gRPC and REST backends in a single GraphQL API:

```rust
use grpc_graphql_gateway::{Gateway, RestConnector, RestEndpoint, HttpMethod, BearerAuthInterceptor};
use std::time::Duration;
use std::sync::Arc;

// Configure a REST connector for an external API
let users_api = RestConnector::builder()
    .base_url("https://api.example.com")
    .timeout(Duration::from_secs(30))
    .default_header("Accept", "application/json")
    .interceptor(Arc::new(BearerAuthInterceptor::new("your-token")))
    .add_endpoint(RestEndpoint::new("getUser", "/users/{id}")
        .method(HttpMethod::GET)
        .response_path("$.data.user")
        .description("Fetch a user by ID"))
    .add_endpoint(RestEndpoint::new("createUser", "/users")
        .method(HttpMethod::POST)
        .body_template(r#"{"name": "{name}", "email": "{email}"}"#))
    .with_cache(1000)  // Cache GET responses
    .build()?;

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    // Add gRPC clients
    .add_grpc_client("UserService", grpc_client)
    // Add REST connectors
    .add_rest_connector("users_api", users_api)
    .build()?;
```

**Features:**

| Feature | Description |
|---------|-------------|
| Path Templates | Use `{variable}` placeholders in URLs |
| Body Templates | Template request bodies for POST/PUT/PATCH |
| Response Extraction | JSONPath expressions to extract nested data |
| Retry Logic | Exponential backoff with configurable retry statuses |
| Authentication | Built-in Bearer and API Key interceptors |
| Caching | LRU cache for GET request responses |

**Use Cases:**

| Scenario | Description |
|----------|-------------|
| Hybrid Architecture | Mix gRPC and REST backends in one GraphQL API |
| Gradual Migration | Migrate from REST to gRPC incrementally |
| Third-Party APIs | Integrate external REST APIs (Stripe, Twilio, etc.) |
| Legacy Systems | Bridge legacy REST services with modern gRPC |

### DoS Protection (Query Limits)

Protect your gateway and gRPC backends from malicious or expensive queries:

```rust
let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .with_query_depth_limit(10)        // Max nesting depth
    .with_query_complexity_limit(100)   // Max query cost
    .add_grpc_client("service", client)
    .build()?;
```

**Query Depth Limiting** prevents deeply nested queries that could overwhelm your backends:

```graphql
# This would be blocked if depth exceeds limit
query {
  users {           # depth 1
    friends {       # depth 2
      friends {     # depth 3
        friends {   # depth 4 - blocked if limit < 4
          name
        }
      }
    }
  }
}
```

**Query Complexity Limiting** caps the total "cost" of a query (each field = 1 by default):

```graphql
# Complexity = 4 (users + friends + name + email)
query {
  users {
    friends {
      name
      email
    }
  }
}
```

**Recommended Values:**

| Use Case | Depth Limit | Complexity Limit |
|----------|-------------|------------------|
| Public API | 5-10 | 50-100 |
| Authenticated Users | 10-15 | 100-500 |
| Internal/Trusted | 15-25 | 500-1000 |

### Health Checks

Enable Kubernetes-compatible health check endpoints:

```rust
let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .enable_health_checks()  // Adds /health and /ready endpoints
    .add_grpc_client("service", client)
    .build()?;
```

**Endpoints:**

| Endpoint | Purpose | Response |
|----------|---------|----------|
| `GET /health` | Liveness probe | `200 OK` if server is running |
| `GET /ready` | Readiness probe | `200 OK` if gRPC clients are configured |

**Kubernetes Deployment:**

```yaml
livenessProbe:
  httpGet:
    path: /health
    port: 8888
  initialDelaySeconds: 5
  periodSeconds: 10

readinessProbe:
  httpGet:
    path: /ready
    port: 8888
  initialDelaySeconds: 5
  periodSeconds: 10
```

### Prometheus Metrics

Enable Prometheus-compatible metrics endpoint:

```rust
let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .enable_metrics()  // Adds /metrics endpoint
    .add_grpc_client("service", client)
    .build()?;
```

**Metrics Exposed:**

| Metric | Type | Description |
|--------|------|-------------|
| `graphql_requests_total` | Counter | Total requests by operation type |
| `graphql_request_duration_seconds` | Histogram | Request latency |
| `graphql_errors_total` | Counter | Errors by type |
| `grpc_backend_requests_total` | Counter | gRPC backend calls |
| `grpc_backend_duration_seconds` | Histogram | gRPC latency |

**Prometheus Scrape Config:**

```yaml
scrape_configs:
  - job_name: 'graphql-gateway'
    static_configs:
      - targets: ['gateway:8888']
    metrics_path: '/metrics'
```

### OpenTelemetry Tracing

Enable distributed tracing for end-to-end visibility:

```rust
use grpc_graphql_gateway::{Gateway, TracingConfig, init_tracer, shutdown_tracer};

// Initialize the tracer (do this once at startup)
let config = TracingConfig::new()
    .with_service_name("my-gateway")
    .with_sample_ratio(1.0);  // Sample all requests

let _provider = init_tracer(&config);

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .enable_tracing()
    .add_grpc_client("service", client)
    .build()?;

// ... run your server ...

// Shutdown on exit
shutdown_tracer();
```

**Spans Created:**

| Span | Kind | Attributes |
|------|------|------------|
| `graphql.query` | Server | `graphql.operation.name`, `graphql.document` |
| `graphql.mutation` | Server | `graphql.operation.name`, `graphql.document` |
| `grpc.call` | Client | `rpc.service`, `rpc.method`, `rpc.grpc.status_code` |

**OTLP Export (Optional):**

```toml
[dependencies]
grpc_graphql_gateway = { version = "0.1", features = ["otlp"] }
```

### Schema Introspection Control

Disable introspection in production for security:

```rust
let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .disable_introspection()  // Block __schema and __type queries
    .add_grpc_client("service", client)
    .build()?;
```

**Environment-Based Toggle:**

```rust
let is_production = std::env::var("ENV").map(|e| e == "production").unwrap_or(false);

let mut builder = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS);

if is_production {
    builder = builder.disable_introspection();
}

let gateway = builder.build()?;
```

### Query Analytics Dashboard

Track and analyze your GraphQL API performance with the built-in analytics dashboard:

```rust
use grpc_graphql_gateway::{Gateway, AnalyticsConfig};

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .enable_analytics(AnalyticsConfig::default())
    .add_grpc_client("service", client)
    .build()?;
```

**Endpoints:**

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/analytics` | GET | Interactive dashboard UI |
| `/analytics/api` | GET | JSON API for analytics data |
| `/analytics/reset` | POST | Reset all analytics data |

**Dashboard Features:**

- 📈 **Most Used Queries** - Track which queries are executed most frequently
- 🐢 **Slowest Queries** - Identify performance bottlenecks with latency metrics
- 🚨 **Error Patterns** - Analyze common errors and their affected queries
- 🎯 **Field Usage Statistics** - See which fields are requested most often
- 📊 **Operation Distribution** - Visualize query/mutation/subscription breakdown
- 💾 **Cache Hit Rate** - Monitor response cache effectiveness (if caching enabled)
- ⏱️ **Uptime Tracking** - Server uptime display

**Configuration Presets:**

```rust
// Balanced settings (default)
.enable_analytics(AnalyticsConfig::default())

// Privacy-focused for production (no query text stored)
.enable_analytics(AnalyticsConfig::production())

// Verbose for development (longer retention, more data)
.enable_analytics(AnalyticsConfig::development())
```

**Custom Configuration:**

```rust
use grpc_graphql_gateway::AnalyticsConfig;
use std::time::Duration;

let config = AnalyticsConfig::new()
    .max_queries(500)                           // Track up to 500 unique queries
    .slow_query_threshold(Duration::from_secs(1)) // Flag queries > 1s as slow
    .disable_query_text();                       // Don't store query text for privacy

let gateway = Gateway::builder()
    .enable_analytics(config)
    .build()?;
```

**JSON API Response:**

```json
{
  "timestamp": 1702576800,
  "total_requests": 15420,
  "total_errors": 23,
  "avg_latency_ms": 45.3,
  "requests_per_minute": 125.5,
  "error_rate": 0.15,
  "cache_hit_rate": 72.3,
  "uptime_seconds": 86400,
  "top_queries": [...],
  "slowest_queries": [...],
  "error_patterns": [...],
  "top_fields": [...],
  "requests_by_type": {"query": 14000, "mutation": 1400, "subscription": 20}
}
```

### Automatic Persisted Queries (APQ)

Reduce bandwidth by caching queries on the server and allowing clients to send query hashes:

```rust
use grpc_graphql_gateway::{Gateway, PersistedQueryConfig};
use std::time::Duration;

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .with_persisted_queries(PersistedQueryConfig {
        cache_size: 1000,                        // Max cached queries
        ttl: Some(Duration::from_secs(3600)),    // 1 hour expiration
    })
    .add_grpc_client("service", client)
    .build()?;
```

**How APQ Works:**

1. **First request**: Client sends hash only → Gateway returns `PERSISTED_QUERY_NOT_FOUND`
2. **Retry**: Client sends hash + full query → Gateway caches and executes
3. **Subsequent requests**: Client sends hash only → Gateway uses cached query

**Client Request Format:**

```json
{
  "extensions": {
    "persistedQuery": {
      "version": 1,
      "sha256Hash": "ecf4edb46db40b5132295c0291d62fb65d6759a9eedfa4d5d612dd5ec54a6b38"
    }
  }
}
```

**Benefits:**
- ✅ Reduces request payload size by ~90% for large queries
- ✅ Compatible with Apollo Client's APQ implementation
- ✅ LRU eviction prevents unbounded memory growth
- ✅ Optional TTL for cache expiration

### Query Whitelisting (Stored Operations)

Restrict which GraphQL queries can be executed for maximum security. This is a **critical feature** for public-facing GraphQL APIs and required for many compliance standards (PCI-DSS, SOC 2).

```rust
use grpc_graphql_gateway::{Gateway, QueryWhitelistConfig, WhitelistMode};
use std::collections::HashMap;

let mut allowed_queries = HashMap::new();
allowed_queries.insert(
    "getUserById".to_string(),
    "query getUserById($id: ID!) { user(id: $id) { id name email } }".to_string()
);
allowed_queries.insert(
   "listProducts".to_string(),
    "query { products { id name price } }".to_string()
);

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .with_query_whitelist(QueryWhitelistConfig {
        mode: WhitelistMode::Enforce,  // Reject non-whitelisted queries
        allowed_queries,
        allow_introspection: false,    // Block introspection in production
    })
    .add_grpc_client(\"service\", client)
    .build()?;
```

**Enforcement Modes:**

| Mode | Behavior | Use Case |
|------|----------|----------|
| `Enforce` | Reject non-whitelisted queries | Production |
| `Warn` | Log warnings, allow all queries | Staging/Testing |
| `Disabled` | No whitelist checking | Development |

**Loading from JSON File:**

```rust
// Load from allowed_queries.json
let config = QueryWhitelistConfig::from_json_file(
    "allowed_queries.json",
    WhitelistMode::Enforce
)?;

let gateway = Gateway::builder()
    .with_query_whitelist(config)
    .build()?;
```

**Example JSON file** (`allowed_queries.json`):

```json
{
  "getUserById": "query getUserById($id: ID!) { user(id: $id) { id name } }",
  "listProducts": "query { products { id name price } }",
  "createOrder": "mutation createOrder($input: OrderInput!) { createOrder(input: $input) { id } }"
}
```

**Client Integration:**

Clients can reference queries by ID using extensions:

```json
{
  "operationName": "getUserById",
  "variables": {"id": "123"},
  "extensions": {
    "operationId": "getUserById"
  }
}
```

**How Validation Works:**

1. **Operation ID**: If client provides `operationId` in extensions, validate by ID
2. **Query Hash**: Calculate SHA-256 hash of query and validate by hash
3. **Introspection**: Optionally allow `__schema` and `__type` queries

**Benefits:**

- ✅ **Security**: Prevents arbitrary/malicious queries
- ✅ **Compliance**: Required for PCI-DSS, HIPAA, SOC 2
- ✅ **Performance**: Known queries can be optimized
- ✅ **Monitoring**: Track exactly which queries are used
- ✅ **Mobile-Friendly**: Perfect for apps with fixed query sets

**Works with APQ:**

| Feature | Purpose | Security Level |
|---------|---------|----------------|
| **APQ** | Bandwidth optimization (caches any query) | Low |
| **Whitelist** | Security (only allows pre-approved queries) | High |
| **Both** | Bandwidth + Security | Maximum |

Use both together for maximum security and performance!

**Runtime Management:**

```rust
// Get whitelist reference
let whitelist = gateway.mux().query_whitelist().unwrap();

// Register new query at runtime
whitelist.register_query(
    "newQuery".to_string(),
    "query { newField }".to_string()
);

// Remove a query
whitelist.remove_query("oldQuery");

// Get statistics
let stats = whitelist.stats();
println!("Total allowed queries: {}", stats.total_queries);
```

### Circuit Breaker

Protect your gateway from cascading failures when backend services are unhealthy:

```rust
use grpc_graphql_gateway::{Gateway, CircuitBreakerConfig};
use std::time::Duration;

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .with_circuit_breaker(CircuitBreakerConfig {
        failure_threshold: 5,                      // Open after 5 failures
        recovery_timeout: Duration::from_secs(30), // Wait 30s before testing recovery
        half_open_max_requests: 3,                 // Allow 3 test requests
    })
    .add_grpc_client("service", client)
    .build()?;
```

**How It Works:**

1. **Closed**: Normal operation, all requests flow through
2. **Open**: After `failure_threshold` consecutive failures, circuit opens → requests fail fast
3. **Half-Open**: After `recovery_timeout`, limited test requests are allowed
4. **Recovery**: If test requests succeed, circuit closes; if they fail, it reopens

**Benefits:**
- ✅ Prevents cascading failures when backends are unhealthy
- ✅ Fast-fail reduces latency (no waiting for timeouts)
- ✅ Automatic recovery testing when services come back
- ✅ Per-service circuit breakers (one failing service doesn't affect others)

**Circuit States:**

| State | Description |
|-------|-------------|
| `Closed` | Normal operation |
| `Open` | Failing fast, returning `SERVICE_UNAVAILABLE` |
| `HalfOpen` | Testing if service recovered |

### Response Caching

Dramatically improve performance with in-memory response caching:

```rust
use grpc_graphql_gateway::{Gateway, CacheConfig};
use std::time::Duration;

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .with_response_cache(CacheConfig {
        max_size: 10_000,                              // Max 10k cached responses
        default_ttl: Duration::from_secs(60),          // 1 minute TTL
        stale_while_revalidate: Some(Duration::from_secs(30)), // Serve stale for 30s
        invalidate_on_mutation: true,                  // Auto-invalidate on mutations
    })
    .add_grpc_client("greeter.Greeter", client)
    .build()?;
```

**How It Works:**

1. **First Query**: Cache miss → Execute gRPC → Cache response → Return
2. **Second Query**: Cache hit → Return cached response immediately (<1ms)
3. **Mutation**: Execute mutation → Invalidate related cache entries
4. **Next Query**: Cache miss (invalidated) → Execute gRPC → Cache → Return

**Example with curl (greeter service):**

```bash
# Start the gateway
cargo run --example greeter

# 1. First query - cache miss, hits gRPC backend
curl -X POST http://localhost:8888/graphql \
  -H "Content-Type: application/json" \
  -d '{"query": "{ sayHello(name: \"Alice\") { message } }"}'
# Response: {"data":{"sayHello":{"message":"Hello Alice!"}}}
# Logs: "Response cache miss" or no cache log

# 2. Same query - cache hit, instant response
curl -X POST http://localhost:8888/graphql \
  -H "Content-Type: application/json" \
  -d '{"query": "{ sayHello(name: \"Alice\") { message } }"}'
# Response: {"data":{"sayHello":{"message":"Hello Alice!"}}}
# Logs: "Response cache hit"

# 3. Mutation - invalidates cache
curl -X POST http://localhost:8888/graphql \
  -H "Content-Type: application/json" \
  -d '{"query": "mutation { updateGreeting(name: \"Alice\", greeting: \"Hi\") { message } }"}'
# Response: {"data":{"updateGreeting":{"message":"Hi Alice!"}}}
# Logs: "Cache invalidated after mutation"

# 4. Query again - cache miss (was invalidated by mutation)
curl -X POST http://localhost:8888/graphql \
  -H "Content-Type: application/json" \
  -d '{"query": "{ sayHello(name: \"Alice\") { message } }"}'
# Response: {"data":{"sayHello":{"message":"Hi Alice!"}}}  <- Fresh data!
# Logs: "Response cache miss"
```

**What Gets Cached:**

| Operation | Cached? | Triggers Invalidation? |
|-----------|---------|------------------------|
| Query | ✅ Yes | No |
| Mutation | ❌ No | ✅ Yes |
| Subscription | ❌ No | No |

**Cache Invalidation Strategies:**

- **TTL-Based**: Entries expire after `default_ttl`
- **Mutation-Based**: Mutations automatically invalidate related cache entries
- **Type-Based**: Invalidate by GraphQL type (e.g., all `User` queries)
- **Entity-Based**: Invalidate by entity ID (e.g., `User#123`)

**Benefits:**
- ✅ Sub-millisecond response times for cached queries
- ✅ Reduced gRPC backend load (10-100x fewer calls)
- ✅ Automatic cache invalidation on mutations
- ✅ Stale-while-revalidate for best UX
- ✅ Zero external dependencies (pure in-memory)

### Request Collapsing

Reduce gRPC round-trips by deduplicating identical in-flight requests:

```rust
use grpc_graphql_gateway::{Gateway, RequestCollapsingConfig};
use std::time::Duration;

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .with_request_collapsing(RequestCollapsingConfig::default())
    .add_grpc_client("service", client)
    .build()?;
```

**How It Works:**

When a GraphQL query contains multiple fields calling the same gRPC method:

```graphql
query {
  user1: getUser(id: "1") { name }
  user2: getUser(id: "2") { name }
  user3: getUser(id: "1") { name }  # Duplicate of user1
}
```

- **Without collapsing**: 3 gRPC calls
- **With collapsing**: 2 gRPC calls (user1 and user3 share the same response)

The first request with a unique key becomes the "leader" and executes the gRPC call.
Subsequent identical requests become "followers" and wait for the leader's result.

**Configuration:**

```rust
use grpc_graphql_gateway::RequestCollapsingConfig;
use std::time::Duration;

let config = RequestCollapsingConfig::new()
    .coalesce_window(Duration::from_millis(100))  // Wait up to 100ms for in-flight
    .max_waiters(200)                             // Max 200 followers per leader
    .max_cache_size(20000);                       // Track up to 20k in-flight requests
```

**Preset Configurations:**

| Preset | Coalesce Window | Max Waiters | Use Case |
|--------|-----------------|-------------|----------|
| `default()` | 50ms | 100 | Balanced for most workloads |
| `high_throughput()` | 100ms | 500 | High-traffic scenarios |
| `low_latency()` | 10ms | 50 | Latency-sensitive apps |
| `disabled()` | - | - | Disable collapsing entirely |

**Benefits:**
- ✅ Reduces duplicate gRPC calls by 30-80% in high-traffic scenarios
- ✅ Works alongside response caching (collapsing for in-flight, caching for completed)
- ✅ Zero configuration for default behavior
- ✅ Built-in metrics for monitoring collapse ratio

**Metrics Available:**

```rust
let registry = mux.request_collapsing().unwrap();
let stats = registry.stats();
println!("In-flight requests: {}", stats.in_flight_count);
```

### Graceful Shutdown

Enable production-ready server lifecycle management with graceful shutdown:

```rust
use grpc_graphql_gateway::{Gateway, ShutdownConfig};
use std::time::Duration;

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .with_graceful_shutdown(ShutdownConfig {
        timeout: Duration::from_secs(30),          // Wait up to 30s for requests to complete
        handle_signals: true,                       // Handle SIGTERM/SIGINT automatically
        force_shutdown_delay: Duration::from_secs(5), // Wait 5s before forcing shutdown
    })
    .add_grpc_client("service", client)
    .serve("0.0.0.0:8888")
    .await?;
```

**How It Works:**

1. **Signal Received**: SIGTERM, SIGINT, or Ctrl+C is received
2. **Stop Accepting**: Server stops accepting new connections
3. **Drain Requests**: In-flight requests are allowed to complete (up to timeout)
4. **Cleanup**: Active subscriptions are cancelled, resources are released
5. **Exit**: Server shuts down gracefully

**Custom Shutdown Signal:**

```rust
use tokio::sync::oneshot;

let (tx, rx) = oneshot::channel::<()>();

// Trigger shutdown after some condition
tokio::spawn(async move {
    tokio::time::sleep(Duration::from_secs(60)).await;
    let _ = tx.send(());
});

Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .add_grpc_client("service", client)
    .serve_with_shutdown("0.0.0.0:8888", async { let _ = rx.await; })
    .await?;
```

**Benefits:**
- ✅ Clean shutdown with no dropped requests
- ✅ Automatic OS signal handling (SIGTERM, SIGINT, Ctrl+C)
- ✅ Configurable timeout for in-flight request draining
- ✅ Active subscription cleanup
- ✅ Kubernetes-compatible (responds to SIGTERM)

### Batch Queries

Execute multiple GraphQL operations in a single HTTP request for improved performance:

**Single Query (standard):**
```bash
curl -X POST http://localhost:8888/graphql \
  -H "Content-Type: application/json" \
  -d '{"query": "{ users { id name } }"}'
```

**Batch Queries (multiple operations):**
```bash
curl -X POST http://localhost:8888/graphql \
  -H "Content-Type: application/json" \
  -d '[
    {"query": "{ users { id name } }"},
    {"query": "{ products { upc price } }"},
    {"query": "mutation { createUser(input: {name: \"Alice\"}) { id } }"}
  ]'
```

**Batch Response:**
```json
[
  {"data": {"users": [{"id": "1", "name": "Bob"}]}},
  {"data": {"products": [{"upc": "123", "price": 99}]}},
  {"data": {"createUser": {"id": "2"}}}
]
```

**Benefits:**
- ✅ Reduces HTTP overhead for multiple operations
- ✅ Automatic support - no configuration required
- ✅ Compatible with Apollo Client batch link
- ✅ Each query in batch supports APQ and middleware
- ✅ Backward compatible - single queries work unchanged

**Use Cases:**
- Fetching data for multiple UI components in one request
- Executing related mutations together
- Reducing latency on mobile/slow networks

### Response Compression

Enable automatic HTTP response compression to reduce bandwidth usage:

```rust
use grpc_graphql_gateway::{Gateway, CompressionConfig, CompressionLevel};

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .with_compression(CompressionConfig {
        enabled: true,
        level: CompressionLevel::Default,
        min_size_bytes: 1024,  // Only compress responses > 1KB
        algorithms: vec!["br".into(), "gzip".into()],
    })
    .add_grpc_client("service", client)
    .build()?;
```

**Preset Configurations:**

```rust
// Fast compression for low latency
Gateway::builder()
    .with_compression(CompressionConfig::fast())

// Best compression for maximum bandwidth savings
Gateway::builder()
    .with_compression(CompressionConfig::best())

// Default balanced configuration
Gateway::builder()
    .with_compression(CompressionConfig::default())

// Disable compression
Gateway::builder()
    .with_compression(CompressionConfig::disabled())
```

**Supported Algorithms:**

| Algorithm | Header | Best For |
|-----------|--------|----------|
| Brotli | `br` | Modern browsers, best compression ratio |
| Gzip | `gzip` | Universal support, good compression |
| Deflate | `deflate` | Legacy compatibility |
| Zstd | `zstd` | Modern, fast compression |

**How It Works:**

1. Client sends request with `Accept-Encoding: gzip, deflate, br`
2. Gateway selects the best algorithm (preferring brotli > gzip > deflate)
3. Response body is compressed before sending
4. Gateway sets `Content-Encoding` header
5. Client decompresses automatically

**Testing with curl:**

```bash
# Request with compression support
curl -X POST http://localhost:8888/graphql \
  -H "Content-Type: application/json" \
  -H "Accept-Encoding: gzip, br" \
  --compressed \
  -d '{"query": "{ users { id name email } }"}'

# Check response headers (will show Content-Encoding: br or gzip)
curl -X POST http://localhost:8888/graphql \
  -H "Content-Type: application/json" \
  -H "Accept-Encoding: gzip, br" \
  -v -o /dev/null \
  -d '{"query": "{ users { id name email } }"}'
```

**Benefits:**
- ✅ 50-90% reduction in response size for JSON data
- ✅ Automatic algorithm selection based on client support
- ✅ Configurable minimum size threshold
- ✅ Works seamlessly with response caching
- ✅ Zero configuration for default behavior

### Header Propagation

Forward HTTP headers from GraphQL requests to gRPC backends as metadata. Essential for authentication, distributed tracing, and multi-tenant applications:

```rust
use grpc_graphql_gateway::{Gateway, HeaderPropagationConfig};

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .with_header_propagation(HeaderPropagationConfig::new()
        .propagate("authorization")       // Forward auth tokens
        .propagate("x-request-id")        // Correlation IDs
        .propagate("x-tenant-id")         // Multi-tenancy
        .propagate_with_prefix("x-custom-"))  // Custom headers by prefix
    .add_grpc_client("service", client)
    .build()?;
```

**Using Common Presets:**

```rust
// Common preset includes:
// - authorization
// - x-request-id, x-correlation-id
// - traceparent, tracestate (W3C Trace Context)
// - x-b3-* (Zipkin B3 propagation)
Gateway::builder()
    .with_header_propagation(HeaderPropagationConfig::common())
```

**Propagation Methods:**

| Method | Example | Use Case |
|--------|---------|----------|
| `.propagate("header")` | `.propagate("authorization")` | Specific header |
| `.propagate_with_prefix("x-")` | `.propagate_with_prefix("x-custom-")` | Header group |
| `.propagate_all_headers()` | `.propagate_all_headers().exclude("cookie")` | All (with exclusions) |

**Testing with curl:**

```bash
# Headers are forwarded to gRPC backends as metadata
curl -X POST http://localhost:8888/graphql \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer my-token" \
  -H "X-Request-ID: req-12345" \
  -H "X-Tenant-ID: tenant-abc" \
  -d '{"query": "{ users { id name } }"}'
```

**Security:**
- ✅ **Allowlist approach** - Only configured headers are forwarded
- ✅ **Explicit exclusions** - Block specific headers with `.exclude()`
- ✅ **Case-insensitive** - Headers normalized to lowercase for gRPC metadata

**Use Cases:**
- Forward `Authorization` for backend service authentication
- Propagate `X-Request-ID` / `traceparent` for distributed tracing
- Pass `X-Tenant-ID` for multi-tenant routing
- Forward `Accept-Language` for localization

### Custom Error Handling

```rust
let gateway = Gateway::builder()
    .with_error_handler(|error| {
        // Log errors, send to monitoring, etc.
        tracing::error!("GraphQL Error: {}", error);
        error
    })
    .build()?;
```

### Response Plucking

Extract nested fields as top-level responses:

```protobuf
message ListUsersResponse {
  repeated User users = 1;
  int32 total = 2;
}

rpc ListUsers(ListUsersRequest) returns (ListUsersResponse) {
  option (graphql.schema) = {
    type: QUERY
    name: "users"
    response {
      pluck: "users"  // Returns [User] instead of ListUsersResponse
    }
  };
}
```

## 📊 Type Mapping

| Protobuf | GraphQL |
|----------|---------|
| `string` | `String` |
| `bool` | `Boolean` |
| `int32`, `uint32` | `Int` |
| `int64`, `uint64` | `String` (avoids precision loss) |
| `float`, `double` | `Float` |
| `bytes` | `Upload` (input) / `String` (output, base64) |
| `repeated T` | `[T]` |
| `message` | `Object` / `InputObject` |
| `enum` | `Enum` |

## 🛠️ Code Generation

Generate a starter gateway:

```bash
# Install the generator
cargo install grpc_graphql_gateway --bin protoc-gen-graphql-template

# Generate gateway code
protoc \
  --plugin=protoc-gen-graphql-template=target/debug/protoc-gen-graphql-template \
  --graphql-template_out=. \
  --proto_path=proto \
  proto/federation_example.proto

# Run the generated gateway
cargo run --bin graphql
```

The generator creates:
- Complete gateway implementation
- Example queries/mutations/subscriptions
- Service configuration
- Ready-to-run code

## 📚 Examples

### Greeter Example

Basic query, mutation, subscription, and file upload:

```bash
cargo run --bin greeter
```

Open `http://localhost:8888/graphql` and try:
```graphql
query { hello(name: "World") { message } }
mutation { updateGreeting(input: {name: "GraphQL", salutation: "Hey"}) { message } }
subscription { streamHello(name: "Stream") { message } }
```

### Federation Example

Complete federated microservices with entity resolution:

```bash
cargo run --bin federation
```

Demonstrates:
- 3 federated subgraphs (User, Product, Review)
- Entity resolution with DataLoader batching
- Cross-subgraph queries
- `@shareable` fields
- Entity extensions

## 🎯 Best Practices

### Federation

1. **Define Clear Boundaries** - Each subgraph owns its entities
2. **Use @shareable Wisely** - Mark fields resolved by multiple subgraphs
3. **Leverage DataLoader** - Prevent N+1 queries with batch resolution
4. **Composite Keys** - Use when entities need multiple identifiers
5. **Minimize @requires** - Only specify truly required fields

### Performance

1. **Enable Connection Pooling** - Reuse gRPC connections
2. **Use Lazy Connections** - Connect on first use
3. **Implement Caching** - Cache frequently accessed entities
4. **Batch Operations** - Use DataLoader for entity resolution
5. **Monitor Metrics** - Track query performance and batch sizes

### Security

1. **Validate Inputs** - Use field-level validation
2. **Omit Sensitive Fields** - Use `omit: true` for internal data
3. **Implement Auth Middleware** - Centralize authentication
4. **Rate Limiting** - Protect against abuse
5. **TLS/SSL** - Secure gRPC connections in production

## 🧪 Testing

```bash
# Run all tests
cargo test

# Run with logging
RUST_LOG=debug cargo test

# Run specific test
cargo test test_federation_config
```

## 📦 Project Structure

```
grpc_graphql_gateway-rs/
├── src/
│   ├── lib.rs              # Public API
│   ├── gateway.rs          # Gateway implementation
│   ├── schema.rs           # Schema builder
│   ├── federation.rs       # Federation support
│   ├── dataloader.rs       # DataLoader for batching
│   ├── grpc_client.rs      # gRPC client management
│   ├── middleware.rs       # Middleware system
│   └── runtime.rs          # HTTP/WebSocket server
├── proto/
│   ├── graphql.proto       # GraphQL annotations
│   └── *.proto            # Your service definitions
├── examples/
│   ├── greeter/           # Basic example
│   └── federation/        # Federation example
└── tests/                 # Integration tests
```

## 🤝 Contributing

Contributions are welcome! Please feel free to submit a Pull Request. For major changes, please open an issue first to discuss what you would like to change.

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## 🙏 Acknowledgments

- Inspired by [grpc_graphql_gateway](https://github.com/ysugimoto/grpc_graphql_gateway) (Go)
- Built with [async-graphql](https://github.com/async-graphql/async-graphql)
- Powered by [tonic](https://github.com/hyperium/tonic)
- Federation based on [Apollo Federation v2](https://www.apollographql.com/docs/federation/)

## 🔗 Links

- [Documentation](https://docs.rs/grpc_graphql_gateway)
- [Crates.io](https://crates.io/crates/grpc_graphql_gateway)
- [Repository](https://github.com/Protocol-Lattice/grpc_graphql_gateway)
- [Examples](https://github.com/Protocol-Lattice/grpc_graphql_gateway/tree/main/examples)

---

**Made with ❤️ by Protocol Lattice**

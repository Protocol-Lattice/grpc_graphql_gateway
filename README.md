# grpc-graphql-gateway

**A high-performance Rust gateway that bridges gRPC services to GraphQL with full Apollo Federation v2 support.**

[![Crates.io](https://img.shields.io/crates/v/grpc-graphql-gateway.svg)](https://crates.io/crates/grpc-graphql-gateway)
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

## 🚀 Quick Start

### Installation

```toml
[dependencies]
grpc-graphql-gateway = "0.1"
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
cargo install grpc-graphql-gateway --bin protoc-gen-graphql-template

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
grpc-graphql-gateway-rs/
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

- Inspired by [grpc-graphql-gateway](https://github.com/ysugimoto/grpc-graphql-gateway) (Go)
- Built with [async-graphql](https://github.com/async-graphql/async-graphql)
- Powered by [tonic](https://github.com/hyperium/tonic)
- Federation based on [Apollo Federation v2](https://www.apollographql.com/docs/federation/)

## 🔗 Links

- [Documentation](https://docs.rs/grpc-graphql-gateway)
- [Crates.io](https://crates.io/crates/grpc-graphql-gateway)
- [Repository](https://github.com/Protocol-Lattice/grpc_graphql_gateway)
- [Examples](https://github.com/Protocol-Lattice/grpc_graphql_gateway/tree/main/examples)
- [Federation Guide](FEDERATION.md)
- [Entity Resolution Guide](ENTITY_RESOLUTION.md)

---

**Made with ❤️ by Protocol Lattice**

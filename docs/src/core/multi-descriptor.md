# Multi-Descriptor Support

Combine multiple protobuf descriptor sets from different microservices into a unified GraphQL schema. This is essential for large microservice architectures where each team owns their proto files.

## Overview

Instead of maintaining a single monolithic proto file, you can:

1. Let each team generate their own descriptor file
2. Combine them at gateway startup
3. Serve a unified GraphQL API

## Basic Usage

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
        .add_grpc_client("users.UserService", 
            GrpcClient::builder("http://users:50051").connect_lazy()?)
        .add_grpc_client("products.ProductService", 
            GrpcClient::builder("http://products:50052").connect_lazy()?)
        .add_grpc_client("orders.OrderService", 
            GrpcClient::builder("http://orders:50053").connect_lazy()?)
        .build()?;

    gateway.serve("0.0.0.0:8888").await?;
    Ok(())
}
```

## File-Based Loading

Load descriptors from files instead of embedding:

```rust
let gateway = Gateway::builder()
    .with_descriptor_set_file("path/to/users.bin")?
    .add_descriptor_set_file("path/to/products.bin")?
    .add_descriptor_set_file("path/to/orders.bin")?
    .build()?;
```

## API Methods

| Method | Description |
|--------|-------------|
| `with_descriptor_set_bytes(bytes)` | Set primary descriptor (clears existing) |
| `add_descriptor_set_bytes(bytes)` | Add additional descriptor |
| `with_descriptor_set_file(path)` | Set primary descriptor from file |
| `add_descriptor_set_file(path)` | Add additional descriptor from file |
| `descriptor_count()` | Get number of configured descriptors |

## Use Cases

### Microservice Architecture

Each team generates their own descriptor:

```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│   Users Team    │     │ Products Team   │     │  Orders Team    │
│                 │     │                 │     │                 │
│  users.proto    │     │ products.proto  │     │  orders.proto   │
│       ↓         │     │       ↓         │     │       ↓         │
│   users.bin     │     │  products.bin   │     │   orders.bin    │
└────────┬────────┘     └────────┬────────┘     └────────┬────────┘
         │                       │                       │
         └───────────────────────┼───────────────────────┘
                                 │
                    ┌────────────▼────────────┐
                    │    GraphQL Gateway      │
                    │                         │
                    │  Unified GraphQL Schema │
                    └─────────────────────────┘
```

### Schema Stitching

Combine services at the gateway level:

```graphql
# From users.bin
type Query {
  user(id: ID!): User
}

# From products.bin  
type Query {
  product(upc: String!): Product
}

# From orders.bin
type Query {
  order(id: ID!): Order
}

# Unified Schema (automatic)
type Query {
  user(id: ID!): User
  product(upc: String!): Product
  order(id: ID!): Order
}
```

### Independent Deployments

Update individual service descriptors without restarting:

```rust
// Hot-reload could be implemented by watching descriptor files
let gateway = Gateway::builder()
    .with_descriptor_set_file("/config/users.bin")?
    .add_descriptor_set_file("/config/products.bin")?
    .build()?;
```

## How It Works

1. **Primary descriptor** is loaded with `with_descriptor_set_bytes/file`
2. **Additional descriptors** are merged using `add_descriptor_set_bytes/file`
3. **Duplicate files** are automatically skipped (same filename)
4. **Services and types** from all descriptors are combined
5. **GraphQL schema** is generated from the merged pool

## Requirements

- All descriptors must include `graphql.proto` with annotations
- Service names should be unique across descriptors
- Type names are namespaced by their proto package

## Logging

The gateway logs merge information:

```
INFO Merged 3 descriptor sets into unified schema (5 services, 42 types)
DEBUG Merged descriptor set #2 (15234 bytes) into schema pool
DEBUG Merged descriptor set #3 (8921 bytes) into schema pool
```

## Error Handling

Common errors and solutions:

| Error | Cause | Solution |
|-------|-------|----------|
| `at least one descriptor set is required` | No descriptors provided | Add at least one with `with_descriptor_set_bytes` |
| `failed to merge descriptor set #N` | Invalid protobuf data | Verify the descriptor file is valid |
| `missing graphql.schema extension` | Annotations not found | Ensure `graphql.proto` is included |

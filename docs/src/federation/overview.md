# Apollo Federation Overview

Build federated GraphQL architectures with multiple subgraphs. The gateway supports Apollo Federation v2, allowing you to compose a supergraph from multiple gRPC services.

## What is Federation?

Apollo Federation is an architecture for building a distributed GraphQL API. Instead of a monolithic schema, you have:

- **Subgraphs**: Individual GraphQL services that own part of the schema
- **Supergraph**: The composed schema combining all subgraphs
- **Router**: Distributes queries to appropriate subgraphs

## Gateway as Subgraph

The gRPC-GraphQL Gateway can act as a federation subgraph:

```
┌─────────────────────────────────────────────────┐
│              Apollo Router / Gateway            │
│               (Supergraph Router)               │
└─────────────────┬─────────────────┬─────────────┘
                  │                 │
     ┌────────────▼──────┐  ┌───────▼────────────┐
     │  gRPC-GraphQL     │  │  Traditional       │
     │  Gateway          │  │  GraphQL Service   │
     │  (Subgraph)       │  │  (Subgraph)        │
     └────────────┬──────┘  └────────────────────┘
                  │
     ┌────────────▼──────────────────┐
     │         gRPC Services         │
     │   Users │ Products │ Orders   │
     └───────────────────────────────┘
```

## Enabling Federation

```rust
let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .enable_federation()  // Enable federation features
    .add_grpc_client("users.UserService", user_client)
    .build()?;
```

## Federation Features

When federation is enabled, the gateway:

1. **Adds `_service` query** - Returns the SDL for schema composition
2. **Adds `_entities` query** - Resolves entity references from other subgraphs
3. **Applies directives** - `@key`, `@shareable`, `@external`, etc.

## Schema Composition

Your proto files define entities with keys:

```protobuf
message User {
  option (graphql.entity) = {
    keys: "id"
    resolvable: true
  };
  
  string id = 1 [(graphql.field) = { required: true }];
  string name = 2;
  string email = 3;
}
```

This generates:

```graphql
type User @key(fields: "id") {
  id: ID!
  name: String
  email: String
}
```

## Running with Apollo Router

1. Start your federation subgraphs
2. Compose the supergraph schema
3. Run Apollo Router

See [Running with Apollo Router](./apollo-router.md) for detailed instructions.

## Next Steps

- [Defining Entities](./entities.md) - Mark types as federation entities
- [Entity Resolution](./entity-resolution.md) - Resolve entity references
- [Federation Directives](./directives.md) - Use `@shareable`, `@external`, etc.

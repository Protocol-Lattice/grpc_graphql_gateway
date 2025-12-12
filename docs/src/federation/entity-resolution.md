# Entity Resolution

When Apollo Router receives a query that spans multiple subgraphs, it needs to resolve entity references. The gateway includes production-ready entity resolution with DataLoader batching.

## How Entity Resolution Works

1. Router sends `_entities` query with representations
2. Gateway receives representations (e.g., `{ __typename: "User", id: "123" }`)
3. Gateway calls your gRPC backend to resolve the entity
4. Gateway returns the resolved entity data

## Configuring Entity Resolution

```rust
use grpc_graphql_gateway::{
    Gateway, GrpcClient, EntityResolverMapping, GrpcEntityResolver
};
use std::sync::Arc;

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
    .register_entity_resolver(
        "Product",
        EntityResolverMapping {
            service_name: "ProductService".to_string(),
            method_name: "GetProduct".to_string(),
            key_field: "upc".to_string(),
        }
    )
    .build();

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .enable_federation()
    .with_entity_resolver(Arc::new(resolver))
    .add_grpc_client("UserService", user_client)
    .add_grpc_client("ProductService", product_client)
    .build()?;
```

## DataLoader Batching

The built-in `GrpcEntityResolver` uses DataLoader to batch entity requests:

```
Query requests:
  - User(id: "1")
  - User(id: "2")  
  - User(id: "3")

Without DataLoader: 3 gRPC calls
With DataLoader: 1 batched gRPC call
```

### Benefits

- ✅ **No N+1 Queries** - Concurrent requests are batched
- ✅ **Automatic Coalescing** - Duplicate keys are deduplicated
- ✅ **Per-Request Caching** - Same entity isn't fetched twice per request

## Custom Entity Resolver

Implement the `EntityResolver` trait for custom logic:

```rust
use grpc_graphql_gateway::federation::{EntityConfig, EntityResolver};
use async_graphql::{Value, indexmap::IndexMap, Name};
use async_trait::async_trait;

struct MyEntityResolver {
    // Your dependencies
}

#[async_trait]
impl EntityResolver for MyEntityResolver {
    async fn resolve_entity(
        &self,
        config: &EntityConfig,
        representation: &IndexMap<Name, Value>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let typename = &config.type_name;
        
        match typename.as_str() {
            "User" => {
                let id = representation.get(&Name::new("id"))
                    .and_then(|v| v.as_str())
                    .ok_or("missing id")?;
                
                // Fetch from your backend
                let user = self.fetch_user(id).await?;
                
                Ok(Value::Object(indexmap! {
                    Name::new("id") => Value::String(user.id),
                    Name::new("name") => Value::String(user.name),
                    Name::new("email") => Value::String(user.email),
                }))
            }
            _ => Err(format!("Unknown entity type: {}", typename).into()),
        }
    }
}
```

## EntityResolverMapping

Configure how each entity type maps to a gRPC method:

| Field | Description |
|-------|-------------|
| `service_name` | The gRPC service name |
| `method_name` | The RPC method to call |
| `key_field` | The field in the request message that holds the key |

## Query Example

When Router sends:

```graphql
query {
  _entities(representations: [
    { __typename: "User", id: "123" }
    { __typename: "User", id: "456" }
  ]) {
    ... on User {
      id
      name
      email
    }
  }
}
```

The gateway:

1. Extracts the representations
2. Groups by `__typename`
3. Batches calls to the appropriate gRPC services
4. Returns resolved entities

## Error Handling

Entity resolution errors are returned per-entity:

```json
{
  "data": {
    "_entities": [
      { "id": "123", "name": "Alice", "email": "alice@example.com" },
      null
    ]
  },
  "errors": [
    {
      "message": "User not found: 456",
      "path": ["_entities", 1]
    }
  ]
}
```

## Performance Tips

1. **Use DataLoader** - Always batch entity requests
2. **Implement bulk fetch** - Have gRPC methods that fetch multiple entities
3. **Cache wisely** - Consider caching frequently accessed entities
4. **Monitor** - Track entity resolution latency with metrics

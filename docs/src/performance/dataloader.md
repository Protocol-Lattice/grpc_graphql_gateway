# DataLoader

The gateway includes a built-in **DataLoader** implementation for batching entity resolution requests. This is essential for preventing the N+1 query problem in federated GraphQL architectures.

## The N+1 Query Problem

Without DataLoader, resolving a list of entities results in one backend call per entity:

```
Query: users { friends { name } }

→ Fetch users (1 call)
→ For each user, fetch friends:
    - User 1's friends (call #2)
    - User 2's friends (call #3)
    - User 3's friends (call #4)
    ... (N more calls)
```

This is the **N+1 problem**: 1 initial query + N follow-up queries.

## How DataLoader Solves This

DataLoader collects all entity resolution requests within a single execution frame and batches them together:

```
Query: users { friends { name } }

→ Fetch users (1 call)
→ Collect all friend IDs
→ Batch fetch all friends (1 call)

Total: 2 calls instead of N+1
```

## EntityDataLoader

The `EntityDataLoader` is the main DataLoader implementation for entity resolution:

```rust
use grpc_graphql_gateway::{EntityDataLoader, EntityConfig};
use grpc_graphql_gateway::federation::EntityResolver;
use std::sync::Arc;
use std::collections::HashMap;

// Your entity resolver implementation
let resolver: Arc<dyn EntityResolver> = /* ... */;

// Entity configurations
let mut entity_configs: HashMap<String, EntityConfig> = HashMap::new();
entity_configs.insert("User".to_string(), user_config);
entity_configs.insert("Product".to_string(), product_config);

// Create the DataLoader
let loader = EntityDataLoader::new(resolver, entity_configs);
```

## API Reference

### `EntityDataLoader::new`

Creates a new DataLoader instance:

```rust
pub fn new(
    resolver: Arc<dyn EntityResolver>,
    entity_configs: HashMap<String, EntityConfig>,
) -> Self
```

- **resolver**: The underlying entity resolver that performs the actual resolution
- **entity_configs**: Map of entity type names to their configurations

### `EntityDataLoader::load`

Load a single entity with automatic batching:

```rust
pub async fn load(
    &self,
    entity_type: &str,
    representation: IndexMap<Name, Value>,
) -> Result<Value>
```

Multiple concurrent calls to `load()` for the same entity type are automatically batched together.

### `EntityDataLoader::load_many`

Load multiple entities in a batch:

```rust
pub async fn load_many(
    &self,
    entity_type: &str,
    representations: Vec<IndexMap<Name, Value>>,
) -> Result<Vec<Value>>
```

Explicitly batch multiple entity resolution requests.

## Integration with Federation

When using Apollo Federation, the DataLoader is typically integrated through the entity resolution pipeline:

```rust
use grpc_graphql_gateway::{
    Gateway, GrpcEntityResolver, EntityDataLoader, EntityConfig
};
use std::sync::Arc;
use std::collections::HashMap;

// 1. Create the base entity resolver
let base_resolver = Arc::new(GrpcEntityResolver::default());

// 2. Configure entity types
let mut entity_configs: HashMap<String, EntityConfig> = HashMap::new();
entity_configs.insert(
    "User".to_string(),
    EntityConfig {
        type_name: "User".to_string(),
        keys: vec![vec!["id".to_string()]],
        extend: false,
        resolvable: true,
        descriptor: user_descriptor,
    },
);

// 3. Wrap with DataLoader
let loader = Arc::new(EntityDataLoader::new(
    base_resolver.clone(),
    entity_configs.clone(),
));

// 4. Build gateway with entity resolution
let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .enable_federation()
    .with_entity_resolver(base_resolver)
    .add_grpc_client("UserService", user_client)
    .build()?;
```

## Custom Entity Resolver with DataLoader

You can wrap a custom entity resolver with DataLoader:

```rust
use grpc_graphql_gateway::EntityDataLoader;
use grpc_graphql_gateway::federation::{EntityConfig, EntityResolver};
use async_graphql::{Value, indexmap::IndexMap, Name};
use async_trait::async_trait;
use std::sync::Arc;

struct DataLoaderResolver {
    loader: Arc<EntityDataLoader>,
}

impl DataLoaderResolver {
    pub fn new(
        base_resolver: Arc<dyn EntityResolver>,
        entity_configs: HashMap<String, EntityConfig>,
    ) -> Self {
        let loader = Arc::new(EntityDataLoader::new(
            base_resolver,
            entity_configs,
        ));
        Self { loader }
    }
}

#[async_trait]
impl EntityResolver for DataLoaderResolver {
    async fn resolve_entity(
        &self,
        config: &EntityConfig,
        representation: &IndexMap<Name, Value>,
    ) -> Result<Value> {
        // Single entity resolution goes through DataLoader
        self.loader.load(&config.type_name, representation.clone()).await
    }

    async fn batch_resolve_entities(
        &self,
        config: &EntityConfig,
        representations: Vec<IndexMap<Name, Value>>,
    ) -> Result<Vec<Value>> {
        // Batch resolution via DataLoader
        self.loader.load_many(&config.type_name, representations).await
    }
}
```

## Key Features

### Automatic Batching

Concurrent entity requests are automatically batched:

```rust
// These concurrent requests are batched into a single backend call
let (user1, user2, user3) = tokio::join!(
    loader.load("User", user1_repr),
    loader.load("User", user2_repr),
    loader.load("User", user3_repr),
);
```

### Deduplication

Identical entity requests are deduplicated:

```rust
// Same user requested twice = only 1 backend call
let user1a = loader.load("User", user1_repr.clone());
let user1b = loader.load("User", user1_repr.clone());

let (result_a, result_b) = tokio::join!(user1a, user1b);
// result_a == result_b, and only 1 backend call was made
```

### Normalized Cache Keys

Entity representations are normalized before caching, so field order doesn't matter:

```rust
// These are treated as the same entity
let repr1 = indexmap! {
    Name::new("id") => Value::String("123".into()),
    Name::new("region") => Value::String("us".into()),
};

let repr2 = indexmap! {
    Name::new("region") => Value::String("us".into()),
    Name::new("id") => Value::String("123".into()),
};

// Only 1 backend call despite different field order
```

### Per-Type Grouping

Entities are grouped by type for efficient batching:

```rust
// Mixed entity types are grouped appropriately
let (user, product, order) = tokio::join!(
    loader.load("User", user_repr),
    loader.load("Product", product_repr),
    loader.load("Order", order_repr),
);
// 3 batched backend calls (1 per entity type)
```

## Performance Benefits

| Scenario | Without DataLoader | With DataLoader |
|----------|-------------------|-----------------|
| 10 users with friends | 11 calls | 2 calls |
| 100 products with reviews | 101 calls | 2 calls |
| N entities, M relations | N*M+1 calls | M+1 calls |

### When to Use DataLoader

✅ **Always use DataLoader for:**
- Federated entity resolution
- Nested field resolution that fetches related entities
- Any resolver that may be called multiple times per query

❌ **DataLoader may not be needed for:**
- Single root queries (no N+1 potential)
- Mutations (typically single entity)
- Subscriptions (streaming, not batched)

## Example: Complete Federation Setup

Here's a complete example demonstrating DataLoader with federation:

```rust
use grpc_graphql_gateway::{
    Gateway, EntityDataLoader, GrpcEntityResolver, EntityConfig,
    federation::EntityResolver,
};
use async_graphql::{Value, indexmap::IndexMap, Name};
use std::sync::Arc;
use std::collections::HashMap;

// Your store or data source
struct InMemoryStore {
    users: HashMap<String, User>,
    products: HashMap<String, Product>,
}

// Entity resolver that uses the DataLoader
struct StoreEntityResolver {
    store: Arc<InMemoryStore>,
    loader: Arc<EntityDataLoader>,
}

impl StoreEntityResolver {
    pub fn new(store: Arc<InMemoryStore>) -> Self {
        // Create base resolver
        let base = Arc::new(DirectStoreResolver { store: store.clone() });
        
        // Configure entities
        let mut configs = HashMap::new();
        configs.insert("User".to_string(), user_entity_config());
        configs.insert("Product".to_string(), product_entity_config());
        
        // Wrap with DataLoader
        let loader = Arc::new(EntityDataLoader::new(base, configs));
        
        Self { store, loader }
    }
}

#[async_trait::async_trait]
impl EntityResolver for StoreEntityResolver {
    async fn resolve_entity(
        &self,
        config: &EntityConfig,
        representation: &IndexMap<Name, Value>,
    ) -> grpc_graphql_gateway::Result<Value> {
        self.loader.load(&config.type_name, representation.clone()).await
    }

    async fn batch_resolve_entities(
        &self,
        config: &EntityConfig,
        representations: Vec<IndexMap<Name, Value>>,
    ) -> grpc_graphql_gateway::Result<Vec<Value>> {
        self.loader.load_many(&config.type_name, representations).await
    }
}
```

## Best Practices

1. **Create DataLoader per request**: For request-scoped caching, create a new DataLoader instance per GraphQL request.

2. **Share across resolvers**: Pass the same DataLoader instance to all resolvers within a request.

3. **Configure appropriate batch sizes**: The underlying resolver should handle batch sizes efficiently.

4. **Monitor batch efficiency**: Track how many entities are batched together to identify optimization opportunities.

5. **Handle partial failures**: The batch resolver should return results in the same order as the input, using `null` for failed items.

## See Also

- [Entity Resolution](../federation/entity-resolution.md) - Complete entity resolution guide
- [Apollo Federation Overview](../federation/overview.md) - Federation concepts
- [Response Caching](./caching.md) - Additional caching strategies

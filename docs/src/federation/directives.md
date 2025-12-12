# Federation Directives

The gateway supports all Apollo Federation v2 directives through proto annotations.

## Directive Reference

| Directive | Proto Option | Purpose |
|-----------|--------------|---------|
| `@key` | `graphql.entity.keys` | Define entity key fields |
| `@shareable` | `graphql.field.shareable` | Field resolvable from multiple subgraphs |
| `@external` | `graphql.field.external` | Field defined in another subgraph |
| `@requires` | `graphql.field.requires` | Fields needed from other subgraphs |
| `@provides` | `graphql.field.provides` | Fields this resolver provides |
| `@extends` | `graphql.entity.extend` | Extending entity from another subgraph |

## @key

Defines how an entity is uniquely identified:

```protobuf
message User {
  option (graphql.entity) = {
    keys: "id"
  };
  string id = 1;
}
```

**Generated:**
```graphql
type User @key(fields: "id") {
  id: ID!
}
```

### Multiple Keys

```protobuf
message Product {
  option (graphql.entity) = {
    keys: "upc"  // Primary key
  };
  string upc = 1;
  string sku = 2;
}
```

### Composite Keys

```protobuf
message Inventory {
  option (graphql.entity) = {
    keys: "warehouseId productId"
  };
  string warehouse_id = 1;
  string product_id = 2;
  int32 quantity = 3;
}
```

## @shareable

Marks fields that can be resolved by multiple subgraphs:

```protobuf
message User {
  string id = 1;
  string name = 2 [(graphql.field) = { shareable: true }];
  string email = 3 [(graphql.field) = { shareable: true }];
}
```

**Generated:**
```graphql
type User {
  id: ID!
  name: String @shareable
  email: String @shareable
}
```

### When to Use

Use `@shareable` when:
- Multiple subgraphs can resolve the same field
- You want redundancy for a commonly accessed field
- Different subgraphs have the same data source

## @external

Marks fields defined in another subgraph that you need to reference:

```protobuf
message User {
  option (graphql.entity) = { extend: true, keys: "id" };
  
  string id = 1 [(graphql.field) = { external: true }];
  string name = 2 [(graphql.field) = { external: true }];
  repeated Review reviews = 3;  // Your field
}
```

**Generated:**
```graphql
type User @extends @key(fields: "id") {
  id: ID! @external
  name: String @external
  reviews: [Review]
}
```

## @requires

Declares that a field requires data from external fields:

```protobuf
message Product {
  option (graphql.entity) = { extend: true, keys: "upc" };
  
  string upc = 1 [(graphql.field) = { external: true }];
  float price = 2 [(graphql.field) = { external: true }];
  float weight = 3 [(graphql.field) = { external: true }];
  
  float shipping_cost = 4 [(graphql.field) = { 
    requires: "price weight" 
  }];
}
```

**Generated:**
```graphql
type Product @extends @key(fields: "upc") {
  upc: ID! @external
  price: Float @external
  weight: Float @external
  shippingCost: Float @requires(fields: "price weight")
}
```

### How It Works

1. Router fetches `price` and `weight` from the owning subgraph
2. Router sends those values to your subgraph
3. Your resolver uses them to calculate `shippingCost`

## @provides

Hints that a resolver provides additional fields on referenced entities:

```protobuf
message Review {
  string id = 1;
  string body = 2;
  
  User author = 3 [(graphql.field) = {
    provides: "name email"
  }];
}
```

**Generated:**
```graphql
type Review {
  id: ID!
  body: String
  author: User @provides(fields: "name email")
}
```

### When to Use

Use `@provides` when:
- Your resolver already has the nested entity's data
- You want to avoid an extra subgraph hop
- You're denormalizing for performance

## Complete Example

**Products Subgraph:**
```protobuf
message Product {
  option (graphql.entity) = {
    keys: "upc"
    resolvable: true
  };
  
  string upc = 1 [(graphql.field) = { required: true }];
  string name = 2 [(graphql.field) = { shareable: true }];
  float price = 3 [(graphql.field) = { shareable: true }];
}
```

**Inventory Subgraph:**
```protobuf
message Product {
  option (graphql.entity) = {
    extend: true
    keys: "upc"
  };
  
  string upc = 1 [(graphql.field) = { external: true }];
  float price = 2 [(graphql.field) = { external: true }];
  float weight = 3 [(graphql.field) = { external: true }];
  
  int32 stock = 4;
  bool in_stock = 5;
  float shipping_estimate = 6 [(graphql.field) = {
    requires: "price weight"
  }];
}
```

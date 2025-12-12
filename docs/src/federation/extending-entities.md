# Extending Entities

Extend entities defined in other subgraphs to add fields that your service owns.

## Basic Extension

Use `extend: true` to extend an entity from another subgraph:

```protobuf
// In Reviews service - extending User from Users service
message User {
  option (graphql.entity) = {
    extend: true
    keys: "id"
  };
  
  // Key field from the original entity
  string id = 1 [(graphql.field) = {
    external: true
    required: true
  }];
  
  // Fields this service adds
  repeated Review reviews = 2 [(graphql.field) = {
    requires: "id"
  }];
}
```

## Generated Schema

The above generates federation-compatible schema:

```graphql
type User @key(fields: "id") @extends {
  id: ID! @external
  reviews: [Review] @requires(fields: "id")
}
```

## Extension Pattern

```
┌─────────────────────────────────────────────────────────────────┐
│                         Supergraph                              │
│                                                                 │
│  type User @key(fields: "id") {                                 │
│    id: ID!           # From Users Service                       │
│    name: String      # From Users Service                       │
│    email: String     # From Users Service                       │
│    reviews: [Review] # From Reviews Service (extension)         │
│  }                                                              │
└─────────────────────────────────────────────────────────────────┘
         ▲                                    ▲
         │                                    │
┌────────┴────────┐              ┌────────────┴────────────┐
│  Users Service  │              │    Reviews Service       │
│                 │              │                          │
│  type User      │              │  type User @extends      │
│    id: ID!      │              │    id: ID! @external     │
│    name: String │              │    reviews: [Review]     │
│    email: String│              │                          │
└─────────────────┘              └──────────────────────────┘
```

## External Fields

Mark fields owned by another subgraph as `external`:

```protobuf
message User {
  option (graphql.entity) = {
    extend: true
    keys: "id"
  };
  
  string id = 1 [(graphql.field) = { 
    external: true  // This field comes from another subgraph
    required: true 
  }];
  
  string name = 2 [(graphql.field) = { 
    external: true  // Also external
  }];
  
  // This service's contribution
  int32 review_count = 3;
}
```

## Requires Directive

Use `requires` when you need data from external fields to resolve a local field:

```protobuf
message Product {
  option (graphql.entity) = {
    extend: true
    keys: "upc"
  };
  
  string upc = 1 [(graphql.field) = { external: true }];
  float price = 2 [(graphql.field) = { external: true }];
  float weight = 3 [(graphql.field) = { external: true }];
  
  // Needs price and weight to calculate
  float shipping_estimate = 4 [(graphql.field) = { 
    requires: "price weight" 
  }];
}
```

The federation router will fetch `price` and `weight` from the owning subgraph before calling your resolver for `shipping_estimate`.

## Provides Directive

Use `provides` to indicate which nested fields your resolver provides:

```protobuf
message Review {
  string id = 1;
  string body = 2;
  
  // When resolving author, we also provide their username
  User author = 3 [(graphql.field) = {
    provides: "username"
  }];
}
```

## Complete Example

**Users Subgraph (owns User):**
```protobuf
message User {
  option (graphql.entity) = {
    keys: "id"
    resolvable: true
  };
  
  string id = 1 [(graphql.field) = { required: true }];
  string name = 2 [(graphql.field) = { shareable: true }];
  string email = 3;
}
```

**Reviews Subgraph (extends User):**
```protobuf
message User {
  option (graphql.entity) = {
    extend: true
    keys: "id"
  };
  
  string id = 1 [(graphql.field) = { external: true, required: true }];
  repeated Review reviews = 2;
}

message Review {
  string id = 1;
  string body = 2;
  int32 rating = 3;
  User author = 4;
}
```

**Composed Query:**
```graphql
query {
  user(id: "123") {
    id
    name          # Resolved by Users subgraph
    email         # Resolved by Users subgraph
    reviews {     # Resolved by Reviews subgraph (extension)
      id
      body
      rating
    }
  }
}
```

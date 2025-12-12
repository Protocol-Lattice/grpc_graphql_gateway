# Defining Entities

Entities are the building blocks of Apollo Federation. They're types that can be resolved across multiple subgraphs using a unique key.

## Basic Entity Definition

Use the `graphql.entity` option on your protobuf messages:

```protobuf
message User {
  option (graphql.entity) = {
    keys: "id"
    resolvable: true
  };
  
  string id = 1 [(graphql.field) = { required: true }];
  string name = 2;
  string email = 3 [(graphql.field) = { shareable: true }];
}
```

## Entity Options

| Option | Type | Description |
|--------|------|-------------|
| `keys` | string | The field(s) that uniquely identify this entity |
| `resolvable` | bool | Whether this subgraph can resolve the entity |
| `extend` | bool | Whether this extends an entity from another subgraph |

## Generated GraphQL

The above proto generates:

```graphql
type User @key(fields: "id") {
  id: ID!
  name: String
  email: String @shareable
}
```

## Composite Keys

Use space-separated fields for composite keys:

```protobuf
message Product {
  option (graphql.entity) = {
    keys: "sku region"
    resolvable: true
  };
  
  string sku = 1 [(graphql.field) = { required: true }];
  string region = 2 [(graphql.field) = { required: true }];
  string name = 3;
}
```

**Generated:**
```graphql
type Product @key(fields: "sku region") {
  sku: ID!
  region: ID!
  name: String
}
```

## Multiple Keys

Define multiple key sets by repeating the `graphql.entity` option or using multiple key definitions:

```protobuf
message User {
  option (graphql.entity) = {
    keys: "id"
    resolvable: true
  };
  
  string id = 1;
  string email = 2;  // Could also be a key
}
```

## Resolvable vs Non-Resolvable

### Resolvable Entities

When `resolvable: true`, this subgraph can fully resolve the entity:

```protobuf
message User {
  option (graphql.entity) = {
    keys: "id"
    resolvable: true  // Can resolve User by id
  };
  
  string id = 1;
  string name = 2;
  string email = 3;
}
```

### Stub Entities

When `resolvable: false`, this subgraph only references the entity:

```protobuf
message User {
  option (graphql.entity) = {
    keys: "id"
    resolvable: false  // Cannot resolve, just references
  };
  
  string id = 1 [(graphql.field) = { external: true }];
}
```

## Real-World Example

**Users Service (owns User entity):**
```protobuf
message User {
  option (graphql.entity) = {
    keys: "id"
    resolvable: true
  };
  
  string id = 1 [(graphql.field) = { required: true }];
  string name = 2 [(graphql.field) = { shareable: true }];
  string email = 3 [(graphql.field) = { shareable: true }];
}
```

**Reviews Service (references User):**
```protobuf
message Review {
  string id = 1;
  string body = 2;
  User author = 3;  // Reference to User from Users service
}

message User {
  option (graphql.entity) = {
    keys: "id"
    extend: true  // Extending User from another subgraph
  };
  
  string id = 1 [(graphql.field) = { external: true, required: true }];
  repeated Review reviews = 2 [(graphql.field) = { requires: "id" }];
}
```

## Key Field Requirements

Key fields should be:

1. **Marked as required** - Use `required: true`
2. **Non-null in responses** - Always return a value
3. **Consistent across subgraphs** - Same type everywhere

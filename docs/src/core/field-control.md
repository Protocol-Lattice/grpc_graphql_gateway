# Field-Level Control

Use the `graphql.field` option to customize how individual fields are exposed in the GraphQL schema.

## Basic Field Options

```protobuf
message User {
  string id = 1 [(graphql.field) = { required: true }];
  string email = 2 [(graphql.field) = { name: "emailAddress" }];
  string internal_id = 3 [(graphql.field) = { omit: true }];
  string password_hash = 4 [(graphql.field) = { omit: true }];
}
```

## Available Options

| Option | Type | Description |
|--------|------|-------------|
| `name` | string | Override the GraphQL field name |
| `omit` | bool | Exclude this field from GraphQL schema |
| `required` | bool | Mark field as non-nullable (`!`) |
| `shareable` | bool | Federation: field can be resolved by multiple subgraphs |
| `external` | bool | Federation: field is defined in another subgraph |
| `requires` | string | Federation: fields needed from other subgraphs |
| `provides` | string | Federation: fields this resolver provides |

## Renaming Fields

Use `name` to map protobuf field names to GraphQL conventions:

```protobuf
message User {
  string user_name = 1 [(graphql.field) = { name: "username" }];
  string email_address = 2 [(graphql.field) = { name: "email" }];
  int64 created_at_unix = 3 [(graphql.field) = { name: "createdAt" }];
}
```

**Generated GraphQL:**
```graphql
type User {
  username: String!
  email: String!
  createdAt: Int!
}
```

## Omitting Fields

Hide sensitive or internal fields:

```protobuf
message User {
  string id = 1;
  string name = 2;
  string password_hash = 3 [(graphql.field) = { omit: true }];
  string internal_notes = 4 [(graphql.field) = { omit: true }];
}
```

**Generated GraphQL:**
```graphql
type User {
  id: String!
  name: String!
  # password_hash and internal_notes are not exposed
}
```

## Required Fields

Mark fields as non-nullable in GraphQL:

```protobuf
message CreateUserInput {
  string name = 1 [(graphql.field) = { required: true }];
  string email = 2 [(graphql.field) = { required: true }];
  string bio = 3;  // Optional
}
```

**Generated GraphQL:**
```graphql
input CreateUserInput {
  name: String!
  email: String!
  bio: String
}
```

## Federation Directives

For Apollo Federation, use field-level directives:

```protobuf
message User {
  string id = 1 [(graphql.field) = { 
    required: true
    shareable: true 
  }];
  
  string email = 2 [(graphql.field) = { 
    external: true 
  }];
  
  repeated Review reviews = 3 [(graphql.field) = { 
    requires: "id" 
  }];
}
```

See [Federation Directives](../federation/directives.md) for more details.

## Combining Options

Options can be combined:

```protobuf
message Product {
  string upc = 1 [(graphql.field) = { 
    required: true
    name: "id"
    shareable: true 
  }];
}
```

## Default Values

Protobuf fields have default values (empty string, 0, false). In GraphQL:

- Fields with defaults may still be nullable
- Use `required: true` to make them non-nullable
- The gateway handles type conversion automatically

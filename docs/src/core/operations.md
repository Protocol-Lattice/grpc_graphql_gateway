# Queries, Mutations & Subscriptions

The gateway supports all three GraphQL operation types, automatically derived from your protobuf service definitions.

## Annotating Proto Methods

Use the `graphql.schema` option to define how each RPC method maps to GraphQL:

```protobuf
service UserService {
  option (graphql.service) = {
    host: "localhost:50051"
    insecure: true
  };

  // Query - for fetching data
  rpc GetUser(GetUserRequest) returns (User) {
    option (graphql.schema) = {
      type: QUERY
      name: "user"
    };
  }

  // Mutation - for modifying data
  rpc CreateUser(CreateUserRequest) returns (User) {
    option (graphql.schema) = {
      type: MUTATION
      name: "createUser"
      request { name: "input" }
    };
  }

  // Subscription - for real-time data (server streaming)
  rpc WatchUser(WatchUserRequest) returns (stream User) {
    option (graphql.schema) = {
      type: SUBSCRIPTION
      name: "userUpdates"
    };
  }
}
```

## Operation Type Mapping

| Proto RPC Type | GraphQL Type | Use Case |
|----------------|--------------|----------|
| Unary | Query/Mutation | Fetch or modify data |
| Server Streaming | Subscription | Real-time updates |
| Client Streaming | Not supported | - |
| Bidirectional | Not supported | - |

## Queries

Queries are used for fetching data:

```graphql
query {
  user(id: "123") {
    id
    name
    email
  }
}
```

### Query Example

**Proto:**
```protobuf
rpc GetUser(GetUserRequest) returns (User) {
  option (graphql.schema) = {
    type: QUERY
    name: "user"
  };
}
```

**GraphQL:**
```graphql
query GetUser {
  user(id: "123") {
    id
    name
    email
  }
}
```

## Mutations

Mutations are used for creating, updating, or deleting data:

```graphql
mutation {
  createUser(input: { name: "Alice", email: "alice@example.com" }) {
    id
    name
  }
}
```

### Using Input Types

The `request` option customizes how the request message is exposed:

```protobuf
rpc CreateUser(CreateUserRequest) returns (User) {
  option (graphql.schema) = {
    type: MUTATION
    name: "createUser"
    request { name: "input" }  // Wrap request fields under "input"
  };
}
```

This creates a GraphQL mutation with an `input` argument containing all fields from `CreateUserRequest`.

## Subscriptions

Subscriptions provide real-time updates via WebSocket:

```graphql
subscription {
  userUpdates(id: "123") {
    id
    name
    status
  }
}
```

### WebSocket Protocol

The gateway supports the `graphql-transport-ws` protocol. Connect to:

```
ws://localhost:8888/graphql/ws
```

### Subscription Example

**Proto (server streaming RPC):**
```protobuf
rpc WatchUser(WatchUserRequest) returns (stream User) {
  option (graphql.schema) = {
    type: SUBSCRIPTION
    name: "userUpdates"
  };
}
```

**JavaScript Client:**
```javascript
import { createClient } from 'graphql-ws';

const client = createClient({
  url: 'ws://localhost:8888/graphql/ws',
});

client.subscribe(
  {
    query: 'subscription { userUpdates(id: "123") { id name status } }',
  },
  {
    next: (data) => console.log('Update:', data),
    error: (err) => console.error('Error:', err),
    complete: () => console.log('Complete'),
  }
);
```

## Multiple Operations

You can run multiple operations in a single request using [Batch Queries](../performance/batch-queries.md):

```json
[
  {"query": "{ users { id name } }"},
  {"query": "{ products { upc price } }"}
]
```

# Live Queries

Live queries provide real-time data updates to clients using the `@live` directive. When underlying data changes, connected clients automatically receive updated results.

## Overview

Unlike traditional GraphQL subscriptions that push specific events, live queries automatically re-execute the query when relevant data mutations occur, sending the complete updated result to the client.

### Key Features

- **`@live` Directive**: Add to any query to make it "live"
- **WebSocket Delivery**: Real-time updates via `/graphql/live` endpoint
- **Invalidation-Based**: Mutations trigger query re-execution
- **Configurable Strategies**: Invalidation, polling, or hash-diff modes
- **Throttling**: Prevent flooding clients with too many updates

## Quick Start

### 1. Client: Send a Live Query

Connect to the WebSocket endpoint and subscribe with the `@live` directive:

```javascript
const ws = new WebSocket('ws://localhost:9000/graphql/live', 'graphql-transport-ws');

ws.onopen = () => {
  // Initialize connection
  ws.send(JSON.stringify({ type: 'connection_init' }));
};

ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);
  
  if (msg.type === 'connection_ack') {
    // Subscribe with @live query
    ws.send(JSON.stringify({
      id: 'users-live',
      type: 'subscribe',
      payload: {
        query: `query @live {
          users {
            id
            name
            status
          }
        }`
      }
    }));
  }
  
  if (msg.type === 'next') {
    console.log('Received update:', msg.payload.data);
  }
};
```

### 2. Proto: Configure Live Query Support

Mark RPC methods as live query compatible:

```protobuf
service UserService {
  rpc ListUsers(Empty) returns (UserList) {
    option (graphql.schema) = { 
      type: QUERY 
      name: "users" 
    };
    option (graphql.live_query) = {
      enabled: true
      strategy: INVALIDATION
      triggers: ["User.create", "User.update", "User.delete"]
      throttle_ms: 100
    };
  }
  
  rpc CreateUser(CreateUserRequest) returns (User) {
    option (graphql.schema) = { 
      type: MUTATION 
      name: "createUser" 
    };
    // Mutations don't need live_query config - they trigger invalidation
  }
}
```

### 3. Server: Trigger Invalidation

After mutations, trigger invalidation to notify live queries:

```rust
use grpc_graphql_gateway::{InvalidationEvent, LiveQueryStore};

// In your mutation handler
async fn create_user(&self, req: CreateUserRequest) -> Result<User, Status> {
    // ... create user logic ...
    
    // Notify live queries that User data changed
    if let Some(store) = &self.live_query_store {
        store.invalidate(InvalidationEvent::new("User", "create"));
    }
    
    Ok(user)
}
```

## Live Query Strategies

### Invalidation (Recommended)

Re-execute query only when relevant mutations occur:

```protobuf
option (graphql.live_query) = {
  enabled: true
  strategy: INVALIDATION
  triggers: ["User.update", "User.delete"]
};
```

### Polling

Periodically re-execute query at fixed intervals:

```protobuf
option (graphql.live_query) = {
  enabled: true
  strategy: POLLING
  poll_interval_ms: 5000  // Every 5 seconds
};
```

### Hash Diff

Only send updates if result actually changed:

```protobuf
option (graphql.live_query) = {
  enabled: true
  strategy: HASH_DIFF
  poll_interval_ms: 1000
};
```

## Configuration Options

| Option | Type | Description |
|--------|------|-------------|
| `enabled` | bool | Enable live query for this operation |
| `strategy` | enum | `INVALIDATION`, `POLLING`, or `HASH_DIFF` |
| `triggers` | string[] | Invalidation event patterns (e.g., "User.update") |
| `throttle_ms` | uint32 | Minimum time between updates (default: 100ms) |
| `poll_interval_ms` | uint32 | Polling interval for POLLING/HASH_DIFF strategies |
| `ttl_seconds` | uint32 | Auto-expire subscription after N seconds |

## API Reference

### Public Functions

```rust
// Check if query contains @live directive
pub fn has_live_directive(query: &str) -> bool;

// Strip @live directive for execution
pub fn strip_live_directive(query: &str) -> String;

// Create a shared live query store
pub fn create_live_query_store() -> SharedLiveQueryStore;

// Create with custom config
pub fn create_live_query_store_with_config(config: LiveQueryConfig) -> SharedLiveQueryStore;
```

### LiveQueryStore Methods

```rust
impl LiveQueryStore {
    // Register a new live query subscription
    pub fn register(&self, query: ActiveLiveQuery, sender: Sender<LiveQueryUpdate>) -> Result<(), LiveQueryError>;
    
    // Unregister a subscription
    pub fn unregister(&self, subscription_id: &str) -> Option<ActiveLiveQuery>;
    
    // Trigger invalidation for matching subscriptions
    pub fn invalidate(&self, event: InvalidationEvent) -> usize;
    
    // Get current statistics
    pub fn stats(&self) -> LiveQueryStats;
}
```

### InvalidationEvent

```rust
// Create an invalidation event
let event = InvalidationEvent::new("User", "update");

// With specific entity ID
let event = InvalidationEvent::with_id("User", "update", "user-123");
```

## WebSocket Protocol

The `/graphql/live` endpoint uses the `graphql-transport-ws` protocol:

### Client → Server

| Message Type | Description |
|-------------|-------------|
| `connection_init` | Initialize connection |
| `subscribe` | Start a live query subscription |
| `complete` | End a subscription |
| `ping` | Keep-alive ping |

### Server → Client

| Message Type | Description |
|-------------|-------------|
| `connection_ack` | Connection accepted |
| `next` | Query result (initial or update) |
| `error` | Error occurred |
| `complete` | Subscription ended |
| `pong` | Keep-alive response |

## Example: Full CRUD with Live Updates

See the complete example at `examples/live_query/`:

```bash
# Run the example
cargo run --example live_query

# In another terminal, run the WebSocket test
node examples/live_query/test_ws.js
```

The test demonstrates:
1. Initial live query returning 3 users
2. Delete mutation removing a user
3. Re-query showing 2 users
4. Create mutation adding a new user
5. Final query showing updated user list

## Best Practices

1. **Use Specific Triggers**: Only subscribe to relevant entity types
2. **Set Appropriate Throttle**: Prevent overwhelming clients (100-500ms)
3. **Use TTL for Temporary Subscriptions**: Auto-cleanup inactive queries
4. **Prefer Invalidation over Polling**: More efficient for most use cases
5. **Handle Reconnection**: Clients should re-subscribe after disconnect

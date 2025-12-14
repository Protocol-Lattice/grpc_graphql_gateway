# Request Collapsing

Request collapsing (also known as request deduplication) is a powerful optimization that reduces the number of gRPC backend calls by identifying and coalescing identical concurrent requests.

## How It Works

When a GraphQL query contains multiple fields that call the same gRPC method with identical arguments, request collapsing ensures only one gRPC call is made:

```graphql
query {
  user1: getUser(id: "1") { name }
  user2: getUser(id: "2") { name }
  user3: getUser(id: "1") { name }  # Duplicate of user1!
}
```

**Without Request Collapsing:** 3 gRPC calls are made.

**With Request Collapsing:** Only 2 gRPC calls are made (user1 and user3 share the same response).

### The Leader-Follower Pattern

1. **Leader**: The first request with a unique key executes the gRPC call
2. **Followers**: Subsequent identical requests wait for the leader's result
3. **Broadcast**: When the leader completes, it broadcasts the result to all followers
4. **Cleanup**: The in-flight entry is removed after broadcasting

## Configuration

```rust
use grpc_graphql_gateway::{Gateway, RequestCollapsingConfig};
use std::time::Duration;

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .with_request_collapsing(RequestCollapsingConfig::default())
    .add_grpc_client("service", client)
    .build()?;
```

### Configuration Options

| Option | Default | Description |
|--------|---------|-------------|
| `coalesce_window` | 50ms | Maximum time to wait for in-flight requests |
| `max_waiters` | 100 | Maximum followers waiting for a single leader |
| `enabled` | true | Enable/disable collapsing |
| `max_cache_size` | 10000 | Maximum in-flight requests to track |

### Builder Pattern

```rust
let config = RequestCollapsingConfig::new()
    .coalesce_window(Duration::from_millis(100))  // Longer window
    .max_waiters(200)                              // More waiters allowed
    .max_cache_size(20000)                         // Larger cache
    .enabled(true);
```

## Presets

Request collapsing comes with several presets for common scenarios:

### Default (Balanced)

```rust
let config = RequestCollapsingConfig::default();
// coalesce_window: 50ms
// max_waiters: 100
// max_cache_size: 10000
```

Best for most workloads with a balance between latency and deduplication.

### High Throughput

```rust
let config = RequestCollapsingConfig::high_throughput();
// coalesce_window: 100ms
// max_waiters: 500
// max_cache_size: 50000
```

Best for high-traffic scenarios where maximizing deduplication is more important than latency.

### Low Latency

```rust
let config = RequestCollapsingConfig::low_latency();
// coalesce_window: 10ms
// max_waiters: 50
// max_cache_size: 5000
```

Best for latency-sensitive applications where quick responses are critical.

### Disabled

```rust
let config = RequestCollapsingConfig::disabled();
```

Completely disables request collapsing.

## Monitoring

You can monitor request collapsing effectiveness using the built-in statistics:

```rust
// Get the registry from ServeMux
if let Some(registry) = mux.request_collapsing() {
    let stats = registry.stats();
    println!("In-flight requests: {}", stats.in_flight_count);
    println!("Max cache size: {}", stats.max_cache_size);
    println!("Enabled: {}", stats.enabled);
}
```

## Request Key Generation

Each request is identified by a SHA-256 hash of:

1. **Service name** - The gRPC service identifier
2. **gRPC path** - The method path (e.g., `/greeter.Greeter/SayHello`)
3. **Request bytes** - The serialized protobuf message

This ensures that only truly identical requests are collapsed.

## Relationship with Other Features

### Response Caching

Request collapsing and response caching work together:

- **Request Collapsing**: Deduplicates *concurrent* identical requests
- **Response Caching**: Caches *completed* responses for future requests

The typical flow is:
1. Check response cache → cache hit? Return cached response
2. Check in-flight requests → follower? Wait for leader
3. Execute gRPC call as leader
4. Broadcast result to followers
5. Cache response for future requests

### Circuit Breaker

Request collapsing works seamlessly with the circuit breaker:

- If the circuit is open, all collapsed requests fail fast together
- The leader request respects circuit breaker state
- Followers receive the same error as the leader

## Best Practices

1. **Start with defaults**: The default configuration works well for most use cases

2. **Monitor collapse ratio**: Track how many requests are being deduplicated
   - Low ratio? Requests may be too unique, consider if collapsing adds value
   - High ratio? Great! You're saving significant backend load

3. **Tune for your workload**:
   - High read traffic? Use `high_throughput()` preset
   - Real-time requirements? Use `low_latency()` preset

4. **Consider request patterns**:
   - GraphQL queries with aliases benefit most
   - Unique requests per field won't see much benefit

## Example: Full Configuration

```rust
use grpc_graphql_gateway::{Gateway, RequestCollapsingConfig, CacheConfig};
use std::time::Duration;

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    // Enable response caching
    .with_response_cache(CacheConfig {
        max_size: 10_000,
        default_ttl: Duration::from_secs(60),
        stale_while_revalidate: Some(Duration::from_secs(30)),
        invalidate_on_mutation: true,
    })
    // Enable request collapsing
    .with_request_collapsing(
        RequestCollapsingConfig::new()
            .coalesce_window(Duration::from_millis(75))
            .max_waiters(150)
    )
    .add_grpc_client("service", client)
    .build()?;
```

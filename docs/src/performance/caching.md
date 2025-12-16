# Response Caching

Dramatically improve performance with in-memory GraphQL response caching.

## Enabling Caching

```rust
use grpc_graphql_gateway::{Gateway, CacheConfig};
use std::time::Duration;

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .with_response_cache(CacheConfig {
        max_size: 10_000,                              // Max cached responses
        default_ttl: Duration::from_secs(60),          // 1 minute TTL
        stale_while_revalidate: Some(Duration::from_secs(30)),
        invalidate_on_mutation: true,
    })
    .build()?;
```

## Configuration Options

| Option | Type | Description |
|--------|------|-------------|
| `max_size` | usize | Maximum number of cached responses |
| `default_ttl` | Duration | Time before entries expire |
| `stale_while_revalidate` | Option\<Duration\> | Serve stale content while refreshing |
| `invalidate_on_mutation` | bool | Clear cache on mutations |
| `redis_url` | Option\<String\> | Redis connection URL for distributed caching |
| `vary_headers` | Vec\<String\> | Headers to include in cache key (default: `["Authorization"]`) |

## Distributed Caching (Redis)

Use Redis for shared caching across multiple gateway instances:

```rust
let gateway = Gateway::builder()
    .with_response_cache(CacheConfig {
        redis_url: Some("redis://127.0.0.1:6379".to_string()),
        default_ttl: Duration::from_secs(60),
        ..Default::default()
    })
    .build()?;
```

## Vary Headers

By default, the cache key includes the `Authorization` header to prevent leaking user data. You can configure which headers affect the cache key:

```rust
CacheConfig {
    // Cache per user and per tenant
    vary_headers: vec!["Authorization".to_string(), "X-Tenant-ID".to_string()],
    ..Default::default()
}
```

## How It Works

1. **First Query**: Cache miss → Execute gRPC → Cache response → Return
2. **Second Query**: Cache hit → Return cached response immediately (\<1ms)
3. **Mutation**: Execute mutation → Invalidate related cache entries
4. **Next Query**: Cache miss (invalidated) → Execute gRPC → Cache → Return

## What Gets Cached

| Operation | Cached? | Triggers Invalidation? |
|-----------|---------|------------------------|
| Query | ✅ Yes | No |
| Mutation | ❌ No | ✅ Yes |
| Subscription | ❌ No | No |

## Cache Key Generation

The cache key is a SHA-256 hash of:
- Normalized query string
- Sorted variables JSON
- Operation name (if provided)

## Stale-While-Revalidate

Serve stale content immediately while refreshing in the background:

```rust
CacheConfig {
    default_ttl: Duration::from_secs(60),
    stale_while_revalidate: Some(Duration::from_secs(30)),
    ..Default::default()
}
```

Timeline:
- 0-60s: Fresh content served
- 60-90s: Stale content served, refresh triggered
- 90s+: Cache miss, fresh fetch

## Mutation Invalidation

When `invalidate_on_mutation: true`:

```rust
// This mutation invalidates cache
mutation { updateUser(id: "123", name: "Alice") { id name } }

// Subsequent queries fetch fresh data
query { user(id: "123") { id name } }
```

## Testing with curl

```bash
# 1. First query - cache miss
curl -X POST http://localhost:8888/graphql \
  -H "Content-Type: application/json" \
  -d '{"query": "{ user(id: \"123\") { name } }"}'

# 2. Same query - cache hit (instant)
curl -X POST http://localhost:8888/graphql \
  -H "Content-Type: application/json" \
  -d '{"query": "{ user(id: \"123\") { name } }"}'

# 3. Mutation - invalidates cache
curl -X POST http://localhost:8888/graphql \
  -H "Content-Type: application/json" \
  -d '{"query": "mutation { updateUser(id: \"123\", name: \"Bob\") { name } }"}'

# 4. Query again - cache miss (fresh data)
curl -X POST http://localhost:8888/graphql \
  -H "Content-Type: application/json" \
  -d '{"query": "{ user(id: \"123\") { name } }"}'
```

## Performance Impact

- Cache hits: \<1ms response time
- 10-100x fewer gRPC backend calls
- Significant reduction in backend load

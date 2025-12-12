# Automatic Persisted Queries (APQ)

Reduce bandwidth by caching queries on the server and sending only hashes.

## Enabling APQ

```rust
use grpc_graphql_gateway::{Gateway, PersistedQueryConfig};
use std::time::Duration;

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .with_persisted_queries(PersistedQueryConfig {
        cache_size: 1000,                        // Max cached queries
        ttl: Some(Duration::from_secs(3600)),    // 1 hour expiration
    })
    .build()?;
```

## How APQ Works

1. **First request**: Client sends hash only → Gateway returns `PERSISTED_QUERY_NOT_FOUND`
2. **Retry**: Client sends hash + full query → Gateway caches and executes
3. **Subsequent requests**: Client sends hash only → Gateway uses cached query

## Client Request Format

**Hash only (after caching):**
```json
{
  "extensions": {
    "persistedQuery": {
      "version": 1,
      "sha256Hash": "ecf4edb46db40b5132295c0291d62fb65d6759a9eedfa4d5d612dd5ec54a6b38"
    }
  }
}
```

**Hash + query (initial):**
```json
{
  "query": "{ user(id: \"123\") { id name } }",
  "extensions": {
    "persistedQuery": {
      "version": 1,
      "sha256Hash": "ecf4edb46db40b5132295c0291d62fb65d6759a9eedfa4d5d612dd5ec54a6b38"
    }
  }
}
```

## Apollo Client Setup

```javascript
import { createPersistedQueryLink } from '@apollo/client/link/persisted-queries';
import { sha256 } from 'crypto-hash';
import { createHttpLink } from '@apollo/client';

const link = createPersistedQueryLink({ sha256 }).concat(
  createHttpLink({ uri: 'http://localhost:8888/graphql' })
);
```

## Configuration Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `cache_size` | usize | 1000 | Max number of cached queries |
| `ttl` | Option\<Duration\> | None | Optional expiration time |

## Benefits

- ✅ 90%+ reduction in request payload size
- ✅ Compatible with Apollo Client APQ
- ✅ LRU eviction prevents unbounded memory growth
- ✅ Optional TTL for cache expiration

## Error Response

When hash is not found:

```json
{
  "errors": [
    {
      "message": "PersistedQueryNotFound",
      "extensions": {
        "code": "PERSISTED_QUERY_NOT_FOUND"
      }
    }
  ]
}
```

## Cache Statistics

Monitor APQ performance through logs and metrics.

# DoS Protection

Protect your gateway and gRPC backends from denial-of-service attacks with query depth and complexity limiting.

## Query Depth Limiting

Prevent deeply nested queries that could overwhelm your backends:

```rust
let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .with_query_depth_limit(10)  // Max 10 levels of nesting
    .build()?;
```

### What It Prevents

```graphql
# This would be blocked if depth exceeds limit
query {
  users {           # depth 1
    friends {       # depth 2
      friends {     # depth 3
        friends {   # depth 4
          friends { # depth 5 - blocked if limit < 5
            name
          }
        }
      }
    }
  }
}
```

### Error Response

```json
{
  "errors": [
    {
      "message": "Query is nested too deep",
      "extensions": {
        "code": "QUERY_TOO_DEEP"
      }
    }
  ]
}
```

## Query Complexity Limiting

Limit the total "cost" of a query:

```rust
let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .with_query_complexity_limit(100)  // Max complexity of 100
    .build()?;
```

### How Complexity is Calculated

Each field adds to the complexity:

```graphql
# Complexity = 4 (users + friends + name + email)
query {
  users {        # +1
    friends {    # +1
      name       # +1
      email      # +1
    }
  }
}
```

### Error Response

```json
{
  "errors": [
    {
      "message": "Query is too complex",
      "extensions": {
        "code": "QUERY_TOO_COMPLEX"
      }
    }
  ]
}
```

## Recommended Values

| Use Case | Depth Limit | Complexity Limit |
|----------|-------------|------------------|
| Public API | 5-10 | 50-100 |
| Authenticated Users | 10-15 | 100-500 |
| Internal/Trusted | 15-25 | 500-1000 |

## Combining Limits

Use both limits together for comprehensive protection:

```rust
let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .with_query_depth_limit(10)
    .with_query_complexity_limit(100)
    .build()?;
```

## Environment-Based Configuration

Adjust limits based on environment:

```rust
let depth_limit = std::env::var("QUERY_DEPTH_LIMIT")
    .ok()
    .and_then(|s| s.parse().ok())
    .unwrap_or(10);

let complexity_limit = std::env::var("QUERY_COMPLEXITY_LIMIT")
    .ok()
    .and_then(|s| s.parse().ok())
    .unwrap_or(100);

let gateway = Gateway::builder()
    .with_query_depth_limit(depth_limit)
    .with_query_complexity_limit(complexity_limit)
    .build()?;
```

## Related Features

- [Rate Limiting](./middleware.md) - Limit requests per time window
- [Introspection Control](./introspection.md) - Disable schema discovery
- [Circuit Breaker](../performance/circuit-breaker.md) - Protect backend services

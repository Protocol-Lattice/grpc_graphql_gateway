# GraphQL `@defer` Support

The `grpc_graphql_gateway` supports the GraphQL `@defer` directive for
incremental delivery. When a client sends a query containing `@defer`,
the gateway streams partial results immediately and delivers deferred
fields as they become available.

## Quick Start

Enable `@defer` in your gateway configuration:

```rust
use grpc_graphql_gateway::{Gateway, GrpcClient, DeferConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let grpc_client = GrpcClient::new("http://localhost:50051").await?;

    let gateway = Gateway::builder()
        .add_grpc_client("greeter", grpc_client)
        .with_defer(DeferConfig::default())
        .build()?;

    let app = gateway.into_router();
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    axum::serve(listener, app).await?;

    Ok(())
}
```

## Client Usage

### Query with @defer

```graphql
query GetUser {
  user(id: "1") {
    id
    name
    ... @defer(label: "details") {
      email
      profilePicture
      bio
    }
  }
}
```

### HTTP Request

Send a POST request with `Accept: multipart/mixed`:

```bash
curl -X POST http://localhost:8080/graphql \
  -H "Content-Type: application/json" \
  -H "Accept: multipart/mixed" \
  -d '{
    "query": "query { user(id: \"1\") { id name ... @defer(label: \"details\") { email bio } } }"
  }'
```

You can also use the dedicated `/graphql/defer` endpoint — it always
returns `multipart/mixed` for queries with `@defer`, regardless of the
`Accept` header.

### Response Format

The response is streamed as `multipart/mixed`:

```
Content-Type: multipart/mixed; boundary="-"

---
Content-Type: application/json; charset=utf-8

{"data":{"user":{"id":"1","name":"Alice"}},"hasNext":true}
---
Content-Type: application/json; charset=utf-8

{"incremental":[{"data":{"email":"alice@example.com","bio":"..."},"path":["user"],"label":"details"}],"hasNext":false}
-----
```

## Endpoints

| Endpoint           | Method | `@defer` behavior                                                         |
| ------------------ | ------ | ------------------------------------------------------------------------- |
| `/graphql`         | POST   | Streams multipart if `Accept: multipart/mixed` header is set              |
| `/graphql/defer`   | POST   | Always streams multipart for queries with `@defer`                        |

## Configuration

| Option                     | Default  | Description                                      |
| -------------------------- | -------- | ------------------------------------------------ |
| `enabled`                  | `true`   | Enable/disable `@defer` processing               |
| `max_deferred_fragments`   | `16`     | Max `@defer` fragments per query                  |
| `fragment_timeout_ms`      | `30000`  | Timeout for each deferred fragment (ms)            |
| `max_total_timeout_ms`     | `60000`  | Max total query time including deferred parts (ms) |
| `multipart_boundary`       | `"-"`    | Multipart boundary string                          |

### Presets

```rust
// Default — balanced settings
DeferConfig::default()

// Production — strict limits (8 fragments, 15s timeout)
DeferConfig::production()

// Development — permissive (64 fragments, 60s timeout)
DeferConfig::development()

// Disabled — execute all fields eagerly
DeferConfig::disabled()
```

## How It Works

1. **Detection**: The gateway checks if the incoming query contains `@defer`.
2. **Stripping**: `@defer` directives are removed from the query.
3. **Eager Execution**: The full query (without `@defer`) is executed against the gRPC backend.
4. **Splitting**: The response is split into an initial payload (without deferred fields) and incremental patches.
5. **Streaming**: The initial payload is sent immediately, and deferred patches follow as multipart parts.

```
Client → POST /graphql (Accept: multipart/mixed)
  │
  ▼
Gateway detects @defer
  │
  ├─ Strip @defer directives
  ├─ Execute full query eagerly
  ├─ Split response into initial + deferred
  │
  ├─ Stream initial payload (hasNext: true)
  ├─ Stream deferred patches ...
  └─ Stream final patch (hasNext: false)
```

## Compatibility

- Works with any gRPC backend — the `@defer` processing happens at the gateway level.
- Compatible with `Apollo Client`, `urql`, and other GraphQL clients that support incremental delivery.
- No changes needed to your `.proto` definitions.
- Deferred queries still go through middlewares, caching, and other gateway features.

## Limitations

- Nested `@defer` within `@defer` is supported syntactically but both fragments are resolved from the same eager execution.
- The `@defer(if: $variable)` conditional is supported; when `if: false`, the fragment is included in the initial payload.
- Field-level deferred resolution (streaming individual fields) is not yet supported — deferred fragments should contain complete field sets.

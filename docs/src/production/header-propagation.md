# Header Propagation

Forward HTTP headers from GraphQL requests to gRPC backends for authentication and tracing.

## Enabling Header Propagation

```rust
use grpc_graphql_gateway::{Gateway, HeaderPropagationConfig};

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .with_header_propagation(
        HeaderPropagationConfig::new()
            .propagate("authorization")
            .propagate("x-request-id")
            .propagate("x-tenant-id")
    )
    .build()?;
```

## Common Headers Preset

Use the preset for common auth and tracing headers:

```rust
Gateway::builder()
    .with_header_propagation(HeaderPropagationConfig::common())
    .build()?;
```

Includes:
- `authorization` - Bearer tokens
- `x-request-id`, `x-correlation-id` - Request tracking
- `traceparent`, `tracestate` - W3C Trace Context
- `x-b3-*` - Zipkin B3 headers

## Configuration Methods

| Method | Description |
|--------|-------------|
| `.propagate("header")` | Propagate exact header name |
| `.propagate_with_prefix("x-custom-")` | Propagate headers with prefix |
| `.propagate_all_headers()` | Propagate all headers (with exclusions) |
| `.exclude("cookie")` | Exclude specific headers |

## Examples

### Exact Match

```rust
HeaderPropagationConfig::new()
    .propagate("authorization")
    .propagate("x-api-key")
```

### Prefix Match

```rust
HeaderPropagationConfig::new()
    .propagate_with_prefix("x-custom-")
    .propagate_with_prefix("x-tenant-")
```

### All with Exclusions

```rust
HeaderPropagationConfig::new()
    .propagate_all_headers()
    .exclude("cookie")
    .exclude("host")
```

## Security

Uses an **allowlist** approach - only explicitly configured headers are forwarded. This prevents accidental leakage of sensitive headers like `Cookie` or `Host`.

## gRPC Backend

Headers become gRPC metadata:

```rust
// In your gRPC service
async fn get_user(&self, request: Request<GetUserRequest>) -> ... {
    let metadata = request.metadata();
    let auth = metadata.get("authorization")
        .map(|v| v.to_str().ok())
        .flatten();
    
    // Use auth for authorization
}
```

## W3C Trace Context

For distributed tracing, propagate trace context headers:

```rust
HeaderPropagationConfig::new()
    .propagate("traceparent")
    .propagate("tracestate")
    .propagate("authorization")
```

# Middleware

The gateway supports an extensible middleware system for authentication, logging, rate limiting, and custom request processing.

## Built-in Middleware

### Rate Limiting

```rust
use grpc_graphql_gateway::{Gateway, RateLimitMiddleware};
use std::time::Duration;

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .add_middleware(RateLimitMiddleware::new(
        100,                          // Max requests
        Duration::from_secs(60),      // Per time window
    ))
    .build()?;
```

## Custom Middleware

Implement the `Middleware` trait:

```rust
use grpc_graphql_gateway::middleware::{Middleware, Context};
use async_trait::async_trait;
use futures::future::BoxFuture;

struct AuthMiddleware {
    secret_key: String,
}

#[async_trait]
impl Middleware for AuthMiddleware {
    async fn call(
        &self,
        ctx: &mut Context,
        next: Box<dyn Fn(&mut Context) -> BoxFuture<'_, Result<()>>>,
    ) -> Result<()> {
        // Extract token from headers
        let token = ctx.headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Error::Unauthorized)?;
        
        // Validate token
        let user = validate_jwt(token, &self.secret_key)?;
        
        // Add user info to context extensions
        ctx.extensions_mut().insert(user);
        
        // Continue to next middleware/handler
        next(ctx).await
    }
}

let gateway = Gateway::builder()
    .add_middleware(AuthMiddleware { secret_key: "secret".into() })
    .build()?;
```

## Middleware Chain

Middlewares execute in order of registration:

```rust
Gateway::builder()
    .add_middleware(LoggingMiddleware)      // 1st: Log request
    .add_middleware(AuthMiddleware)         // 2nd: Authenticate
    .add_middleware(RateLimitMiddleware)    // 3rd: Rate limit
    .build()?
```

## Context Object

The `Context` provides access to:

| Method | Description |
|--------|-------------|
| `headers()` | HTTP request headers |
| `extensions()` | Shared data between middlewares |
| `extensions_mut()` | Mutable access to extensions |

## Logging Middleware Example

```rust
struct LoggingMiddleware;

#[async_trait]
impl Middleware for LoggingMiddleware {
    async fn call(
        &self,
        ctx: &mut Context,
        next: Box<dyn Fn(&mut Context) -> BoxFuture<'_, Result<()>>>,
    ) -> Result<()> {
        let start = std::time::Instant::now();
        
        let result = next(ctx).await;
        
        tracing::info!(
            duration_ms = start.elapsed().as_millis(),
            success = result.is_ok(),
            "GraphQL request completed"
        );
        
        result
    }
}
```

## Error Handling

Return errors from middleware to reject requests:

```rust
#[async_trait]
impl Middleware for AuthMiddleware {
    async fn call(
        &self,
        ctx: &mut Context,
        next: Box<dyn Fn(&mut Context) -> BoxFuture<'_, Result<()>>>,
    ) -> Result<()> {
        if !self.is_authorized(ctx) {
            return Err(Error::new("Unauthorized").extend_with(|_, e| {
                e.set("code", "UNAUTHORIZED");
            }));
        }
        
        next(ctx).await
    }
}
```

## Error Handler

Set a global error handler for logging or transforming errors:

```rust
Gateway::builder()
    .with_error_handler(|errors| {
        for error in &errors {
            tracing::error!(
                message = %error.message,
                "GraphQL error"
            );
        }
    })
    .build()?
```

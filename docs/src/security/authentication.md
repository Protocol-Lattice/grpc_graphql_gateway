# Authentication

The gateway provides a robust, built-in **Enhanced Authentication Middleware** designed for production use. It supports multiple authentication schemes, flexible token validation, and rich user context propagation.

## Quick Start

```rust
use grpc_graphql_gateway::{
    Gateway, 
    EnhancedAuthMiddleware, 
    AuthConfig, 
    AuthClaims,
    TokenValidator,
    Result
};
use std::sync::Arc;
use async_trait::async_trait;

// 1. Define your token validator
struct MyJwtValidator;

#[async_trait]
impl TokenValidator for MyJwtValidator {
    async fn validate(&self, token: &str) -> Result<AuthClaims> {
        // Implement your JWT validation logic here
        // e.g., decode(token, &decoding_key, &validation)...
        
        Ok(AuthClaims {
            sub: Some("user_123".to_string()),
            roles: vec!["admin".to_string()],
            ..Default::default()
        })
    }
}

// 2. Configure and build the gateway
let auth_middleware = EnhancedAuthMiddleware::new(
    AuthConfig::required()
        .with_scheme(AuthScheme::Bearer),
    Arc::new(MyJwtValidator),
);

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .add_middleware(auth_middleware)
    .build()?;
```

## Configuration

The `AuthConfig` builder allows you to customize how authentication is handled:

```rust
use grpc_graphql_gateway::{AuthConfig, AuthScheme};

let config = AuthConfig::required()
    // Allow multiple schemes
    .with_scheme(AuthScheme::Bearer)
    .with_scheme(AuthScheme::ApiKey)
    .with_api_key_header("x-service-token")
    
    // Public paths that don't need auth
    .skip_path("/health")
    .skip_path("/metrics")
    
    // Whether to require auth for introspection (default: true)
    .with_skip_introspection(false);

// Or create an optional config (allow unauthenticated requests)
let optional_config = AuthConfig::optional();
```

### Supported Schemes

| Scheme | Description | Header Example |
|--------|-------------|----------------|
| `AuthScheme::Bearer` | Standard Bearer token | `Authorization: Bearer <token>` |
| `AuthScheme::Basic` | Basic auth credentials | `Authorization: Basic <base64>` |
| `AuthScheme::ApiKey` | Custom header API key | `x-api-key: <key>` |
| `AuthScheme::Custom` | Custom prefix | `Authorization: Custom <token>` |

## Token Validation

You can implement the `TokenValidator` trait for reusable logic, or use a closure for simple cases.

### Using a Closure

```rust
let auth = EnhancedAuthMiddleware::with_fn(
    AuthConfig::required(),
    |token| Box::pin(async move {
        if token == "secret-password" {
            Ok(AuthClaims {
                sub: Some("admin".to_string()),
                ..Default::default()
            })
        } else {
            Err(Error::Unauthorized("Invalid token".into()))
        }
    })
);
```

## User Context (AuthClaims)

The middleware extracts user information into `AuthClaims`, which are available in the GraphQL context.

| Field | Type | Description |
|-------|------|-------------|
| `sub` | `Option<String>` | Subject (User ID) |
| `roles` | `Vec<String>` | User roles |
| `iss` | `Option<String>` | Issuer |
| `aud` | `Option<Vec<String>>` | Audience |
| `exp` | `Option<i64>` | Expiration (Unix timestamp) |
| `custom` | `HashMap` | Custom claims |

### Accessing Claims in Resolvers

In your custom resolvers or middleware, you can access these claims via the context:

```rust
async fn my_resolver(ctx: &Context) -> Result<String> {
    // Convenience methods
    let user_id = ctx.user_id();     // Option<String>
    let roles = ctx.user_roles();    // Vec<String>
    
    // Check authentication status
    if ctx.get("auth.authenticated") == Some(&serde_json::json!(true)) {
        // ...
    }
    
    // Access full claims
    if let Some(claims) = ctx.get_typed::<AuthClaims>("auth.claims") {
        println!("User: {:?}", claims.sub);
    }
}
```

## Error Handling

- **Missing Token**: If `AuthConfig::required()` is used, returns 401 Unauthorized immediately.
- **Invalid Token**: Returns 401 Unauthorized with error details.
- **Expired Token**: Automatically checks `exp` claim and returns 401 if expired.

To permit unauthenticated access (e.g. for public parts of the graph), use `AuthConfig::optional()`. The request will proceed, but `ctx.user_id()` will be `None`.

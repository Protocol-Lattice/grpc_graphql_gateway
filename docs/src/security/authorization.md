# Authorization

Once a user is authenticated, **Authorization** determines what they are allowed to do. The gateway facilitates this by making user roles and claims available to your resolvers and downstream services.

## Role-Based Access Control (RBAC)

The `AuthClaims` object includes a `roles` field (`Vec<String>`) which works out-of-the-box for RBAC.

### Checking Roles in Logic

You can check roles programmatically within your custom resolvers or middleware:

```rust
async fn delete_user(ctx: &Context, id: String) -> Result<String> {
    let claims = ctx.get_typed::<AuthClaims>("auth.claims")
        .ok_or(Error::Unauthorized("No claims found".into()))?;
        
    if !claims.has_role("admin") {
        return Err(Error::Forbidden("Admins only".into()));
    }
    
    // Proceed with deletion...
}
```

## Propagating Auth to Backends

The most common pattern in a gateway is to offload fine-grained authorization to the backend services. The gateway's job is to securely propagate the identity.

### Header Propagation

You can forward authentication headers directly to your gRPC services:

```rust
// Forward the 'Authorization' header automatically
let gateway = Gateway::builder()
    .with_header_propagation(HeaderPropagationConfig {
        forward_headers: vec!["authorization".to_string()],
        ..Default::default()
    })
    // ...
```

### Metadata Propagation

Alternatively, you can extract claims and inject them as gRPC metadata (headers) for your backends. `EnhancedAuthMiddleware` does not do this automatically, but you can write a custom middleware to run *after* it:

```rust
struct AuthPropagationMiddleware;

#[async_trait]
impl Middleware for AuthPropagationMiddleware {
    async fn call(&self, ctx: &mut Context) -> Result<()> {
        if let Some(user_id) = ctx.user_id() {
            // Add to headers that will be sent to gRPC backend
            ctx.headers.insert("x-user-id", user_id.parse()?);
        }
        
        if let Some(roles) = ctx.get("auth.roles") {
             // Serialize roles to a header
             let roles_str = serde_json::to_string(roles)?;
             ctx.headers.insert("x-user-roles", roles_str.parse()?);
        }
        
        Ok(())
    }
}
```

## Query Whitelisting

For strict control over what operations can be executed, see the [Query Whitelisting](../production/query-whitelisting.md) feature. This acts as a coarse-grained authorization layer, preventing unauthorized query shapes entirely.

## Field-Level Authorization

For advanced field-level authorization (e.g., hiding specific fields based on roles), you currently need to implement this logic in your custom resolvers or within the backend services themselves. The gateway ensures the necessary identity data is present for these decisions to be made.

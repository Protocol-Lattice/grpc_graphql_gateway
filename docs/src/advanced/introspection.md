# Introspection Control

Disable GraphQL introspection in production to prevent schema discovery attacks.

## Disabling Introspection

```rust
let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .disable_introspection()
    .build()?;
```

## What It Blocks

When introspection is disabled, these queries return errors:

```graphql
# Blocked
{
  __schema {
    types {
      name
    }
  }
}

# Blocked
{
  __type(name: "User") {
    fields {
      name
    }
  }
}
```

## Error Response

```json
{
  "errors": [
    {
      "message": "Introspection is disabled",
      "extensions": {
        "code": "INTROSPECTION_DISABLED"
      }
    }
  ]
}
```

## Environment-Based Toggle

Enable introspection only in development:

```rust
let is_production = std::env::var("ENV")
    .map(|e| e == "production")
    .unwrap_or(false);

let mut builder = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS);

if is_production {
    builder = builder.disable_introspection();
}

let gateway = builder.build()?;
```

## Security Benefits

Disabling introspection:
- Prevents attackers from discovering your schema structure
- Reduces attack surface for GraphQL-specific exploits
- Hides internal type names and field descriptions

## When to Disable

| Environment | Introspection |
|-------------|---------------|
| Development | ✅ Enabled |
| Staging | ⚠️ Consider disabling |
| Production | ❌ Disabled |

## Alternative: Authorization

Instead of fully disabling, you can selectively allow introspection:

```rust
struct IntrospectionMiddleware {
    allowed_keys: HashSet<String>,
}

impl Middleware for IntrospectionMiddleware {
    async fn call(&self, ctx: &mut Context, next: ...) -> Result<()> {
        // Check if request is introspection
        if is_introspection_query(ctx) {
            let api_key = ctx.headers().get("x-api-key");
            if !self.allowed_keys.contains(api_key) {
                return Err(Error::new("Introspection not allowed"));
            }
        }
        next(ctx).await
    }
}
```

## See Also

- **[Query Whitelisting](../production/query-whitelisting.md)** - For maximum security, combine introspection control with query whitelisting to restrict both schema discovery and query execution
- [DoS Protection](dos-protection.md) - Query depth and complexity limits

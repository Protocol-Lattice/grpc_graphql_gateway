# Query Whitelisting

Query Whitelisting (also known as **Stored Operations** or **Persisted Operations**) is a critical security feature that restricts which GraphQL queries can be executed. This is essential for public-facing GraphQL APIs and required for many compliance standards.

## Why Query Whitelisting?

### Security Benefits

- **Prevents Arbitrary Queries**: Only pre-approved queries can be executed
- **Reduces Attack Surface**: Prevents schema exploration and DoS attacks
- **Compliance**: Required for PCI-DSS, HIPAA, SOC 2, and other standards
- **Performance**: Known queries can be optimized and monitored
- **Audit Trail**: Track exactly which queries are being used

### Common Use Cases

1. **Public APIs**: Prevent malicious actors from crafting expensive queries
2. **Mobile Applications**: Apps typically have a fixed set of queries
3. **Third-Party Integrations**: Control exactly what partners can query
4. **Compliance Requirements**: Meet security standards for regulated industries

## Configuration

### Basic Setup

```rust
use grpc_graphql_gateway::{Gateway, QueryWhitelistConfig, WhitelistMode};
use std::collections::HashMap;

let mut allowed_queries = HashMap::new();
allowed_queries.insert(
    "getUserById".to_string(),
    "query getUserById($id: ID!) { user(id: $id) { id name } }".to_string()
);

let gateway = Gateway::builder()
    .with_query_whitelist(QueryWhitelistConfig {
        mode: WhitelistMode::Enforce,
        allowed_queries,
        allow_introspection: false,
    })
    .build()?;
```

### Loading from JSON File

For production deployments, it's recommended to load queries from a configuration file:

```rust
let config = QueryWhitelistConfig::from_json_file(
    "config/allowed_queries.json",
    WhitelistMode::Enforce
)?;

let gateway = Gateway::builder()
    .with_query_whitelist(config)
    .build()?;
```

**Example JSON file** (`allowed_queries.json`):

```json
{
  "getUserById": "query getUserById($id: ID!) { user(id: $id) { id name email } }",
  "listProducts": "query { products { id name price } }",
  "createOrder": "mutation createOrder($input: OrderInput!) { createOrder(input: $input) { id } }"
}
```

## Enforcement Modes

### Enforce Mode (Production)

Rejects non-whitelisted queries with an error.

```rust
QueryWhitelistConfig {
    mode: WhitelistMode::Enforce,
    // ...
}
```

**Error response:**
```json
{
  "errors": [{
    "message": "Query not in whitelist: Operation 'unknownQuery' (hash: 1234abcd...)",
    "extensions": {
      "code": "QUERY_NOT_WHITELISTED"
    }
  }]
}
```

### Warn Mode (Staging)

Logs warnings but allows all queries. Useful for testing and identifying missing queries.

```rust
QueryWhitelistConfig {
    mode: WhitelistMode::Warn,
    // ...
}
```

**Server log:**
```
WARN grpc_graphql_gateway::query_whitelist: Query not in whitelist (allowed in Warn mode): Query hash: 0eb2d2f2e9111722
```

### Disabled Mode (Development)

No whitelist checking. Same as not configuring a whitelist.

```rust
QueryWhitelistConfig::disabled()
```

## Validation Methods

The whitelist supports two validation methods that can be used together:

### 1. Hash-Based Validation

Queries are validated by their SHA-256 hash. This is automatic and requires no client changes.

```graphql
# This query's hash is calculated automatically
query { user(id: "123") { name } }
```

### 2. Operation ID Validation

Clients can explicitly reference queries by ID using GraphQL extensions:

**Client request:**
```json
{
  "query": "query getUserById($id: ID!) { user(id: $id) { name } }",
  "variables": {"id": "123"},
  "extensions": {
    "operationId": "getUserById"
  }
}
```

The gateway validates the `operationId` against the whitelist.

## Introspection Control

You can optionally allow introspection queries even in Enforce mode:

```rust
QueryWhitelistConfig {
    mode: WhitelistMode::Enforce,
    allowed_queries: queries,
    allow_introspection: true,  // Allow __schema and __type queries
}
```

This is useful for development and staging environments where developers need to explore the schema.

## Runtime Management

The whitelist supports runtime modifications for dynamic use cases:

```rust
// Get whitelist reference
let whitelist = gateway.mux().query_whitelist().unwrap();

// Register new query at runtime
whitelist.register_query(
    "newQuery".to_string(),
    "query { newField }".to_string()
);

// Remove a query
whitelist.remove_query("oldQuery");

// Get statistics
let stats = whitelist.stats();
println!("Total allowed queries: {}", stats.total_queries);
println!("Mode: {:?}", stats.mode);
```

## Best Practices

### 1. Use Enforce Mode in Production

Always use `WhitelistMode::Enforce` in production environments:

```rust
let mode = if std::env::var("ENV")? == "production" {
    WhitelistMode::Enforce
} else {
    WhitelistMode::Warn
};
```

### 2. Start with Warn Mode

When first implementing whitelisting:

1. Deploy with `Warn` mode in staging
2. Monitor logs to identify all queries
3. Add missing queries to whitelist
4. Switch to `Enforce` mode once complete

### 3. Version Control Your Whitelist

Store `allowed_queries.json` in version control alongside your application code.

### 4. Automated Query Extraction

For frontend applications, consider using tools to automatically extract queries from your codebase:

- **GraphQL Code Generator**: Extract queries from React/Vue components
- **Apollo CLI**: Generate persisted query manifests
- **Relay Compiler**: Built-in persisted query support

### 5. CI/CD Integration

Validate the whitelist file in your CI pipeline:

```bash
# Validate JSON syntax
jq empty allowed_queries.json

# Run gateway with test queries
cargo test --test query_whitelist_validation
```

## Working with APQ

Query Whitelisting and Automatic Persisted Queries (APQ) serve different purposes and work well together:

| Feature | Purpose | Security Level |
|---------|---------|----------------|
| **APQ** | Bandwidth optimization (caches any query) | Low |
| **Whitelist** | Security (only allows pre-approved queries) | High |
| **Both** | Bandwidth savings + Security | Maximum |

**Example configuration with both:**

```rust
Gateway::builder()
    // APQ for bandwidth optimization
    .with_persisted_queries(PersistedQueryConfig {
        cache_size: 1000,
        ttl: Some(Duration::from_secs(3600)),
    })
    // Whitelist for security
    .with_query_whitelist(QueryWhitelistConfig {
        mode: WhitelistMode::Enforce,
        allowed_queries: load_queries()?,
        allow_introspection: false,
    })
    .build()?
```

## Migration Guide

### Step 1: Inventory Queries

Use Warn mode to identify all queries currently in use:

```rust
.with_query_whitelist(QueryWhitelistConfig {
    mode: WhitelistMode::Warn,
    allowed_queries: HashMap::new(),
    allow_introspection: true,
})
```

Monitor logs for 1-2 weeks to capture all query variations.

### Step 2: Build Whitelist

Extract unique query hashes from logs and build your whitelist file.

### Step 3: Test in Staging

Deploy with the whitelist in Warn mode to staging:

```bash
# Monitor for any warnings
grep "Query not in whitelist" /var/log/gateway.log
```

### Step 4: Production Deployment

Once confident, switch to Enforce mode:

```rust
.with_query_whitelist(QueryWhitelistConfig {
    mode: WhitelistMode::Enforce,
    allowed_queries: load_queries()?,
    allow_introspection: false,  // Disable in production
})
```

## Troubleshooting

### Query Rejected Despite Being in Whitelist

**Problem**: Query is in the whitelist but still gets rejected.

**Solution**: Ensure the query string exactly matches, including whitespace. Consider normalizing queries or using operation IDs.

### Too Many Warnings in Warn Mode

**Problem**: Logs are flooded with warnings.

**Solution**: This is expected when first implementing. Collect all unique queries and add them to the whitelist.

### Performance Impact

**Problem**: Concerned about validation overhead.

**Solution**: Hash calculation is fast (SHA-256). For 1000 RPS, overhead is <1ms. Consider caching if needed.

## Example: Complete Production Setup

```rust
use grpc_graphql_gateway::{Gateway, QueryWhitelistConfig, WhitelistMode};
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Determine mode from environment
    let is_production = std::env::var("ENV")
        .map(|e| e == "production")
        .unwrap_or(false);
    
    // Load whitelist configuration
    let whitelist_config = if Path::new("config/allowed_queries.json").exists() {
        QueryWhitelistConfig::from_json_file(
            "config/allowed_queries.json",
            if is_production {
                WhitelistMode::Enforce
            } else {
                WhitelistMode::Warn
            }
        )?
    } else {
        QueryWhitelistConfig::disabled()
    };
    
    // Build gateway with production settings
    let gateway = Gateway::builder()
        .with_descriptor_set_bytes(DESCRIPTORS)
        .with_query_whitelist(whitelist_config)
        .with_response_cache(CacheConfig::default())
        .with_circuit_breaker(CircuitBreakerConfig::default())
        .with_compression(CompressionConfig::default())
        .build()?;
    
    gateway.serve("0.0.0.0:8888").await?;
    Ok(())
}
```

## See Also

- [Introspection Control](../advanced/introspection.md) - Disabling schema introspection
- [Automatic Persisted Queries](../performance/apq.md) - Bandwidth optimization
- [DoS Protection](./dos-protection.md) - Query depth and complexity limits

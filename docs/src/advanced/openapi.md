# OpenAPI Integration

The gateway can automatically generate REST connectors from OpenAPI (Swagger) specification files. This enables quick integration of REST APIs without manual endpoint configuration.

## Supported Formats

| Format | Extension | Feature Required |
|--------|-----------|------------------|
| OpenAPI 3.0.x | `.json` | None |
| OpenAPI 3.1.x | `.json` | None |
| Swagger 2.0 | `.json` | None |
| YAML (any version) | `.yaml`, `.yml` | `yaml` |

## Quick Start

```rust
use grpc_graphql_gateway::{Gateway, OpenApiParser};

// Parse OpenAPI spec and create REST connector
let connector = OpenApiParser::from_file("petstore.yaml")?
    .with_base_url("https://api.petstore.io/v2")
    .build()?;

let gateway = Gateway::builder()
    .add_rest_connector("petstore", connector)
    .build()?;
```

## Loading Options

### From a File

```rust
// JSON file
let connector = OpenApiParser::from_file("api.json")?.build()?;

// YAML file (requires 'yaml' feature)
let connector = OpenApiParser::from_file("api.yaml")?.build()?;
```

### From a URL

```rust
let connector = OpenApiParser::from_url("https://api.example.com/openapi.json")
    .await?
    .build()?;
```

### From a String

```rust
let json_content = r#"{"openapi": "3.0.0", ...}"#;
let connector = OpenApiParser::from_string(json_content, false)?.build()?;

// For YAML content
let yaml_content = "openapi: '3.0.0'\n...";
let connector = OpenApiParser::from_string(yaml_content, true)?.build()?;
```

### From JSON Value

```rust
let json_value: serde_json::Value = serde_json::from_str(content)?;
let connector = OpenApiParser::from_json(json_value)?.build()?;
```

## Configuration Options

### Base URL Override

Override the server URL from the spec:

```rust
let connector = OpenApiParser::from_file("api.json")?
    .with_base_url("https://api.staging.example.com")  // Use staging
    .build()?;
```

### Timeout

Set a default timeout for all endpoints:

```rust
use std::time::Duration;

let connector = OpenApiParser::from_file("api.json")?
    .with_timeout(Duration::from_secs(60))
    .build()?;
```

### Operation Prefix

Add a prefix to all operation names to avoid conflicts:

```rust
let connector = OpenApiParser::from_file("petstore.json")?
    .with_prefix("petstore_")  // listPets -> petstore_listPets
    .build()?;
```

## Filtering Operations

### By Tags

Only include operations with specific tags:

```rust
let connector = OpenApiParser::from_file("api.json")?
    .with_tags(vec!["pets".to_string(), "store".to_string()])
    .build()?;
```

### Custom Filter

Use a predicate function for fine-grained control:

```rust
let connector = OpenApiParser::from_file("api.json")?
    .filter_operations(|operation_id, path| {
        // Only include non-deprecated v2 endpoints
        !operation_id.contains("deprecated") && path.starts_with("/api/v2")
    })
    .build()?;
```

## What Gets Generated

The parser automatically generates:

### Endpoints

Each path operation becomes a GraphQL field:

| OpenAPI | GraphQL |
|---------|---------|
| `GET /pets` | `listPets` query |
| `POST /pets` | `createPet` mutation |
| `GET /pets/{petId}` | `getPet` query |
| `DELETE /pets/{petId}` | `deletePet` mutation |

### Arguments

- **Path parameters** → Required field arguments
- **Query parameters** → Optional field arguments
- **Request body** → Input arguments (auto-templated)

### Response Types

Response schemas are converted to GraphQL types:

```yaml
# OpenAPI
Pet:
  type: object
  properties:
    id:
      type: integer
    name:
      type: string
    tag:
      type: string
```

Becomes a GraphQL type with field selection.

## Listing Operations

Before building, you can list all available operations:

```rust
let parser = OpenApiParser::from_file("api.json")?;

for op in parser.list_operations() {
    println!("{}: {} {} (tags: {:?})", 
        op.operation_id, 
        op.method, 
        op.path,
        op.tags
    );
    if let Some(summary) = op.summary {
        println!("  {}", summary);
    }
}
```

## Accessing Spec Information

```rust
let parser = OpenApiParser::from_file("api.json")?;

let info = parser.info();
println!("API: {} v{}", info.title, info.version);
if let Some(desc) = &info.description {
    println!("Description: {}", desc);
}
```

## YAML Support

To enable YAML parsing, add the `yaml` feature:

```toml
[dependencies]
grpc_graphql_gateway = { version = "0.3", features = ["yaml"] }
```

## Example: Petstore Integration

```rust
use grpc_graphql_gateway::{Gateway, OpenApiParser};
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse the Petstore OpenAPI spec
    let petstore = OpenApiParser::from_url(
        "https://petstore3.swagger.io/api/v3/openapi.json"
    )
    .await?
    .with_base_url("https://petstore3.swagger.io/api/v3")
    .with_timeout(Duration::from_secs(30))
    .with_tags(vec!["pet".to_string()])  // Only pet operations
    .with_prefix("pet_")                  // Namespace operations
    .build()?;

    // Create the gateway
    let gateway = Gateway::builder()
        .with_descriptor_set_bytes(DESCRIPTORS)
        .add_grpc_client("service", grpc_client)
        .add_rest_connector("petstore", petstore)
        .serve("0.0.0.0:8888".to_string())
        .await?;

    Ok(())
}
```

## Multiple REST APIs

Combine multiple OpenAPI specs:

```rust
// Payment API
let stripe = OpenApiParser::from_file("stripe-openapi.json")?
    .with_prefix("stripe_")
    .build()?;

// Email API
let sendgrid = OpenApiParser::from_file("sendgrid-openapi.json")?
    .with_prefix("email_")
    .build()?;

// User service (gRPC)
let gateway = Gateway::builder()
    .with_descriptor_set_bytes(USER_DESCRIPTORS)
    .add_grpc_client("users", users_client)
    .add_rest_connector("stripe", stripe)
    .add_rest_connector("sendgrid", sendgrid)
    .build()?;
```

## Best Practices

1. **Use prefixes** when combining multiple APIs to avoid naming conflicts

2. **Filter by tags** to include only the operations you need

3. **Override base URLs** for different environments (dev, staging, prod)

4. **Check available operations** before building to understand what will be generated

5. **Enable YAML** feature only if you need it (adds serde_yaml dependency)

## Limitations

- **Authentication**: OpenAPI security schemes are not automatically applied. Use request interceptors for auth.
- **Complex schemas**: Very complex schemas (allOf, oneOf, anyOf) may be simplified.
- **Webhooks**: OpenAPI 3.1 webhooks are not supported.
- **Callbacks**: Async callbacks are not supported.

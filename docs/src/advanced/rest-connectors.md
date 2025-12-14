# REST API Connectors

The gateway supports **REST API Connectors**, enabling hybrid architectures where GraphQL fields can resolve data from both gRPC services and REST APIs. This is perfect for gradual migrations, integrating third-party APIs, or bridging legacy systems.

## Quick Start

```rust
use grpc_graphql_gateway::{Gateway, RestConnector, RestEndpoint, HttpMethod};
use std::time::Duration;

let rest_connector = RestConnector::builder()
    .base_url("https://api.example.com")
    .timeout(Duration::from_secs(30))
    .default_header("Accept", "application/json")
    .add_endpoint(RestEndpoint::new("getUser", "/users/{id}")
        .method(HttpMethod::GET)
        .response_path("$.data")
        .description("Fetch a user by ID"))
    .add_endpoint(RestEndpoint::new("createUser", "/users")
        .method(HttpMethod::POST)
        .body_template(r#"{"name": "{name}", "email": "{email}"}"#))
    .build()?;

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .add_rest_connector("users_api", rest_connector)
    .add_grpc_client("UserService", grpc_client)
    .build()?;
```

## GraphQL Schema Integration

REST endpoints are **automatically exposed as GraphQL fields**. The gateway generates:
- **Query fields** for GET endpoints
- **Mutation fields** for POST/PUT/PATCH/DELETE endpoints

Field names use the endpoint name directly (e.g., `getUser`, `createPost`).

### Example GraphQL Queries

```graphql
# Query a REST endpoint (GET /users/{id})
query {
  getUser(id: "123")
}

# Mutation to create via REST (POST /users)
mutation {
  createUser(name: "Alice", email: "alice@example.com")
}
```

### Example Response

```json
{
  "data": {
    "getUser": {
      "id": 123,
      "name": "Alice",
      "email": "alice@example.com"
    }
  }
}
```

REST responses are returned as the `JSON` scalar type, preserving the full structure from the API.

## RestConnector

The `RestConnector` is the main entry point for REST API integration.

### Builder Methods

| Method | Description |
|--------|-------------|
| `base_url(url)` | **Required**. Base URL for all endpoints |
| `timeout(duration)` | Default timeout (default: 30s) |
| `default_header(key, value)` | Add header to all requests |
| `retry(config)` | Custom retry configuration |
| `no_retry()` | Disable retries |
| `log_bodies(true)` | Enable request/response body logging |
| `with_cache(size)` | Enable LRU response cache for GET requests |
| `interceptor(interceptor)` | Add request interceptor |
| `transformer(transformer)` | Custom response transformer |
| `add_endpoint(endpoint)` | Add a REST endpoint |

## RestEndpoint

Define individual REST endpoints with flexible configuration.

```rust
use grpc_graphql_gateway::{RestEndpoint, HttpMethod};

let endpoint = RestEndpoint::new("getUser", "/users/{id}")
    .method(HttpMethod::GET)
    .header("X-Custom-Header", "value")
    .query_param("include", "profile")
    .response_path("$.data.user")
    .timeout(Duration::from_secs(10))
    .description("Fetch a user by ID")
    .return_type("User");
```

### Path Templates

Use `{variable}` placeholders in paths:

```rust
RestEndpoint::new("getOrder", "/users/{userId}/orders/{orderId}")
```

When called with `{ "userId": "123", "orderId": "456" }`, resolves to:
```
/users/123/orders/456
```

### Query Parameters

Add templated query parameters:

```rust
RestEndpoint::new("searchUsers", "/users")
    .query_param("q", "{query}")
    .query_param("limit", "{limit}")
```

### Body Templates

For POST/PUT/PATCH, define request body templates:

```rust
RestEndpoint::new("createUser", "/users")
    .method(HttpMethod::POST)
    .body_template(r#"{
        "name": "{name}",
        "email": "{email}",
        "role": "{role}"
    }"#)
```

If no body template is provided, arguments are automatically serialized as JSON.

### Response Extraction

Extract nested data from responses using JSONPath:

```rust
// API returns: { "status": "ok", "data": { "user": { "id": "123" } } }
RestEndpoint::new("getUser", "/users/{id}")
    .response_path("$.data.user")  // Returns just the user object
```

Supported JSONPath:
- `$.field` - Access field
- `$.field.nested` - Nested access
- `$.array[0]` - Array index
- `$.array[0].field` - Combined

## Typed Responses

By default, REST endpoints return a `JSON` scalar blob. To enable **field selection** in GraphQL queries (e.g. `{ getUser { name email } }`), you can define a response schema:

```rust
use grpc_graphql_gateway::{RestResponseSchema, RestResponseField};

RestEndpoint::new("getUser", "/users/{id}")
    .with_response_schema(RestResponseSchema::new("User")
        .field(RestResponseField::int("id"))
        .field(RestResponseField::string("name"))
        .field(RestResponseField::string("email"))
        // Define a nested object field
        .field(RestResponseField::object("address", "Address"))
    )
```

This registers a `User` type in the schema and allows clients to select only the fields they need.

### Mutations vs Queries

Endpoints are automatically classified:
- **Queries**: GET requests
- **Mutations**: POST, PUT, PATCH, DELETE

Override explicitly:

```rust
// Force a POST to be a query (e.g., search endpoint)
RestEndpoint::new("searchUsers", "/users/search")
    .method(HttpMethod::POST)
    .as_query()
```

## HTTP Methods

```rust
use grpc_graphql_gateway::HttpMethod;

HttpMethod::GET      // Read operations
HttpMethod::POST     // Create operations
HttpMethod::PUT      // Full update
HttpMethod::PATCH    // Partial update
HttpMethod::DELETE   // Delete operations
```

## Authentication

### Bearer Token

```rust
use grpc_graphql_gateway::{RestConnector, BearerAuthInterceptor};
use std::sync::Arc;

let connector = RestConnector::builder()
    .base_url("https://api.example.com")
    .interceptor(Arc::new(BearerAuthInterceptor::new("your-token")))
    .build()?;
```

The interceptor adds: `Authorization: Bearer your-token`

### API Key

```rust
use grpc_graphql_gateway::{RestConnector, ApiKeyInterceptor};
use std::sync::Arc;

let connector = RestConnector::builder()
    .base_url("https://api.example.com")
    .interceptor(Arc::new(ApiKeyInterceptor::x_api_key("your-api-key")))
    .build()?;
```

The interceptor adds: `X-API-Key: your-api-key`

### Custom Interceptor

Implement the `RequestInterceptor` trait for custom auth:

```rust
use grpc_graphql_gateway::{RequestInterceptor, RestRequest, Result};
use async_trait::async_trait;

struct CustomAuthInterceptor {
    // Your auth logic
}

#[async_trait]
impl RequestInterceptor for CustomAuthInterceptor {
    async fn intercept(&self, request: &mut RestRequest) -> Result<()> {
        // Add custom headers, modify URL, etc.
        request.headers.insert(
            "X-Custom-Auth".to_string(),
            "custom-value".to_string()
        );
        Ok(())
    }
}
```

## Retry Configuration

Configure automatic retries with exponential backoff:

```rust
use grpc_graphql_gateway::{RestConnector, RetryConfig};
use std::time::Duration;

let connector = RestConnector::builder()
    .base_url("https://api.example.com")
    .retry(RetryConfig {
        max_retries: 3,
        initial_backoff: Duration::from_millis(100),
        max_backoff: Duration::from_secs(10),
        multiplier: 2.0,
        retry_statuses: vec![429, 500, 502, 503, 504],
    })
    .build()?;
```

### Preset Configurations

```rust
// Disable retries
RetryConfig::disabled()

// Aggressive retries for critical endpoints
RetryConfig::aggressive()
```

## Response Caching

Enable LRU caching for GET requests:

```rust
let connector = RestConnector::builder()
    .base_url("https://api.example.com")
    .with_cache(1000)  // Cache up to 1000 responses
    .build()?;

// Clear cache manually
connector.clear_cache().await;
```

Cache keys are based on endpoint name + arguments.

## Multiple Connectors

Register multiple REST connectors for different services:

```rust
let users_api = RestConnector::builder()
    .base_url("https://users.example.com")
    .add_endpoint(RestEndpoint::new("getUser", "/users/{id}"))
    .build()?;

let products_api = RestConnector::builder()
    .base_url("https://products.example.com")
    .add_endpoint(RestEndpoint::new("getProduct", "/products/{id}"))
    .build()?;

let orders_api = RestConnector::builder()
    .base_url("https://orders.example.com")
    .add_endpoint(RestEndpoint::new("getOrder", "/orders/{id}"))
    .build()?;

let gateway = Gateway::builder()
    .add_rest_connector("users", users_api)
    .add_rest_connector("products", products_api)
    .add_rest_connector("orders", orders_api)
    .build()?;
```

## Executing Endpoints

Execute endpoints programmatically:

```rust
use std::collections::HashMap;
use serde_json::json;

let mut args = HashMap::new();
args.insert("id".to_string(), json!("123"));

let result = connector.execute("getUser", args).await?;
```

## Custom Response Transformer

Transform responses before returning to GraphQL:

```rust
use grpc_graphql_gateway::{ResponseTransformer, RestResponse, Result};
use async_trait::async_trait;
use serde_json::Value as JsonValue;
use std::sync::Arc;

struct SnakeToCamelTransformer;

#[async_trait]
impl ResponseTransformer for SnakeToCamelTransformer {
    async fn transform(&self, endpoint: &str, response: RestResponse) -> Result<JsonValue> {
        // Transform snake_case keys to camelCase
        Ok(transform_keys(response.body))
    }
}

let connector = RestConnector::builder()
    .base_url("https://api.example.com")
    .transformer(Arc::new(SnakeToCamelTransformer))
    .build()?;
```

## Use Cases

| Scenario | Description |
|----------|-------------|
| **Hybrid Architecture** | Mix gRPC and REST backends in one GraphQL API |
| **Gradual Migration** | Migrate from REST to gRPC incrementally |
| **Third-Party APIs** | Integrate external REST APIs (Stripe, Twilio, etc.) |
| **Legacy Systems** | Bridge legacy REST services with modern infrastructure |
| **Multi-Protocol** | Support teams using different backend technologies |

## Best Practices

1. **Set Appropriate Timeouts**: Use shorter timeouts for internal services, longer for external APIs.

2. **Enable Retries for Idempotent Operations**: GET, PUT, DELETE are typically safe to retry.

3. **Use Response Extraction**: Extract only needed data with `response_path` to reduce payload size.

4. **Cache Read-Heavy Endpoints**: Enable caching for frequently-accessed, rarely-changing data.

5. **Secure Credentials**: Use environment variables for API keys and tokens, not hardcoded values.

6. **Log Bodies in Development Only**: Enable `log_bodies` only in development to avoid leaking sensitive data.

## See Also

- [gRPC Client Configuration](../getting-started/installation.md)
- [Header Propagation](../production/header-propagation.md)
- [Response Caching](./caching.md)

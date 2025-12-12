# OpenTelemetry Tracing

Enable distributed tracing for end-to-end visibility across your system.

## Setting Up Tracing

```rust
use grpc_graphql_gateway::{Gateway, TracingConfig, init_tracer, shutdown_tracer};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize the tracer
    let config = TracingConfig::new()
        .with_service_name("my-gateway")
        .with_sample_ratio(1.0);  // Sample all requests

    let _provider = init_tracer(&config);

    let gateway = Gateway::builder()
        .with_descriptor_set_bytes(DESCRIPTORS)
        .enable_tracing()
        .build()?;

    gateway.serve("0.0.0.0:8888").await?;

    // Shutdown on exit
    shutdown_tracer();
    Ok(())
}
```

## Spans Created

| Span | Kind | Description |
|------|------|-------------|
| `graphql.query` | Server | GraphQL query operation |
| `graphql.mutation` | Server | GraphQL mutation operation |
| `grpc.call` | Client | gRPC backend call |

## Span Attributes

### GraphQL Spans

| Attribute | Description |
|-----------|-------------|
| `graphql.operation.name` | The operation name if provided |
| `graphql.operation.type` | query, mutation, or subscription |
| `graphql.document` | The GraphQL query (truncated) |

### gRPC Spans

| Attribute | Description |
|-----------|-------------|
| `rpc.service` | gRPC service name |
| `rpc.method` | gRPC method name |
| `rpc.grpc.status_code` | gRPC status code |

## OTLP Export

Enable OTLP export by adding the feature:

```toml
[dependencies]
grpc_graphql_gateway = { version = "0.2", features = ["otlp"] }
```

Then configure the exporter:

```rust
use grpc_graphql_gateway::TracingConfig;

let config = TracingConfig::new()
    .with_service_name("my-gateway")
    .with_otlp_endpoint("http://jaeger:4317");
```

## Jaeger Integration

Run Jaeger locally:

```bash
docker run -d --name jaeger \
  -p 4317:4317 \
  -p 16686:16686 \
  jaegertracing/jaeger:1.47
```

View traces at: `http://localhost:16686`

## Sampling Configuration

| Sample Ratio | Description |
|--------------|-------------|
| `1.0` | Sample all requests (dev) |
| `0.1` | Sample 10% (staging) |
| `0.01` | Sample 1% (production) |

```rust
TracingConfig::new()
    .with_sample_ratio(0.1)  // 10% sampling
```

## Context Propagation

The gateway automatically propagates trace context:
- Incoming HTTP headers (`traceparent`, `tracestate`)
- Outgoing gRPC metadata

Enable [Header Propagation](../production/header-propagation.md) for distributed tracing headers.

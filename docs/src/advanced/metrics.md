# Prometheus Metrics

Enable a `/metrics` endpoint exposing Prometheus-compatible metrics.

## Enabling Metrics

```rust
let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .enable_metrics()
    .build()?;
```

## Available Metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `graphql_requests_total` | Counter | `operation_type` | Total GraphQL requests |
| `graphql_request_duration_seconds` | Histogram | `operation_type` | Request latency |
| `graphql_errors_total` | Counter | `error_type` | Total GraphQL errors |
| `grpc_backend_requests_total` | Counter | `service`, `method` | gRPC backend calls |
| `grpc_backend_duration_seconds` | Histogram | `service`, `method` | gRPC latency |

## Prometheus Scrape Configuration

```yaml
scrape_configs:
  - job_name: 'graphql-gateway'
    static_configs:
      - targets: ['gateway:8888']
    metrics_path: '/metrics'
    scrape_interval: 15s
```

## Example Metrics Output

```prometheus
# HELP graphql_requests_total Total number of GraphQL requests
# TYPE graphql_requests_total counter
graphql_requests_total{operation_type="query"} 1523
graphql_requests_total{operation_type="mutation"} 234
graphql_requests_total{operation_type="subscription"} 56

# HELP graphql_request_duration_seconds Request duration in seconds
# TYPE graphql_request_duration_seconds histogram
graphql_request_duration_seconds_bucket{operation_type="query",le="0.01"} 1200
graphql_request_duration_seconds_bucket{operation_type="query",le="0.05"} 1480
graphql_request_duration_seconds_bucket{operation_type="query",le="0.1"} 1510
graphql_request_duration_seconds_bucket{operation_type="query",le="+Inf"} 1523

# HELP grpc_backend_requests_total Total gRPC backend calls
# TYPE grpc_backend_requests_total counter
grpc_backend_requests_total{service="UserService",method="GetUser"} 892
grpc_backend_requests_total{service="ProductService",method="GetProduct"} 631
```

## Grafana Dashboard

Create dashboards for:

- Request rate and latency percentiles
- Error rates by type
- gRPC backend health
- Operation type distribution

### Example Queries

**Request Rate:**
```promql
rate(graphql_requests_total[5m])
```

**P99 Latency:**
```promql
histogram_quantile(0.99, rate(graphql_request_duration_seconds_bucket[5m]))
```

**Error Rate:**
```promql
rate(graphql_errors_total[5m]) / rate(graphql_requests_total[5m])
```

## Programmatic Access

Use the metrics API directly:

```rust
use grpc_graphql_gateway::{GatewayMetrics, RequestTimer};

// Record custom metrics
let timer = GatewayMetrics::global().start_request_timer("query");
// ... process request
timer.observe_duration();

// Record gRPC calls
let grpc_timer = GatewayMetrics::global().start_grpc_timer("UserService", "GetUser");
// ... make gRPC call
grpc_timer.observe_duration();
```

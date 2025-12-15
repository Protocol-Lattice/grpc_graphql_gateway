# Configuration Reference

Complete reference for all gateway configuration options.

## GatewayBuilder Methods

### Core Configuration

| Method | Description |
|--------|-------------|
| `.with_descriptor_set_bytes(bytes)` | Set primary proto descriptor |
| `.add_descriptor_set_bytes(bytes)` | Add additional proto descriptor |
| `.with_descriptor_set_file(path)` | Load primary descriptor from file |
| `.add_descriptor_set_file(path)` | Load additional descriptor from file |
| `.add_grpc_client(name, client)` | Register a gRPC backend client |
| `.with_services(services)` | Restrict to specific services |

### Federation

| Method | Description |
|--------|-------------|
| `.enable_federation()` | Enable Apollo Federation v2 |
| `.with_entity_resolver(resolver)` | Custom entity resolver |

### Security

| Method | Description |
|--------|-------------|
| `.with_query_depth_limit(n)` | Max query nesting depth |
| `.with_query_complexity_limit(n)` | Max query complexity |
| `.disable_introspection()` | Block `__schema` queries |

### Middleware

| Method | Description |
|--------|-------------|
| `.add_middleware(middleware)` | Add custom middleware |
| `.with_error_handler(handler)` | Custom error handler |

### Performance

| Method | Description |
|--------|-------------|
| `.with_response_cache(config)` | Enable response caching |
| `.with_compression(config)` | Enable response compression |
| `.with_persisted_queries(config)` | Enable APQ |
| `.with_circuit_breaker(config)` | Enable circuit breaker |

### Production

| Method | Description |
|--------|-------------|
| `.enable_health_checks()` | Add `/health` and `/ready` endpoints |
| `.enable_metrics()` | Add `/metrics` Prometheus endpoint |
| `.enable_tracing()` | Enable OpenTelemetry tracing |
| `.with_graceful_shutdown(config)` | Enable graceful shutdown |
| `.with_header_propagation(config)` | Forward headers to gRPC |

## Environment Variables

Configure via environment variables:

```bash
# Query limits
QUERY_DEPTH_LIMIT=10
QUERY_COMPLEXITY_LIMIT=100

# Environment
ENV=production  # Affects introspection default

# Tracing
OTEL_EXPORTER_OTLP_ENDPOINT=http://jaeger:4317
OTEL_SERVICE_NAME=graphql-gateway
```

## Configuration Structs

### CacheConfig

```rust
CacheConfig {
    max_size: 10_000,
    default_ttl: Duration::from_secs(60),
    stale_while_revalidate: Some(Duration::from_secs(30)),
    invalidate_on_mutation: true,
    redis_url: Some("redis://127.0.0.1:6379".to_string()),
}
```

### CompressionConfig

```rust
CompressionConfig {
    enabled: true,
    level: CompressionLevel::Default,
    min_size_bytes: 1024,
    algorithms: vec!["br".into(), "gzip".into()],
}

// Presets
CompressionConfig::fast()
CompressionConfig::best()
CompressionConfig::default()
CompressionConfig::disabled()
```

### CircuitBreakerConfig

```rust
CircuitBreakerConfig {
    failure_threshold: 5,
    recovery_timeout: Duration::from_secs(30),
    half_open_max_requests: 3,
}
```

### PersistedQueryConfig

```rust
PersistedQueryConfig {
    cache_size: 1000,
    ttl: Some(Duration::from_secs(3600)),
}
```

### ShutdownConfig

```rust
ShutdownConfig {
    timeout: Duration::from_secs(30),
    handle_signals: true,
    force_shutdown_delay: Duration::from_secs(5),
}
```

### HeaderPropagationConfig

```rust
HeaderPropagationConfig::new()
    .propagate("authorization")
    .propagate_with_prefix("x-custom-")
    .exclude("cookie")

// Preset
HeaderPropagationConfig::common()
```

### TracingConfig

```rust
TracingConfig::new()
    .with_service_name("my-gateway")
    .with_sample_ratio(0.1)
    .with_otlp_endpoint("http://jaeger:4317")
```

# Circuit Breaker

Protect your gateway from cascading failures when backend services are unhealthy.

## Enabling Circuit Breaker

```rust
use grpc_graphql_gateway::{Gateway, CircuitBreakerConfig};
use std::time::Duration;

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .with_circuit_breaker(CircuitBreakerConfig {
        failure_threshold: 5,                      // Open after 5 failures
        recovery_timeout: Duration::from_secs(30), // Wait 30s before testing
        half_open_max_requests: 3,                 // Allow 3 test requests
    })
    .build()?;
```

## Circuit States

```
   ┌─────────────────────────────────────────────────┐
   │                                                 │
   ▼                                                 │
┌──────┐  failure_threshold  ┌──────┐  recovery   ┌─────────┐
│CLOSED│ ─────────────────▶  │ OPEN │ ──────────▶ │HALF-OPEN│
└──────┘     reached         └──────┘   timeout   └─────────┘
   ▲                                                 │
   │         success                                 │
   └─────────────────────────────────────────────────┘
```

| State | Description |
|-------|-------------|
| **Closed** | Normal operation, all requests flow through |
| **Open** | Service unhealthy, requests fail fast |
| **Half-Open** | Testing recovery with limited requests |

## Configuration Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `failure_threshold` | u32 | 5 | Consecutive failures to open circuit |
| `recovery_timeout` | Duration | 30s | Time before testing recovery |
| `half_open_max_requests` | u32 | 3 | Test requests in half-open state |

## How It Works

1. **Closed**: Requests flow normally, failures are counted
2. **Threshold reached**: Circuit opens after N consecutive failures
3. **Open**: Requests fail immediately with `SERVICE_UNAVAILABLE`
4. **Timeout**: After recovery timeout, circuit enters half-open
5. **Half-Open**: Limited requests test if service recovered
6. **Success**: Circuit closes, normal operation resumes
7. **Failure**: Circuit reopens, back to step 3

## Error Response

When circuit is open:

```json
{
  "errors": [
    {
      "message": "Service unavailable: circuit breaker is open",
      "extensions": {
        "code": "SERVICE_UNAVAILABLE",
        "service": "UserService"
      }
    }
  ]
}
```

## Per-Service Circuits

Each gRPC service has its own circuit breaker:

- `UserService` circuit open doesn't affect `ProductService`
- Failures are isolated to their respective services

## Benefits

- ✅ Prevents cascading failures
- ✅ Fast-fail reduces latency when services are down
- ✅ Automatic recovery testing
- ✅ Per-service isolation

## Monitoring

Track circuit breaker state through logs:

```
WARN Circuit breaker opened for UserService
INFO Circuit breaker half-open for UserService (testing recovery)
INFO Circuit breaker closed for UserService (service recovered)
```

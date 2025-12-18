# LZ4 Ultra-Fast Compression

LZ4 is an extremely fast compression algorithm ideal for high-throughput scenarios where CPU time is more valuable than bandwidth.

## Benefits

| Feature | LZ4 | Gzip | Brotli |
|---------|-----|------|--------|
| **Compression Speed** | **700 MB/s** | 35 MB/s | 8 MB/s |
| **Decompression Speed** | **4 GB/s** | 300 MB/s | 400 MB/s |
| **Compression Ratio** | 50-60% | 70-80% | 75-85% |
| **CPU Usage** | **Very Low** | Medium | High |
| **Best For** | **High throughput, real-time** | General use | Bandwidth-constrained |

## When to Use LZ4

✅ **Use LZ4 when:**
- **High throughput** (100k+ req/s)
- **Low latency** is critical (< 10ms P99)
- **CPU is more expensive than bandwidth**
- **Real-time applications**
- **Internal APIs** (microservices communication)

❌ **Don't use LZ4 when:**
- Bandwidth is extremely expensive
- Users on slow connections (use Brotli)
- Maximum compression ratio needed

## Quick Start

### Basic Configuration

```rust
use grpc_graphql_gateway::{Gateway, CompressionConfig};

let gateway = Gateway::builder()
    .with_compression(CompressionConfig::ultra_fast())  // LZ4!
    .build()?;
```

### Advanced Configuration

```rust
use grpc_graphql_gateway::CompressionConfig;

let config = CompressionConfig {
    enabled: true,
    level: CompressionLevel::Fast,
    min_size_bytes: 256,  // Lower threshold for LZ4
    algorithms: vec!["lz4".into()],
};
```

### Multi-Algorithm Support

```rust
// Prefer LZ4, fallback to gzip for browsers
let config = CompressionConfig {
    algorithms: vec![
        "lz4".into(),    // For high-performance clients
        "gzip".into(),   // For browsers
    ],
    ..Default::default()
};
```

## Client Support

### JavaScript/TypeScript

```typescript
// Axios example
import axios from 'axios';

const client = axios.create({
  baseURL: 'http://localhost:8888/graphql',
  headers: {
    'Accept-Encoding': 'lz4, gzip, deflate',
  },
  // Add LZ4 decompression
  transformResponse: [(data) => {
    // Handle LZ4 decompression if needed
    return JSON.parse(data);
  }],
});
```

### Rust Client

```rust
use reqwest::Client;

let client = Client::builder()
    .gzip(true)
    .build()?;

// The gateway will automatically use LZ4 if client supports it
let response = client
    .post("http://localhost:8888/graphql")
    .header("Accept-Encoding", "lz4, gzip")
    .json(&graphql_query)
    .send()
    .await?;
```

### Go Client

```go
import (
    "github.com/pierrec/lz4"
    "net/http"
)

client := &http.Client{
    Transport: &lz4Transport{},
}

// Add LZ4 decompression support
type lz4Transport struct{}

func (t *lz4Transport) RoundTrip(req *http.Request) (*http.Response, error) {
    req.Header.Set("Accept-Encoding", "lz4, gzip")
    // ... handle LZ4 decompression
}
```

## Performance Comparison

### Benchmark: 1KB GraphQL Response

| Algorithm | Compression Time | Decompression Time | Compressed Size |
|-----------|-----------------|-------------------|-----------------|
| **LZ4** | **0.002ms** | **0.001ms** | 580 bytes |
| Gzip | 0.15ms | 0.05ms | 320 bytes |
| Brotli | 2.5ms | 0.08ms | 280 bytes |

**Result**: LZ4 is **75x faster** to compress than gzip with acceptable size.

### Benchmark: 100KB GraphQL Response

| Algorithm | Compression Time | Decompression Time | Compressed Size |
|-----------|-----------------|-------------------|-----------------|
| **LZ4** | **0.14ms** | **0.05ms** | 52 KB |
| Gzip | 12ms | 3ms | 28 KB |
| Brotli | 180ms | 4ms | 24 KB |

**Result**: LZ4 is **85x faster** to compress, **60x faster** to decompress.

## Cost Impact at 100k req/s

### Scenario: 2KB average response size

**With Gzip:**
```
CPU: 4 cores @ 100% = 4 vCPU
Cost: ~$140/mo
Bandwidth: 155 MB/s compressed
Latency: +2ms P99
```

**With LZ4:**
```
CPU: 2 cores @ 40% = 0.8 vCPU
Cost: ~$28/mo (80% reduction!)
Bandwidth: 180 MB/s compressed
Latency: +0.3ms P99
```

**Savings: $112/month on compression CPU alone**

## Integration Examples

### Example 1: Ultra-Fast Internal APIs

For microservices communication where throughput matters more than bandwidth:

```rust
let gateway = Gateway::builder()
    .with_compression(CompressionConfig {
        enabled: true,
        algorithms: vec!["lz4".into()],
        min_size_bytes: 256,
        level: CompressionLevel::Fast,
    })
    .build()?;
```

### Example 2: Hybrid Strategy

Use LZ4 for internal calls, Brotli for external:

```rust
// In middleware
async fn compression_selector(req: Request) -> CompressionConfig {
    if is_internal_request(&req) {
        CompressionConfig::ultra_fast()  // LZ4
    } else {
        CompressionConfig::best()  // Brotli
    }
}
```

### Example 3: Content-Type Based

Use LZ4 for JSON, Gzip for HTML:

```rust
let config = if response_is_json {
    CompressionConfig::ultra_fast()
} else {
    CompressionConfig::default()
};
```

## Cache Optimization with LZ4

Use LZ4 to compress cached responses for better memory efficiency:

```rust
use grpc_graphql_gateway::Lz4CacheCompressor;

// Store in cache
let json = serde_json::to_string(&response)?;
let compressed = Lz4CacheCompressor::compress(&json)?;
cache.set("key", compressed).await?;

// Retrieve from cache
let compressed = cache.get("key").await?;
let json = Lz4CacheCompressor::decompress(&compressed)?;
let response: GraphQLResponse = serde_json::from_str(&json)?;
```

**Result**: 50-60% memory savings in cache with minimal CPU overhead.

## Advanced: Custom Middleware

Add LZ4 compression as custom middleware:

```rust
use grpc_graphql_gateway::lz4_compression_middleware;
use axum::{Router, middleware};

let app = Router::new()
    .route("/graphql", post(graphql_handler))
    .layer(middleware::from_fn(lz4_compression_middleware));
```

## Monitoring

Track LZ4 compression effectiveness:

```rust
// Export metrics
gauge!("compression_ratio_lz4", compression_ratio);
histogram!("compression_time_lz4_ms", compression_time.as_millis() as f64);
counter!("bytes_saved_lz4", bytes_saved);
```

## Best Practices

### 1. Set Reasonable Thresholds

```rust
CompressionConfig {
    min_size_bytes: 256,  // Don't compress tiny responses
    // ...
}
```

### 2. Combine with Caching

LZ4 + caching = maximum performance:

```rust
Gateway::builder()
    .with_response_cache(cache_config)
    .with_compression(CompressionConfig::ultra_fast())
    .build()?
```

### 3. Monitor CPU vs Bandwidth Trade-off

```rust
// If CPU > 80%: Use LZ4
// If bandwidth > 80%: Use Brotli
// Otherwise: Use Gzip
let config = match (cpu_usage, bandwidth_usage) {
    (cpu, _) if cpu > 0.8 => CompressionConfig::ultra_fast(),
    (_, bw) if bw > 0.8 => CompressionConfig::best(),
    _ => CompressionConfig::default(),
};
```

### 4. Test with Your Data

```rust
use grpc_graphql_gateway::{compress_lz4, compression};

let sample_response = get_typical_graphql_response();
let compressed = compress_lz4(sample_response.as_bytes())?;

let ratio = compressed.len() as f64 / sample_response.len() as f64;
println!("Compression ratio: {:.1}%", ratio * 100.0);
```

## Production Deployment

### Kubernetes

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: graphql-gateway
spec:
  template:
    spec:
      containers:
      - name: gateway
        env:
        - name: COMPRESSION_ALGORITHM
          value: "lz4"
        - name: COMPRESSION_MIN_SIZE
          value: "256"
        resources:
          requests:
            cpu: "500m"  # LZ4 uses less CPU
            memory: "512Mi"
```

### Docker

```dockerfile
FROM rust:1.75-alpine AS builder
RUN apk add --no-cache lz4-dev

# ... build gateway with LZ4 support

ENTRYPOINT ["./gateway", "--compression=lz4"]
```

## FAQ

**Q: Is LZ4 supported by browsers?**
A: Not natively. Use gzip/brotli for browser clients, LZ4 for server-to-server.

**Q: Can I use both LZ4 and Gzip?**
A: Yes! The gateway automatically selects based on `Accept-Encoding` header.

**Q: Does LZ4 work with CloudFlare?**
A: CloudFlare doesn't support LZ4. Use it for origin-to-CloudFlare, let CloudFlare handle client compression.

**Q: How much CPU does LZ4 save?**
A: 60-80% less CPU than gzip at 100k req/s (see benchmarks above).

## Related Documentation

- [Response Compression](./compression.md)
- [High Performance Mode](./high-performance.md)
- [Cost Optimization](../production/cost-optimization-strategies.md)

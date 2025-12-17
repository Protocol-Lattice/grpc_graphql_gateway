# High Performance Optimization

The `grpc_graphql_gateway` is designed for extreme throughput requirements, capable of handling **100,000+ requests per second (RPS)** per instance. To achieve these targets, the gateway employs several advanced architectural optimizations.

## Performance Targets

With High-Performance mode enabled:
- **100K+ RPS**: For cached queries serving from memory.
- **50K+ RPS**: For uncached queries performing gRPC backend calls.
- **Sub-millisecond P99**: Latency for cache hits.

## Key Optimizations

### SIMD-Accelerated JSON Parsing
Standard JSON parsing is often the primary bottleneck in GraphQL gateways. We use `simd-json`, which employs SIMD (Single Instruction, Multiple Data) instructions (AVX2, SSE4.2, NEON) to parse JSON.
- **2x – 5x faster** than `serde_json` for typical payloads.
- **Reduced CPU cycles** per request, allowing more concurrency on the same hardware.

### Lock-Free Sharded Caching
Global locks cause severe contention as CPU core counts increase. Our `ShardedCache` implementation:
- Splits the cache into **64 – 128 independent shards**.
- Uses **lock-free reads** and independent write locks per shard.
- Eliminates the "Global Lock" bottleneck.

### Object Pooling
Memory allocation is expensive at 100K RPS. We use high-performance object pools for request/response buffers:
- **Zero-allocation** steady state for many request patterns.
- Pre-allocated buffers are returned to a lock-free `ArrayQueue` for reuse.

### Connection Pool Tuning
The gateway automatically tunes gRPC and HTTP/2 settings for maximum throughput:
- **HTTP/2 Prior Knowledge**: Skips nested version negotiation.
- **Adaptive Window Sizes**: Optimizes flow control for high-bandwidth/low-latency local networks.
- **TCP NoDelay**: Disables Nagle's algorithm for immediate packet dispatch.

## Configuration

Enable High-Performance mode in your `GatewayBuilder`:

```rust
use grpc_graphql_gateway::{Gateway, HighPerfConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    Gateway::builder()
        // ... standard config ...
        .with_high_performance(HighPerfConfig::ultra_fast())
        .build()?
        .serve("0.0.0.0:8888")
        .await
}
```

### Configuration Profiles

We provide three pre-tuned profiles:

| Profile | Use Case |
|---------|----------|
| `ultra_fast()` | **Maximum Throughput**: Optimized for 100K+ RPS. |
| `balanced()` | **Balanced**: Good mix of throughput and latency. |
| `low_latency()` | **Low Latency**: Optimized for minimal response time over raw RPS. |

## Benchmarking

We include a performance benchmark suite in the repository.

```bash
# Start the example server
cargo run --example greeter --release

# Run the benchmark
cargo run --bin benchmark --release -- --concurrency=200 --duration=30
```

For a complete automated test, use `./benchmark.sh` which handles builds and runs multiple profiles.

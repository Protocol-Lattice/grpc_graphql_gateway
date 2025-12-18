# GBP+LZ4 Ultra-Fast Compression

GraphQL Binary Protocol (GBP) combined with LZ4 provides a novel, ultra-high-performance binary encoding for GraphQL responses. While standard LZ4 is fast, GBP+LZ4 achieves near-maximal compression ratios (up to 99%) by exploiting the structural redundancy of GraphQL data *before* applying block compression.

## Benefits

| Feature | GBP+LZ4 | Standard LZ4 | Gzip | Brotli |
|---------|---------|--------------|------|--------|
| **Compression Ratio** | **95-99%** | 50-60% | 70-80% | 75-85% |
| **Compression Speed** | **Ultra Fast** | Ultra Fast | Fast | Slow |
| **Deduplication** | **Structural** | Byte-level | Byte-level | Byte-level |
| **Best For** | **GraphQL Lists** | Generic Binary | Browsers | Static Assets |

## Why GBP?

Standard compression algorithms (Gzip, Brotli, LZ4) treat the response as a bucket of bytes. **GBP (GraphQL Binary Protocol)** understands the GraphQL structure:

1.  **Structural Templates (Shapes)**: It identifies that `users { id name }` always has the same keys and only encodes the "shape" once.
2.  **Value Pooling**: Repeated strings like `__typename` or enum values are stored in a pool and replaced by small integer references.
3.  **Columnar Storage**: Lists of objects are transformed into columns, allowing the compression algorithm to see similar data types together, which drastically increases the compression ratio.
4.  **Recursive Deduplication**: identical sub-objects or arrays are detected and replaced with 4-byte references.

## Quick Start

### Basic Configuration

```rust
use grpc_graphql_gateway::{Gateway, CompressionConfig};

let gateway = Gateway::builder()
    // ultra_fast() now defaults to GBP+LZ4
    .with_compression(CompressionConfig::ultra_fast())
    .build()?;
```

### Manual Configuration

```rust
use grpc_graphql_gateway::CompressionConfig;

let config = CompressionConfig {
    enabled: true,
    min_size_bytes: 128, // GBP is efficient even for small fragments
    algorithms: vec!["gbp-lz4".into(), "lz4".into()],
    ..Default::default()
};
```

## Client Support

### Accept-Encoding Header

Clients must opt-in to the binary protocol by sending the following header:

```http
Accept-Encoding: gbp-lz4, lz4, gzip
```

The gateway will respond with:
- `Content-Encoding: gbp-lz4`
- `Content-Type: application/graphql-response+gbp`

### Decoding in Rust

The `GbpDecoder` is available for server-to-server communication:

```rust
use grpc_graphql_gateway::gbp::GbpDecoder;

let bytes = response.bytes().await?;
let mut decoder = GbpDecoder::new();
let json_value = decoder.decode_lz4(&bytes)?;
```

## Performance Benchmarks

### 10MB GraphQL Response (10k Users)

| Metric | Original JSON | Standard Gzip | GBP+LZ4 (Ultra) |
|--------|---------------|---------------|-----------------|
| **Size** | 10.2 MB | 2.1 MB | **73 KB** |
| **Reduction** | 0% | 79% | **99.28%** |
| **Latency** | - | +45ms | **+3ms** |

**Result**: GBP+LZ4 is **28x smaller** than Gzip and **15x faster** to process for large GraphQL datasets.

## Use Cases

✅ **Internal Microservices**: Use GBP+LZ4 for all internal service-to-service GraphQL communication to minimize network overhead and CPU usage.
✅ **High-Density Mobile Apps**: Large lists of data can be sent to mobile clients in a fraction of the time, saving battery and data plans (requires custom decoder).
✅ **Cache Optimization**: Store GBP-encoded data in Redis or in-memory caches to fit 10-50x more data in the same memory space.

## Related Documentation

- [Response Compression](./compression.md)
- [High Performance Optimization](./high-performance.md)
- [Cost Optimization Strategies](../production/cost-optimization-strategies.md)

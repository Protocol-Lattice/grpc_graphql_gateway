# Response Compression

Reduce bandwidth with automatic response compression.

## Enabling Compression

```rust
use grpc_graphql_gateway::{Gateway, CompressionConfig, CompressionLevel};

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .with_compression(CompressionConfig {
        enabled: true,
        level: CompressionLevel::Default,
        min_size_bytes: 1024,  // Only compress responses > 1KB
        algorithms: vec!["br".into(), "gzip".into()],
    })
    .build()?;
```

## Preset Configurations

```rust
// Fast compression for low latency
Gateway::builder().with_compression(CompressionConfig::fast())

// Best compression for bandwidth savings
Gateway::builder().with_compression(CompressionConfig::best())

// Default balanced configuration
Gateway::builder().with_compression(CompressionConfig::default())

// Disable compression
Gateway::builder().with_compression(CompressionConfig::disabled())
```

## Supported Algorithms

| Algorithm | Accept-Encoding | Compression Ratio | Speed |
|-----------|-----------------|-------------------|-------|
| Brotli | `br` | Best | Slower |
| Gzip | `gzip` | Good | Fast |
| Deflate | `deflate` | Good | Fast |
| Zstd | `zstd` | Excellent | Fast |

## Algorithm Selection

The gateway selects the best algorithm based on client `Accept-Encoding`:

```http
Accept-Encoding: br, gzip, deflate
```

Priority order matches your `algorithms` configuration.

## Compression Levels

| Level | Description | Use Case |
|-------|-------------|----------|
| `Fast` | Minimal compression, fast | Low latency APIs |
| `Default` | Balanced | Most applications |
| `Best` | Maximum compression | Bandwidth-constrained |

## Configuration Options

| Option | Type | Description |
|--------|------|-------------|
| `enabled` | bool | Enable/disable compression |
| `level` | CompressionLevel | Compression speed vs ratio |
| `min_size_bytes` | usize | Skip compression for small responses |
| `algorithms` | Vec\<String\> | Enabled algorithms in priority order |

## Testing Compression

```bash
# Request with brotli
curl -H "Accept-Encoding: br" \
  -X POST http://localhost:8888/graphql \
  -H "Content-Type: application/json" \
  -d '{"query": "{ users { id name email } }"}' \
  --compressed -v

# Check Content-Encoding header in response
< Content-Encoding: br
```

## Performance Considerations

- JSON responses typically compress 50-90%
- Set `min_size_bytes` to skip small responses
- Use `CompressionLevel::Fast` for latency-sensitive apps
- Balance CPU cost vs. bandwidth savings

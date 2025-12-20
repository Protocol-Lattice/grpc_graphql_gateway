# GBP+LZ4 Ultra-Fast Compression

GraphQL Binary Protocol (GBP) combined with LZ4 provides a novel, ultra-high-performance binary encoding for GraphQL responses. While standard LZ4 is fast, GBP+LZ4 achieves near-maximal compression ratios (up to 99%) by exploiting the structural redundancy of GraphQL data *before* applying block compression.

## Benefits

| Feature | GBP+LZ4 (Turbo O(1)) | Standard LZ4 | Gzip | Brotli |
|---------|---------|--------------|------|--------|
| **Compression Ratio** | **50-99%** | 50-60% | 70-80% | 75-85% |
| **Compression Speed** | **Ultra Fast (O(1))** | Ultra Fast | Fast | Slow |
| **Deduplication** | **Zero-Clone Structural** | Byte-level | Byte-level | Byte-level |
| **Scale Support** | **1GB+ Payloads** | Generic Binary | Browsers | Static Assets |

## Compression Scenarios

GBP compression effectiveness depends on the **repetitiveness of your data**. Here's what to expect:

### Data Pattern Guide

| Data Pattern | Compression | Example |
|--------------|-------------|---------|
| **Highly Repetitive** | **95-99%** | Lists where most fields repeat (same status, permissions, metadata) |
| **Moderately Repetitive** | **70-85%** | Typical production data with shared types and enums |
| **Unique/Varied** | **50%** | Unique strings per item (names, descriptions, unique IDs) |

### Scenario 1: Highly Repetitive (99% Compression)

Best case for GBP - data with structural repetition:

```json
{
  "products": [
    { "id": 1, "status": "ACTIVE", "category": "Electronics", "org": { "id": "org-1", "name": "Acme" } },
    { "id": 2, "status": "ACTIVE", "category": "Electronics", "org": { "id": "org-1", "name": "Acme" } },
    // ... 20,000 more items with same status, category, org
  ]
}
```

**Result**: 41 MB → 266 KB (**99.37% reduction**)

GBP leverages:
- String interning for repeated values ("ACTIVE", "Electronics", "Acme")
- Shape deduplication for identical object structures
- Columnar encoding for arrays of objects
- Run-length encoding for consecutive identical values

### Scenario 2: Moderately Repetitive (70-85% Compression)

Typical production data with some variation:

```json
{
  "users": [
    { "id": 1, "name": "Alice", "role": "ADMIN", "status": "ACTIVE", "region": "US" },
    { "id": 2, "name": "Bob", "role": "USER", "status": "ACTIVE", "region": "EU" },
    // ... users with unique names but repeated roles/statuses/regions
  ]
}
```

**Result**: ~75% compression typical

GBP benefits from:
- Repeated enum values (role, status, region)
- Shape deduplication (all User objects have same structure)
- `__typename` field repetition

### Scenario 3: Unique/Varied (50% Compression)

Worst case - highly unique data:

```json
{
  "logs": [
    { "id": "uuid-1", "message": "Unique log message 1", "timestamp": "2024-01-01T00:00:01Z" },
    { "id": "uuid-2", "message": "Different log message 2", "timestamp": "2024-01-01T00:00:02Z" },
    // ... every field is unique
  ]
}
```

**Result**: ~50% compression

GBP still provides:
- Binary encoding (smaller than JSON text)
- LZ4 block compression
- Shape deduplication (structure is same even if values differ)

### Real-World Expectations

Most production GraphQL responses fall into the **moderately repetitive** category:

| Repeated Elements | Unique Elements |
|-------------------|-----------------|
| `__typename` values | Entity IDs |
| Enum values (status, role) | Timestamps |
| Nested references (org, category) | User-generated content |
| Boolean flags | Unique identifiers |

**Realistic expectation: 70-85% compression** for typical production workloads.

### Maximizing Compression

To achieve higher compression rates:

1. **Use enums** instead of freeform strings for status fields
2. **Normalize data** with shared references (e.g., all products reference same `category` object)
3. **Batch similar queries** to increase repetition within responses
4. **Design schemas** with repeated metadata objects

## Why GBP? (O(1) Turbo Mode)

Standard compression algorithms (Gzip, Brotli, LZ4) treat the response as a bucket of bytes. **GBP (GraphQL Binary Protocol) v9** understands the GraphQL structure at the memory level:

1.  **Positional References (O(1))**: Starting in v0.5.9, GBP eliminates expensive value cloning. It uses buffer position references for deduplication, resulting in constant-time lookups and zero additional memory overhead per duplicate.
2.  **Shallow Hashing**: Replaced recursive tree-walking hashes with O(1) shallow hashing for large structures. This enables massive 1GB+ payloads to be processed without quadratic performance degradation.
3.  **Structural Templates (Shapes)**: It identifies that `users { id name }` always has the same keys and only encodes the "shape" once.
4.  **Columnar Storage**: Lists of objects are transformed into columns, allowing the compression algorithm to see similar data types together, which drastically increases the compression ratio.

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

```rust
use grpc_graphql_gateway::gbp::GbpDecoder;

let bytes = response.bytes().await?;
let mut decoder = GbpDecoder::new();
let json_value = decoder.decode_lz4(&bytes)?;
```

### Decoding in Browser (TypeScript/JavaScript)

Use the official [`@protocol-lattice/gbp-decoder`](https://www.npmjs.com/package/@protocol-lattice/gbp-decoder) library:

```bash
npm install @protocol-lattice/gbp-decoder
```

```typescript
import { GbpDecoder } from '@protocol-lattice/gbp-decoder';

const decoder = new GbpDecoder();

// Recommended for browsers: Gzip-compressed GBP
const decoded = decoder.decodeGzip(uint8Array);

// For ultra-performance: LZ4-compressed GBP
const decodedLz4 = decoder.decodeLz4(uint8Array);
```

## Performance Benchmarks

### 100MB+ GraphQL Behemoth (200k Users)

| Metric | Original JSON | Standard Gzip (Est.) | GBP+LZ4 (Turbo O(1)) |
|--------|---------------|----------------------|----------------------|
| **Size** | 107.1 MB | ~22.0 MB | **804 KB** |
| **Reduction** | 0% | ~79% | **99.25%** |
| **Throughput** | - | ~25 MB/s | **195.7 MB/s** |
| **Integrity** | - | - | **100% Verified** |

**Result**: With **Turbo O(1) Mode**, GBP+LZ4 is **133x smaller** than the original JSON and scales effortlessly to 1GB+ payloads with minimal CPU and memory overhead.

## Use Cases

✅ **Internal Microservices**: Use GBP+LZ4 for all internal service-to-service GraphQL communication to minimize network overhead and CPU usage.
✅ **High-Density Mobile Apps**: Large lists of data can be sent to mobile clients in a fraction of the time, saving battery and data plans (requires custom decoder).
✅ **Cache Optimization**: Store GBP-encoded data in Redis or in-memory caches to fit 10-50x more data in the same memory space.

## Related Documentation

- [Response Compression](./compression.md)
- [High Performance Optimization](./high-performance.md)
- [Cost Optimization Strategies](../production/cost-optimization-strategies.md)

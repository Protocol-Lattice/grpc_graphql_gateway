# @protocol-lattice/gbp-decoder

High-performance GraphQL Binary Protocol (GBP) decoder for TypeScript.

GBP is a structural binary encoding specifically designed for GraphQL responses, achieving up to 99% compression ratios by performing cross-request and intra-request structural deduplication.

## Features

- **Structural Deduplication**: Efficiently handles highly redundant GraphQL data.
- **Columnar Encoding**: Optimized for large arrays of objects.
- **Ultra-Fast**: Optimized for low-latency decoding in frontend and edge environments.
- **Schema-Aware**: Designed to work seamlessly with the `grpc-graphql-gateway`.

## Installation

```bash
npm install @protocol-lattice/gbp-decoder
```

## Usage

```typescript
import { GbpDecoder } from '@protocol-lattice/gbp-decoder';

const decoder = new GbpDecoder();

// Decode from LZ4-compressed GBP bytes
const lz4Response = decoder.decodeLz4(compressedUint8Array);

// Decode from Gzip-compressed GBP bytes (recommended for broad compatibility)
const gzipResponse = decoder.decodeGzip(gzipUint8Array);

console.log(lz4Response.data);
```

## Protocol Details

GBP (v8) uses a multi-pass encoding strategy:
1. **String Pooling**: All strings are deduplicated and indexed.
2. **Shape Pooling**: Object keys are hashed and indexed into "shapes".
3. **Value Pooling**: Identical sub-trees (objects/arrays) are deduplicated.
4. **Columnar Transformation**: Large arrays of objects are transposed for better compression.

## License

MIT

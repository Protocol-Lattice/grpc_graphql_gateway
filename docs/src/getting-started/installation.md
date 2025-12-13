# Installation

## Add to Cargo.toml

```toml
[dependencies]
grpc_graphql_gateway = "0.2"
tokio = { version = "1", features = ["full"] }
tonic = "0.12"
```

## Optional Features

The gateway supports optional features that can be enabled in `Cargo.toml`:

```toml
[dependencies]
grpc_graphql_gateway = { version = "0.2", features = ["otlp"] }
```

| Feature | Description |
|---------|-------------|
| `otlp` | Enable OpenTelemetry Protocol export for distributed tracing |

## Prerequisites

Before using the gateway, ensure you have:

1. **Rust 1.70+** - The gateway uses modern Rust features
2. **Protobuf Compiler** - `protoc` for generating descriptor files
3. **gRPC Services** - Backend services to proxy requests to

## Installing protoc

### macOS
```bash
brew install protobuf
```

### Ubuntu/Debian
```bash
sudo apt-get install protobuf-compiler
```

### Windows
Download from the [protobuf releases page](https://github.com/protocolbuffers/protobuf/releases).

## Proto Annotations

To use the gateway, your `.proto` files need GraphQL annotations. Copy the `graphql.proto` file from the repository:

```bash
curl -o proto/graphql.proto https://raw.githubusercontent.com/Protocol-Lattice/grpc_graphql_gateway/main/proto/graphql.proto
```

This file defines the custom options like `graphql.schema`, `graphql.field`, and `graphql.entity` that the gateway uses to generate the GraphQL schema.

## Next Steps

Once installed, proceed to the [Quick Start](./quick-start.md) guide to create your first gateway.

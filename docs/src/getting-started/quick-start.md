# Quick Start

This guide will get you up and running with a basic gRPC-GraphQL gateway in minutes.

## Basic Gateway

```rust
use grpc_graphql_gateway::{Gateway, GrpcClient};

const DESCRIPTORS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/graphql_descriptor.bin"));

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let gateway = Gateway::builder()
        .with_descriptor_set_bytes(DESCRIPTORS)
        .add_grpc_client(
            "greeter.Greeter",
            GrpcClient::builder("http://127.0.0.1:50051").connect_lazy()?,
        )
        .build()?;

    gateway.serve("0.0.0.0:8888").await?;
    Ok(())
}
```

## What This Does

1. **Loads protobuf descriptors** - The binary descriptor file contains your service definitions
2. **Connects to gRPC backend** - Lazily connects to your gRPC service
3. **Generates GraphQL schema** - Automatically creates types, queries, and mutations
4. **Starts HTTP server** - Serves GraphQL at `/graphql`

## Endpoints

Once running, your gateway exposes:

| Endpoint | Description |
|----------|-------------|
| `http://localhost:8888/graphql` | GraphQL HTTP endpoint (POST) |
| `ws://localhost:8888/graphql/ws` | GraphQL WebSocket for subscriptions |

## Testing Your Gateway

### Using curl

```bash
curl -X POST http://localhost:8888/graphql \
  -H "Content-Type: application/json" \
  -d '{"query": "{ sayHello(name: \"World\") { message } }"}'
```

### Using GraphQL Playground

The gateway includes a built-in GraphQL Playground. Open your browser and navigate to:

```
http://localhost:8888/graphql
```

## Example Proto File

Here's a simple proto file that works with the gateway:

```protobuf
syntax = "proto3";

package greeter;

import "graphql.proto";

service Greeter {
  option (graphql.service) = {
    host: "localhost:50051"
    insecure: true
  };

  rpc SayHello(HelloRequest) returns (HelloReply) {
    option (graphql.schema) = {
      type: QUERY
      name: "sayHello"
    };
  }
}

message HelloRequest {
  string name = 1;
}

message HelloReply {
  string message = 1;
}
```

## Next Steps

- Learn how to [Generate Descriptors](./generating-descriptors.md) from your proto files
- Explore [Queries, Mutations & Subscriptions](../core/operations.md)
- Enable [Apollo Federation](../federation/overview.md) for microservice architectures

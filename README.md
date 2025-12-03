# grpc-graphql-gateway-rs

Bridge your gRPC services to GraphQL. This crate builds an `async-graphql` schema directly from protobuf descriptors (including custom `(graphql.*)` options) and routes requests to your gRPC backends via `tonic`.

## Highlights
- GraphQL **queries**, **mutations**, and **subscriptions** from gRPC methods (unary + server streaming)
- Dynamic schema generation from descriptor sets; optional pluck/rename/omit field directives
- Lazy or eager TLS/plain gRPC clients via a small builder API
- Axum HTTP + WebSocket (graphql-ws) integration out of the box
- Middleware and error-hook support for auth/logging/observability
- `protoc-gen-graphql-template` helper that emits a starter gateway file and prints example operations

## Install
```toml
[dependencies]
grpc-graphql-gateway = "0.1"
tokio = { version = "1", features = ["full"] }
tonic = "0.12"
```

## Generate descriptors
Use `tonic-build` (already wired in `build.rs`) to emit `graphql_descriptor.bin`:
```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/graphql.proto");
    let out_dir = std::env::var("OUT_DIR")?;
    let proto_include = std::env::var("PROTOC_INCLUDE").unwrap_or_else(|_| "/usr/local/include".to_string());

    tonic_build::configure()
        .build_server(false)
        .build_client(false)
        .file_descriptor_set_path(std::path::PathBuf::from(&out_dir).join("graphql_descriptor.bin"))
        .compile_protos(&["proto/graphql.proto"], &["proto", &proto_include])?;
    Ok(())
}
```

## Quick start
```rust
use grpc_graphql_gateway::{Gateway, GrpcClient, Result};

const DESCRIPTORS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/graphql_descriptor.bin"));

#[tokio::main]
async fn main() -> Result<()> {
    let builder = Gateway::builder()
        .with_descriptor_set_bytes(DESCRIPTORS)
        .add_grpc_client(
            "greeter.Greeter",
            GrpcClient::builder("http://127.0.0.1:50051").lazy(true).connect_lazy()?,
        );

    builder.serve("0.0.0.0:8888").await
}
```
- HTTP GraphQL endpoint: `POST /graphql`
- WebSocket subscriptions: `GET /graphql/ws` (graphql-ws)

### Built-in greeter example
The repository ships a runnable greeter service defined in `proto/greeter.proto` that exercises queries, mutations, subscriptions, and a resolver. Run both the gRPC backend and the GraphQL gateway (this is the default `cargo run` target):
```bash
cargo run            # or: cargo run --bin greeter
```
gRPC listens on `127.0.0.1:50051`, and GraphQL (HTTP + websocket) is at `http://127.0.0.1:8888/graphql` (`ws://127.0.0.1:8888/graphql/ws` for subscriptions).

Sample operations you can paste into GraphiQL or curl:
```graphql
query { hello(name: "GraphQL") { message meta { correlationId from { id displayName trusted } } } }
mutation { updateGreeting(input: { name: "GraphQL", salutation: "Howdy" }) { message } }
subscription { streamHello(name: "GraphQL") { message meta { correlationId } } }
query { user(id: "demo") { id displayName trusted } }
```

Upload mutation (uses the GraphQL `Upload` scalar; send as multipart):
```graphql
mutation ($file: Upload!) {
  uploadAvatar(input: { userId: "demo", avatar: $file }) { userId size }
}
```
```
curl http://127.0.0.1:8888/graphql \
  --form 'operations={ "query": "mutation ($file: Upload!) { uploadAvatar(input:{ userId:\"demo\", avatar:$file }) { userId size } }", "variables": { "file": null } }' \
  --form 'map={ "0": ["variables.file"] }' \
  --form '0=@./proto/greeter.proto;type=application/octet-stream'
```

Multi-upload mutation (list of `Upload`):
```graphql
mutation ($files: [Upload!]!) {
  uploadAvatars(input: { userId: "demo", avatars: $files }) { userId sizes }
}
```
```
curl http://127.0.0.1:8888/graphql \
  --form 'operations={ "query": "mutation ($files: [Upload!]!) { uploadAvatars(input:{ userId:\"demo\", avatars:$files }) { userId sizes } }", "variables": { "files": [null, null] } }' \
  --form 'map={ "0": ["variables.files.0"], "1": ["variables.files.1"] }' \
  --form '0=@./proto/greeter.proto;type=application/octet-stream' \
  --form '1=@./README.md;type=text/plain'
```

## How it fits together
A quick view of how protobuf descriptors, the generated schema, and gRPC clients are wired to serve GraphQL over HTTP and WebSocket:
```mermaid
flowchart LR
  subgraph Client["GraphQL clients"]
    http["HTTP POST /graphql"]
    ws["WebSocket /graphql/ws"]
  end

  subgraph Gateway["grpc-graphql-gateway"]
    desc["Descriptor set\n(graphql_descriptor.bin)"]
    schema["SchemaBuilder\nasync-graphql schema"]
    mux["ServeMux\nAxum routes + middlewares\n(optional error hooks)"]
    pool["GrpcClient pool\n(lazy/eager, TLS/plain)"]
  end

  subgraph Services["Your gRPC backends"]
    svc1["Service 1"]
    svc2["Service N"]
  end

  http --> mux
  ws --> mux
  desc --> schema
  pool --> schema
  schema --> mux
  mux --> pool
  pool --> svc1
  pool --> svc2
```

## Proto annotations (from `proto/graphql.proto`)
- Service defaults:
  ```proto
  service Greeter {
    option (graphql.service) = { host: "127.0.0.1:50051", insecure: true };
    rpc SayHello(HelloRequest) returns (HelloReply) {
      option (graphql.schema) = { type: QUERY, name: "hello" };
    }
  }
  ```
- Method options (`graphql.schema`):
  - `type`: QUERY | MUTATION | SUBSCRIPTION | RESOLVER
  - `name`: override GraphQL field name
  - `request.name`: wrap input as a single argument
  - `response.pluck`: expose a nested field instead of the whole message
  - `response.required`: mark return type non-null
- Field options (`graphql.field`):
  - `required`: non-null in GraphQL
  - `name`: rename field
  - `omit`: skip field entirely

## Using the template generator
`protoc-gen-graphql-template` emits a ready-to-run `graphql_gateway.rs` that wires clients, logs discovered operations, and prints example queries/mutations/subscriptions.
```bash
protoc \
  --plugin=protoc-gen-graphql-template=target/debug/protoc-gen-graphql-template \
  --graphql-template_out=. \
  --proto_path=proto \
  your_service.proto
```
Open the generated file, point endpoints at your services, and run it.

### Example: Greeter
```bash
cargo build --bin protoc-gen-graphql-template

protoc \
  --plugin=protoc-gen-graphql-template=target/debug/protoc-gen-graphql-template \
  --graphql-template_out=./generated \
  --proto_path=proto \
  proto/greeter.proto

rustc ./generated/graphql_gateway.rs -L target/debug/deps
./graphql_gateway
```
The generated gateway will log which queries/mutations/subscriptions it found and print example GraphQL operations such as:
```
Example queries:
  query { hello }
Example mutations:
  mutation { createUser }
Example subscriptions:
  subscription { streamHello }
```

## Middleware and error hooks
Attach middlewares and inspect errors:
```rust
use grpc_graphql_gateway::{Gateway, GrpcClient};
use grpc_graphql_gateway::middleware::LoggingMiddleware;

let builder = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .add_grpc_client("service", GrpcClient::builder("http://localhost:50051").connect_lazy()?)
    .add_middleware(LoggingMiddleware);
```
`GatewayBuilder::with_error_handler` lets you capture `GraphQLError`s before responses are returned.

## Type mapping (protobuf -> GraphQL)
- `string` -> `String`
- `bool` -> `Boolean`
- `int32`/`uint32` -> `Int`
- `int64`/`uint64` -> `String` (to avoid precision loss)
- `float`/`double` -> `Float`
- `bytes` -> `Upload` (inputs via multipart) / `String` (base64 responses)
- `repeated` -> `[T]`
- `message` -> `Object` / `InputObject`
- `enum` -> `Enum`

`Upload` inputs follow the GraphQL multipart request spec and are valid on mutations.

## Development
- Format: `cargo fmt`
- Lint/tests: `cargo test`
- A runnable example lives at `examples/greeter`, wired up to `proto/greeter.proto` (query/mutation/subscription/resolver).

## License
MIT. See [LICENSE](./LICENSE).

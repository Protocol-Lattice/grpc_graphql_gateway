# Generating Descriptors

The gateway reads protobuf descriptor files (`.bin`) to understand your service definitions. This page explains how to generate them.

## Using build.rs (Recommended)

Add a `build.rs` file to your project:

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = std::env::var("OUT_DIR")?;
    
    tonic_build::configure()
        .build_server(false)
        .build_client(false)
        .file_descriptor_set_path(
            std::path::PathBuf::from(&out_dir).join("graphql_descriptor.bin")
        )
        .compile_protos(&["proto/your_service.proto"], &["proto"])?;
    
    Ok(())
}
```

### Build Dependencies

Add to your `Cargo.toml`:

```toml
[build-dependencies]
tonic-build = "0.12"
```

## Loading Descriptors

In your main code, load the generated descriptor:

```rust
const DESCRIPTORS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/graphql_descriptor.bin"));

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .build()?;
```

## Using protoc Directly

You can also generate descriptors using `protoc` directly:

```bash
protoc \
  --descriptor_set_out=descriptor.bin \
  --include_imports \
  --include_source_info \
  -I proto \
  proto/your_service.proto
```

Then load it from a file:

```rust
let gateway = Gateway::builder()
    .with_descriptor_set_file("descriptor.bin")?
    .build()?;
```

## Multiple Proto Files

If you have multiple proto files, include them all:

```rust
tonic_build::configure()
    .file_descriptor_set_path(
        std::path::PathBuf::from(&out_dir).join("descriptor.bin")
    )
    .compile_protos(
        &[
            "proto/users.proto",
            "proto/products.proto",
            "proto/orders.proto",
        ],
        &["proto"]
    )?;
```

## Multi-Descriptor Support

For microservice architectures where each team owns their proto files, you can combine multiple descriptor sets:

```rust
const USERS_DESCRIPTORS: &[u8] = include_bytes!("path/to/users.bin");
const PRODUCTS_DESCRIPTORS: &[u8] = include_bytes!("path/to/products.bin");

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(USERS_DESCRIPTORS)
    .add_descriptor_set_bytes(PRODUCTS_DESCRIPTORS)
    .build()?;
```

See [Multi-Descriptor Support](../core/multi-descriptor.md) for more details.

## Required Proto Imports

Your proto files must import the GraphQL annotations:

```protobuf
import "graphql.proto";
```

Make sure `graphql.proto` is in your include path when compiling.

## Troubleshooting

### Missing graphql.schema extension

If you see this error:
```
missing graphql.schema extension
```

Ensure that:
1. `graphql.proto` is included in your proto compilation
2. You're using `--include_imports` with protoc
3. Your tonic-build includes all necessary proto files

### Descriptor file not found

If the descriptor file isn't found at runtime:
1. Check that `OUT_DIR` is set correctly
2. Verify the file was generated during build
3. Use `cargo clean && cargo build` to regenerate

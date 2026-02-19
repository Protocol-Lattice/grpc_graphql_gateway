# Plugin System

The gRPC-GraphQL Gateway includes a powerful plugin system that allows you to extend the gateway's functionality by hooking into key lifecycle events.

## Creating a Plugin

To create a plugin, implement the `Plugin` trait:

```rust
use grpc_graphql_gateway::plugin::Plugin;
use grpc_graphql_gateway::middleware::Context;
use async_graphql::{Request, Response};
use async_trait::async_trait;

pub struct MyPlugin;

#[async_trait]
impl Plugin for MyPlugin {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn name(&self) -> &str {
        "MyPlugin"
    }

    async fn on_request(&self, _ctx: &Context, req: &Request) -> Result<(), Self::Error> {
        println!("Request received: {:?}", req.query);
        Ok(())
    }

    async fn on_response(&self, _ctx: &Context, res: &Response) -> Result<(), Self::Error> {
        println!("Response sent");
        Ok(())
    }

    async fn on_subgraph_request(&self, service_name: &str, metadata: &mut tonic::metadata::MetadataMap) -> Result<(), Self::Error> {
        println!("Sending request to subgraph: {}", service_name);
        metadata.insert("x-custom-header", "value".parse().unwrap());
        Ok(())
    }
    
    async fn on_schema_build(&self, builder: &mut grpc_graphql_gateway::schema::SchemaBuilder) -> Result<(), Self::Error> {
        // Modify schema builder here
        Ok(())
    }
}
```

## Hook Points

The following hooks are available:

- `on_request`: Called before the GraphQL request is executed.
- `on_response`: Called after the GraphQL request is executed.
- `on_schema_build`: Called during schema construction (allows modifying the schema).
- `on_subgraph_request`: Called before a gRPC request is sent to a subgraph service.

## Registering Plugins

Register plugins using `GatewayBuilder`:

```rust
let gateway = Gateway::builder()
    .register_plugin(MyPlugin)
    .build()?;
```

## Dynamic Loading via Feature Flags

You can use Cargo features to conditionally compile and enable plugins.

In `Cargo.toml`:

```toml
[features]
my-plugin = []
```

In your code:

```rust
#[cfg(feature = "my-plugin")]
builder = builder.register_plugin(MyPlugin);
```

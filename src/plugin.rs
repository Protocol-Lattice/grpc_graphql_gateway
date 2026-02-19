use async_trait::async_trait;
use crate::middleware::Context;
use async_graphql::{Request, Response};
use std::sync::Arc;

/// The `Plugin` trait defines hook points for extending the gateway's functionality.
///
/// plugins allow you to tap into the request lifecycle, modify schema generation,
/// and intercept subgraph requests without strictly tying into the middleware system.
#[async_trait]
pub trait Plugin: Send + Sync + 'static {
    /// Result type for plugin operations
    type Error: std::fmt::Display + Send + Sync + 'static;

    /// Unique name for the plugin
    fn name(&self) -> &str;

    /// Hook callede before the GraphQL request is processed.
    ///
    /// This is a good place to inspect headers, log incoming requests, or modify
    /// the request context.
    async fn on_request(&self, _ctx: &Context, _req: &Request) -> Result<(), Self::Error> {
        Ok(())
    }

    /// Hook called after the GraphQL response is generated but before it's sent.
    ///
    /// Use this to log response times, errors, or modify the response extensions.
    async fn on_response(&self, _ctx: &Context, _res: &Response) -> Result<(), Self::Error> {
        Ok(())
    }

    /// Hook called during schema building.
    ///
    /// Allows the plugin to inspect or modify the `SchemaBuilder` before the
    /// schema is finalized. Use this to add custom directives, types, or
    /// modify the SDL.
    async fn on_schema_build(&self, _builder: &mut crate::schema::SchemaBuilder) -> Result<(), Self::Error> {
        Ok(())
    }

    /// Hook called before a gRPC request is sent to a subgraph/service.
    ///
    /// The `headers` map allows you to inject metadata (like tracing headers)
    /// into the outgoing gRPC call.
    async fn on_subgraph_request(
        &self, 
        _service_name: &str, 
        _headers: &mut tonic::metadata::MetadataMap
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// A registry to hold and manage multiple plugins.
#[derive(Default, Clone)]
pub struct PluginRegistry {
    plugins: Vec<Arc<dyn Plugin<Error = Box<dyn std::error::Error + Send + Sync>>>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self { plugins: Vec::new() }
    }

    pub fn register<P>(&mut self, plugin: P)
    where
        P: Plugin + 'static,
        P::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        // Wrapper to erase the specific Error type into Box<dyn Error>
        struct ErasurePlugin<T>(T);

        #[async_trait]
        impl<T> Plugin for ErasurePlugin<T>
        where
            T: Plugin + Send + Sync,
            T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
        {
            type Error = Box<dyn std::error::Error + Send + Sync>;

            fn name(&self) -> &str {
                self.0.name()
            }

            async fn on_request(&self, ctx: &Context, req: &Request) -> Result<(), Self::Error> {
                self.0.on_request(ctx, req).await.map_err(Into::into)
            }

            async fn on_response(&self, ctx: &Context, res: &Response) -> Result<(), Self::Error> {
                self.0.on_response(ctx, res).await.map_err(Into::into)
            }

            async fn on_schema_build(&self, builder: &mut crate::schema::SchemaBuilder) -> Result<(), Self::Error> {
                self.0.on_schema_build(builder).await.map_err(Into::into)
            }

            async fn on_subgraph_request(
                &self, 
                service_name: &str, 
                headers: &mut tonic::metadata::MetadataMap
            ) -> Result<(), Self::Error> {
                self.0.on_subgraph_request(service_name, headers).await.map_err(Into::into)
            }
        }

        self.plugins.push(Arc::new(ErasurePlugin(plugin)));
    }

    pub async fn on_request(&self, ctx: &Context, req: &Request) -> crate::error::Result<()> {
        for plugin in &self.plugins {
            plugin.on_request(ctx, req).await.map_err(|e| crate::error::Error::Plugin(e.to_string()))?;
        }
        Ok(())
    }

    pub async fn on_response(&self, ctx: &Context, res: &Response) -> crate::error::Result<()> {
        for plugin in &self.plugins {
            plugin.on_response(ctx, res).await.map_err(|e| crate::error::Error::Plugin(e.to_string()))?;
        }
        Ok(())
    }
    
    pub async fn on_schema_build(&self, builder: &mut crate::schema::SchemaBuilder) -> crate::error::Result<()> {
        for plugin in &self.plugins {
            plugin.on_schema_build(builder).await.map_err(|e| crate::error::Error::Plugin(e.to_string()))?;
        }
        Ok(())
    }

    pub async fn on_subgraph_request(&self, service_name: &str, headers: &mut tonic::metadata::MetadataMap) -> crate::error::Result<()> {
        for plugin in &self.plugins {
            plugin.on_subgraph_request(service_name, headers).await.map_err(|e| crate::error::Error::Plugin(e.to_string()))?;
        }
        Ok(())
    }
}

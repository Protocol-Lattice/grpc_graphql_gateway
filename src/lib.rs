//! # grpc-graphql-gateway-rs
//!
//! A high-performance Rust gateway that bridges gRPC services to GraphQL with full Apollo Federation v2 support.
//!
//! ## Features
//!
//! - **Dynamic Schema Generation**: Automatic GraphQL schema from protobuf descriptors
//! - **Federation v2**: Complete Apollo Federation support with entity resolution and `@shareable`
//! - **Batching**: Built-in [`EntityDataLoader`] for efficient N+1 query prevention
//! - **Subscriptions**: Real-time data via GraphQL subscriptions (WebSocket)
//! - **Multiplex Subscriptions**: Support for multiple concurrent subscriptions per WebSocket connection
//! - **Middleware**: Extensible middleware system for auth and logging
//! - **Response Caching**: In-memory LRU cache with TTL and mutation-triggered invalidation
//!
//! ## Main Components
//!
//! - [`Gateway`]: The main entry point for creating and running the gateway.
//! - [`GatewayBuilder`]: Configuration builder for the gateway.
//! - [`SchemaBuilder`]: Low-level builder for the dynamic GraphQL schema.
//! - [`GrpcClient`]: Manages connections to gRPC services.
//! - [`GrpcEntityResolver`]: Handles federation entity resolution.
//!
//! ## Federation
//!
//! To enable federation, use the [`GatewayBuilder::enable_federation`] method and configure
//! an entity resolver. See [`GrpcEntityResolver`] and [`EntityDataLoader`] for details.
//!
//! ## Example
//!
//! ```rust,no_run
//! use grpc_graphql_gateway::{Gateway, GrpcClient};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let grpc_client = GrpcClient::new("http://localhost:50051").await?;
//!     
//!     let gateway = Gateway::builder()
//!         .add_grpc_client("greeter", grpc_client)
//!         .build()?;
//!     
//!     let app = gateway.into_router();
//!     
//!     let listener = tokio::net::TcpListener::bind("0.0.0.0:8888").await?;
//!     axum::serve(listener, app).await?;
//!     
//!     Ok(())
//! }
//! ```

/// Generated types for graphql.proto options.
#[allow(clippy::all)]
pub mod graphql {
    include!("generated/graphql.rs");
}

pub mod cache;
pub mod circuit_breaker;
pub mod dataloader;
pub mod error;
pub mod federation;
pub mod gateway;
pub mod grpc_client;
pub mod health;
pub mod metrics;
pub mod middleware;
pub mod persisted_queries;
pub mod runtime;
pub mod schema;
pub mod shutdown;
pub mod subscription;
pub mod tracing_otel;
pub mod types;

pub use cache::{
    create_response_cache, is_mutation, CacheConfig, CacheLookupResult, CacheStats,
    CachedResponse, ResponseCache, SharedResponseCache,
};
pub use circuit_breaker::{
    create_circuit_breaker_registry, CircuitBreaker, CircuitBreakerConfig, CircuitBreakerError,
    CircuitBreakerRegistry, CircuitState, SharedCircuitBreakerRegistry,
};
pub use dataloader::EntityDataLoader;
pub use error::{Error, Result};
pub use federation::{
    EntityConfig, EntityResolver, EntityResolverMapping, FederationConfig, GrpcEntityResolver,
    GrpcEntityResolverBuilder,
};
pub use gateway::{Gateway, GatewayBuilder};
pub use grpc_client::GrpcClient;
pub use health::{ComponentHealth, HealthResponse, HealthState, HealthStatus};
pub use metrics::{GatewayMetrics, GrpcTimer, RequestTimer};
pub use middleware::{Context, Middleware, RateLimitMiddleware};
pub use persisted_queries::{
    create_apq_store, process_apq_request, PersistedQueryConfig, PersistedQueryError,
    PersistedQueryExtension, PersistedQueryStore, SharedPersistedQueryStore,
};
pub use runtime::ServeMux;
pub use schema::SchemaBuilder;
pub use shutdown::{
    run_with_graceful_shutdown, os_signal_shutdown, RequestGuard, ShutdownConfig,
    ShutdownCoordinator, ShutdownState,
};
pub use subscription::{
    GrpcSubscriptionMapping, GrpcSubscriptionResolver, MultiplexSubscription,
    MultiplexSubscriptionBuilder, ProtocolMessage, SubscriptionConfig, SubscriptionInfo,
    SubscriptionPayload, SubscriptionRegistry, SubscriptionResolver, SubscriptionState,
};
pub use tracing_otel::{init_tracer, shutdown_tracer, GraphQLSpan, GrpcSpan, TracingConfig};


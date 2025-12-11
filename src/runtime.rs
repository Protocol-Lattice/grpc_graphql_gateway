//! Runtime support for GraphQL gateway - HTTP and WebSocket integration.

use crate::error::{GraphQLError, Result};
use crate::grpc_client::GrpcClientPool;
use crate::health::{health_handler, readiness_handler, HealthState};
use crate::metrics::GatewayMetrics;
use crate::middleware::{Context, Middleware};
use crate::schema::{DynamicSchema, GrpcResponseCache};
use async_graphql::ServerError;
use async_graphql_axum::{GraphQLRequest, GraphQLResponse, GraphQLSubscription};
use axum::{
    extract::State,
    http::HeaderMap,
    response::{Html, IntoResponse},
    routing::{get, get_service, post},
    Extension, Router,
};
use std::sync::Arc;

/// ServeMux - main gateway handler
///
/// The `ServeMux` handles the routing of GraphQL requests, executing middlewares,
/// and invoking the dynamic schema. It can be converted into an Axum router.
pub struct ServeMux {
    schema: DynamicSchema,
    middlewares: Vec<Arc<dyn Middleware>>,
    error_handler: Option<Arc<dyn Fn(Vec<GraphQLError>) + Send + Sync>>,
    /// gRPC client pool for health checks
    client_pool: Option<GrpcClientPool>,
    /// Enable health check endpoints
    health_checks_enabled: bool,
    /// Enable metrics endpoint
    metrics_enabled: bool,
}

impl ServeMux {
    /// Create a new ServeMux with an already built schema
    pub fn new(schema: DynamicSchema) -> Self {
        Self {
            schema,
            middlewares: Vec::new(),
            error_handler: None,
            client_pool: None,
            health_checks_enabled: false,
            metrics_enabled: false,
        }
    }

    /// Set the gRPC client pool (needed for health checks)
    pub fn set_client_pool(&mut self, pool: GrpcClientPool) {
        self.client_pool = Some(pool);
    }

    /// Enable health check endpoints
    pub fn enable_health_checks(&mut self) {
        self.health_checks_enabled = true;
    }

    /// Enable metrics endpoint
    pub fn enable_metrics(&mut self) {
        self.metrics_enabled = true;
    }

    /// Add middleware to the execution pipeline
    ///
    /// Middlewares are executed in the order they are added.
    pub fn add_middleware(&mut self, middleware: Arc<dyn Middleware>) {
        self.middlewares.push(middleware);
    }

    /// Use middleware (builder pattern)
    pub fn with_middleware(mut self, middleware: Arc<dyn Middleware>) -> Self {
        self.add_middleware(middleware);
        self
    }

    /// Set error handler from an `Arc` for cases where the caller already shares ownership.
    pub fn set_error_handler_arc(&mut self, handler: Arc<dyn Fn(Vec<GraphQLError>) + Send + Sync>) {
        self.error_handler = Some(handler);
    }

    /// Set error handler
    pub fn set_error_handler<F>(&mut self, handler: F)
    where
        F: Fn(Vec<GraphQLError>) + Send + Sync + 'static,
    {
        self.set_error_handler_arc(Arc::new(handler));
    }

    async fn execute_with_middlewares(
        &self,
        headers: HeaderMap,
        request: GraphQLRequest,
    ) -> Result<async_graphql::Response> {
        let mut ctx = Context {
            headers: headers.clone(),
            extensions: std::collections::HashMap::new(),
        };

        for middleware in &self.middlewares {
            middleware.call(&mut ctx).await?;
        }

        let mut gql_request = request.into_inner();
        gql_request = gql_request.data(ctx);
        gql_request = gql_request.data(GrpcResponseCache::default());

        Ok(self.schema.execute(gql_request).await)
    }

    /// Handle GraphQL HTTP request
    ///
    /// This method executes the request pipeline:
    /// 1. Creates a context from headers
    /// 2. Runs all middlewares
    /// 3. Executes the GraphQL query against the schema
    /// 4. Handles any errors
    pub async fn handle_http(
        &self,
        headers: HeaderMap,
        request: GraphQLRequest,
    ) -> GraphQLResponse {
        match self.execute_with_middlewares(headers, request).await {
            Ok(resp) => resp.into(),
            Err(err) => {
                let gql_err: GraphQLError = err.into();
                if let Some(handler) = &self.error_handler {
                    handler(vec![gql_err.clone()]);
                }
                let server_err = ServerError::new(gql_err.message.clone(), None);
                async_graphql::Response::from_errors(vec![server_err]).into()
            }
        }
    }

    /// Convert to Axum router
    pub fn into_router(self) -> Router {
        let health_checks_enabled = self.health_checks_enabled;
        let metrics_enabled = self.metrics_enabled;
        let client_pool = self.client_pool.clone();
        
        let state = Arc::new(self);
        let subscription = GraphQLSubscription::new(state.schema.executor());

        let mut router = Router::new()
            .route(
                "/graphql",
                post(handle_graphql_post).get(graphql_playground),
            )
            .route_service("/graphql/ws", get_service(subscription))
            .layer(Extension(state.schema.executor()))
            .with_state(state);

        // Add health check routes if enabled
        if health_checks_enabled {
            let health_state = Arc::new(HealthState::new(
                client_pool.unwrap_or_else(GrpcClientPool::new)
            ));
            router = router
                .route("/health", get(health_handler))
                .route("/ready", get(readiness_handler).with_state(health_state));
        }

        // Add metrics route if enabled
        if metrics_enabled {
            router = router.route("/metrics", get(metrics_handler));
        }

        router
    }
}

impl Clone for ServeMux {
    fn clone(&self) -> Self {
        Self {
            schema: self.schema.clone(),
            middlewares: self.middlewares.clone(),
            error_handler: self.error_handler.clone(),
            client_pool: self.client_pool.clone(),
            health_checks_enabled: self.health_checks_enabled,
            metrics_enabled: self.metrics_enabled,
        }
    }
}

/// Handler for POST requests to /graphql
async fn handle_graphql_post(
    State(mux): State<Arc<ServeMux>>,
    headers: HeaderMap,
    request: GraphQLRequest,
) -> impl IntoResponse {
    mux.handle_http(headers, request).await
}

/// Serve the GraphQL Playground UI for ad-hoc exploration.
async fn graphql_playground() -> impl IntoResponse {
    Html(async_graphql::http::playground_source(
        async_graphql::http::GraphQLPlaygroundConfig::new("/graphql")
            .subscription_endpoint("/graphql/ws"),
    ))
}

/// Handler for Prometheus metrics endpoint
async fn metrics_handler() -> impl IntoResponse {
    let metrics = GatewayMetrics::global();
    let body = metrics.render();
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    const GREETER_DESCRIPTOR: &[u8] = include_bytes!("generated/greeter_descriptor.bin");

    fn build_router() -> Router {
        let schema = crate::schema::SchemaBuilder::new()
            .with_descriptor_set_bytes(GREETER_DESCRIPTOR)
            .build(&crate::grpc_client::GrpcClientPool::new())
            .expect("schema builds");

        ServeMux::new(schema).into_router()
    }

    #[tokio::test]
    async fn playground_served_on_get() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/graphql")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("receive response");

        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("read body");
        let body_str = String::from_utf8(body.to_vec()).expect("utf8 body");

        assert!(
            body_str.contains("GraphQL Playground"),
            "playground HTML should be returned"
        );
        assert!(
            body_str.contains("/graphql/ws"),
            "websocket endpoint should be linked"
        );
    }
}

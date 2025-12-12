//! Runtime support for GraphQL gateway - HTTP and WebSocket integration.

use crate::cache::{CacheConfig, CacheLookupResult, SharedResponseCache};
use crate::circuit_breaker::{CircuitBreakerConfig, SharedCircuitBreakerRegistry};
use crate::error::{GraphQLError, Result};
use crate::grpc_client::GrpcClientPool;
use crate::health::{health_handler, readiness_handler, HealthState};
use crate::metrics::GatewayMetrics;
use crate::middleware::{Context, Middleware};
use crate::persisted_queries::{
    process_apq_request, PersistedQueryConfig, PersistedQueryError, SharedPersistedQueryStore,
};
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
use std::collections::HashSet;
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
    /// APQ store for persisted queries
    apq_store: Option<SharedPersistedQueryStore>,
    /// Circuit breaker registry
    circuit_breaker: Option<SharedCircuitBreakerRegistry>,
    /// Response cache
    response_cache: Option<SharedResponseCache>,
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
            apq_store: None,
            circuit_breaker: None,
            response_cache: None,
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

    /// Enable Automatic Persisted Queries (APQ)
    pub fn enable_persisted_queries(&mut self, config: PersistedQueryConfig) {
        self.apq_store = Some(crate::persisted_queries::create_apq_store(config));
    }

    /// Enable Circuit Breaker for gRPC backend resilience
    pub fn enable_circuit_breaker(&mut self, config: CircuitBreakerConfig) {
        self.circuit_breaker = Some(crate::circuit_breaker::create_circuit_breaker_registry(config));
    }

    /// Get the circuit breaker registry (if enabled)
    pub fn circuit_breaker(&self) -> Option<&SharedCircuitBreakerRegistry> {
        self.circuit_breaker.as_ref()
    }

    /// Enable response caching
    pub fn enable_response_cache(&mut self, config: CacheConfig) {
        self.response_cache = Some(crate::cache::create_response_cache(config));
    }

    /// Get the response cache (if enabled)
    pub fn response_cache(&self) -> Option<&SharedResponseCache> {
        self.response_cache.as_ref()
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
        request: async_graphql::Request,
    ) -> Result<async_graphql::Response> {
        let mut ctx = Context {
            headers: headers.clone(),
            extensions: std::collections::HashMap::new(),
        };

        for middleware in &self.middlewares {
            middleware.call(&mut ctx).await?;
        }

        let mut gql_request = request;
        gql_request = gql_request.data(ctx);
        gql_request = gql_request.data(GrpcResponseCache::default());

        Ok(self.schema.execute(gql_request).await)
    }

    /// Handle GraphQL HTTP request
    ///
    /// This method executes the request pipeline:
    /// 1. Process APQ (if enabled) - lookup/cache query by hash
    /// 2. Check response cache (if enabled) - return cached response if available
    /// 3. Creates a context from headers
    /// 4. Runs all middlewares
    /// 5. Executes the GraphQL query against the schema
    /// 6. Caches response (if cacheable)
    /// 7. Handles any errors
    pub async fn handle_http(
        &self,
        headers: HeaderMap,
        request: GraphQLRequest,
    ) -> GraphQLResponse {
        let inner_request = request.into_inner();
        
        // Process APQ if enabled
        let processed_request = if let Some(ref apq_store) = self.apq_store {
            match self.process_apq_request(apq_store, inner_request) {
                Ok(req) => req,
                Err(apq_err) => {
                    return self.apq_error_response(apq_err);
                }
            }
        } else {
            inner_request
        };

        // Check if this is a mutation (mutations are never cached and trigger invalidation)
        let is_mutation = crate::cache::is_mutation(&processed_request.query);

        // Try cache lookup for non-mutations
        if !is_mutation {
            if let Some(ref cache) = self.response_cache {
                let cache_key = crate::cache::ResponseCache::generate_cache_key(
                    &processed_request.query,
                    Some(&serde_json::to_value(&processed_request.variables).unwrap_or_default()),
                    processed_request.operation_name.as_deref(),
                );

                match cache.get(&cache_key) {
                    CacheLookupResult::Hit(cached) => {
                        tracing::debug!("Response cache hit");
                        return self.cached_to_response(cached.data);
                    }
                    CacheLookupResult::Stale(cached) => {
                        // Return stale immediately, could trigger background revalidation
                        // For simplicity, we just return stale data
                        tracing::debug!("Response cache stale hit");
                        return self.cached_to_response(cached.data);
                    }
                    CacheLookupResult::Miss => {
                        // Continue to execute
                    }
                }
            }
        }

        // Store query info for potential caching (need this before moving processed_request)
        let cache_query_info = if self.response_cache.is_some() && !is_mutation {
            Some((
                processed_request.query.clone(),
                serde_json::to_value(&processed_request.variables).unwrap_or_default(),
                processed_request.operation_name.clone(),
            ))
        } else {
            None
        };

        // Execute the query
        match self.execute_with_middlewares(headers, processed_request).await {
            Ok(resp) => {
                // Handle mutation cache invalidation
                if is_mutation {
                    if let Some(ref cache) = self.response_cache {
                        if let Ok(resp_json) = serde_json::to_value(&resp) {
                            cache.invalidate_for_mutation(&resp_json);
                        }
                    }
                } else if let Some((query, vars, op_name)) = cache_query_info {
                    // Cache the response for queries
                    if let Some(ref cache) = self.response_cache {
                        let cache_key = crate::cache::ResponseCache::generate_cache_key(
                            &query,
                            Some(&vars),
                            op_name.as_deref(),
                        );

                        if let Ok(resp_json) = serde_json::to_value(&resp) {
                            // Extract types and entities for invalidation tracking
                            let types = extract_types_from_response(&resp_json);
                            let entities = extract_entities_from_response(&resp_json);
                            cache.put(cache_key, resp_json, types, entities);
                        }
                    }
                }
                resp.into()
            }
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

    /// Convert cached JSON to GraphQL response
    fn cached_to_response(&self, data: serde_json::Value) -> GraphQLResponse {
        // Try to deserialize as Response, or create a simple data response
        match serde_json::from_value::<async_graphql::Response>(data.clone()) {
            Ok(resp) => resp.into(),
            Err(_) => {
                // Fallback: wrap in a simple response
                let resp = async_graphql::Response::new(
                    serde_json::from_value::<async_graphql::Value>(data)
                        .unwrap_or(async_graphql::Value::Null)
                );
                resp.into()
            }
        }
    }

    /// Process APQ for a request
    fn process_apq_request(
        &self,
        store: &SharedPersistedQueryStore,
        mut request: async_graphql::Request,
    ) -> std::result::Result<async_graphql::Request, PersistedQueryError> {
        // Get the query and extensions from the request
        let query = if request.query.is_empty() {
            None
        } else {
            Some(request.query.as_str())
        };
        
        // Convert extensions to serde_json::Value for APQ processing
        let extensions_value = if request.extensions.is_empty() {
            None
        } else {
            serde_json::to_value(&request.extensions).ok()
        };

        // Process APQ
        match process_apq_request(store, query, extensions_value.as_ref())? {
            Some(resolved_query) => {
                request.query = resolved_query;
                Ok(request)
            }
            None => {
                // No query (shouldn't happen if APQ processing succeeded)
                Err(PersistedQueryError::NotFound)
            }
        }
    }

    /// Create an error response for APQ errors
    fn apq_error_response(&self, err: PersistedQueryError) -> GraphQLResponse {
        let error_extensions = err.to_extensions();
        let code = error_extensions.get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("PERSISTED_QUERY_ERROR");
        
        let mut server_err = ServerError::new(err.to_string(), None);
        server_err.extensions = Some({
            let mut ext = async_graphql::ErrorExtensionValues::default();
            ext.set("code", code);
            ext
        });
        
        async_graphql::Response::from_errors(vec![server_err]).into()
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
            apq_store: self.apq_store.clone(),
            circuit_breaker: self.circuit_breaker.clone(),
            response_cache: self.response_cache.clone(),
        }
    }
}

/// Extract type names from __typename fields in response
fn extract_types_from_response(response: &serde_json::Value) -> HashSet<String> {
    let mut types = HashSet::new();
    extract_types_recursive(response, &mut types);
    types
}

fn extract_types_recursive(value: &serde_json::Value, types: &mut HashSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(type_name)) = map.get("__typename") {
                types.insert(type_name.clone());
            }
            for v in map.values() {
                extract_types_recursive(v, types);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                extract_types_recursive(item, types);
            }
        }
        _ => {}
    }
}

/// Extract entity keys (Type#id) from response
fn extract_entities_from_response(response: &serde_json::Value) -> HashSet<String> {
    let mut entities = HashSet::new();
    extract_entities_recursive(response, &mut entities);
    entities
}

fn extract_entities_recursive(value: &serde_json::Value, entities: &mut HashSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            let type_name = map.get("__typename").and_then(|t| t.as_str());
            let id = map
                .get("id")
                .and_then(|i| i.as_str())
                .or_else(|| map.get("_id").and_then(|i| i.as_str()));

            if let (Some(tn), Some(id_val)) = (type_name, id) {
                entities.insert(format!("{}#{}", tn, id_val));
            }

            for v in map.values() {
                extract_entities_recursive(v, entities);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                extract_entities_recursive(item, entities);
            }
        }
        _ => {}
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

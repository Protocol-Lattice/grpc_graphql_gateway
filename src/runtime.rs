//! Runtime support for GraphQL gateway - HTTP and WebSocket integration.

use crate::analytics::{AnalyticsConfig, SharedQueryAnalytics};
use crate::cache::{CacheConfig, CacheLookupResult, SharedResponseCache};
use crate::circuit_breaker::{CircuitBreakerConfig, SharedCircuitBreakerRegistry};
use crate::compression::{create_compression_layer, CompressionConfig};
use crate::error::{GraphQLError, Result};
use crate::grpc_client::GrpcClientPool;
use crate::health::{health_handler, readiness_handler, HealthState};
use crate::high_performance::{
    pin_to_core, recommended_workers, FastJsonParser, HighPerfConfig, PerfMetrics,
    ResponseTemplates, ShardedCache,
};
use crate::metrics::GatewayMetrics;
use crate::middleware::{Context, Middleware};
use crate::persisted_queries::{
    process_apq_request, PersistedQueryConfig, PersistedQueryError, SharedPersistedQueryStore,
};
use crate::query_whitelist::{QueryWhitelistConfig, SharedQueryWhitelist};
use crate::request_collapsing::{RequestCollapsingConfig, SharedRequestCollapsingRegistry};
use crate::schema::{DynamicSchema, GrpcResponseCache};
use crate::defer::{
    has_defer_directive, strip_defer_directives, extract_deferred_fragments,
    DeferConfig, DeferredExecution, DeferredPart,
    format_initial_part, format_subsequent_part, MULTIPART_CONTENT_TYPE,
};
use crate::plugin::PluginRegistry;
use async_graphql::ServerError;
use async_graphql_axum::{GraphQLRequest, GraphQLResponse, GraphQLSubscription};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::HeaderMap,
    response::{Html, IntoResponse, Json},
    routing::{get, get_service, post},
    Extension, Router,
};
use futures::{SinkExt, StreamExt};
use bytes::Bytes;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    /// Enable GraphQL Playground
    playground_enabled: bool,
    /// APQ store for persisted queries
    apq_store: Option<SharedPersistedQueryStore>,
    /// Circuit breaker registry
    circuit_breaker: Option<SharedCircuitBreakerRegistry>,
    /// Response cache
    response_cache: Option<SharedResponseCache>,
    /// Response compression configuration
    compression_config: Option<CompressionConfig>,
    /// Query whitelist for security
    query_whitelist: Option<SharedQueryWhitelist>,
    /// Query analytics engine
    analytics: Option<SharedQueryAnalytics>,
    /// Request collapsing registry for deduplication
    request_collapsing: Option<SharedRequestCollapsingRegistry>,
    /// High-performance configuration
    high_perf_config: Option<HighPerfConfig>,
    /// Fast JSON parser (SIMD-accelerated)
    json_parser: Arc<FastJsonParser>,
    /// High-performance sharded cache
    sharded_cache: Option<Arc<ShardedCache<Bytes>>>,
    /// Performance metrics tracking
    perf_metrics: Arc<PerfMetrics>,
    /// Pre-computed response templates
    response_templates: Arc<ResponseTemplates>,
    /// @defer incremental delivery configuration
    defer_config: Option<DeferConfig>,
    /// Plugin registry for extension hooks
    plugins: PluginRegistry,
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
            playground_enabled: std::env::var("ENABLE_GRAPHQL_PLAYGROUND")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            apq_store: None,
            circuit_breaker: None,
            response_cache: None,
            compression_config: None,
            query_whitelist: None,
            analytics: None,
            request_collapsing: None,
            high_perf_config: None,
            json_parser: Arc::new(FastJsonParser::default()),
            sharded_cache: None,
            perf_metrics: Arc::new(PerfMetrics::default()),
            response_templates: Arc::new(ResponseTemplates::new()),
            defer_config: None,
            plugins: PluginRegistry::new(),
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

    /// Enable GraphQL Playground
    pub fn enable_playground(&mut self) {
        self.playground_enabled = true;
    }

    /// Enable Automatic Persisted Queries (APQ)
    pub fn enable_persisted_queries(&mut self, config: PersistedQueryConfig) {
        self.apq_store = Some(crate::persisted_queries::create_apq_store(config));
    }

    /// Enable Circuit Breaker for gRPC backend resilience
    pub fn enable_circuit_breaker(&mut self, config: CircuitBreakerConfig) {
        self.circuit_breaker = Some(crate::circuit_breaker::create_circuit_breaker_registry(
            config,
        ));
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

    /// Enable response compression
    pub fn enable_compression(&mut self, config: CompressionConfig) {
        self.compression_config = Some(config);
    }

    /// Get the compression config (if enabled)
    pub fn compression_config(&self) -> Option<&CompressionConfig> {
        self.compression_config.as_ref()
    }

    /// Enable query whitelist
    pub fn enable_query_whitelist(&mut self, config: QueryWhitelistConfig) {
        self.query_whitelist = Some(Arc::new(crate::query_whitelist::QueryWhitelist::new(
            config,
        )));
    }

    /// Get the query whitelist (if enabled)
    pub fn query_whitelist(&self) -> Option<&SharedQueryWhitelist> {
        self.query_whitelist.as_ref()
    }

    /// Enable query analytics
    pub fn enable_analytics(&mut self, config: AnalyticsConfig) {
        self.analytics = Some(crate::analytics::create_analytics(config));
    }

    /// Get the analytics engine (if enabled)
    pub fn analytics(&self) -> Option<&SharedQueryAnalytics> {
        self.analytics.as_ref()
    }

    /// Enable request collapsing for deduplicating identical gRPC calls
    pub fn enable_request_collapsing(&mut self, config: RequestCollapsingConfig) {
        self.request_collapsing =
            Some(crate::request_collapsing::create_request_collapsing_registry(config));
    }

    /// Get the request collapsing registry (if enabled)
    pub fn request_collapsing(&self) -> Option<&SharedRequestCollapsingRegistry> {
        self.request_collapsing.as_ref()
    }

    /// Enable high-performance optimizations for 100K+ RPS
    pub fn enable_high_performance(&mut self, config: HighPerfConfig) {
        self.json_parser = Arc::new(FastJsonParser::new(config.buffer_pool_size));
        self.sharded_cache = Some(Arc::new(ShardedCache::new(
            config.cache_shards,
            config.max_entries_per_shard,
        )));

        // Optional CPU affinity pinning
        if config.cpu_affinity {
            let num_cores = recommended_workers();
            for i in 0..num_cores {
                // Pinning usually happens in thread creation, but we can try for current
                let _ = pin_to_core(i);
            }
        }

        self.high_perf_config = Some(config);
    }

    /// Get high-performance metrics
    pub fn perf_metrics(&self) -> &PerfMetrics {
        &self.perf_metrics
    }

    /// Enable `@defer` incremental delivery
    pub fn enable_defer(&mut self, config: DeferConfig) {
        self.defer_config = Some(config);
    }

    /// Get the defer config (if enabled)
    pub fn defer_config(&self) -> Option<&DeferConfig> {
        self.defer_config.as_ref()
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

    pub fn set_plugins(&mut self, plugins: PluginRegistry) {
        self.plugins = plugins;
    }

    async fn execute_with_middlewares(
        &self,
        headers: HeaderMap,
        request: async_graphql::Request,
    ) -> Result<async_graphql::Response> {
        // Create context with request ID and timing info
        let mut ctx = Context {
            headers: headers.clone(),
            extensions: std::collections::HashMap::new(),
            request_start: std::time::Instant::now(),
            request_id: headers
                .get("x-request-id")
                .and_then(|v| v.to_str().ok())
                .map(String::from)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            client_ip: headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.split(',').next())
                .map(|s| s.trim().to_string())
                .or_else(|| {
                    headers
                        .get("x-real-ip")
                        .and_then(|v| v.to_str().ok())
                        .map(String::from)
                }),
            encryption_key: None,
        };

        for middleware in &self.middlewares {
            middleware.call(&mut ctx).await?;
        }

        // Plugin Hook: on_request
        self.plugins.on_request(&ctx, &request).await?;

        let mut gql_request = request;
        // Make context available to resolvers via data
        gql_request = gql_request.data(ctx.clone());
        gql_request = gql_request.data(self.plugins.clone());
        gql_request = gql_request.data(GrpcResponseCache::default());

        let response = self.schema.execute(gql_request).await;
        
        // Plugin Hook: on_response
        self.plugins.on_response(&ctx, &response).await?;

        Ok(response)
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
        request: async_graphql::Request,
    ) -> async_graphql::Response {
        let processed_request = if let Some(ref apq_store) = self.apq_store {
            match self.process_apq_request(apq_store, request) {
                Ok(req) => req,
                Err(apq_err) => {
                    return self.apq_error_response(apq_err);
                }
            }
        } else {
            request
        };

        // Handle @live directive - detect and strip before execution
        // The @live directive indicates the client wants live/reactive updates
        let is_live_query = crate::live_query::has_live_directive(&processed_request.query);
        let processed_request = if is_live_query {
            // Strip the @live directive so async-graphql doesn't reject it
            let stripped_query = crate::live_query::strip_live_directive(&processed_request.query);
            tracing::debug!(
                is_live = is_live_query,
                "Live query detected, stripping @live directive"
            );
            let mut new_request = async_graphql::Request::new(stripped_query)
                .variables(processed_request.variables);
            if let Some(op_name) = processed_request.operation_name {
                new_request = new_request.operation_name(op_name);
            }
            new_request
        } else {
            processed_request
        };


        // WAF Security Check: Validate request for SQL Injection patterns
        if let Err(err) = crate::waf::validate_request(&processed_request) {
            tracing::warn!("WAF blocked request: {}", err);
            let mut server_err = ServerError::new(err.to_string(), None);
            server_err.extensions = Some({
                let mut ext = async_graphql::ErrorExtensionValues::default();
                ext.set("code", "VALIDATION_ERROR");
                ext
            });
            return async_graphql::Response::from_errors(vec![server_err]);
        }

        // Validate query against whitelist if enabled
        if let Some(ref whitelist) = self.query_whitelist {
            // Extract operation ID from extensions if present
            let operation_id = processed_request
                .extensions
                .get("operationId")
                .and_then(|v| serde_json::to_value(v).ok())
                .and_then(|v| v.as_str().map(String::from));

            if let Err(err) =
                whitelist.validate_query(&processed_request.query, operation_id.as_deref())
            {
                tracing::warn!("Query whitelist validation failed: {}", err);
                let mut server_err = ServerError::new(err.to_string(), None);
                server_err.extensions = Some({
                    let mut ext = async_graphql::ErrorExtensionValues::default();
                    ext.set("code", "QUERY_NOT_WHITELISTED");
                    ext
                });
                return async_graphql::Response::from_errors(vec![server_err]);
            }
        }

        // Check if this is a mutation (mutations are never cached and trigger invalidation)
        let is_mutation = crate::cache::is_mutation(&processed_request.query);
        let operation_type = if is_mutation { "mutation" } else { "query" };

        // Store analytics info before processing
        let analytics_query = processed_request.query.clone();
        let analytics_op_name = processed_request.operation_name.clone();
        let request_start = Instant::now();

        // Extract vary headers for cache key generation
        let vary_header_values = if let Some(ref cache) = self.response_cache {
            cache
                .config
                .vary_headers
                .iter()
                .map(|h| {
                    let val = headers.get(h).and_then(|v| v.to_str().ok()).unwrap_or("");
                    format!("{}:{}", h, val)
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        // Try cache lookup for non-mutations
        if !is_mutation {
            if let Some(ref cache) = self.response_cache {
                let cache_key = crate::cache::ResponseCache::generate_cache_key(
                    &processed_request.query,
                    Some(&serde_json::to_value(&processed_request.variables).unwrap_or_default()),
                    processed_request.operation_name.as_deref(),
                    &vary_header_values,
                );

                match cache.get(&cache_key).await {
                    CacheLookupResult::Hit(cached) => {
                        tracing::debug!("Response cache hit");
                        // Track cache hit in analytics
                        if let Some(ref analytics) = self.analytics {
                            analytics.record_cache_access(true);
                            analytics.record_query(
                                &analytics_query,
                                analytics_op_name.as_deref(),
                                operation_type,
                                request_start.elapsed(),
                                false,
                                None,
                            );
                        }
                        return self.cached_to_response(cached.data);
                    }
                    CacheLookupResult::Stale(cached) => {
                        // Return stale immediately, could trigger background revalidation
                        // For simplicity, we just return stale data
                        tracing::debug!("Response cache stale hit");
                        // Track stale hit as a hit
                        if let Some(ref analytics) = self.analytics {
                            analytics.record_cache_access(true);
                            analytics.record_query(
                                &analytics_query,
                                analytics_op_name.as_deref(),
                                operation_type,
                                request_start.elapsed(),
                                false,
                                None,
                            );
                        }
                        return self.cached_to_response(cached.data);
                    }
                    CacheLookupResult::Miss => {
                        // Track cache miss
                        if let Some(ref analytics) = self.analytics {
                            analytics.record_cache_access(false);
                        }
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
        match self
            .execute_with_middlewares(headers, processed_request)
            .await
        {
            Ok(resp) => {
                let duration = request_start.elapsed();
                let had_error = !resp.errors.is_empty();

                // Track in analytics
                if let Some(ref analytics) = self.analytics {
                    let error_details = if had_error {
                        resp.errors.first().map(|e| {
                            let code = e
                                .extensions
                                .as_ref()
                                .and_then(|ext| ext.get("code"))
                                .map(|c| c.to_string())
                                .unwrap_or_else(|| "GRAPHQL_ERROR".to_string());
                            (code, e.message.clone())
                        })
                    } else {
                        None
                    };

                    analytics.record_query(
                        &analytics_query,
                        analytics_op_name.as_deref(),
                        operation_type,
                        duration,
                        had_error,
                        error_details
                            .as_ref()
                            .map(|(c, m)| (c.as_str(), m.as_str())),
                    );
                }

                // Handle mutation cache invalidation
                if is_mutation {
                    if let Some(ref cache) = self.response_cache {
                        if let Ok(resp_json) = serde_json::to_value(&resp) {
                            cache.invalidate_for_mutation(&resp_json).await;
                        }
                    }
                    if let Some(ref sharded) = self.sharded_cache {
                        sharded.clear(); // Simple invalidation for sharded cache on mutation
                    }
                } else if let Some((query, vars, op_name)) = cache_query_info {
                    // Cache the response for queries
                    if let Some(ref cache) = self.response_cache {
                        let cache_key = crate::cache::ResponseCache::generate_cache_key(
                            &query,
                            Some(&vars),
                            op_name.as_deref(),
                            &vary_header_values,
                        );

                        if let Ok(resp_json) = serde_json::to_value(&resp) {
                            // Extract types and entities for invalidation tracking
                            let types = extract_types_from_response(&resp_json);
                            let entities = extract_entities_from_response(&resp_json);
                            cache
                                .put(cache_key.clone(), resp_json.clone(), types, entities)
                                .await;

                            // Explicitly cache entities (Cache by Field result)
                            cache.put_all_entities(&resp_json, None).await;

                            // Also cache in sharded cache if enabled
                            if let Some(ref sharded) = self.sharded_cache {
                                if let Ok(resp_bytes) = self.json_parser.serialize(&resp) {
                                    sharded.insert(&cache_key, resp_bytes, Duration::from_secs(60));
                                }
                            }
                        }
                    }
                }
                resp
            }
            Err(err) => {
                let duration = request_start.elapsed();
                let gql_err: GraphQLError = err.into();

                // Track error in analytics
                if let Some(ref analytics) = self.analytics {
                    analytics.record_query(
                        &analytics_query,
                        analytics_op_name.as_deref(),
                        operation_type,
                        duration,
                        true,
                        Some(("INTERNAL_ERROR", &gql_err.message)),
                    );
                }

                if let Some(handler) = &self.error_handler {
                    handler(vec![gql_err.clone()]);
                }
                let server_err = ServerError::new(gql_err.message.clone(), None);
                async_graphql::Response::from_errors(vec![server_err])
            }
        }
    }

    /// High-performance GraphQL handler for maximum throughput
    pub async fn handle_fast(&self, headers: HeaderMap, body: Bytes) -> axum::response::Response {
        let start = Instant::now();

        // 1. SIMD JSON parsing
        let request_val = match self.json_parser.parse_bytes(&body) {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!("Failed to parse JSON with SIMD: {}", err);
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    self.response_templates
                        .errors
                        .get("PARSE_ERROR")
                        .cloned()
                        .unwrap_or_else(|| {
                            Bytes::from(r#"{"errors":[{"message":"Invalid JSON"}]}"#)
                        }),
                )
                    .into_response();
            }
        };

        let query = request_val["query"].as_str().unwrap_or("");
        let variables = &request_val["variables"];
        let operation_name = request_val["operationName"].as_str();

        // 2. High-performance cache lookup
        if let Some(ref sharded) = self.sharded_cache {
            let vary_header_values: Vec<String> = if let Some(ref cache) = self.response_cache {
                cache
                    .config
                    .vary_headers
                    .iter()
                    .map(|h| {
                        format!(
                            "{}:{}",
                            h,
                            headers.get(h).and_then(|v| v.to_str().ok()).unwrap_or("")
                        )
                    })
                    .collect()
            } else {
                Vec::new()
            };

            let cache_key = crate::cache::ResponseCache::generate_cache_key(
                query,
                Some(variables),
                operation_name,
                &vary_header_values,
            );

            if let Some(cached_bytes) = sharded.get(&cache_key) {
                self.perf_metrics
                    .record(start.elapsed().as_nanos() as u64, true);
                return (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    cached_bytes,
                )
                    .into_response();
            }
        }

        // 3. Fallback to normal execution for cache miss
        // Convert to async_graphql::Request
        let mut gql_req = async_graphql::Request::new(query);
        if !variables.is_null() {
            if let Ok(vars) = serde_json::from_value(variables.clone()) {
                gql_req = gql_req.variables(vars);
            }
        }
        if let Some(op) = operation_name {
            gql_req = gql_req.operation_name(op);
        }

        let resp = self.handle_http(headers, gql_req).await;

        // Record metrics
        self.perf_metrics
            .record(start.elapsed().as_nanos() as u64, false);

        GraphQLResponse::from(resp).into_response()
    }

    /// Convert cached JSON to GraphQL response
    fn cached_to_response(&self, data: serde_json::Value) -> async_graphql::Response {
        // Try to deserialize as Response, or create a simple data response
        match serde_json::from_value::<async_graphql::Response>(data.clone()) {
            Ok(resp) => resp,
            Err(_) => {
                // Fallback: wrap in a simple response
                async_graphql::Response::new(
                    serde_json::from_value::<async_graphql::Value>(data)
                        .unwrap_or(async_graphql::Value::Null),
                )
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
    fn apq_error_response(&self, err: PersistedQueryError) -> async_graphql::Response {
        let error_extensions = err.to_extensions();
        let code = error_extensions
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("PERSISTED_QUERY_ERROR");

        let mut server_err = ServerError::new(err.to_string(), None);
        server_err.extensions = Some({
            let mut ext = async_graphql::ErrorExtensionValues::default();
            ext.set("code", code);
            ext
        });

        async_graphql::Response::from_errors(vec![server_err])
    }

    /// Convert to Axum router
    ///
    /// # Security Features
    ///
    /// - Request body limit (1MB default) to prevent memory exhaustion
    /// - Security headers (X-Content-Type-Options, X-Frame-Options)
    /// - GraphQL Playground disabled by default (enable with ENABLE_GRAPHQL_PLAYGROUND=true)
    /// - Analytics endpoints require ANALYTICS_API_KEY for access
    pub fn into_router(self) -> Router {
        use axum::middleware as axum_mw;
        use axum::response::Response;

        let health_checks_enabled = self.health_checks_enabled;
        let metrics_enabled = self.metrics_enabled;
        let analytics_enabled = self.analytics.is_some();
        let client_pool = self.client_pool.clone();
        let compression_config = self.compression_config.clone();

        let state = Arc::new(self.clone());
        let subscription = GraphQLSubscription::new(state.schema.executor());

        // SECURITY: Check if playground should be enabled (default: disabled)
        let playground_enabled = self.playground_enabled;

        let state = Arc::new(self);
        let use_fast_path = state.high_perf_config.is_some();

        let router = Router::new();
        let router = if use_fast_path {
            // When fast path is enabled, still route @defer queries through the standard handler
            router
                .route("/graphql", post(handle_graphql_fast_or_defer))
        } else {
            router.route("/graphql", post(handle_graphql_post))
        };

        let router = if playground_enabled {
            router.route("/graphql", get(graphql_playground))
        } else {
            router
        };

        let mut router = router
            .route_service("/graphql/ws", get_service(subscription))
            .route("/graphql/live", get(handle_live_query_ws))
            .route("/graphql/defer", post(handle_graphql_defer))
            .layer(Extension(state.schema.executor()))
            .with_state(state.clone());

        // SECURITY: Add request body limit (1MB) to prevent memory exhaustion
        router = router.layer(axum::extract::DefaultBodyLimit::max(1024 * 1024));

        // SECURITY: Add security headers using axum middleware
        async fn add_security_headers(
            req: axum::http::Request<axum::body::Body>,
            next: axum_mw::Next,
        ) -> Response {
            // Handle OPTIONS preflight for CORS
            if req.method() == axum::http::Method::OPTIONS {
                let mut response = Response::builder()
                    .status(axum::http::StatusCode::NO_CONTENT)
                    .body(axum::body::Body::empty())
                    .unwrap();
                let headers = response.headers_mut();
                // CORS headers for preflight
                headers.insert(
                    axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
                    axum::http::HeaderValue::from_static("*"),
                );
                headers.insert(
                    axum::http::header::ACCESS_CONTROL_ALLOW_METHODS,
                    axum::http::HeaderValue::from_static("GET, POST, OPTIONS"),
                );
                headers.insert(
                    axum::http::header::ACCESS_CONTROL_ALLOW_HEADERS,
                    axum::http::HeaderValue::from_static(
                        "Content-Type, Authorization, X-Request-ID",
                    ),
                );
                headers.insert(
                    axum::http::header::ACCESS_CONTROL_MAX_AGE,
                    axum::http::HeaderValue::from_static("86400"),
                );
                return response;
            }

            let mut response = next.run(req).await;
            let headers = response.headers_mut();

            // Core security headers
            headers.insert(
                axum::http::header::X_CONTENT_TYPE_OPTIONS,
                axum::http::HeaderValue::from_static("nosniff"),
            );
            headers.insert(
                axum::http::header::X_FRAME_OPTIONS,
                axum::http::HeaderValue::from_static("DENY"),
            );

            // HSTS (Strict-Transport-Security) - tells browsers to only use HTTPS
            // max-age=31536000 (1 year), includeSubDomains for comprehensive protection
            headers.insert(
                axum::http::header::STRICT_TRANSPORT_SECURITY,
                axum::http::HeaderValue::from_static("max-age=31536000; includeSubDomains"),
            );

            // Prevent caching of sensitive responses
            headers.insert(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-store, no-cache, must-revalidate"),
            );

            // XSS Protection (legacy but still useful for older browsers)
            headers.insert(
                axum::http::header::HeaderName::from_static("x-xss-protection"),
                axum::http::HeaderValue::from_static("1; mode=block"),
            );

            // Content Security Policy - restrict resource loading
            // Added object-src 'none', base-uri 'self', frame-ancestors 'none' for stricter security
            // Added https://cdn.jsdelivr.net and https://unpkg.com to allow playground to work
            headers.insert(
                axum::http::header::CONTENT_SECURITY_POLICY,
                axum::http::HeaderValue::from_static(
                    "default-src 'self'; script-src 'self' 'unsafe-inline' https://cdn.jsdelivr.net https://unpkg.com; style-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'self'; frame-ancestors 'none'",
                ),
            );

            // Referrer Policy - limit referrer information leakage
            headers.insert(
                axum::http::header::REFERRER_POLICY,
                axum::http::HeaderValue::from_static("strict-origin-when-cross-origin"),
            );

            // Permissions Policy - Limit browser features
            headers.insert(
                axum::http::header::HeaderName::from_static("permissions-policy"),
                axum::http::HeaderValue::from_static("camera=(), microphone=(), geolocation=(), browsing-topics=(), payment=()"),
            );

            // DNS Prefetch Control - Privacy
            headers.insert(
                axum::http::header::HeaderName::from_static("x-dns-prefetch-control"),
                axum::http::HeaderValue::from_static("off"),
            );

            // Cross-Origin policies
            headers.insert(
                axum::http::header::HeaderName::from_static("cross-origin-opener-policy"),
                axum::http::HeaderValue::from_static("same-origin"),
            );
            headers.insert(
                axum::http::header::HeaderName::from_static("cross-origin-embedder-policy"),
                axum::http::HeaderValue::from_static("require-corp"),
            );
            headers.insert(
                axum::http::header::HeaderName::from_static("cross-origin-resource-policy"),
                axum::http::HeaderValue::from_static("same-origin"),
            );

            // CORS headers for regular requests
            headers.insert(
                axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
                axum::http::HeaderValue::from_static("*"),
            );

            response
        }

        router = router.layer(axum_mw::from_fn(add_security_headers));

        // Add compression layer if enabled
        if let Some(ref config) = compression_config {
            if config.enabled {
                // Add standard compression (brotli, gzip, etc.)
                router = router.layer(create_compression_layer(config));

                // Add custom ultra-fast compression (LZ4, GBP-LZ4)
                if config.lz4_enabled() || config.gbp_lz4_enabled() {
                    router = router.layer(axum::middleware::from_fn(
                        crate::lz4_compression::lz4_compression_middleware,
                    ));
                }
            }
        }

        // Add health check routes if enabled
        if health_checks_enabled {
            let health_state = Arc::new(HealthState::new(
                client_pool.unwrap_or_default(),
            ));
            router = router
                .route("/health", get(health_handler))
                .route("/ready", get(readiness_handler).with_state(health_state));
        }

        // Add metrics route if enabled (consider adding auth in production)
        if metrics_enabled {
            router = router.route("/metrics", get(metrics_handler));
        }

        // Add analytics routes if enabled (protected by API key)
        if analytics_enabled {
            router = router
                .route("/analytics", get(analytics_dashboard_handler))
                .route(
                    "/analytics/api",
                    get(analytics_api_handler).with_state(state.clone()),
                )
                .route(
                    "/analytics/reset",
                    post(analytics_reset_handler).with_state(state),
                );
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
            playground_enabled: self.playground_enabled,
            apq_store: self.apq_store.clone(),
            circuit_breaker: self.circuit_breaker.clone(),
            response_cache: self.response_cache.clone(),
            compression_config: self.compression_config.clone(),
            query_whitelist: self.query_whitelist.clone(),
            analytics: self.analytics.clone(),
            request_collapsing: self.request_collapsing.clone(),
            high_perf_config: self.high_perf_config.clone(),
            json_parser: self.json_parser.clone(),
            sharded_cache: self.sharded_cache.clone(),
            perf_metrics: self.perf_metrics.clone(),
            response_templates: self.response_templates.clone(),
            defer_config: self.defer_config.clone(),
            plugins: self.plugins.clone(),
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
///
/// This handler also supports `@defer` — when a query contains `@defer` and
/// the `Accept` header includes `multipart/mixed`, the response is streamed
/// as incremental delivery. Otherwise, a regular JSON response is returned.
async fn handle_graphql_post(
    State(mux): State<Arc<ServeMux>>,
    headers: HeaderMap,
    request: GraphQLRequest,
) -> axum::response::Response {
    let gql_request = request.into_inner();

    // Check if the client accepts multipart/mixed and the query contains @defer
    let accepts_multipart = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("multipart/mixed"));

    if accepts_multipart && has_defer_directive(&gql_request.query) {
        if let Some(config) = mux.defer_config().cloned() {
            if config.enabled {
                let query = gql_request.query.clone();
                let fragments = extract_deferred_fragments(&query);

                if fragments.len() <= config.max_deferred_fragments {
                    let stripped_query = strip_defer_directives(&query);
                    let mut eager_request = async_graphql::Request::new(stripped_query)
                        .variables(gql_request.variables);
                    if let Some(op_name) = gql_request.operation_name {
                        eager_request = eager_request.operation_name(op_name);
                    }

                    let full_response = mux.handle_http(headers, eager_request).await;
                    let full_json = serde_json::to_value(&full_response).unwrap_or_else(|_| {
                        serde_json::json!({"data": null, "errors": [{"message": "Serialization failed"}]})
                    });

                    let boundary = config.multipart_boundary.clone();
                    let (exec, mut rx) = DeferredExecution::new(config, fragments);

                    tokio::spawn(async move {
                        if let Err(e) = exec.execute(full_json).await {
                            tracing::warn!(error = %e, "Deferred execution failed");
                        }
                    });

                    let stream = async_stream::stream! {
                        while let Some(part) = rx.recv().await {
                            match part {
                                DeferredPart::Initial(payload) => {
                                    yield Ok::<_, std::convert::Infallible>(
                                        format_initial_part(&payload, &boundary)
                                    );
                                }
                                DeferredPart::Subsequent(payload) => {
                                    let is_last = !payload.has_next;
                                    yield Ok::<_, std::convert::Infallible>(
                                        format_subsequent_part(&payload, &boundary)
                                    );
                                    if is_last {
                                        break;
                                    }
                                }
                            }
                        }
                    };

                    let body = axum::body::Body::from_stream(stream);
                    return axum::response::Response::builder()
                        .header("Content-Type", MULTIPART_CONTENT_TYPE)
                        .header("Transfer-Encoding", "chunked")
                        .header("Cache-Control", "no-cache")
                        .body(body)
                        .unwrap_or_else(|_| {
                            axum::response::Response::builder()
                                .status(500)
                                .body(axum::body::Body::from("Internal Server Error"))
                                .unwrap()
                        });
                }
            }
        }
    }

    // Default: regular JSON response
    GraphQLResponse::from(mux.handle_http(headers, gql_request).await).into_response()
}

/// Handler for high-performance POST requests to /graphql
#[allow(dead_code)]
async fn handle_graphql_fast(
    State(mux): State<Arc<ServeMux>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    mux.handle_fast(headers, body).await
}

/// Combined handler: fast path for normal queries, standard path for @defer.
///
/// When `Accept: multipart/mixed` is present, the request is parsed as a
/// GraphQL request and routed through `handle_graphql_post` which handles
/// `@defer`. Otherwise, the high-performance fast path is used.
async fn handle_graphql_fast_or_defer(
    State(mux): State<Arc<ServeMux>>,
    headers: HeaderMap,
    body: Bytes,
) -> axum::response::Response {
    let accepts_multipart = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("multipart/mixed"));

    if accepts_multipart && mux.defer_config().is_some_and(|c| c.enabled) {
        // Parse the body as a GraphQL request and handle with defer support
        if let Ok(gql_request) = serde_json::from_slice::<async_graphql::Request>(&body) {
            if has_defer_directive(&gql_request.query) {
                let config = mux.defer_config().unwrap().clone();
                let query = gql_request.query.clone();
                let fragments = extract_deferred_fragments(&query);

                if fragments.len() <= config.max_deferred_fragments {
                    let stripped_query = strip_defer_directives(&query);
                    let mut eager_request = async_graphql::Request::new(stripped_query)
                        .variables(gql_request.variables);
                    if let Some(op_name) = gql_request.operation_name {
                        eager_request = eager_request.operation_name(op_name);
                    }

                    let full_response = mux.handle_http(headers, eager_request).await;
                    let full_json = serde_json::to_value(&full_response).unwrap_or_else(|_| {
                        serde_json::json!({"data": null, "errors": [{"message": "Serialization failed"}]})
                    });

                    let boundary = config.multipart_boundary.clone();
                    let (exec, mut rx) = DeferredExecution::new(config, fragments);

                    tokio::spawn(async move {
                        if let Err(e) = exec.execute(full_json).await {
                            tracing::warn!(error = %e, "Deferred execution failed");
                        }
                    });

                    let stream = async_stream::stream! {
                        while let Some(part) = rx.recv().await {
                            match part {
                                DeferredPart::Initial(payload) => {
                                    yield Ok::<_, std::convert::Infallible>(
                                        format_initial_part(&payload, &boundary)
                                    );
                                }
                                DeferredPart::Subsequent(payload) => {
                                    let is_last = !payload.has_next;
                                    yield Ok::<_, std::convert::Infallible>(
                                        format_subsequent_part(&payload, &boundary)
                                    );
                                    if is_last {
                                        break;
                                    }
                                }
                            }
                        }
                    };

                    let body = axum::body::Body::from_stream(stream);
                    return axum::response::Response::builder()
                        .header("Content-Type", MULTIPART_CONTENT_TYPE)
                        .header("Transfer-Encoding", "chunked")
                        .header("Cache-Control", "no-cache")
                        .body(body)
                        .unwrap_or_else(|_| {
                            axum::response::Response::builder()
                                .status(500)
                                .body(axum::body::Body::from("Internal Server Error"))
                                .unwrap()
                        });
                }
            }
        }
    }

    mux.handle_fast(headers, body).await.into_response()
}

/// Handler for POST requests to /graphql/defer — `@defer` incremental delivery
///
/// This endpoint supports the `@defer` directive by returning a
/// `multipart/mixed` response. The first part contains the eagerly-resolved
/// initial payload and subsequent parts contain incremental patches for
/// deferred fragments.
///
/// If the query does not contain `@defer`, or if defer is disabled in the
/// gateway configuration, it falls back to a regular JSON response.
///
/// # Multipart Response Format
///
/// ```text
/// Content-Type: multipart/mixed; boundary="-"
///
/// ---
/// Content-Type: application/json; charset=utf-8
///
/// {"data":{"user":{"id":"1","name":"Alice"}},"hasNext":true}
/// ---
/// Content-Type: application/json; charset=utf-8
///
/// {"incremental":[{"data":{"email":"alice@example.com"},"path":["user"],"label":"details"}],"hasNext":false}
/// -----
/// ```
async fn handle_graphql_defer(
    State(mux): State<Arc<ServeMux>>,
    headers: HeaderMap,
    request: GraphQLRequest,
) -> axum::response::Response {
    let gql_request = request.into_inner();
    let query = gql_request.query.clone();

    // Check if @defer is present and enabled
    let defer_config = mux.defer_config().cloned();
    let is_deferred = has_defer_directive(&query);

    if !is_deferred || defer_config.as_ref().is_none_or(|c| !c.enabled) {
        // No @defer or disabled — fall back to normal execution
        let resp = mux.handle_http(headers, gql_request).await;
        return GraphQLResponse::from(resp).into_response();
    }

    let config = defer_config.unwrap();

    // Extract deferred fragments from the original query
    let fragments = extract_deferred_fragments(&query);

    // Validate fragment count
    if fragments.len() > config.max_deferred_fragments {
        let err = ServerError::new(
            format!(
                "Too many @defer fragments ({}/{})",
                fragments.len(),
                config.max_deferred_fragments
            ),
            None,
        );
        let resp = async_graphql::Response::from_errors(vec![err]);
        return GraphQLResponse::from(resp).into_response();
    }

    // Strip @defer directives and execute the full query eagerly
    let stripped_query = strip_defer_directives(&query);
    let mut eager_request = async_graphql::Request::new(stripped_query)
        .variables(gql_request.variables);
    if let Some(op_name) = gql_request.operation_name {
        eager_request = eager_request.operation_name(op_name);
    }

    tracing::debug!(
        deferred_fragments = fragments.len(),
        "Executing @defer query with eager resolution"
    );

    // Execute the full query
    let full_response = mux.handle_http(headers, eager_request).await;

    // Convert to serde_json::Value for splitting
    let full_json = serde_json::to_value(&full_response).unwrap_or_else(|_| {
        serde_json::json!({"data": null, "errors": [{"message": "Serialization failed"}]})
    });

    // Create the deferred execution engine
    let boundary = config.multipart_boundary.clone();
    let (exec, mut rx) = DeferredExecution::new(config, fragments);

    // Spawn the deferred execution in the background
    tokio::spawn(async move {
        if let Err(e) = exec.execute(full_json).await {
            tracing::warn!(error = %e, "Deferred execution failed");
        }
    });

    // Build a streaming body from the receiver
    let stream = async_stream::stream! {
        while let Some(part) = rx.recv().await {
            match part {
                DeferredPart::Initial(payload) => {
                    yield Ok::<_, std::convert::Infallible>(
                        format_initial_part(&payload, &boundary)
                    );
                }
                DeferredPart::Subsequent(payload) => {
                    let is_last = !payload.has_next;
                    yield Ok::<_, std::convert::Infallible>(
                        format_subsequent_part(&payload, &boundary)
                    );
                    if is_last {
                        break;
                    }
                }
            }
        }
    };

    let body = axum::body::Body::from_stream(stream);

    axum::response::Response::builder()
        .header("Content-Type", MULTIPART_CONTENT_TYPE)
        .header("Transfer-Encoding", "chunked")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .body(body)
        .unwrap_or_else(|_| {
            axum::response::Response::builder()
                .status(500)
                .body(axum::body::Body::from("Internal Server Error"))
                .unwrap()
        })
}

/// Serve the GraphQL Playground UI for ad-hoc exploration.
///
/// # Security
///
/// This endpoint is only available when ENABLE_GRAPHQL_PLAYGROUND=true.
/// It should be disabled in production to prevent schema exploration.
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
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        body,
    )
}

/// Handler for analytics dashboard HTML
async fn analytics_dashboard_handler() -> impl IntoResponse {
    Html(crate::analytics::analytics_dashboard_html())
}

/// Handler for analytics API endpoint (JSON)
///
/// # Security
///
/// This endpoint exposes internal metrics. In production, set ANALYTICS_API_KEY
/// environment variable to require authentication.
async fn analytics_api_handler(
    State(mux): State<Arc<ServeMux>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // SECURITY: Check API key if configured
    if let Ok(required_key) = std::env::var("ANALYTICS_API_KEY") {
        let provided_key = headers
            .get("x-analytics-key")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if provided_key != required_key {
            return Json(serde_json::json!({
                "error": "Unauthorized",
                "message": "Valid x-analytics-key header required"
            }));
        }
    }

    if let Some(ref analytics) = mux.analytics {
        let snapshot = analytics.get_snapshot();
        Json(
            serde_json::to_value(snapshot)
                .unwrap_or_else(|_| serde_json::json!({"error": "Failed to serialize analytics"})),
        )
    } else {
        Json(serde_json::json!({"error": "Analytics not enabled"}))
    }
}

/// Handler for analytics reset endpoint
///
/// # Security
///
/// This endpoint can reset analytics data. Protected by ANALYTICS_API_KEY.
async fn analytics_reset_handler(
    State(mux): State<Arc<ServeMux>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // SECURITY: Check API key if configured
    if let Ok(required_key) = std::env::var("ANALYTICS_API_KEY") {
        let provided_key = headers
            .get("x-analytics-key")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if provided_key != required_key {
            return Json(serde_json::json!({
                "error": "Unauthorized",
                "message": "Valid x-analytics-key header required"
            }));
        }
    }

    if let Some(ref analytics) = mux.analytics {
        analytics.reset();
        Json(serde_json::json!({"status": "ok", "message": "Analytics reset successfully"}))
    } else {
        Json(serde_json::json!({"error": "Analytics not enabled"}))
    }
}

// =============================================================================
// SO_REUSEPORT Multi-Listener Server (High-Throughput)
// =============================================================================

/// Build a TCP listener with performance-oriented socket options.
///
/// Sets:
/// - `TCP_NODELAY`: Disables Nagle's algorithm for lower latency.
/// - `SO_REUSEADDR`: Allows reuse of the address after restart.
/// - `SO_REUSEPORT` (Linux/macOS): Allows multiple sockets on the same port
///   so the kernel can distribute `accept()` load across threads.
/// - Large backlog (4096): Handles burst connection queues without dropping.
///
/// # Platform
///
/// `SO_REUSEPORT` is supported on Linux ≥ 3.9 and macOS ≥ 10.9.
/// On other platforms this falls back to a regular `TcpListener`.
pub fn build_tcp_listener_tuned(addr: &str) -> std::io::Result<std::net::TcpListener> {
    use std::net::SocketAddr;

    let addr: SocketAddr = addr.parse().map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("invalid addr: {e}"))
    })?;

    let socket = socket2::Socket::new(
        if addr.is_ipv6() {
            socket2::Domain::IPV6
        } else {
            socket2::Domain::IPV4
        },
        socket2::Type::STREAM,
        None,
    )?;

    socket.set_reuse_address(true)?;

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    socket.set_reuse_port(true)?;

    socket.set_nodelay(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    // Large backlog for burst connection handling
    socket.listen(4096)?;

    Ok(socket.into())
}

/// High-throughput server using one `SO_REUSEPORT` listener per worker.
///
/// Creates `num_workers` independent `TcpListener`s bound to the same address
/// with `SO_REUSEPORT`. The kernel load-balances incoming connections across
/// them at the socket level — eliminating the single shared `accept()` queue
/// bottleneck that exists with a single listener.
///
/// # Performance Impact
///
/// - Eliminates accept-queue lock contention at high connection rates.
/// - Enables true per-core connection handling (same technique used by nginx).
/// - Best combined with `HighPerfConfig::ultra_fast()` and `mimalloc`.
///
/// # Example
///
/// ```rust,no_run
/// use grpc_graphql_gateway::runtime::serve_reuseport;
/// use axum::Router;
///
/// # async fn example() -> anyhow::Result<()> {
/// let app = Router::new();
/// serve_reuseport("0.0.0.0:8080", 8, app).await?;
/// # Ok(())
/// # }
/// ```
pub async fn serve_reuseport(
    addr: &str,
    num_workers: usize,
    app: axum::Router,
) -> crate::error::Result<()> {
    let mut handles = Vec::with_capacity(num_workers);
    let addr_owned = addr.to_string();

    for i in 0..num_workers {
        let std_listener = build_tcp_listener_tuned(addr).map_err(|e| {
            crate::error::Error::Internal(format!(
                "Failed to bind worker {i} listener on {addr}: {e}"
            ))
        })?;

        std_listener.set_nonblocking(true).map_err(|e| {
            crate::error::Error::Internal(format!("set_nonblocking failed: {e}"))
        })?;

        let listener = tokio::net::TcpListener::from_std(std_listener).map_err(|e| {
            crate::error::Error::Internal(format!("from_std failed: {e}"))
        })?;

        let app = app.clone();
        let addr_clone = addr_owned.clone();
        let handle = tokio::spawn(async move {
            tracing::info!("Worker {i} accepting on {addr_clone} (SO_REUSEPORT)");
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!("Worker {i} server error: {e}");
            }
        });

        handles.push(handle);
    }

    // Wait for all workers
    for handle in handles {
        let _ = handle.await;
    }

    Ok(())
}

/// WebSocket handler for live queries
///
/// This endpoint handles `@live` queries by:
/// 1. Executing the query as a regular GraphQL query
/// 2. Returning the initial result immediately
/// 3. Keeping the connection open to push updates when data changes
///
/// Protocol (same as graphql-transport-ws):
/// - Client sends: `{"type": "connection_init"}`
/// - Server responds: `{"type": "connection_ack"}`
/// - Client sends: `{"type": "subscribe", "id": "1", "payload": {"query": "query @live { ... }"}}`
/// - Server responds: `{"type": "next", "id": "1", "payload": {"data": {...}}}`
/// - Server can send more `next` messages when data updates
/// - Client or server sends: `{"type": "complete", "id": "1"}` to end
async fn handle_live_query_ws(
    ws: WebSocketUpgrade,
    State(mux): State<Arc<ServeMux>>,
) -> impl IntoResponse {
    ws.protocols(["graphql-transport-ws"])
        .on_upgrade(move |socket| handle_live_socket(socket, mux))
}

/// Handle the live query WebSocket connection with auto-push updates and GBP compression
async fn handle_live_socket(socket: WebSocket, mux: Arc<ServeMux>) {
    use std::collections::HashMap;
    use tokio::sync::mpsc;
    
    let (sender, mut receiver) = socket.split();
    
    #[derive(serde::Deserialize)]
    struct WsMessage {
        #[serde(rename = "type")]
        msg_type: String,
        id: Option<String>,
        payload: Option<serde_json::Value>,
    }
    
    #[derive(serde::Serialize, Clone)]
    struct WsResponse {
        #[serde(rename = "type")]
        msg_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        payload: Option<serde_json::Value>,
    }
    
    // Track active live subscriptions
    #[derive(Clone)]
    struct LiveSubscription {
        id: String,
        query: String,
        variables: Option<serde_json::Value>,
        operation_name: Option<String>,
        triggers: Vec<String>,
        is_live: bool,
    }
    
    // Track compression preference (negotiated during connection_init)
    let use_gbp_compression = Arc::new(parking_lot::RwLock::new(false));
    
    let mut connection_initialized = false;
    let active_subscriptions: Arc<parking_lot::RwLock<HashMap<String, LiveSubscription>>> = 
        Arc::new(parking_lot::RwLock::new(HashMap::new()));
    
    // Channel to send messages to the WebSocket
    let (ws_tx, mut ws_rx) = mpsc::channel::<WsResponse>(100);
    
    // Get live query store for invalidation events
    let live_query_store = crate::live_query::create_live_query_store();
    let mut invalidation_rx = live_query_store.subscribe_invalidations();
    
    // Spawn task to forward messages to WebSocket
    let _ws_tx_clone = ws_tx.clone();
    let sender = Arc::new(tokio::sync::Mutex::new(sender));
    let sender_clone = sender.clone();
    
    let use_compression_clone = use_gbp_compression.clone();
    let forward_task = tokio::spawn(async move {
        while let Some(msg) = ws_rx.recv().await {
            let mut sender = sender_clone.lock().await;
            
            // Check if GBP compression is enabled for this connection
            if *use_compression_clone.read() {
                // Use GBP binary compression
                if let Some(payload) = &msg.payload {
                    match crate::gbp::GbpEncoder::new().encode_lz4(payload) {
                        Ok(compressed) => {
                            // Create envelope: {type, id, compressed_payload}
                            let envelope = serde_json::json!({
                                "type": msg.msg_type,
                                "id": msg.id,
                                "compressed": true
                            });
                            let envelope_json = serde_json::to_string(&envelope).unwrap();
                            
                            // Send envelope + binary payload as separate frames
                            // Frame 1: JSON envelope
                            if sender.send(Message::Text(envelope_json.into())).await.is_err() {
                                break;
                            }
                            // Frame 2: Binary GBP payload
                            if sender.send(Message::Binary(compressed.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to compress with GBP, falling back to JSON: {}", e);
                            // Fallback to JSON
                            let json = serde_json::to_string(&msg).unwrap();
                            if sender.send(Message::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                } else {
                    // No payload, send as JSON
                    let json = serde_json::to_string(&msg).unwrap();
                    if sender.send(Message::Text(json.into())).await.is_err() {
                        break;
                    }
                }
            } else {
                // Use standard JSON (backward compatible)
                let json = serde_json::to_string(&msg).unwrap();
                if sender.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
        }
    });
    
    // Spawn task to handle invalidation events and push updates
    let subscriptions_clone = active_subscriptions.clone();
    let mux_clone = mux.clone();
    let ws_tx_for_invalidation = ws_tx.clone();
    
    let invalidation_task = tokio::spawn(async move {
        loop {
            match invalidation_rx.recv().await {
                Ok(event) => {
                    let trigger_pattern = format!("{}.{}", event.type_name, event.action);
                    
                    // Find subscriptions that match this invalidation
                    let matching_subs: Vec<LiveSubscription> = {
                        let subs = subscriptions_clone.read();
                        subs.values()
                            .filter(|sub| {
                                sub.is_live && sub.triggers.iter().any(|t| {
                                    t == &trigger_pattern || 
                                    t == &format!("{}.*", event.type_name) ||
                                    t == &format!("*.{}", event.action) ||
                                    t == "*.*"
                                })
                            })
                            .cloned()
                            .collect()
                    };
                    
                    // Re-execute and push updates for matching subscriptions
                    for sub in matching_subs {
                        tracing::info!(
                            subscription_id = %sub.id,
                            trigger = %trigger_pattern,
                            "Re-executing live query due to invalidation"
                        );
                        
                        // Build request
                        let mut gql_request = async_graphql::Request::new(&sub.query);
                        if let Some(vars) = &sub.variables {
                            if let Ok(variables) = serde_json::from_value(vars.clone()) {
                                gql_request = gql_request.variables(variables);
                            }
                        }
                        if let Some(op_name) = &sub.operation_name {
                            gql_request = gql_request.operation_name(op_name);
                        }
                        
                        // Execute
                        let response = mux_clone.handle_http(HeaderMap::new(), gql_request).await;
                        let response_json = serde_json::to_value(&response).unwrap_or_default();
                        
                        // Send update
                        let update = WsResponse {
                            msg_type: "next".to_string(),
                            id: Some(sub.id.clone()),
                            payload: Some(response_json),
                        };
                        
                        if ws_tx_for_invalidation.send(update).await.is_err() {
                            break;
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // Missed some events, continue
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    });
    
    // Main message loop
    while let Some(msg) = receiver.next().await {
        let msg = match msg {
            Ok(Message::Text(text)) => text,
            Ok(Message::Close(_)) => break,
            _ => continue,
        };
        
        let parsed: WsMessage = match serde_json::from_str(&msg) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("Failed to parse WebSocket message: {}", e);
                continue;
            }
        };
        
        match parsed.msg_type.as_str() {
            "connection_init" => {
                connection_initialized = true;
                
                // Check if client requests GBP compression
                if let Some(payload) = &parsed.payload {
                    if let Some(compression) = payload.get("compression").and_then(|c| c.as_str()) {
                        if compression == "gbp-lz4" || compression == "gbp" {
                            *use_gbp_compression.write() = true;
                            tracing::info!("GBP compression enabled for live query connection");
                        }
                    }
                }
                
                let mut ack_payload = serde_json::json!({});
                if *use_gbp_compression.read() {
                    ack_payload["compression"] = serde_json::json!("gbp-lz4");
                    ack_payload["compressionInfo"] = serde_json::json!({
                        "algorithm": "GBP Ultra + LZ4",
                        "expectedReduction": "90-99%",
                        "format": "binary"
                    });
                }
                
                let ack = WsResponse {
                    msg_type: "connection_ack".to_string(),
                    id: None,
                    payload: if ack_payload.as_object().unwrap().is_empty() {
                        None
                    } else {
                        Some(ack_payload)
                    },
                };
                if ws_tx.send(ack).await.is_err() {
                    break;
                }
            }
            
            "ping" => {
                let pong = WsResponse {
                    msg_type: "pong".to_string(),
                    id: None,
                    payload: None,
                };
                let _ = ws_tx.send(pong).await;
            }
            
            "subscribe" => {
                if !connection_initialized {
                    tracing::warn!("Received subscribe before connection_init");
                    continue;
                }
                
                let id = parsed.id.clone().unwrap_or_default();
                
                // Extract query from payload
                let query = parsed.payload
                    .as_ref()
                    .and_then(|p| p.get("query"))
                    .and_then(|q| q.as_str())
                    .unwrap_or("");
                
                // Check for @live directive
                let is_live = crate::live_query::has_live_directive(query);
                
                // Strip @live directive and convert subscription to query for live queries
                let clean_query = if is_live {
                    let stripped = crate::live_query::strip_live_directive(query);
                    // Convert "subscription" to "query" because standard GraphQL doesn't allow 
                    // Subscription operations without a Subscription root in the schema
                    if stripped.trim_start().starts_with("subscription") {
                        stripped.replacen("subscription", "query", 1)
                    } else {
                        stripped
                    }
                } else {
                    query.to_string()
                };
                
                let variables = parsed.payload.as_ref().and_then(|p| p.get("variables")).cloned();
                let operation_name = parsed.payload.as_ref()
                    .and_then(|p| p.get("operationName"))
                    .and_then(|n| n.as_str())
                    .map(|s| s.to_string());
                
                tracing::info!(
                    subscription_id = %id,
                    is_live = is_live,
                    "Live query subscription started"
                );
                
                // Build and execute the GraphQL request
                let mut gql_request = async_graphql::Request::new(&clean_query);
                
                if let Some(vars) = &variables {
                    if let Ok(v) = serde_json::from_value(vars.clone()) {
                        gql_request = gql_request.variables(v);
                    }
                }
                
                if let Some(ref op_name) = operation_name {
                    gql_request = gql_request.operation_name(op_name);
                }
                
                // Execute initial query
                let response = mux.handle_http(HeaderMap::new(), gql_request).await;
                let response_json = serde_json::to_value(&response).unwrap_or_default();
                
                // Send initial result
                let next_msg = WsResponse {
                    msg_type: "next".to_string(),
                    id: Some(id.clone()),
                    payload: Some(response_json),
                };
                
                if ws_tx.send(next_msg).await.is_err() {
                    break;
                }
                
                if is_live {
                    // Improve trigger detection using Schema Config
                    let configs = mux.schema.live_query_configs();
                    let mut triggers = std::collections::HashSet::new();

                    if !configs.is_empty() {
                         // Heuristic: check if any configured operation name appears in the query
                         // In a robust implementation, we would parse the query to find the root field
                         for (op_name, config) in configs {
                             if clean_query.contains(op_name) {
                                 for trigger in &config.triggers {
                                     triggers.insert(trigger.clone());
                                 }
                                 tracing::info!(
                                     operation = %op_name, 
                                     found_triggers = ?config.triggers,
                                     "Configured live query triggers found"
                                 );
                             }
                         }
                    }

                    // Fallback to defaults if no config found or no triggers specified
                    if triggers.is_empty() {
                        tracing::debug!("No configured triggers found, using defaults");
                        triggers.insert("User.create".to_string());
                        triggers.insert("User.update".to_string());
                        triggers.insert("User.delete".to_string());
                        triggers.insert("*.*".to_string());
                    }
                    
                    let subscription = LiveSubscription {
                        id: id.clone(),
                        query: clean_query,
                        variables,
                        operation_name,
                        triggers: triggers.into_iter().collect(),
                        is_live: true,
                    };
                    
                    active_subscriptions.write().insert(id.clone(), subscription);
                    tracing::info!(subscription_id = %id, "Live subscription registered for updates");
                } else {
                    // Non-live query: send complete immediately
                    let complete_msg = WsResponse {
                        msg_type: "complete".to_string(),
                        id: Some(id),
                        payload: None,
                    };
                    if ws_tx.send(complete_msg).await.is_err() {
                        break;
                    }
                }
            }
            
            "complete" => {
                if let Some(id) = parsed.id {
                    active_subscriptions.write().remove(&id);
                    tracing::debug!(subscription_id = %id, "Client completed subscription");
                }
            }
            
            _ => {
                tracing::debug!("Unknown message type: {}", parsed.msg_type);
            }
        }
    }
    
    // Cleanup
    forward_task.abort();
    invalidation_task.abort();
    tracing::debug!("Live query WebSocket connection closed");
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

    fn build_router_mux() -> ServeMux {
        let schema = crate::schema::SchemaBuilder::new()
            .with_descriptor_set_bytes(GREETER_DESCRIPTOR)
            .build(&crate::grpc_client::GrpcClientPool::new())
            .expect("schema builds");

        ServeMux::new(schema)
    }

    #[tokio::test]
    async fn playground_served_on_get() {
        let mut mux = build_router_mux();
        mux.enable_playground();
        let app = mux.into_router();

        // ... rest of test
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

    // Category 1: Configuration Tests (Quick Wins!)

    #[tokio::test]
    async fn test_servemux_new() {
        let mux = build_router_mux();
        // Basic creation should work
        assert!(mux.circuit_breaker().is_none());
        assert!(mux.response_cache().is_none());
    }

    #[tokio::test]
    async fn test_servemux_clone() {
        let mux = build_router_mux();
        let cloned = mux.clone();
        // Should be able to clone
        let _router1 = mux.into_router();
        let _router2 = cloned.into_router();
    }

    #[tokio::test]
    async fn test_enable_health_checks() {
        let mut mux = build_router_mux();
        mux.set_client_pool(crate::grpc_client::GrpcClientPool::new());
        mux.enable_health_checks();
        // Health checks enabled (verified by endpoint test below)
    }

    #[tokio::test]
    async fn test_enable_metrics() {
        let mut mux = build_router_mux();
        mux.enable_metrics();
        // Metrics enabled (verified by endpoint test below)
    }

    #[tokio::test]
    async fn test_enable_circuit_breaker() {
        let mut mux = build_router_mux();
        let config = crate::circuit_breaker::CircuitBreakerConfig::default();
        mux.enable_circuit_breaker(config);
        assert!(mux.circuit_breaker().is_some());
    }

    #[tokio::test]
    async fn test_enable_response_cache() {
        let mut mux = build_router_mux();
        let config = crate::cache::CacheConfig::default();
        mux.enable_response_cache(config);
        assert!(mux.response_cache().is_some());
    }

    #[tokio::test]
    async fn test_enable_compression() {
        let mut mux = build_router_mux();
        let config = crate::compression::CompressionConfig::default();
        mux.enable_compression(config);
        assert!(mux.compression_config().is_some());
    }

    #[tokio::test]
    async fn test_enable_query_whitelist() {
        let mut mux = build_router_mux();
        let config = crate::query_whitelist::QueryWhitelistConfig::warn();
        mux.enable_query_whitelist(config);
        assert!(mux.query_whitelist().is_some());
    }

    #[tokio::test]
    async fn test_enable_analytics() {
        let mut mux = build_router_mux();
        let config = crate::analytics::AnalyticsConfig::default();
        mux.enable_analytics(config);
        assert!(mux.analytics().is_some());
    }

    #[tokio::test]
    async fn test_enable_request_collapsing() {
        let mut mux = build_router_mux();
        let config = crate::request_collapsing::RequestCollapsingConfig::default();
        mux.enable_request_collapsing(config);
        assert!(mux.request_collapsing().is_some());
    }

    #[tokio::test]
    async fn test_enable_high_performance() {
        let mut mux = build_router_mux();
        let config = crate::high_performance::HighPerfConfig::default();
        mux.enable_high_performance(config);
        // High perf enabled
    }

    #[tokio::test]
    async fn test_perf_metrics() {
        let mux = build_router_mux();
        let _metrics = mux.perf_metrics();
        // Metrics accessible
    }

    // Category 2: Health & Metrics Endpoints

    #[tokio::test]
    async fn test_health_endpoint() {
        let mut mux = build_router_mux();
        mux.set_client_pool(crate::grpc_client::GrpcClientPool::new());
        mux.enable_health_checks();
        let app = mux.into_router();

        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_readiness_endpoint() {
        let mut mux = build_router_mux();
        mux.set_client_pool(crate::grpc_client::GrpcClientPool::new());
        mux.enable_health_checks();
        let app = mux.into_router();

        let response = app
            .oneshot(Request::builder().uri("/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();

        // Should return some status
        assert!(response.status().is_success() || response.status().is_server_error());
    }

    #[tokio::test]
    async fn test_metrics_endpoint() {
        let mut mux = build_router_mux();
        mux.enable_metrics();
        let app = mux.into_router();

        let response = app
            .oneshot(Request::builder().uri("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_analytics_endpoint() {
        let mut mux = build_router_mux();
        mux.enable_analytics(crate::analytics::AnalyticsConfig::default());
        let app = mux.into_router();

        let response = app
            .oneshot(Request::builder().uri("/analytics").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    // Category 3: GraphQL Request Handling

    #[tokio::test]
    async fn test_graphql_post_query() {
        let mux = build_router_mux();
        let app = mux.into_router();

        let query = r#"{ __schema { queryType { name } } }"#;
        let request_body = serde_json::json!({
            "query": query
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/graphql")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_graphql_post_introspection() {
        let mux = build_router_mux();
        let app = mux.into_router();

        let query = r#"{ __schema { types { name } } }"#;
        let request_body = serde_json::json!({
            "query": query
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/graphql")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_graphql_post_empty_body() {
        let mux = build_router_mux();
        let app = mux.into_router();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/graphql")
                    .header("content-type", "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // Should return error status
        assert!(!response.status().is_success());
    }

    #[tokio::test]
    async fn test_graphql_post_invalid_json() {
        let mux = build_router_mux();
        let app = mux.into_router();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/graphql")
                    .header("content-type", "application/json")
                    .body(Body::from("{invalid json"))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Should return error status
        assert!(!response.status().is_success());
    }

    #[tokio::test]
    async fn test_graphql_with_variables() {
        let mux = build_router_mux();
        let app = mux.into_router();

        let query = r#"query Test($name: String!) { __type(name: $name) { name } }"#;
        let request_body = serde_json::json!({
            "query": query,
            "variables": { "name": "String" }
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/graphql")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_graphql_with_operation_name() {
        let mux = build_router_mux();
        let app = mux.into_router();

        let query = r#"
            query First { __schema { queryType { name } } }
            query Second { __schema { mutationType { name } } }
        "#;
        let request_body = serde_json::json!({
            "query": query,
            "operationName": "First"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/graphql")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    // Category 4: Builder Pattern

    #[tokio::test]
    async fn test_with_middleware_builder() {
        let mux = build_router_mux();
        // Should not panic
        let _router = mux.into_router();
    }

    #[tokio::test]
    async fn test_multiple_configurations() {
        let mut mux = build_router_mux();
        
        // Enable multiple features
        mux.enable_metrics();
        mux.enable_playground();
        mux.enable_analytics(crate::analytics::AnalyticsConfig::default());
        mux.enable_compression(crate::compression::CompressionConfig::default());
        
        let app = mux.into_router();
        
        // Verify routes work
        let response = app
            .oneshot(Request::builder().uri("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_into_router_consumes_mux() {
        let mux = build_router_mux();
        let _router = mux.into_router();
        // mux is consumed, can't use it again (compile-time check)
    }

    // Category 5: Security

    #[tokio::test]
    async fn test_security_headers_present() {
        let mux = build_router_mux();
        let app = mux.into_router();

        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let headers = response.headers();
        
        // Check core security headers are present and have correct values
        assert_eq!(
            headers.get("x-content-type-options").unwrap().to_str().unwrap(),
            "nosniff"
        );
        assert_eq!(
            headers.get("x-frame-options").unwrap().to_str().unwrap(),
            "DENY"
        );
        assert_eq!(
            headers.get("x-xss-protection").unwrap().to_str().unwrap(),
            "1; mode=block"
        );
        
        // Check new 0.9.0 security headers
        assert_eq!(
            headers.get("strict-transport-security").unwrap().to_str().unwrap(),
            "max-age=31536000; includeSubDomains"
        );
        assert_eq!(
            headers.get("cache-control").unwrap().to_str().unwrap(),
            "no-store, no-cache, must-revalidate"
        );
        assert_eq!(
            headers.get("referrer-policy").unwrap().to_str().unwrap(),
            "strict-origin-when-cross-origin"
        );
        assert_eq!(
            headers.get("x-dns-prefetch-control").unwrap().to_str().unwrap(),
            "off"
        );

        // Check Permissions-Policy
        let p_policy = headers.get("permissions-policy").unwrap().to_str().unwrap();
        assert!(p_policy.contains("camera=()"));
        assert!(p_policy.contains("microphone=()"));
        assert!(p_policy.contains("geolocation=()"));
        
        // Check Content-Security-Policy
        let csp = headers.get("content-security-policy").unwrap().to_str().unwrap();
        assert!(csp.contains("default-src 'self'"));
        assert!(csp.contains("object-src 'none'"));
        assert!(csp.contains("base-uri 'self'"));
        assert!(csp.contains("frame-ancestors 'none'"));

        // Check Isolation Headers
        assert_eq!(
            headers.get("cross-origin-opener-policy").unwrap().to_str().unwrap(),
            "same-origin"
        );
        assert_eq!(
            headers.get("cross-origin-embedder-policy").unwrap().to_str().unwrap(),
            "require-corp"
        );
        assert_eq!(
            headers.get("cross-origin-resource-policy").unwrap().to_str().unwrap(),
            "same-origin"
        );
    }

    #[tokio::test]
    async fn test_cors_headers_present() {
        let mux = build_router_mux();
        let app = mux.into_router();

        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let headers = response.headers();
        assert!(headers.get("access-control-allow-origin").is_some());
    }
}

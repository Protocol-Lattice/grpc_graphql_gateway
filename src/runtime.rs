//! Runtime support for GraphQL gateway - HTTP and WebSocket integration.

use crate::analytics::{AnalyticsConfig, SharedQueryAnalytics};
use crate::cache::{CacheConfig, CacheLookupResult, SharedResponseCache};
use crate::circuit_breaker::{CircuitBreakerConfig, SharedCircuitBreakerRegistry};
use crate::compression::{create_compression_layer, CompressionConfig};
use crate::error::{GraphQLError, Result};
use crate::grpc_client::GrpcClientPool;
use crate::health::{health_handler, readiness_handler, HealthState};
use crate::metrics::GatewayMetrics;
use crate::middleware::{Context, Middleware};
use crate::persisted_queries::{
    process_apq_request, PersistedQueryConfig, PersistedQueryError, SharedPersistedQueryStore,
};
use crate::query_whitelist::{QueryWhitelistConfig, SharedQueryWhitelist};
use crate::request_collapsing::{RequestCollapsingConfig, SharedRequestCollapsingRegistry};
use crate::schema::{DynamicSchema, GrpcResponseCache};
use crate::high_performance::{
    HighPerfConfig, FastJsonParser, ShardedCache, PerfMetrics, ResponseTemplates,
    recommended_workers, pin_to_core,
};
use bytes::Bytes;
use async_graphql::ServerError;
use async_graphql_axum::{GraphQLRequest, GraphQLResponse, GraphQLSubscription};
use axum::{
    extract::State,
    http::HeaderMap,
    response::{Html, IntoResponse, Json},
    routing::{get, get_service, post},
    Extension, Router,
};
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
            compression_config: None,
            query_whitelist: None,
            analytics: None,
            request_collapsing: None,
            high_perf_config: None,
            json_parser: Arc::new(FastJsonParser::default()),
            sharded_cache: None,
            perf_metrics: Arc::new(PerfMetrics::default()),
            response_templates: Arc::new(ResponseTemplates::new()),
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
        self.query_whitelist = Some(Arc::new(crate::query_whitelist::QueryWhitelist::new(config)));
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
        self.request_collapsing = Some(crate::request_collapsing::create_request_collapsing_registry(config));
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

        // Validate query against whitelist if enabled
        if let Some(ref whitelist) = self.query_whitelist {
            // Extract operation ID from extensions if present
            let operation_id = processed_request.extensions.get("operationId")
                .and_then(|v| serde_json::to_value(v).ok())
                .and_then(|v| v.as_str().map(String::from));

            if let Err(err) = whitelist.validate_query(
                &processed_request.query, 
                operation_id.as_deref()
            ) {
                tracing::warn!("Query whitelist validation failed: {}", err);
                let mut server_err = ServerError::new(err.to_string(), None);
                server_err.extensions = Some({
                    let mut ext = async_graphql::ErrorExtensionValues::default();
                    ext.set("code", "QUERY_NOT_WHITELISTED");
                    ext
                });
                return async_graphql::Response::from_errors(vec![server_err]).into();
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
            cache.config.vary_headers.iter()
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
        match self.execute_with_middlewares(headers, processed_request).await {
            Ok(resp) => {
                let duration = request_start.elapsed();
                let had_error = !resp.errors.is_empty();
                
                // Track in analytics
                if let Some(ref analytics) = self.analytics {
                    let error_details = if had_error {
                        resp.errors.first().map(|e| {
                            let code = e.extensions.as_ref()
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
                        error_details.as_ref().map(|(c, m)| (c.as_str(), m.as_str())),
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
                            cache.put(cache_key.clone(), resp_json.clone(), types, entities).await;
                            
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
    pub async fn handle_fast(
        &self,
        headers: HeaderMap,
        body: Bytes,
    ) -> axum::response::Response {
        let start = Instant::now();
        
        // 1. SIMD JSON parsing
        let request_val = match self.json_parser.parse_bytes(&body) {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!("Failed to parse JSON with SIMD: {}", err);
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    self.response_templates.errors.get("PARSE_ERROR").cloned()
                        .unwrap_or_else(|| Bytes::from(r#"{"errors":[{"message":"Invalid JSON"}]}"#))
                ).into_response();
            }
        };

        let query = request_val["query"].as_str().unwrap_or("");
        let variables = &request_val["variables"];
        let operation_name = request_val["operationName"].as_str();

        // 2. High-performance cache lookup
        if let Some(ref sharded) = self.sharded_cache {
            let vary_header_values: Vec<String> = if let Some(ref cache) = self.response_cache {
                cache.config.vary_headers.iter()
                    .map(|h| format!("{}:{}", h, headers.get(h).and_then(|v| v.to_str().ok()).unwrap_or("")))
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
                self.perf_metrics.record(start.elapsed().as_nanos() as u64, true);
                return (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    cached_bytes
                ).into_response();
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
        self.perf_metrics.record(start.elapsed().as_nanos() as u64, false);
        
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
                        .unwrap_or(async_graphql::Value::Null)
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
        let code = error_extensions.get("code")
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
        let playground_enabled = std::env::var("ENABLE_GRAPHQL_PLAYGROUND")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let state = Arc::new(self);
        let use_fast_path = state.high_perf_config.is_some();

        let router = Router::new();
        let router = if use_fast_path {
            router.route("/graphql", post(handle_graphql_fast))
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
                    axum::http::HeaderValue::from_static("Content-Type, Authorization, X-Request-ID"),
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
            headers.insert(
                axum::http::header::CONTENT_SECURITY_POLICY,
                axum::http::HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'"),
            );
            
            // Referrer Policy - limit referrer information leakage
            headers.insert(
                axum::http::header::REFERRER_POLICY,
                axum::http::HeaderValue::from_static("strict-origin-when-cross-origin"),
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
                router = router.layer(create_compression_layer(config));
            }
        }

        // Add health check routes if enabled
        if health_checks_enabled {
            let health_state = Arc::new(HealthState::new(
                client_pool.unwrap_or_else(GrpcClientPool::new)
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
                .route("/analytics/api", get(analytics_api_handler).with_state(state.clone()))
                .route("/analytics/reset", post(analytics_reset_handler).with_state(state));
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
            compression_config: self.compression_config.clone(),
            query_whitelist: self.query_whitelist.clone(),
            analytics: self.analytics.clone(),
            request_collapsing: self.request_collapsing.clone(),
            high_perf_config: self.high_perf_config.clone(),
            json_parser: self.json_parser.clone(),
            sharded_cache: self.sharded_cache.clone(),
            perf_metrics: self.perf_metrics.clone(),
            response_templates: self.response_templates.clone(),
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
    GraphQLResponse::from(mux.handle_http(headers, request.into_inner()).await)
}

/// Handler for high-performance POST requests to /graphql
async fn handle_graphql_fast(
    State(mux): State<Arc<ServeMux>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    mux.handle_fast(headers, body).await
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
        [(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")],
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
        Json(serde_json::to_value(snapshot).unwrap_or_else(|_| serde_json::json!({"error": "Failed to serialize analytics"})))
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

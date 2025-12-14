//! REST API Connector for hybrid gRPC/REST GraphQL gateways
//!
//! This module provides the ability to resolve GraphQL fields from REST APIs,
//! enabling hybrid architectures where some data comes from gRPC services
//! and some from existing REST endpoints.
//!
//! # Example
//!
//! ```rust,ignore
//! use grpc_graphql_gateway::{Gateway, RestConnector, RestEndpoint, HttpMethod};
//! use std::time::Duration;
//!
//! let rest_connector = RestConnector::builder()
//!     .base_url("https://api.example.com")
//!     .timeout(Duration::from_secs(30))
//!     .add_endpoint(RestEndpoint::new("getUser", "/users/{id}")
//!         .method(HttpMethod::GET)
//!         .response_path("$.data"))
//!     .add_endpoint(RestEndpoint::new("createUser", "/users")
//!         .method(HttpMethod::POST)
//!         .body_template(r#"{"name": "{name}", "email": "{email}"}"#))
//!     .build()?;
//!
//! let gateway = Gateway::builder()
//!     .with_descriptor_set_bytes(DESCRIPTORS)
//!     .add_rest_connector("users", rest_connector)
//!     .build()?;
//! ```

use crate::{Error, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, error, info, instrument, warn};

/// HTTP methods supported by REST connectors
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    #[default]
    GET,
    POST,
    PUT,
    PATCH,
    DELETE,
}

impl std::fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpMethod::GET => write!(f, "GET"),
            HttpMethod::POST => write!(f, "POST"),
            HttpMethod::PUT => write!(f, "PUT"),
            HttpMethod::PATCH => write!(f, "PATCH"),
            HttpMethod::DELETE => write!(f, "DELETE"),
        }
    }
}

/// Defines the GraphQL scalar type for a REST response field
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RestFieldType {
    /// String type
    String,
    /// Integer type
    Int,
    /// Float type
    Float,
    /// Boolean type
    Boolean,
    /// Nested object (reference to another type)
    Object(String),
    /// List of a type
    List(Box<RestFieldType>),
}

impl RestFieldType {
    /// Convert to GraphQL TypeRef string
    pub fn to_type_ref(&self) -> String {
        match self {
            RestFieldType::String => "String".to_string(),
            RestFieldType::Int => "Int".to_string(),
            RestFieldType::Float => "Float".to_string(),
            RestFieldType::Boolean => "Boolean".to_string(),
            RestFieldType::Object(name) => name.clone(),
            RestFieldType::List(inner) => format!("[{}]", inner.to_type_ref()),
        }
    }
}

/// A field in a REST response schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestResponseField {
    /// Field name
    pub name: String,
    /// Field type
    pub field_type: RestFieldType,
    /// Whether the field is nullable
    pub nullable: bool,
    /// Optional description
    pub description: Option<String>,
}

impl RestResponseField {
    /// Create a new required string field
    pub fn string(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            field_type: RestFieldType::String,
            nullable: false,
            description: None,
        }
    }

    /// Create a new required int field
    pub fn int(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            field_type: RestFieldType::Int,
            nullable: false,
            description: None,
        }
    }

    /// Create a new required float field
    pub fn float(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            field_type: RestFieldType::Float,
            nullable: false,
            description: None,
        }
    }

    /// Create a new required boolean field
    pub fn boolean(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            field_type: RestFieldType::Boolean,
            nullable: false,
            description: None,
        }
    }

    /// Create a new object field (reference to another type)
    pub fn object(name: impl Into<String>, type_name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            field_type: RestFieldType::Object(type_name.into()),
            nullable: false,
            description: None,
        }
    }

    /// Create a list field
    pub fn list(name: impl Into<String>, item_type: RestFieldType) -> Self {
        Self {
            name: name.into(),
            field_type: RestFieldType::List(Box::new(item_type)),
            nullable: false,
            description: None,
        }
    }

    /// Make this field nullable
    pub fn nullable(mut self) -> Self {
        self.nullable = true;
        self
    }

    /// Add description
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }
}

/// Schema definition for a REST response type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestResponseSchema {
    /// Type name (e.g., "Post", "User")
    pub type_name: String,
    /// Fields in this type
    pub fields: Vec<RestResponseField>,
    /// Optional description
    pub description: Option<String>,
}

impl RestResponseSchema {
    /// Create a new response schema
    pub fn new(type_name: impl Into<String>) -> Self {
        Self {
            type_name: type_name.into(),
            fields: Vec::new(),
            description: None,
        }
    }

    /// Add a field
    pub fn field(mut self, field: RestResponseField) -> Self {
        self.fields.push(field);
        self
    }

    /// Add description
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }
}

/// Configuration for a REST endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestEndpoint {
    /// Unique name for this endpoint (used as GraphQL field name)
    pub name: String,
    /// URL path template (supports `{variable}` placeholders)
    pub path: String,
    /// HTTP method
    pub method: HttpMethod,
    /// Request body template for POST/PUT/PATCH (supports `{variable}` placeholders)
    pub body_template: Option<String>,
    /// JSONPath expression to extract response data (e.g., `$.data.users`)
    pub response_path: Option<String>,
    /// Static headers to include with this endpoint
    pub headers: HashMap<String, String>,
    /// Query parameters template (supports `{variable}` placeholders)
    pub query_params: HashMap<String, String>,
    /// Timeout override for this specific endpoint
    pub timeout: Option<Duration>,
    /// Whether this endpoint is a mutation (default: inferred from method)
    pub is_mutation: Option<bool>,
    /// Description for GraphQL schema
    pub description: Option<String>,
    /// Expected return type (for schema generation)
    pub return_type: Option<String>,
    /// Response schema for typed responses (enables field selection)
    pub response_schema: Option<RestResponseSchema>,
}

impl RestEndpoint {
    /// Create a new REST endpoint configuration
    pub fn new(name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            method: HttpMethod::GET,
            body_template: None,
            response_path: None,
            headers: HashMap::new(),
            query_params: HashMap::new(),
            timeout: None,
            is_mutation: None,
            description: None,
            return_type: None,
            response_schema: None,
        }
    }

    /// Set the HTTP method
    pub fn method(mut self, method: HttpMethod) -> Self {
        self.method = method;
        self
    }

    /// Set the request body template
    pub fn body_template(mut self, template: impl Into<String>) -> Self {
        self.body_template = Some(template.into());
        self
    }

    /// Set the JSONPath to extract from response
    pub fn response_path(mut self, path: impl Into<String>) -> Self {
        self.response_path = Some(path.into());
        self
    }

    /// Add a static header
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Add a query parameter template
    pub fn query_param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.query_params.insert(key.into(), value.into());
        self
    }

    /// Set the timeout for this endpoint
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Mark this endpoint as a mutation
    pub fn as_mutation(mut self) -> Self {
        self.is_mutation = Some(true);
        self
    }

    /// Mark this endpoint as a query
    pub fn as_query(mut self) -> Self {
        self.is_mutation = Some(false);
        self
    }

    /// Set the description for GraphQL schema
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set the expected return type
    pub fn return_type(mut self, type_name: impl Into<String>) -> Self {
        self.return_type = Some(type_name.into());
        self
    }

    /// Set the response schema for typed responses
    /// 
    /// This enables GraphQL field selection on REST responses.
    /// 
    /// # Example
    /// 
    /// ```rust,ignore
    /// use grpc_graphql_gateway::{RestEndpoint, RestResponseSchema, RestResponseField};
    /// 
    /// let endpoint = RestEndpoint::new("getPost", "/posts/{id}")
    ///     .with_response_schema(RestResponseSchema::new("Post")
    ///         .field(RestResponseField::int("id"))
    ///         .field(RestResponseField::string("title"))
    ///         .field(RestResponseField::string("body"))
    ///         .field(RestResponseField::int("userId")));
    /// ```
    pub fn with_response_schema(mut self, schema: RestResponseSchema) -> Self {
        self.response_schema = Some(schema);
        self
    }

    /// Determine if this is a mutation based on method or explicit setting
    pub fn is_mutation(&self) -> bool {
        self.is_mutation.unwrap_or_else(|| {
            matches!(
                self.method,
                HttpMethod::POST | HttpMethod::PUT | HttpMethod::PATCH | HttpMethod::DELETE
            )
        })
    }
}

/// Configuration for the REST connector
#[derive(Debug, Clone)]
pub struct RestConnectorConfig {
    /// Base URL for all endpoints
    pub base_url: String,
    /// Default timeout for requests
    pub timeout: Duration,
    /// Default headers for all requests
    pub default_headers: HashMap<String, String>,
    /// Retry configuration
    pub retry: RetryConfig,
    /// Whether to log request/response bodies
    pub log_bodies: bool,
}

impl Default for RestConnectorConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            timeout: Duration::from_secs(30),
            default_headers: HashMap::new(),
            retry: RetryConfig::default(),
            log_bodies: false,
        }
    }
}

/// Retry configuration for failed requests
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts
    pub max_retries: u32,
    /// Initial backoff duration
    pub initial_backoff: Duration,
    /// Maximum backoff duration
    pub max_backoff: Duration,
    /// Backoff multiplier
    pub multiplier: f64,
    /// HTTP status codes that should trigger a retry
    pub retry_statuses: Vec<u16>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(10),
            multiplier: 2.0,
            retry_statuses: vec![429, 500, 502, 503, 504],
        }
    }
}

impl RetryConfig {
    /// Disable retries
    pub fn disabled() -> Self {
        Self {
            max_retries: 0,
            ..Default::default()
        }
    }

    /// Aggressive retry configuration for critical endpoints
    pub fn aggressive() -> Self {
        Self {
            max_retries: 5,
            initial_backoff: Duration::from_millis(50),
            max_backoff: Duration::from_secs(30),
            multiplier: 2.0,
            retry_statuses: vec![408, 429, 500, 502, 503, 504],
        }
    }
}

/// Result of a REST API call
#[derive(Debug, Clone)]
pub struct RestResponse {
    /// HTTP status code
    pub status: u16,
    /// Response headers
    pub headers: HashMap<String, String>,
    /// Response body as JSON
    pub body: JsonValue,
    /// Request duration
    pub duration: Duration,
}

impl RestResponse {
    /// Check if the response indicates success (2xx status)
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Extract data using a JSONPath expression
    pub fn extract(&self, path: &str) -> Option<JsonValue> {
        extract_json_path(&self.body, path)
    }
}

/// Trait for custom REST response transformers
#[async_trait]
pub trait ResponseTransformer: Send + Sync {
    /// Transform the REST response before returning to GraphQL
    async fn transform(&self, endpoint: &str, response: RestResponse) -> Result<JsonValue>;
}

/// Default response transformer that extracts data via JSONPath
#[derive(Default)]
pub struct DefaultTransformer;

#[async_trait]
impl ResponseTransformer for DefaultTransformer {
    async fn transform(&self, _endpoint: &str, response: RestResponse) -> Result<JsonValue> {
        Ok(response.body)
    }
}

/// Trait for request interceptors (for auth, logging, etc.)
#[async_trait]
pub trait RequestInterceptor: Send + Sync {
    /// Intercept and potentially modify the request before sending
    async fn intercept(&self, request: &mut RestRequest) -> Result<()>;
}

/// Represents a REST request to be made
#[derive(Debug, Clone)]
pub struct RestRequest {
    /// Full URL
    pub url: String,
    /// HTTP method
    pub method: HttpMethod,
    /// Request headers
    pub headers: HashMap<String, String>,
    /// Request body (for POST/PUT/PATCH)
    pub body: Option<String>,
    /// Timeout for this request
    pub timeout: Duration,
}

/// REST connector for making HTTP requests
pub struct RestConnector {
    config: RestConnectorConfig,
    endpoints: HashMap<String, RestEndpoint>,
    client: reqwest::Client,
    transformer: Arc<dyn ResponseTransformer>,
    interceptors: Vec<Arc<dyn RequestInterceptor>>,
    /// Cache for GET requests
    cache: Option<Arc<RwLock<RestCache>>>,
}

impl std::fmt::Debug for RestConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestConnector")
            .field("config", &self.config)
            .field("endpoints", &self.endpoints.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Clone for RestConnector {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            endpoints: self.endpoints.clone(),
            client: self.client.clone(),
            transformer: self.transformer.clone(),
            interceptors: self.interceptors.clone(),
            cache: self.cache.clone(),
        }
    }
}

impl RestConnector {
    /// Create a new builder for RestConnector
    pub fn builder() -> RestConnectorBuilder {
        RestConnectorBuilder::default()
    }

    /// Get the base URL
    pub fn base_url(&self) -> &str {
        &self.config.base_url
    }

    /// Get all configured endpoints
    pub fn endpoints(&self) -> &HashMap<String, RestEndpoint> {
        &self.endpoints
    }

    /// Get a specific endpoint by name
    pub fn get_endpoint(&self, name: &str) -> Option<&RestEndpoint> {
        self.endpoints.get(name)
    }

    /// Execute a REST endpoint with the given arguments
    #[instrument(skip(self, args), fields(endpoint = %endpoint_name))]
    pub async fn execute(
        &self,
        endpoint_name: &str,
        args: HashMap<String, JsonValue>,
    ) -> Result<JsonValue> {
        let endpoint = self
            .endpoints
            .get(endpoint_name)
            .ok_or_else(|| Error::Schema(format!("Unknown REST endpoint: {}", endpoint_name)))?;

        // Build the request
        let mut request = self.build_request(endpoint, &args)?;

        // Apply interceptors
        for interceptor in &self.interceptors {
            interceptor.intercept(&mut request).await?;
        }

        // Check cache for GET requests
        if endpoint.method == HttpMethod::GET {
            if let Some(cache) = &self.cache {
                let cache_key = format!("{}:{}", endpoint_name, serde_json::to_string(&args)?);
                let cache_read = cache.read().await;
                if let Some(cached) = cache_read.get(&cache_key) {
                    debug!("REST cache hit for {}", endpoint_name);
                    return Ok(cached.clone());
                }
            }
        }

        // Execute the request with retry logic
        let response = self.execute_with_retry(&request, endpoint).await?;

        // Apply transformer (which may extract data via response_path)
        let data = self.transformer.transform(endpoint_name, response).await?;

        // Cache successful GET responses
        if endpoint.method == HttpMethod::GET {
            if let Some(cache) = &self.cache {
                let cache_key = format!("{}:{}", endpoint_name, serde_json::to_string(&args)?);
                let mut cache_write = cache.write().await;
                cache_write.insert(cache_key, data.clone());
            }
        }

        Ok(data)
    }

    /// Build a request from an endpoint and arguments
    fn build_request(
        &self,
        endpoint: &RestEndpoint,
        args: &HashMap<String, JsonValue>,
    ) -> Result<RestRequest> {
        // Build URL with path parameters
        let mut path = endpoint.path.clone();
        for (key, value) in args {
            let placeholder = format!("{{{}}}", key);
            let value_str = json_value_to_string(value);
            path = path.replace(&placeholder, &value_str);
        }

        // Build query string
        let mut query_parts = Vec::new();
        for (key, template) in &endpoint.query_params {
            let mut value = template.clone();
            for (arg_key, arg_value) in args {
                let placeholder = format!("{{{}}}", arg_key);
                let value_str = json_value_to_string(arg_value);
                value = value.replace(&placeholder, &value_str);
            }
            // Only include if all placeholders were replaced
            if !value.contains('{') {
                query_parts.push(format!("{}={}", key, urlencoding::encode(&value)));
            }
        }

        let url = if query_parts.is_empty() {
            format!("{}{}", self.config.base_url, path)
        } else {
            format!("{}{}?{}", self.config.base_url, path, query_parts.join("&"))
        };

        // Build headers
        let mut headers = self.config.default_headers.clone();
        headers.extend(endpoint.headers.clone());

        // Build body
        let body = if let Some(ref template) = endpoint.body_template {
            let mut body = template.clone();
            for (key, value) in args {
                let placeholder = format!("{{{}}}", key);
                let value_str = json_value_to_string(value);
                body = body.replace(&placeholder, &value_str);
            }
            Some(body)
        } else if matches!(
            endpoint.method,
            HttpMethod::POST | HttpMethod::PUT | HttpMethod::PATCH
        ) {
            // Auto-serialize args as JSON body if no template
            Some(serde_json::to_string(args)?)
        } else {
            None
        };

        let timeout = endpoint.timeout.unwrap_or(self.config.timeout);

        Ok(RestRequest {
            url,
            method: endpoint.method,
            headers,
            body,
            timeout,
        })
    }

    async fn execute_with_retry(
        &self,
        request: &RestRequest,
        _endpoint: &RestEndpoint,
    ) -> Result<RestResponse> {
        let retry_config = &self.config.retry;
        let mut attempts = 0;
        let mut backoff = retry_config.initial_backoff;

        loop {
            attempts += 1;

            match self.execute_request(request).await {
                Ok(response) => {
                    if response.is_success() {
                        return Ok(response);
                    }

                    // Check if we should retry this status
                    if retry_config.retry_statuses.contains(&response.status)
                        && attempts <= retry_config.max_retries
                    {
                        warn!(
                            "REST {} {} returned {}, retrying in {:?} (attempt {}/{})",
                            request.method,
                            request.url,
                            response.status,
                            backoff,
                            attempts,
                            retry_config.max_retries
                        );
                        tokio::time::sleep(backoff).await;
                        backoff = std::cmp::min(
                            Duration::from_secs_f64(backoff.as_secs_f64() * retry_config.multiplier),
                            retry_config.max_backoff,
                        );
                        continue;
                    }

                    // Return error for non-retryable status
                    return Err(Error::Schema(format!(
                        "REST {} {} failed with status {}: {}",
                        request.method,
                        request.url,
                        response.status,
                        response.body
                    )));
                }
                Err(e) => {
                    if attempts <= retry_config.max_retries {
                        warn!(
                            "REST {} {} failed: {}, retrying in {:?} (attempt {}/{})",
                            request.method, request.url, e, backoff, attempts, retry_config.max_retries
                        );
                        tokio::time::sleep(backoff).await;
                        backoff = std::cmp::min(
                            Duration::from_secs_f64(backoff.as_secs_f64() * retry_config.multiplier),
                            retry_config.max_backoff,
                        );
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    }

    /// Execute a single request
    async fn execute_request(&self, request: &RestRequest) -> Result<RestResponse> {
        let start = std::time::Instant::now();

        let mut req_builder = match request.method {
            HttpMethod::GET => self.client.get(&request.url),
            HttpMethod::POST => self.client.post(&request.url),
            HttpMethod::PUT => self.client.put(&request.url),
            HttpMethod::PATCH => self.client.patch(&request.url),
            HttpMethod::DELETE => self.client.delete(&request.url),
        };

        // Add headers
        for (key, value) in &request.headers {
            req_builder = req_builder.header(key, value);
        }

        // Add body
        if let Some(ref body) = request.body {
            req_builder = req_builder
                .header("Content-Type", "application/json")
                .body(body.clone());
        }

        // Set timeout
        req_builder = req_builder.timeout(request.timeout);

        if self.config.log_bodies {
            debug!(
                "REST {} {} body={:?}",
                request.method, request.url, request.body
            );
        }

        let response = req_builder.send().await.map_err(|e| {
            error!("REST request failed: {}", e);
            Error::Schema(format!("REST request failed: {}", e))
        })?;

        let status = response.status().as_u16();
        let headers: HashMap<String, String> = response
            .headers()
            .iter()
            .filter_map(|(k, v)| Some((k.to_string(), v.to_str().ok()?.to_string())))
            .collect();

        let body_text = response.text().await.map_err(|e| {
            error!("Failed to read REST response: {}", e);
            Error::Schema(format!("Failed to read REST response: {}", e))
        })?;

        let body: JsonValue =
            serde_json::from_str(&body_text).unwrap_or(JsonValue::String(body_text));

        let duration = start.elapsed();

        if self.config.log_bodies {
            debug!(
                "REST {} {} -> {} ({:?}) body={:?}",
                request.method, request.url, status, duration, body
            );
        } else {
            debug!(
                "REST {} {} -> {} ({:?})",
                request.method, request.url, status, duration
            );
        }

        Ok(RestResponse {
            status,
            headers,
            body,
            duration,
        })
    }

    /// Clear the response cache
    pub async fn clear_cache(&self) {
        if let Some(cache) = &self.cache {
            cache.write().await.clear();
        }
    }
}

/// Builder for RestConnector
#[derive(Default)]
pub struct RestConnectorBuilder {
    config: RestConnectorConfig,
    endpoints: HashMap<String, RestEndpoint>,
    transformer: Option<Arc<dyn ResponseTransformer>>,
    interceptors: Vec<Arc<dyn RequestInterceptor>>,
    enable_cache: bool,
    cache_size: usize,
}

impl RestConnectorBuilder {
    /// Set the base URL for all endpoints
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.config.base_url = url.into().trim_end_matches('/').to_string();
        self
    }

    /// Set the default timeout
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.config.timeout = timeout;
        self
    }

    /// Add a default header for all requests
    pub fn default_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.config.default_headers.insert(key.into(), value.into());
        self
    }

    /// Set the retry configuration
    pub fn retry(mut self, retry: RetryConfig) -> Self {
        self.config.retry = retry;
        self
    }

    /// Disable retries
    pub fn no_retry(mut self) -> Self {
        self.config.retry = RetryConfig::disabled();
        self
    }

    /// Enable request/response body logging
    pub fn log_bodies(mut self, enabled: bool) -> Self {
        self.config.log_bodies = enabled;
        self
    }

    /// Add an endpoint
    pub fn add_endpoint(mut self, endpoint: RestEndpoint) -> Self {
        self.endpoints.insert(endpoint.name.clone(), endpoint);
        self
    }

    /// Add multiple endpoints
    pub fn add_endpoints(mut self, endpoints: Vec<RestEndpoint>) -> Self {
        for endpoint in endpoints {
            self.endpoints.insert(endpoint.name.clone(), endpoint);
        }
        self
    }

    /// Set a custom response transformer
    pub fn transformer(mut self, transformer: Arc<dyn ResponseTransformer>) -> Self {
        self.transformer = Some(transformer);
        self
    }

    /// Add a request interceptor
    pub fn interceptor(mut self, interceptor: Arc<dyn RequestInterceptor>) -> Self {
        self.interceptors.push(interceptor);
        self
    }

    /// Enable response caching for GET requests
    pub fn with_cache(mut self, max_entries: usize) -> Self {
        self.enable_cache = true;
        self.cache_size = max_entries;
        self
    }

    /// Build the RestConnector
    pub fn build(self) -> Result<RestConnector> {
        if self.config.base_url.is_empty() {
            return Err(Error::Schema("REST connector requires a base_url".into()));
        }

        let client = reqwest::Client::builder()
            .timeout(self.config.timeout)
            .build()
            .map_err(|e| Error::Schema(format!("Failed to create HTTP client: {}", e)))?;

        let cache = if self.enable_cache {
            Some(Arc::new(RwLock::new(RestCache::new(self.cache_size))))
        } else {
            None
        };

        info!(
            "REST connector configured with {} endpoints at {}",
            self.endpoints.len(),
            self.config.base_url
        );

        Ok(RestConnector {
            config: self.config,
            endpoints: self.endpoints,
            client,
            transformer: self.transformer.unwrap_or_else(|| Arc::new(DefaultTransformer)),
            interceptors: self.interceptors,
            cache,
        })
    }
}

/// Simple LRU cache for REST responses
struct RestCache {
    entries: HashMap<String, JsonValue>,
    order: Vec<String>,
    max_size: usize,
}

impl RestCache {
    fn new(max_size: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: Vec::new(),
            max_size,
        }
    }

    fn get(&self, key: &str) -> Option<&JsonValue> {
        self.entries.get(key)
    }

    fn insert(&mut self, key: String, value: JsonValue) {
        if self.entries.contains_key(&key) {
            // Move to end of order
            self.order.retain(|k| k != &key);
            self.order.push(key.clone());
            self.entries.insert(key, value);
            return;
        }

        // Evict oldest if at capacity
        while self.entries.len() >= self.max_size && !self.order.is_empty() {
            if let Some(oldest) = self.order.first().cloned() {
                self.order.remove(0);
                self.entries.remove(&oldest);
            }
        }

        self.entries.insert(key.clone(), value);
        self.order.push(key);
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
}

/// Extract data from JSON using a simple JSONPath expression
/// Supports: $.field, $.field.nested, $.field[0], $[0].field
fn extract_json_path(value: &JsonValue, path: &str) -> Option<JsonValue> {
    if path.is_empty() || path == "$" {
        return Some(value.clone());
    }

    let path = path.strip_prefix("$.").or_else(|| path.strip_prefix("$"))?;

    let mut current = value.clone();
    for segment in path.split('.') {
        // Handle array index: field[0] or just [0]
        if let Some(bracket_pos) = segment.find('[') {
            let field_name = &segment[..bracket_pos];
            let index_str = segment[bracket_pos + 1..].trim_end_matches(']');

            if !field_name.is_empty() {
                current = current.get(field_name)?.clone();
            }

            if let Ok(index) = index_str.parse::<usize>() {
                current = current.get(index)?.clone();
            }
        } else if !segment.is_empty() {
            current = current.get(segment)?.clone();
        }
    }

    Some(current)
}

/// Convert a JSON value to a string for URL interpolation
fn json_value_to_string(value: &JsonValue) -> String {
    match value {
        JsonValue::String(s) => s.clone(),
        JsonValue::Number(n) => n.to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Null => "null".to_string(),
        _ => value.to_string(),
    }
}

/// Bearer token authentication interceptor
pub struct BearerAuthInterceptor {
    token: Arc<RwLock<String>>,
}

impl BearerAuthInterceptor {
    /// Create a new bearer auth interceptor with a static token
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: Arc::new(RwLock::new(token.into())),
        }
    }

    /// Update the token (for token refresh scenarios)
    pub async fn set_token(&self, token: impl Into<String>) {
        *self.token.write().await = token.into();
    }
}

#[async_trait]
impl RequestInterceptor for BearerAuthInterceptor {
    async fn intercept(&self, request: &mut RestRequest) -> Result<()> {
        let token = self.token.read().await;
        request
            .headers
            .insert("Authorization".to_string(), format!("Bearer {}", *token));
        Ok(())
    }
}

/// API key authentication interceptor
pub struct ApiKeyInterceptor {
    header_name: String,
    api_key: String,
}

impl ApiKeyInterceptor {
    /// Create a new API key interceptor
    pub fn new(header_name: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            header_name: header_name.into(),
            api_key: api_key.into(),
        }
    }

    /// Create with standard X-API-Key header
    pub fn x_api_key(api_key: impl Into<String>) -> Self {
        Self::new("X-API-Key", api_key)
    }
}

#[async_trait]
impl RequestInterceptor for ApiKeyInterceptor {
    async fn intercept(&self, request: &mut RestRequest) -> Result<()> {
        request
            .headers
            .insert(self.header_name.clone(), self.api_key.clone());
        Ok(())
    }
}

/// Registry for managing multiple REST connectors
#[derive(Default)]
pub struct RestConnectorRegistry {
    connectors: HashMap<String, Arc<RestConnector>>,
}

impl RestConnectorRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a REST connector with a name
    pub fn register(&mut self, name: impl Into<String>, connector: RestConnector) {
        self.connectors.insert(name.into(), Arc::new(connector));
    }

    /// Get a connector by name
    pub fn get(&self, name: &str) -> Option<Arc<RestConnector>> {
        self.connectors.get(name).cloned()
    }

    /// Get all connector names
    pub fn names(&self) -> Vec<&String> {
        self.connectors.keys().collect()
    }

    /// Get all connectors
    pub fn connectors(&self) -> &HashMap<String, Arc<RestConnector>> {
        &self.connectors
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.connectors.is_empty()
    }

    /// Execute an endpoint on a specific connector
    pub async fn execute(
        &self,
        connector_name: &str,
        endpoint_name: &str,
        args: HashMap<String, JsonValue>,
    ) -> Result<JsonValue> {
        let connector = self
            .get(connector_name)
            .ok_or_else(|| Error::Schema(format!("Unknown REST connector: {}", connector_name)))?;

        connector.execute(endpoint_name, args).await
    }

    /// Build GraphQL fields for all REST endpoints in this registry
    /// 
    /// This generates dynamic GraphQL fields that can be added to Query and Mutation types.
    /// GET endpoints become queries, POST/PUT/PATCH/DELETE become mutations.
    /// 
    /// Field names use the endpoint name directly (e.g., `getPost`, `createUser`).
    /// Ensure endpoint names are unique across all connectors to avoid conflicts.
    /// 
    /// # Returns
    /// 
    /// A tuple of (query_fields, mutation_fields) as vectors of RestGraphQLField
    pub fn build_graphql_fields(&self) -> (Vec<RestGraphQLField>, Vec<RestGraphQLField>) {
        let mut query_fields = Vec::new();
        let mut mutation_fields = Vec::new();

        for (connector_name, connector) in &self.connectors {
            for (endpoint_name, endpoint) in connector.endpoints() {
                // Use endpoint name directly (no connector prefix)
                let field_name = endpoint_name.clone();
                let description = endpoint.description.clone().unwrap_or_else(|| {
                    format!("REST {} {}{}", endpoint.method, connector.base_url(), endpoint.path)
                });

                // Extract path parameters from path template
                let path_params = extract_path_params(&endpoint.path);
                
                // Combine with query params
                let mut all_params = path_params;
                for key in endpoint.query_params.keys() {
                    if !all_params.contains(key) {
                        all_params.push(key.clone());
                    }
                }

                // For POST/PUT/PATCH, add body template params
                if let Some(ref body_template) = endpoint.body_template {
                    let body_params = extract_template_params(body_template);
                    for param in body_params {
                        if !all_params.contains(&param) {
                            all_params.push(param);
                        }
                    }
                }

                let field = RestGraphQLField {
                    name: field_name,
                    description,
                    parameters: all_params,
                    connector_name: connector_name.clone(),
                    endpoint_name: endpoint_name.clone(),
                    connector: connector.clone(),
                    response_schema: endpoint.response_schema.clone(),
                };

                if endpoint.is_mutation() {
                    mutation_fields.push(field);
                } else {
                    query_fields.push(field);
                }
            }
        }

        (query_fields, mutation_fields)
    }
}

/// Represents a GraphQL field generated from a REST endpoint
#[derive(Clone)]
pub struct RestGraphQLField {
    /// GraphQL field name
    pub name: String,
    /// Field description
    pub description: String,
    /// Parameter names (GraphQL arguments)
    pub parameters: Vec<String>,
    /// Source connector name
    pub connector_name: String,
    /// Source endpoint name
    pub endpoint_name: String,
    /// Reference to the connector
    pub connector: Arc<RestConnector>,
    /// Optional typed response schema
    pub response_schema: Option<RestResponseSchema>,
}

impl std::fmt::Debug for RestGraphQLField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestGraphQLField")
            .field("name", &self.name)
            .field("description", &self.description)
            .field("parameters", &self.parameters)
            .field("connector_name", &self.connector_name)
            .field("endpoint_name", &self.endpoint_name)
            .finish()
    }
}

impl RestGraphQLField {
    /// Execute this REST field with the given arguments
    pub async fn execute(&self, args: HashMap<String, JsonValue>) -> Result<JsonValue> {
        self.connector.execute(&self.endpoint_name, args).await
    }
}

/// Extract parameter names from a path template (e.g., "/users/{id}" -> ["id"])
fn extract_path_params(path: &str) -> Vec<String> {
    let mut params = Vec::new();
    let mut in_param = false;
    let mut param_name = String::new();

    for c in path.chars() {
        if c == '{' {
            in_param = true;
            param_name.clear();
        } else if c == '}' {
            if in_param && !param_name.is_empty() {
                params.push(param_name.clone());
            }
            in_param = false;
        } else if in_param {
            param_name.push(c);
        }
    }

    params
}

/// Extract parameter names from any template string
fn extract_template_params(template: &str) -> Vec<String> {
    extract_path_params(template)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_path_simple() {
        let json: JsonValue = serde_json::json!({
            "data": {
                "user": {
                    "id": "123",
                    "name": "Alice"
                }
            }
        });

        assert_eq!(
            extract_json_path(&json, "$.data.user.id"),
            Some(JsonValue::String("123".into()))
        );
        assert_eq!(
            extract_json_path(&json, "$.data.user"),
            Some(serde_json::json!({"id": "123", "name": "Alice"}))
        );
    }

    #[test]
    fn test_extract_json_path_array() {
        let json: JsonValue = serde_json::json!({
            "users": [
                {"id": "1", "name": "Alice"},
                {"id": "2", "name": "Bob"}
            ]
        });

        assert_eq!(
            extract_json_path(&json, "$.users[0].name"),
            Some(JsonValue::String("Alice".into()))
        );
        assert_eq!(
            extract_json_path(&json, "$.users[1].id"),
            Some(JsonValue::String("2".into()))
        );
    }

    #[test]
    fn test_endpoint_builder() {
        let endpoint = RestEndpoint::new("getUser", "/users/{id}")
            .method(HttpMethod::GET)
            .header("Accept", "application/json")
            .query_param("include", "profile")
            .response_path("$.data")
            .description("Get a user by ID");

        assert_eq!(endpoint.name, "getUser");
        assert_eq!(endpoint.path, "/users/{id}");
        assert_eq!(endpoint.method, HttpMethod::GET);
        assert!(!endpoint.is_mutation());
    }

    #[test]
    fn test_post_endpoint_is_mutation() {
        let endpoint = RestEndpoint::new("createUser", "/users").method(HttpMethod::POST);

        assert!(endpoint.is_mutation());
    }

    #[test]
    fn test_explicit_query_override() {
        let endpoint = RestEndpoint::new("deleteUser", "/users/{id}")
            .method(HttpMethod::DELETE)
            .as_query();

        assert!(!endpoint.is_mutation());
    }

    #[tokio::test]
    async fn test_rest_cache() {
        let mut cache = RestCache::new(2);

        cache.insert("key1".to_string(), JsonValue::String("value1".into()));
        cache.insert("key2".to_string(), JsonValue::String("value2".into()));

        assert!(cache.get("key1").is_some());
        assert!(cache.get("key2").is_some());

        // Adding third item should evict key1
        cache.insert("key3".to_string(), JsonValue::String("value3".into()));

        assert!(cache.get("key1").is_none());
        assert!(cache.get("key2").is_some());
        assert!(cache.get("key3").is_some());
    }
}

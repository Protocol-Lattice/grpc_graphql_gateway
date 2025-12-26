//! Middleware support for the gateway
//!
//! This module provides extensible middleware for authentication, logging, rate limiting,
//! and other cross-cutting concerns. Middleware is applied to all GraphQL requests
//! before they reach the resolver layer.

use crate::error::Result;
use axum::http::Request;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

/// Context passed to middleware
///
/// Contains request metadata and an extensible data store for middleware
/// to communicate with each other and with resolvers.
#[derive(Debug)]
pub struct Context {
    /// Request headers and metadata
    pub headers: axum::http::HeaderMap,

    /// Additional context data (user claims, request ID, etc.)
    pub extensions: HashMap<String, serde_json::Value>,

    /// Request start time for timing
    pub request_start: Instant,

    /// Unique request identifier
    pub request_id: String,

    /// Client IP address (if available)
    pub client_ip: Option<String>,

    /// Encryption key for field-level encryption
    pub encryption_key: Option<Vec<u8>>,
}

impl Context {
    /// Create a new context from request
    ///
    /// # Security
    ///
    /// IP address extraction validates the format to prevent spoofing.
    /// Invalid IPs are discarded. In production behind trusted proxies,
    /// configure your proxy to set X-Real-IP correctly.
    pub fn from_request<B>(req: &Request<B>) -> Self {
        // Extract or generate request ID
        let request_id = req
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .map(String::from)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        // Try to extract client IP from headers (supports proxies)
        // SECURITY: Validate IP format to prevent spoofing
        let client_ip = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(',').next())
            .map(|s| s.trim())
            .filter(|ip| Self::is_valid_ip(ip))
            .map(String::from)
            .or_else(|| {
                req.headers()
                    .get("x-real-ip")
                    .and_then(|v| v.to_str().ok())
                    .filter(|ip| Self::is_valid_ip(ip))
                    .map(String::from)
            });

        Self {
            headers: req.headers().clone(),
            extensions: HashMap::new(),
            request_start: Instant::now(),
            request_id,
            client_ip,
            encryption_key: None,
        }
    }

    /// Set encryption key for the request scope
    pub fn set_encryption_key(&mut self, key: Vec<u8>) {
        self.encryption_key = Some(key);
    }

    /// Get encryption key if set
    pub fn encryption_key(&self) -> Option<&[u8]> {
        self.encryption_key.as_deref()
    }

    /// Validate IP address format
    ///
    /// # Security
    ///
    /// Validates both IPv4 and IPv6 formats to prevent header injection attacks.
    fn is_valid_ip(ip: &str) -> bool {
        use std::net::IpAddr;
        ip.parse::<IpAddr>().is_ok()
    }

    /// Insert extension data
    pub fn insert(&mut self, key: String, value: serde_json::Value) {
        self.extensions.insert(key, value);
    }

    /// Get extension data
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.extensions.get(key)
    }

    /// Get typed extension data
    pub fn get_typed<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.extensions
            .get(key)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    /// Check if a key exists in extensions
    pub fn contains(&self, key: &str) -> bool {
        self.extensions.contains_key(key)
    }

    /// Get elapsed time since request start
    pub fn elapsed(&self) -> std::time::Duration {
        self.request_start.elapsed()
    }

    /// Get user ID from auth context (convenience method)
    pub fn user_id(&self) -> Option<String> {
        self.get("auth.user_id")
            .and_then(|v| v.as_str())
            .map(String::from)
    }

    /// Get user roles from auth context (convenience method)
    pub fn user_roles(&self) -> Vec<String> {
        self.get("auth.roles")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Encrypt a value if an encryption key is present
    pub fn encrypt_value(&self, value: &str) -> crate::error::Result<String> {
        if let Some(key) = &self.encryption_key {
            // Simple XOR encryption for demonstration
            // In production, use AES-GCM or ChaCha20-Poly1305
            // This is just a placeholder to demonstrate the architecture
            let encrypted: Vec<u8> = value
                .bytes()
                .enumerate()
                .map(|(i, b)| b ^ key[i % key.len()])
                .collect();
            use base64::Engine;
            Ok(base64::engine::general_purpose::STANDARD.encode(encrypted))
        } else {
            Ok(value.to_string())
        }
    }

    /// Decrypt a value if an encryption key is present
    pub fn decrypt_value(&self, value: &str) -> crate::error::Result<String> {
        if let Some(key) = &self.encryption_key {
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD.decode(value).map_err(|e| {
                crate::error::Error::Validation(format!("Failed to decode base64: {}", e))
            })?;
            
            let decrypted: Vec<u8> = bytes
                .iter()
                .enumerate()
                .map(|(i, b)| b ^ key[i % key.len()])
                .collect();
                
            String::from_utf8(decrypted).map_err(|e| {
                crate::error::Error::Validation(format!("Failed to decode UTF-8: {}", e))
            })
        } else {
            Ok(value.to_string())
        }
    }
}

/// Middleware trait for processing requests
///
/// Middleware can intercept requests before they are processed by the GraphQL engine.
/// Middleware is executed in order, and any error will short-circuit the chain.
///
/// # Example
///
/// ```rust
/// use grpc_graphql_gateway::middleware::{Middleware, Context};
/// use grpc_graphql_gateway::Result;
///
/// struct MyMiddleware;
///
/// #[async_trait::async_trait]
/// impl Middleware for MyMiddleware {
///     async fn call(&self, ctx: &mut Context) -> Result<()> {
///         println!("Processing request {}", ctx.request_id);
///         Ok(())
///     }
/// }
/// ```
#[async_trait::async_trait]
pub trait Middleware: Send + Sync {
    /// Process the request context
    async fn call(&self, ctx: &mut Context) -> Result<()>;

    /// Middleware name for logging/debugging
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

/// Type alias for boxed middleware
pub type BoxMiddleware = Box<dyn Middleware>;

/// Middleware function type
pub type MiddlewareFn =
    Arc<dyn Fn(&mut Context) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>;

// ============================================================================
// CORS Middleware
// ============================================================================

/// CORS middleware
///
/// Handles Cross-Origin Resource Sharing (CORS) headers.
/// Note: This is a placeholder implementation. In a real application,
/// you should use `tower_http::cors::CorsLayer` with Axum.
#[derive(Debug, Clone)]
pub struct CorsMiddleware {
    pub allow_origins: Vec<String>,
    pub allow_methods: Vec<String>,
    pub allow_headers: Vec<String>,
}

impl Default for CorsMiddleware {
    fn default() -> Self {
        Self {
            allow_origins: vec!["*".to_string()],
            allow_methods: vec!["GET".to_string(), "POST".to_string()],
            allow_headers: vec!["Content-Type".to_string(), "Authorization".to_string()],
        }
    }
}

#[async_trait::async_trait]
impl Middleware for CorsMiddleware {
    async fn call(&self, _ctx: &mut Context) -> Result<()> {
        // CORS is handled by tower-http, this is just a placeholder
        Ok(())
    }

    fn name(&self) -> &'static str {
        "CorsMiddleware"
    }
}

// ============================================================================
// Authentication Middleware
// ============================================================================

/// Authentication scheme types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthScheme {
    /// Bearer token (JWT or opaque)
    Bearer,
    /// Basic authentication
    Basic,
    /// API key in header
    ApiKey,
    /// Custom scheme
    Custom(String),
}

impl AuthScheme {
    /// Parse scheme from Authorization header
    pub fn from_header(header: &str) -> Option<(Self, String)> {
        let parts: Vec<&str> = header.splitn(2, ' ').collect();
        if parts.len() != 2 {
            return None;
        }

        let scheme = match parts[0].to_lowercase().as_str() {
            "bearer" => AuthScheme::Bearer,
            "basic" => AuthScheme::Basic,
            other => AuthScheme::Custom(other.to_string()),
        };

        Some((scheme, parts[1].to_string()))
    }
}

/// Authentication result containing validated claims
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[derive(Default)]
pub struct AuthClaims {
    /// Subject (user ID)
    pub sub: Option<String>,
    /// Issuer
    pub iss: Option<String>,
    /// Audience
    pub aud: Option<Vec<String>>,
    /// Expiration time (Unix timestamp)
    pub exp: Option<i64>,
    /// Issued at (Unix timestamp)
    pub iat: Option<i64>,
    /// Roles/permissions
    pub roles: Vec<String>,
    /// Custom claims
    #[serde(flatten)]
    pub custom: HashMap<String, serde_json::Value>,
}


impl AuthClaims {
    /// Check if claims have expired
    pub fn is_expired(&self) -> bool {
        if let Some(exp) = self.exp {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            return exp < now;
        }
        false
    }

    /// Check if user has a specific role
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }

    /// Check if user has any of the specified roles
    pub fn has_any_role(&self, roles: &[&str]) -> bool {
        roles.iter().any(|r| self.has_role(r))
    }
}

/// Token validator trait for custom validation logic
#[async_trait::async_trait]
pub trait TokenValidator: Send + Sync {
    /// Validate token and return claims
    async fn validate(&self, token: &str) -> Result<AuthClaims>;
}

/// Simple closure-based token validator
pub struct FnValidator<F>
where
    F: Fn(&str) -> std::pin::Pin<Box<dyn Future<Output = Result<AuthClaims>> + Send>> + Send + Sync,
{
    validate_fn: F,
}

impl<F> FnValidator<F>
where
    F: Fn(&str) -> std::pin::Pin<Box<dyn Future<Output = Result<AuthClaims>> + Send>> + Send + Sync,
{
    pub fn new(f: F) -> Self {
        Self { validate_fn: f }
    }
}

#[async_trait::async_trait]
impl<F> TokenValidator for FnValidator<F>
where
    F: Fn(&str) -> std::pin::Pin<Box<dyn Future<Output = Result<AuthClaims>> + Send>> + Send + Sync,
{
    async fn validate(&self, token: &str) -> Result<AuthClaims> {
        (self.validate_fn)(token).await
    }
}

/// Configuration for authentication middleware
#[derive(Clone)]
pub struct AuthConfig {
    /// Required authentication (if false, unauthenticated requests pass through)
    pub required: bool,
    /// Allowed authentication schemes
    pub allowed_schemes: Vec<AuthScheme>,
    /// Header name for API key authentication
    pub api_key_header: String,
    /// Skip authentication for these paths (e.g., health checks)
    pub skip_paths: HashSet<String>,
    /// Skip authentication for introspection queries
    pub skip_introspection: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            required: true,
            allowed_schemes: vec![AuthScheme::Bearer],
            api_key_header: "x-api-key".to_string(),
            skip_paths: HashSet::new(),
            skip_introspection: false,
        }
    }
}

impl AuthConfig {
    /// Create a new auth config with required authentication
    pub fn required() -> Self {
        Self::default()
    }

    /// Create a new auth config with optional authentication
    pub fn optional() -> Self {
        Self {
            required: false,
            ..Self::default()
        }
    }

    /// Add a scheme to allowed schemes
    pub fn with_scheme(mut self, scheme: AuthScheme) -> Self {
        if !self.allowed_schemes.contains(&scheme) {
            self.allowed_schemes.push(scheme);
        }
        self
    }

    /// Set custom API key header
    pub fn with_api_key_header(mut self, header: impl Into<String>) -> Self {
        self.api_key_header = header.into();
        self
    }

    /// Add a path to skip authentication
    pub fn skip_path(mut self, path: impl Into<String>) -> Self {
        self.skip_paths.insert(path.into());
        self
    }

    /// Skip authentication for introspection
    pub fn with_skip_introspection(mut self, skip: bool) -> Self {
        self.skip_introspection = skip;
        self
    }
}

/// Enhanced Authentication middleware
///
/// Validates authentication using configurable validators and schemes.
/// Supports Bearer tokens (JWT), Basic auth, and API keys.
///
/// # Example
///
/// ```rust,no_run
/// use grpc_graphql_gateway::middleware::{EnhancedAuthMiddleware, AuthConfig, AuthClaims};
/// use std::sync::Arc;
///
/// // Simple token validation
/// let auth = EnhancedAuthMiddleware::new(
///     AuthConfig::required().with_scheme(grpc_graphql_gateway::middleware::AuthScheme::Bearer),
///     Arc::new(|token: &str| {
///         Box::pin(async move {
///             // Validate JWT token here
///             Ok(AuthClaims::default())
///         })
///     }),
/// );
/// ```
pub struct EnhancedAuthMiddleware {
    config: AuthConfig,
    validator: Arc<dyn TokenValidator>,
}

impl EnhancedAuthMiddleware {
    /// Create a new auth middleware with custom validator
    pub fn new(config: AuthConfig, validator: Arc<dyn TokenValidator>) -> Self {
        Self { config, validator }
    }

    /// Create with a simple validation function
    pub fn with_fn<F>(config: AuthConfig, f: F) -> Self
    where
        F: Fn(&str) -> std::pin::Pin<Box<dyn Future<Output = Result<AuthClaims>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            config,
            validator: Arc::new(FnValidator::new(f)),
        }
    }

    /// Extract token from request based on configured schemes
    fn extract_token(&self, ctx: &Context) -> Option<(AuthScheme, String)> {
        // Try Authorization header first
        if let Some(auth_header) = ctx.headers.get("authorization") {
            if let Ok(header_str) = auth_header.to_str() {
                if let Some((scheme, token)) = AuthScheme::from_header(header_str) {
                    if self.config.allowed_schemes.contains(&scheme) {
                        return Some((scheme, token));
                    }
                }
            }
        }

        // Try API key header
        if self.config.allowed_schemes.contains(&AuthScheme::ApiKey) {
            if let Some(api_key) = ctx.headers.get(self.config.api_key_header.as_str()) {
                if let Ok(key) = api_key.to_str() {
                    return Some((AuthScheme::ApiKey, key.to_string()));
                }
            }
        }

        None
    }
}

#[async_trait::async_trait]
impl Middleware for EnhancedAuthMiddleware {
    async fn call(&self, ctx: &mut Context) -> Result<()> {
        // Extract and validate token
        match self.extract_token(ctx) {
            Some((scheme, token)) => {
                // Validate token
                let claims = self.validator.validate(&token).await.map_err(|e| {
                    tracing::warn!(
                        request_id = %ctx.request_id,
                        scheme = ?scheme,
                        error = %e,
                        "Authentication failed"
                    );
                    crate::error::Error::Unauthorized(format!("Token validation failed: {}", e))
                })?;

                // Check expiration
                if claims.is_expired() {
                    tracing::warn!(
                        request_id = %ctx.request_id,
                        user_id = ?claims.sub,
                        "Token expired"
                    );
                    return Err(crate::error::Error::Unauthorized(
                        "Token has expired".to_string(),
                    ));
                }

                // Store claims in context
                ctx.insert(
                    "auth.scheme".to_string(),
                    serde_json::json!(format!("{:?}", scheme)),
                );
                if let Some(ref user_id) = claims.sub {
                    ctx.insert("auth.user_id".to_string(), serde_json::json!(user_id));
                }
                ctx.insert("auth.roles".to_string(), serde_json::json!(claims.roles));
                ctx.insert(
                    "auth.claims".to_string(),
                    serde_json::to_value(&claims).unwrap_or_default(),
                );
                ctx.insert("auth.authenticated".to_string(), serde_json::json!(true));

                tracing::debug!(
                    request_id = %ctx.request_id,
                    user_id = ?claims.sub,
                    roles = ?claims.roles,
                    "Authentication successful"
                );

                Ok(())
            }
            None => {
                if self.config.required {
                    tracing::warn!(
                        request_id = %ctx.request_id,
                        client_ip = ?ctx.client_ip,
                        "Missing authentication"
                    );
                    Err(crate::error::Error::Unauthorized(
                        "Authentication required".to_string(),
                    ))
                } else {
                    // Optional auth - allow unauthenticated requests
                    ctx.insert("auth.authenticated".to_string(), serde_json::json!(false));
                    Ok(())
                }
            }
        }
    }

    fn name(&self) -> &'static str {
        "EnhancedAuthMiddleware"
    }
}

/// Simple authentication middleware (backwards compatible)
///
/// Validates the `Authorization` header using a provided validation function.
/// If validation fails, it returns an `Unauthorized` error.
#[derive(Clone)]
pub struct AuthMiddleware {
    pub validate: Arc<dyn Fn(&str) -> bool + Send + Sync>,
}

impl AuthMiddleware {
    /// Create a new auth middleware with a validation function
    pub fn new<F>(validate: F) -> Self
    where
        F: Fn(&str) -> bool + Send + Sync + 'static,
    {
        Self {
            validate: Arc::new(validate),
        }
    }

    /// Create an auth middleware that accepts any token
    pub fn allow_all() -> Self {
        Self::new(|_| true)
    }

    /// Create an auth middleware that requires a specific token
    pub fn require_token(expected_token: String) -> Self {
        Self::new(move |token| {
            token
                .strip_prefix("Bearer ")
                .map(|t| t == expected_token)
                .unwrap_or(false)
        })
    }
}

#[async_trait::async_trait]
impl Middleware for AuthMiddleware {
    async fn call(&self, ctx: &mut Context) -> Result<()> {
        if let Some(auth_header) = ctx.headers.get("authorization") {
            if let Ok(token) = auth_header.to_str() {
                if (self.validate)(token) {
                    ctx.insert("auth.authenticated".to_string(), serde_json::json!(true));
                    return Ok(());
                }
            }
        }
        Err(crate::error::Error::Unauthorized(
            "Invalid or missing authorization".to_string(),
        ))
    }

    fn name(&self) -> &'static str {
        "AuthMiddleware"
    }
}

// ============================================================================
// Logging Middleware
// ============================================================================

/// Log level for the logging middleware
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum LogLevel {
    /// Trace level (most verbose)
    Trace,
    /// Debug level
    Debug,
    /// Info level (recommended for production)
    #[default]
    Info,
    /// Warn level
    Warn,
    /// Error level (errors only)
    Error,
}


/// Headers that should be masked in logs
const DEFAULT_SENSITIVE_HEADERS: &[&str] = &[
    "authorization",
    "x-api-key",
    "cookie",
    "set-cookie",
    "x-auth-token",
    "x-access-token",
    "x-refresh-token",
    "proxy-authorization",
];

/// Configuration for logging middleware
#[derive(Clone)]
pub struct LoggingConfig {
    /// Log level
    pub level: LogLevel,
    /// Whether to log request headers
    pub log_headers: bool,
    /// Headers to mask (sensitive data)
    pub sensitive_headers: HashSet<String>,
    /// Whether to log request body (be careful with PII)
    pub log_body: bool,
    /// Maximum body length to log
    pub max_body_log_length: usize,
    /// Whether to log timing information
    pub log_timing: bool,
    /// Slow request threshold (log as warning if exceeded)
    pub slow_request_threshold: std::time::Duration,
    /// Whether to include structured fields
    pub structured: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::Info,
            log_headers: true,
            sensitive_headers: DEFAULT_SENSITIVE_HEADERS
                .iter()
                .map(|s| s.to_lowercase())
                .collect(),
            log_body: false,
            max_body_log_length: 1024,
            log_timing: true,
            slow_request_threshold: std::time::Duration::from_secs(3),
            structured: true,
        }
    }
}

impl LoggingConfig {
    /// Create a minimal logging config (less verbose)
    pub fn minimal() -> Self {
        Self {
            level: LogLevel::Info,
            log_headers: false,
            log_body: false,
            log_timing: true,
            structured: true,
            ..Self::default()
        }
    }

    /// Create a verbose logging config (for debugging)
    pub fn verbose() -> Self {
        Self {
            level: LogLevel::Debug,
            log_headers: true,
            log_body: true,
            log_timing: true,
            structured: true,
            ..Self::default()
        }
    }

    /// Add a sensitive header to mask
    pub fn mask_header(mut self, header: impl Into<String>) -> Self {
        self.sensitive_headers.insert(header.into().to_lowercase());
        self
    }

    /// Set log level
    pub fn with_level(mut self, level: LogLevel) -> Self {
        self.level = level;
        self
    }

    /// Enable/disable header logging
    pub fn with_headers(mut self, log: bool) -> Self {
        self.log_headers = log;
        self
    }

    /// Set slow request threshold
    pub fn with_slow_threshold(mut self, threshold: std::time::Duration) -> Self {
        self.slow_request_threshold = threshold;
        self
    }
}

/// Enhanced Logging middleware
///
/// Provides structured logging with request IDs, timing, and sensitive data masking.
///
/// # Example
///
/// ```rust,no_run
/// use grpc_graphql_gateway::middleware::{EnhancedLoggingMiddleware, LoggingConfig, LogLevel};
///
/// // Production config
/// let logging = EnhancedLoggingMiddleware::new(
///     LoggingConfig::default().with_level(LogLevel::Info)
/// );
///
/// // Debug config
/// let debug_logging = EnhancedLoggingMiddleware::new(
///     LoggingConfig::verbose()
/// );
/// ```
#[derive(Clone)]
pub struct EnhancedLoggingMiddleware {
    config: LoggingConfig,
}

impl EnhancedLoggingMiddleware {
    /// Create a new logging middleware with config
    pub fn new(config: LoggingConfig) -> Self {
        Self { config }
    }

    /// Create with default config
    pub fn default_config() -> Self {
        Self::new(LoggingConfig::default())
    }

    /// Mask sensitive header values
    fn mask_headers(&self, headers: &axum::http::HeaderMap) -> HashMap<String, String> {
        headers
            .iter()
            .map(|(name, value)| {
                let name_str = name.as_str().to_lowercase();
                let value_str = if self.config.sensitive_headers.contains(&name_str) {
                    "[REDACTED]".to_string()
                } else {
                    value.to_str().unwrap_or("[binary]").to_string()
                };
                (name_str, value_str)
            })
            .collect()
    }
}

impl Default for EnhancedLoggingMiddleware {
    fn default() -> Self {
        Self::default_config()
    }
}

#[async_trait::async_trait]
impl Middleware for EnhancedLoggingMiddleware {
    async fn call(&self, ctx: &mut Context) -> Result<()> {
        let masked_headers = if self.config.log_headers {
            Some(self.mask_headers(&ctx.headers))
        } else {
            None
        };

        // Log based on configured level
        match self.config.level {
            LogLevel::Trace => {
                tracing::trace!(
                    request_id = %ctx.request_id,
                    client_ip = ?ctx.client_ip,
                    headers = ?masked_headers,
                    "GraphQL request received"
                );
            }
            LogLevel::Debug => {
                tracing::debug!(
                    request_id = %ctx.request_id,
                    client_ip = ?ctx.client_ip,
                    headers = ?masked_headers,
                    "GraphQL request received"
                );
            }
            LogLevel::Info => {
                if self.config.structured {
                    tracing::info!(
                        request_id = %ctx.request_id,
                        client_ip = ?ctx.client_ip,
                        "GraphQL request received"
                    );
                } else {
                    tracing::info!(
                        "GraphQL request {} from {:?}",
                        ctx.request_id,
                        ctx.client_ip
                    );
                }
            }
            LogLevel::Warn | LogLevel::Error => {
                // Only log warnings/errors for slow requests or errors
            }
        }

        // Store logging context for later use
        ctx.insert(
            "logging.headers_logged".to_string(),
            serde_json::json!(self.config.log_headers),
        );

        Ok(())
    }

    fn name(&self) -> &'static str {
        "EnhancedLoggingMiddleware"
    }
}

/// Simple logging middleware (backwards compatible)
///
/// Logs incoming GraphQL requests using the `tracing` crate.
#[derive(Debug, Clone, Default)]
pub struct LoggingMiddleware;

impl LoggingMiddleware {
    /// Create a new logging middleware
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Middleware for LoggingMiddleware {
    async fn call(&self, ctx: &mut Context) -> Result<()> {
        tracing::debug!(
            request_id = %ctx.request_id,
            "Processing GraphQL request"
        );
        Ok(())
    }

    fn name(&self) -> &'static str {
        "LoggingMiddleware"
    }
}

// ============================================================================
// Rate Limiting Middleware
// ============================================================================

/// Rate Limiting middleware
///
/// Limits the number of requests using a token bucket algorithm.
/// This implementation uses a global rate limiter.
pub struct RateLimitMiddleware {
    limiter: Arc<
        governor::RateLimiter<
            governor::state::NotKeyed,
            governor::state::InMemoryState,
            governor::clock::DefaultClock,
        >,
    >,
}

impl RateLimitMiddleware {
    /// Create a new rate limiter with the specified requests per second capacity
    pub fn new(requests_per_second: u32, burst_size: u32) -> Self {
        use governor::{Quota, RateLimiter};
        use std::num::NonZeroU32;

        let quota = Quota::per_second(NonZeroU32::new(requests_per_second).unwrap())
            .allow_burst(NonZeroU32::new(burst_size).unwrap());

        Self {
            limiter: Arc::new(RateLimiter::direct(quota)),
        }
    }

    /// Create a per-minute rate limiter
    pub fn per_minute(requests_per_minute: u32, burst_size: u32) -> Self {
        use governor::{Quota, RateLimiter};
        use std::num::NonZeroU32;

        let quota = Quota::per_minute(NonZeroU32::new(requests_per_minute).unwrap())
            .allow_burst(NonZeroU32::new(burst_size).unwrap());

        Self {
            limiter: Arc::new(RateLimiter::direct(quota)),
        }
    }
}

#[async_trait::async_trait]
impl Middleware for RateLimitMiddleware {
    async fn call(&self, ctx: &mut Context) -> Result<()> {
        if self.limiter.check().is_err() {
            tracing::warn!(
                request_id = %ctx.request_id,
                client_ip = ?ctx.client_ip,
                "Rate limit exceeded"
            );
            return Err(crate::error::Error::TooManyRequests(
                "Rate limit exceeded".to_string(),
            ));
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        "RateLimitMiddleware"
    }
}

// ============================================================================
// Middleware Chain
// ============================================================================

/// Middleware chain for combining multiple middleware
pub struct MiddlewareChain {
    middlewares: Vec<Arc<dyn Middleware>>,
}

impl MiddlewareChain {
    /// Create a new empty middleware chain
    pub fn new() -> Self {
        Self {
            middlewares: Vec::new(),
        }
    }

    /// Add a middleware to the chain
    pub fn add<M: Middleware + 'static>(mut self, middleware: M) -> Self {
        self.middlewares.push(Arc::new(middleware));
        self
    }

    /// Add an Arc-wrapped middleware
    pub fn add_arc(mut self, middleware: Arc<dyn Middleware>) -> Self {
        self.middlewares.push(middleware);
        self
    }

    /// Execute all middleware in order
    pub async fn execute(&self, ctx: &mut Context) -> Result<()> {
        for middleware in &self.middlewares {
            middleware.call(ctx).await?;
        }
        Ok(())
    }

    /// Get middleware count
    pub fn len(&self) -> usize {
        self.middlewares.len()
    }

    /// Check if chain is empty
    pub fn is_empty(&self) -> bool {
        self.middlewares.is_empty()
    }
}

impl Default for MiddlewareChain {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_scheme_parsing() {
        let (scheme, token) = AuthScheme::from_header("Bearer abc123").unwrap();
        assert_eq!(scheme, AuthScheme::Bearer);
        assert_eq!(token, "abc123");

        let (scheme, _) = AuthScheme::from_header("Basic dXNlcjpwYXNz").unwrap();
        assert_eq!(scheme, AuthScheme::Basic);

        assert!(AuthScheme::from_header("InvalidHeader").is_none());
    }

    #[test]
    fn test_auth_claims_expiration() {
        let claims = AuthClaims {
            exp: Some(0), // Expired in 1970
            ..Default::default()
        };
        assert!(claims.is_expired());

        let claims = AuthClaims {
            exp: Some(i64::MAX), // Far future
            ..Default::default()
        };
        assert!(!claims.is_expired());
    }

    #[test]
    fn test_auth_claims_roles() {
        let claims = AuthClaims {
            roles: vec!["admin".to_string(), "user".to_string()],
            ..Default::default()
        };

        assert!(claims.has_role("admin"));
        assert!(claims.has_role("user"));
        assert!(!claims.has_role("superadmin"));
        assert!(claims.has_any_role(&["admin", "superadmin"]));
        assert!(!claims.has_any_role(&["superadmin", "guest"]));
    }

    #[test]
    fn test_logging_config_masking() {
        let config = LoggingConfig::default().mask_header("x-custom-secret");

        assert!(config.sensitive_headers.contains("authorization"));
        assert!(config.sensitive_headers.contains("x-custom-secret"));
    }

    #[test]
    fn test_auth_config_builder() {
        let config = AuthConfig::required()
            .with_scheme(AuthScheme::Bearer)
            .with_scheme(AuthScheme::ApiKey)
            .with_api_key_header("x-my-api-key")
            .skip_path("/health");

        assert!(config.required);
        assert!(config.allowed_schemes.contains(&AuthScheme::Bearer));
        assert!(config.allowed_schemes.contains(&AuthScheme::ApiKey));
        assert_eq!(config.api_key_header, "x-my-api-key");
        assert!(config.skip_paths.contains("/health"));
    }

    #[tokio::test]
    async fn test_middleware_chain() {
        use axum::http::Request;

        struct CounterMiddleware {
            counter: Arc<std::sync::atomic::AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl Middleware for CounterMiddleware {
            async fn call(&self, _ctx: &mut Context) -> Result<()> {
                self.counter
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }
        }

        let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let chain = MiddlewareChain::new()
            .add(CounterMiddleware {
                counter: counter.clone(),
            })
            .add(CounterMiddleware {
                counter: counter.clone(),
            });

        let req = Request::builder().uri("/graphql").body(()).unwrap();
        let mut ctx = Context::from_request(&req);

        chain.execute(&mut ctx).await.unwrap();
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn test_auth_config_optional() {
        let config = AuthConfig::optional();
        assert!(!config.required);
    }

    #[test]
    fn test_auth_config_required() {
        let config = AuthConfig::required();
        assert!(config.required);
    }

    #[test]
    fn test_auth_config_with_scheme_no_duplicates() {
        let config = AuthConfig::default()
            .with_scheme(AuthScheme::Bearer)
            .with_scheme(AuthScheme::Bearer);
        
        assert_eq!(config.allowed_schemes.len(), 1);
    }

    #[test]
    fn test_auth_config_skip_introspection() {
        let config = AuthConfig::default().with_skip_introspection(true);
        assert!(config.skip_introspection);
    }

    #[test]
    fn test_auth_claims_default() {
        let claims = AuthClaims::default();
        assert!(claims.sub.is_none());
        assert!(claims.exp.is_none());
        assert!(claims.roles.is_empty());
    }

    #[test]
    fn test_logging_config_default() {
        let config = LoggingConfig::default();
        assert_eq!(config.level, LogLevel::Info);
        assert!(config.log_headers);
        assert!(!config.log_body);
        assert!(config.log_timing);
    }

    #[test]
    fn test_logging_config_minimal() {
        let config = LoggingConfig::minimal();
        assert!(!config.log_headers);
        assert!(!config.log_body);
        assert!(config.log_timing);
    }

    #[test]
    fn test_logging_config_verbose() {
        let config = LoggingConfig::verbose();
        assert_eq!(config.level, LogLevel::Debug);
        assert!(config.log_headers);
        assert!(config.log_body);
    }

    #[test]
    fn test_logging_config_builder() {
        let config = LoggingConfig::default()
            .with_level(LogLevel::Warn)
            .with_headers(false)
            .with_slow_threshold(std::time::Duration::from_secs(5));
        
        assert_eq!(config.level, LogLevel::Warn);
        assert!(!config.log_headers);
        assert_eq!(config.slow_request_threshold, std::time::Duration::from_secs(5));
    }

    #[test]
    fn test_log_level_default() {
        assert_eq!(LogLevel::default(), LogLevel::Info);
    }

    #[test]
    fn test_log_level_equality() {
        assert_eq!(LogLevel::Info, LogLevel::Info);
        assert_ne!(LogLevel::Info, LogLevel::Debug);
    }

    #[tokio::test]
    async fn test_logging_middleware_new() {
        let middleware = LoggingMiddleware::new();
        assert_eq!(middleware.name(), "LoggingMiddleware");
    }

    #[tokio::test]
    async fn test_logging_middleware_default() {
        let middleware = LoggingMiddleware::default();
        assert_eq!(middleware.name(), "LoggingMiddleware");
    }

    #[tokio::test]
    async fn test_enhanced_logging_middleware_new() {
        let middleware = EnhancedLoggingMiddleware::new(LoggingConfig::default());
        assert_eq!(middleware.name(), "EnhancedLoggingMiddleware");
    }

    #[tokio::test]
    async fn test_enhanced_logging_middleware_default() {
        let middleware = EnhancedLoggingMiddleware::default();
        assert_eq!(middleware.name(), "EnhancedLoggingMiddleware");
    }

    #[tokio::test]
    async fn test_auth_middleware_allow_all() {
        use axum::http::Request;
        
        let middleware = AuthMiddleware::allow_all();
        let req = Request::builder()
            .header("authorization", "Bearer any-token")
            .uri("/graphql")
            .body(())
            .unwrap();
        let mut ctx = Context::from_request(&req);
        
        assert!(middleware.call(&mut ctx).await.is_ok());
    }

    #[tokio::test]
    async fn test_auth_middleware_require_token() {
        use axum::http::Request;
        
        let middleware = AuthMiddleware::require_token("secret123".to_string());
        
        // Valid token
        let req = Request::builder()
            .header("authorization", "Bearer secret123")
            .uri("/graphql")
            .body(())
            .unwrap();
        let mut ctx = Context::from_request(&req);
        assert!(middleware.call(&mut ctx).await.is_ok());
        
        // Invalid token
        let req = Request::builder()
            .header("authorization", "Bearer wrong")
            .uri("/graphql")
            .body(())
            .unwrap();
        let mut ctx = Context::from_request(&req);
        assert!(middleware.call(&mut ctx).await.is_err());
    }

    #[tokio::test]
    async fn test_auth_middleware_missing_header() {
        use axum::http::Request;
        
        let middleware = AuthMiddleware::allow_all();
        let req = Request::builder()
            .uri("/graphql")
            .body(())
            .unwrap();
        let mut ctx = Context::from_request(&req);
        
        assert!(middleware.call(&mut ctx).await.is_err());
    }

    #[tokio::test]
    async fn test_rate_limit_middleware() {
        use axum::http::Request;
        
        let middleware = RateLimitMiddleware::new(10, 10);
        let req = Request::builder().uri("/graphql").body(()).unwrap();
        let mut ctx = Context::from_request(&req);
        
        // First request should succeed
        assert!(middleware.call(&mut ctx).await.is_ok());
        assert_eq!(middleware.name(), "RateLimitMiddleware");
    }

    #[tokio::test]
    async fn test_rate_limit_per_minute() {
        let middleware = RateLimitMiddleware::per_minute(60, 10);
        assert_eq!(middleware.name(), "RateLimitMiddleware");
    }

    #[test]
    fn test_middleware_chain_new() {
        let chain = MiddlewareChain::new();
        assert_eq!(chain.len(), 0);
        assert!(chain.is_empty());
    }

    #[test]
    fn test_middleware_chain_default() {
        let chain = MiddlewareChain::default();
        assert_eq!(chain.len(), 0);
    }

    #[tokio::test]
    async fn test_middleware_chain_add() {
        let chain = MiddlewareChain::new()
            .add(LoggingMiddleware::new())
            .add(LoggingMiddleware::new());
        
        assert_eq!(chain.len(), 2);
        assert!(!chain.is_empty());
    }

    #[tokio::test]
    async fn test_middleware_chain_add_arc() {
        use std::sync::Arc;
        
        let chain = MiddlewareChain::new()
            .add_arc(Arc::new(LoggingMiddleware::new()));
        
        assert_eq!(chain.len(), 1);
    }

}

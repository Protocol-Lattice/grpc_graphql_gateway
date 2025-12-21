//! OpenTelemetry distributed tracing support for the GraphQL gateway.
//!
//! This module provides distributed tracing integration using OpenTelemetry,
//! enabling end-to-end visibility across GraphQL requests and gRPC backend calls.
//!
//! # Overview
//!
//! Tracing creates spans that track the execution flow:
//!
//! ```text
//! └─ graphql.execute (operation: GetUser)
//!    ├─ graphql.parse
//!    ├─ graphql.validate
//!    └─ graphql.resolve
//!       ├─ grpc.call (UserService/GetUser)
//!       └─ grpc.call (ProfileService/GetProfile)
//! ```
//!
//! # Example
//!
//! ```rust,no_run
//! use grpc_graphql_gateway::{Gateway, TracingConfig};
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let gateway = Gateway::builder()
//!     .enable_tracing()  // Enable OpenTelemetry tracing
//!     .build()?;
//! # Ok(())
//! # }
//! ```
//!
//! # Configuration
//!
//! For OTLP export, enable the `otlp` feature and configure the exporter:
//!
//! ```toml
//! [dependencies]
//! grpc_graphql_gateway = { version = "0.1", features = ["otlp"] }
//! ```

use opentelemetry::{
    global,
    trace::{Span, SpanKind, Status, Tracer},
    KeyValue,
};
use opentelemetry_sdk::{
    trace::{Config, TracerProvider},
    Resource,
};

/// Service name for OpenTelemetry traces
const SERVICE_NAME: &str = "graphql-gateway";

/// Tracing configuration for the gateway
#[derive(Clone, Debug)]
pub struct TracingConfig {
    /// Service name for traces
    pub service_name: String,
    /// Whether tracing is enabled
    pub enabled: bool,
    /// Sample ratio (0.0 = none, 1.0 = all)
    pub sample_ratio: f64,
}

impl Default for TracingConfig {
    fn default() -> Self {
        Self {
            service_name: SERVICE_NAME.to_string(),
            enabled: true,
            sample_ratio: 1.0,
        }
    }
}

impl TracingConfig {
    /// Create a new tracing config with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the service name
    pub fn with_service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = name.into();
        self
    }

    /// Set the sample ratio (0.0 to 1.0)
    pub fn with_sample_ratio(mut self, ratio: f64) -> Self {
        self.sample_ratio = ratio.clamp(0.0, 1.0);
        self
    }

    /// Disable tracing
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }
}

/// Initialize the OpenTelemetry tracer with the given configuration
///
/// Returns the TracerProvider which should be kept alive for the duration of the application.
pub fn init_tracer(config: &TracingConfig) -> TracerProvider {
    let provider = TracerProvider::builder()
        .with_config(
            Config::default().with_resource(Resource::new(vec![KeyValue::new(
                "service.name",
                config.service_name.clone(),
            )])),
        )
        .build();

    global::set_tracer_provider(provider.clone());
    provider
}

/// Shutdown the OpenTelemetry tracer (call on application shutdown)
pub fn shutdown_tracer() {
    global::shutdown_tracer_provider();
}

/// A span for tracing GraphQL operations
pub struct GraphQLSpan {
    span: opentelemetry::global::BoxedSpan,
}

impl GraphQLSpan {
    /// Create a new span for a GraphQL operation
    pub fn new(operation_name: &str, operation_type: &str, query: &str) -> Self {
        let tracer = global::tracer("graphql-gateway");
        let mut span = tracer
            .span_builder(format!("graphql.{}", operation_type))
            .with_kind(SpanKind::Server)
            .start(&tracer);

        span.set_attribute(KeyValue::new(
            "graphql.operation.name",
            operation_name.to_string(),
        ));
        span.set_attribute(KeyValue::new(
            "graphql.operation.type",
            operation_type.to_string(),
        ));
        span.set_attribute(KeyValue::new("graphql.document", query.to_string()));

        Self { span }
    }

    /// Record a successful completion
    pub fn ok(mut self) {
        self.span.set_status(Status::Ok);
        self.span.end();
    }

    /// Record an error
    pub fn error(mut self, message: &str) {
        self.span.set_status(Status::error(message.to_string()));
        self.span.set_attribute(KeyValue::new("error", true));
        self.span
            .set_attribute(KeyValue::new("error.message", message.to_string()));
        self.span.end();
    }

    /// Add a custom attribute
    pub fn set_attribute(&mut self, key: &str, value: impl Into<opentelemetry::Value>) {
        self.span
            .set_attribute(KeyValue::new(key.to_string(), value.into()));
    }
}

/// A span for tracing gRPC backend calls
pub struct GrpcSpan {
    span: opentelemetry::global::BoxedSpan,
}

impl GrpcSpan {
    /// Create a new span for a gRPC call
    pub fn new(service: &str, method: &str) -> Self {
        let tracer = global::tracer("graphql-gateway");
        let mut span = tracer
            .span_builder(format!("{}/{}", service, method))
            .with_kind(SpanKind::Client)
            .start(&tracer);

        span.set_attribute(KeyValue::new("rpc.system", "grpc"));
        span.set_attribute(KeyValue::new("rpc.service", service.to_string()));
        span.set_attribute(KeyValue::new("rpc.method", method.to_string()));

        Self { span }
    }

    /// Record a successful completion
    pub fn ok(mut self) {
        self.span.set_status(Status::Ok);
        self.span
            .set_attribute(KeyValue::new("rpc.grpc.status_code", 0i64));
        self.span.end();
    }

    /// Record an error with gRPC status code
    pub fn error(mut self, code: i32, message: &str) {
        self.span.set_status(Status::error(message.to_string()));
        self.span
            .set_attribute(KeyValue::new("rpc.grpc.status_code", code as i64));
        self.span.set_attribute(KeyValue::new("error", true));
        self.span
            .set_attribute(KeyValue::new("error.message", message.to_string()));
        self.span.end();
    }

    /// Add a custom attribute
    pub fn set_attribute(&mut self, key: &str, value: impl Into<opentelemetry::Value>) {
        self.span
            .set_attribute(KeyValue::new(key.to_string(), value.into()));
    }
}

/// Tracing middleware that can be used to instrument requests
#[derive(Clone)]
pub struct TracingMiddleware {
    config: TracingConfig,
}

impl TracingMiddleware {
    /// Create a new tracing middleware
    pub fn new() -> Self {
        Self {
            config: TracingConfig::default(),
        }
    }

    /// Create with custom configuration
    pub fn with_config(config: TracingConfig) -> Self {
        Self { config }
    }

    /// Check if tracing is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }
}

impl Default for TracingMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tracing_config_default() {
        let config = TracingConfig::default();
        assert!(config.enabled);
        assert_eq!(config.sample_ratio, 1.0);
        assert_eq!(config.service_name, SERVICE_NAME);
    }

    #[test]
    fn test_tracing_config_disabled() {
        let config = TracingConfig::disabled();
        assert!(!config.enabled);
    }

    #[test]
    fn test_tracing_config_builder() {
        let config = TracingConfig::new()
            .with_service_name("my-gateway")
            .with_sample_ratio(0.5);

        assert_eq!(config.service_name, "my-gateway");
        assert_eq!(config.sample_ratio, 0.5);
    }

    #[test]
    fn test_sample_ratio_clamping() {
        let config = TracingConfig::new().with_sample_ratio(2.0);
        assert_eq!(config.sample_ratio, 1.0);

        let config = TracingConfig::new().with_sample_ratio(-1.0);
        assert_eq!(config.sample_ratio, 0.0);
    }

    #[test]
    fn test_tracing_middleware_creation() {
        let middleware = TracingMiddleware::new();
        assert!(middleware.is_enabled());

        let middleware = TracingMiddleware::with_config(TracingConfig::disabled());
        assert!(!middleware.is_enabled());
    }

    #[test]
    fn test_graphql_span_lifecycle() {
        // Ensure we can create and manipulate spans without panicking
        // Note: Use no-op global tracer implicitly
        let mut span = GraphQLSpan::new("getUsers", "query", "{ users { id } }");
        
        span.set_attribute("custom.tag", "value");
        span.set_attribute("complexity", 100);
        
        span.ok();
        // Should not panic on end()
    }

    #[test]
    fn test_graphql_span_error() {
        let span = GraphQLSpan::new("badQuery", "query", "{ error }");
        span.error("Something went wrong");
    }

    #[test]
    fn test_grpc_span_lifecycle() {
        let mut span = GrpcSpan::new("UserService", "GetUser");
        
        span.set_attribute("peer.address", "127.0.0.1");
        span.set_attribute("retry", 1);
        
        span.ok();
    }

    #[test]
    fn test_grpc_span_error() {
        let span = GrpcSpan::new("UserService", "GetUser");
        span.error(13, "Internal Error"); // 13 = INTERNAL
    }

    #[test]
    fn test_init_tracer() {
        // Just verify it runs without crashing. 
        // Real verification would require checking the global state which is hard in parallel tests.
        let config = TracingConfig::disabled();
        let _provider = init_tracer(&config);
    }
}

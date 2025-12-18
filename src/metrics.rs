//! Metrics and observability support for the GraphQL gateway.
//!
//! This module provides Prometheus metrics collection for monitoring
//! GraphQL requests, latencies, and gRPC backend performance.
//!
//! # Metrics Exposed
//!
//! - `graphql_requests_total` - Total number of GraphQL requests by operation type
//! - `graphql_request_duration_seconds` - Request latency histogram
//! - `graphql_errors_total` - Total number of GraphQL errors
//! - `grpc_backend_requests_total` - Total gRPC backend calls by service
//! - `grpc_backend_duration_seconds` - gRPC backend latency histogram
//!
//! # Example
//!
//! ```rust,no_run
//! use grpc_graphql_gateway::{Gateway, GatewayMetrics};
//!
//! // Access metrics
//! let metrics = GatewayMetrics::global();
//! println!("Total requests: {}", metrics.requests_total());
//! ```

use once_cell::sync::Lazy;
use prometheus::{
    register_histogram_vec, register_int_counter_vec, Encoder, HistogramVec, IntCounterVec,
    TextEncoder,
};
use std::time::Instant;

/// Default histogram buckets for latency measurements (in seconds)
const LATENCY_BUCKETS: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// Global metrics registry for the gateway
static METRICS: Lazy<GatewayMetrics> = Lazy::new(GatewayMetrics::new);

/// Gateway metrics for Prometheus monitoring.
///
/// Provides counters and histograms for tracking GraphQL request performance
/// and gRPC backend health.
#[derive(Clone)]
pub struct GatewayMetrics {
    /// Total GraphQL requests by operation type (query, mutation, subscription)
    pub graphql_requests: IntCounterVec,
    /// GraphQL request duration in seconds
    pub graphql_duration: HistogramVec,
    /// Total GraphQL errors by type
    pub graphql_errors: IntCounterVec,
    /// Total gRPC backend requests by service
    pub grpc_requests: IntCounterVec,
    /// gRPC backend request duration in seconds
    pub grpc_duration: HistogramVec,
    /// gRPC backend errors by service
    pub grpc_errors: IntCounterVec,
}

impl GatewayMetrics {
    /// Create a new metrics instance with registered Prometheus metrics
    pub fn new() -> Self {
        Self {
            graphql_requests: register_int_counter_vec!(
                "graphql_requests_total",
                "Total number of GraphQL requests",
                &["operation"]
            )
            .expect("metric can be created"),

            graphql_duration: register_histogram_vec!(
                "graphql_request_duration_seconds",
                "GraphQL request duration in seconds",
                &["operation"],
                LATENCY_BUCKETS.to_vec()
            )
            .expect("metric can be created"),

            graphql_errors: register_int_counter_vec!(
                "graphql_errors_total",
                "Total number of GraphQL errors",
                &["error_type"]
            )
            .expect("metric can be created"),

            grpc_requests: register_int_counter_vec!(
                "grpc_backend_requests_total",
                "Total number of gRPC backend requests",
                &["service", "method"]
            )
            .expect("metric can be created"),

            grpc_duration: register_histogram_vec!(
                "grpc_backend_duration_seconds",
                "gRPC backend request duration in seconds",
                &["service", "method"],
                LATENCY_BUCKETS.to_vec()
            )
            .expect("metric can be created"),

            grpc_errors: register_int_counter_vec!(
                "grpc_backend_errors_total",
                "Total number of gRPC backend errors",
                &["service", "method", "code"]
            )
            .expect("metric can be created"),
        }
    }

    /// Get the global metrics instance
    pub fn global() -> &'static Self {
        &METRICS
    }

    /// Record a GraphQL request
    pub fn record_graphql_request(&self, operation: &str) {
        self.graphql_requests.with_label_values(&[operation]).inc();
    }

    /// Record GraphQL request duration
    pub fn record_graphql_duration(&self, operation: &str, duration_secs: f64) {
        self.graphql_duration
            .with_label_values(&[operation])
            .observe(duration_secs);
    }

    /// Record a GraphQL error
    pub fn record_graphql_error(&self, error_type: &str) {
        self.graphql_errors.with_label_values(&[error_type]).inc();
    }

    /// Record a gRPC backend request
    pub fn record_grpc_request(&self, service: &str, method: &str) {
        self.grpc_requests
            .with_label_values(&[service, method])
            .inc();
    }

    /// Record gRPC backend request duration
    pub fn record_grpc_duration(&self, service: &str, method: &str, duration_secs: f64) {
        self.grpc_duration
            .with_label_values(&[service, method])
            .observe(duration_secs);
    }

    /// Record a gRPC backend error
    pub fn record_grpc_error(&self, service: &str, method: &str, code: &str) {
        self.grpc_errors
            .with_label_values(&[service, method, code])
            .inc();
    }

    /// Get total request count (for health checks / debugging)
    pub fn requests_total(&self) -> u64 {
        let mut total = 0;
        for operation in &["query", "mutation", "subscription"] {
            total += self.graphql_requests.with_label_values(&[operation]).get();
        }
        total
    }

    /// Render all metrics in Prometheus text format
    pub fn render(&self) -> String {
        let encoder = TextEncoder::new();
        let metric_families = prometheus::gather();
        let mut buffer = Vec::new();
        encoder
            .encode(&metric_families, &mut buffer)
            .expect("encoding metrics");
        String::from_utf8(buffer).expect("valid utf8")
    }
}

impl Default for GatewayMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// A timer that records duration to a histogram when dropped
pub struct RequestTimer {
    start: Instant,
    operation: String,
    metrics: &'static GatewayMetrics,
}

impl RequestTimer {
    /// Start a new request timer for the given operation type
    pub fn new(operation: impl Into<String>) -> Self {
        let operation = operation.into();
        let metrics = GatewayMetrics::global();
        metrics.record_graphql_request(&operation);
        Self {
            start: Instant::now(),
            operation,
            metrics,
        }
    }

    /// Record an error for this request
    pub fn record_error(&self, error_type: &str) {
        self.metrics.record_graphql_error(error_type);
    }
}

impl Drop for RequestTimer {
    fn drop(&mut self) {
        let duration = self.start.elapsed().as_secs_f64();
        self.metrics
            .record_graphql_duration(&self.operation, duration);
    }
}

/// Timer for gRPC backend calls
pub struct GrpcTimer {
    start: Instant,
    service: String,
    method: String,
    metrics: &'static GatewayMetrics,
}

impl GrpcTimer {
    /// Start a new gRPC timer
    pub fn new(service: impl Into<String>, method: impl Into<String>) -> Self {
        let service = service.into();
        let method = method.into();
        let metrics = GatewayMetrics::global();
        metrics.record_grpc_request(&service, &method);
        Self {
            start: Instant::now(),
            service,
            method,
            metrics,
        }
    }

    /// Record an error for this gRPC call
    pub fn record_error(&self, code: &str) {
        self.metrics
            .record_grpc_error(&self.service, &self.method, code);
    }
}

impl Drop for GrpcTimer {
    fn drop(&mut self) {
        let duration = self.start.elapsed().as_secs_f64();
        self.metrics
            .record_grpc_duration(&self.service, &self.method, duration);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_creation() {
        // This will use the global metrics
        let metrics = GatewayMetrics::global();

        // Record some requests
        metrics.record_graphql_request("query");
        metrics.record_graphql_request("query");
        metrics.record_graphql_request("mutation");

        // Verify counts
        assert!(metrics.requests_total() >= 3);
    }

    #[test]
    fn test_metrics_render() {
        let metrics = GatewayMetrics::global();
        metrics.record_graphql_request("query");

        let output = metrics.render();
        assert!(output.contains("graphql_requests_total"));
    }

    #[test]
    fn test_request_timer() {
        let _timer = RequestTimer::new("query");
        // Timer will auto-record duration when dropped
    }

    #[test]
    fn test_grpc_timer() {
        let timer = GrpcTimer::new("UserService", "GetUser");
        timer.record_error("NOT_FOUND");
        // Timer will auto-record duration when dropped
    }
}

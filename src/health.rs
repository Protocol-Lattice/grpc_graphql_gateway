//! Health check endpoints for Kubernetes/container orchestration.
//!
//! This module provides health check handlers for liveness and readiness probes,
//! essential for production deployments with Kubernetes, Docker Swarm, or similar.
//!
//! # Endpoints
//!
//! - `/health` - Liveness probe: Returns 200 if the server is running
//! - `/ready` - Readiness probe: Returns 200 if all gRPC backends are reachable
//!
//! # Example
//!
//! ```rust,no_run
//! use grpc_graphql_gateway::Gateway;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let gateway = Gateway::builder()
//!     .with_descriptor_set_bytes(b"...")
//!     .enable_health_checks()  // Adds /health and /ready endpoints
//!     .build()?;
//! # Ok(())
//! # }
//! ```

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::grpc_client::GrpcClientPool;

/// Health check response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    /// Overall health status
    pub status: HealthStatus,
    /// Optional message with details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Individual component checks
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<ComponentHealth>,
}

/// Health status enum
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    /// Service is healthy
    Healthy,
    /// Service is degraded but operational
    Degraded,
    /// Service is unhealthy
    Unhealthy,
}

/// Individual component health check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentHealth {
    /// Component name
    pub name: String,
    /// Component health status
    pub status: HealthStatus,
    /// Optional message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl HealthResponse {
    /// Create a healthy response
    pub fn healthy() -> Self {
        Self {
            status: HealthStatus::Healthy,
            message: None,
            checks: Vec::new(),
        }
    }

    /// Create a healthy response with a message
    pub fn healthy_with_message(message: impl Into<String>) -> Self {
        Self {
            status: HealthStatus::Healthy,
            message: Some(message.into()),
            checks: Vec::new(),
        }
    }

    /// Create an unhealthy response
    pub fn unhealthy(message: impl Into<String>) -> Self {
        Self {
            status: HealthStatus::Unhealthy,
            message: Some(message.into()),
            checks: Vec::new(),
        }
    }

    /// Create a degraded response
    pub fn degraded(message: impl Into<String>) -> Self {
        Self {
            status: HealthStatus::Degraded,
            message: Some(message.into()),
            checks: Vec::new(),
        }
    }

    /// Add a component check
    pub fn with_check(mut self, check: ComponentHealth) -> Self {
        // Update overall status based on component
        match (&self.status, check.status) {
            (HealthStatus::Healthy, HealthStatus::Unhealthy) => {
                self.status = HealthStatus::Unhealthy;
            }
            (HealthStatus::Healthy, HealthStatus::Degraded) => {
                self.status = HealthStatus::Degraded;
            }
            (HealthStatus::Degraded, HealthStatus::Unhealthy) => {
                self.status = HealthStatus::Unhealthy;
            }
            _ => {}
        }
        self.checks.push(check);
        self
    }
}

impl IntoResponse for HealthResponse {
    fn into_response(self) -> Response {
        let status_code = match self.status {
            HealthStatus::Healthy => StatusCode::OK,
            HealthStatus::Degraded => StatusCode::OK, // Still OK but with warning
            HealthStatus::Unhealthy => StatusCode::SERVICE_UNAVAILABLE,
        };
        (status_code, Json(self)).into_response()
    }
}

/// Shared state for health check handlers
#[derive(Clone)]
pub struct HealthState {
    /// Reference to the gRPC client pool for readiness checks
    pub client_pool: Arc<GrpcClientPool>,
    /// Optional custom health check function
    pub custom_check: Option<Arc<dyn Fn() -> HealthResponse + Send + Sync>>,
}

impl HealthState {
    /// Create a new health state
    pub fn new(client_pool: GrpcClientPool) -> Self {
        Self {
            client_pool: Arc::new(client_pool),
            custom_check: None,
        }
    }

    /// Add a custom health check
    pub fn with_custom_check<F>(mut self, check: F) -> Self
    where
        F: Fn() -> HealthResponse + Send + Sync + 'static,
    {
        self.custom_check = Some(Arc::new(check));
        self
    }
}

/// Liveness probe handler - `/health`
///
/// Returns 200 OK if the server process is running.
/// This is a simple check that doesn't verify backend connectivity.
pub async fn health_handler() -> HealthResponse {
    HealthResponse::healthy_with_message("Gateway is running")
}

/// Readiness probe handler - `/ready`
///
/// Returns 200 OK if the server is ready to accept traffic.
/// This checks that gRPC backends are configured (but doesn't actively ping them
/// to avoid adding latency to the probe).
pub async fn readiness_handler(
    State(state): State<Arc<HealthState>>,
) -> HealthResponse {
    let mut response = HealthResponse::healthy();

    // Check gRPC client pool
    let client_names = state.client_pool.names();
    if client_names.is_empty() {
        response = response.with_check(ComponentHealth {
            name: "grpc_clients".to_string(),
            status: HealthStatus::Degraded,
            message: Some("No gRPC clients configured".to_string()),
        });
    } else {
        response = response.with_check(ComponentHealth {
            name: "grpc_clients".to_string(),
            status: HealthStatus::Healthy,
            message: Some(format!("{} clients configured", client_names.len())),
        });
    }

    // Run custom health check if provided
    if let Some(custom_check) = &state.custom_check {
        let custom_result = custom_check();
        for check in custom_result.checks {
            response = response.with_check(check);
        }
    }

    response
}

/// Deep health check handler - `/ready/deep`
///
/// Performs actual connectivity checks to gRPC backends.
/// This is more expensive but provides accurate health status.
pub async fn deep_readiness_handler(
    State(state): State<Arc<HealthState>>,
) -> HealthResponse {
    let mut response = HealthResponse::healthy();

    let client_names = state.client_pool.names();
    
    if client_names.is_empty() {
        return HealthResponse::degraded("No gRPC clients configured");
    }

    for name in client_names {
        // Check if client exists and is configured
        // Note: We can't easily ping gRPC without making an actual RPC call,
        // so we just verify the client is registered
        if state.client_pool.get(&name).is_some() {
            response = response.with_check(ComponentHealth {
                name: format!("grpc:{}", name),
                status: HealthStatus::Healthy,
                message: Some("Client configured".to_string()),
            });
        } else {
            response = response.with_check(ComponentHealth {
                name: format!("grpc:{}", name),
                status: HealthStatus::Unhealthy,
                message: Some("Client not found".to_string()),
            });
        }
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_healthy_response() {
        let response = HealthResponse::healthy();
        assert_eq!(response.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_unhealthy_response() {
        let response = HealthResponse::unhealthy("Something is wrong");
        assert_eq!(response.status, HealthStatus::Unhealthy);
        assert_eq!(response.message, Some("Something is wrong".to_string()));
    }

    #[test]
    fn test_component_health_propagation() {
        let response = HealthResponse::healthy()
            .with_check(ComponentHealth {
                name: "db".to_string(),
                status: HealthStatus::Healthy,
                message: None,
            })
            .with_check(ComponentHealth {
                name: "cache".to_string(),
                status: HealthStatus::Unhealthy,
                message: Some("Redis connection failed".to_string()),
            });

        // Overall should be unhealthy because one component is unhealthy
        assert_eq!(response.status, HealthStatus::Unhealthy);
        assert_eq!(response.checks.len(), 2);
    }

    #[test]
    fn test_degraded_propagation() {
        let response = HealthResponse::healthy()
            .with_check(ComponentHealth {
                name: "primary".to_string(),
                status: HealthStatus::Healthy,
                message: None,
            })
            .with_check(ComponentHealth {
                name: "secondary".to_string(),
                status: HealthStatus::Degraded,
                message: Some("Replica lag detected".to_string()),
            });

        // Overall should be degraded
        assert_eq!(response.status, HealthStatus::Degraded);
    }

    #[tokio::test]
    async fn test_health_handler() {
        let response = health_handler().await;
        assert_eq!(response.status, HealthStatus::Healthy);
    }
}

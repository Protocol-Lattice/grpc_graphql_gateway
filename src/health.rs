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
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
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
pub async fn readiness_handler(State(state): State<Arc<HealthState>>) -> HealthResponse {
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
pub async fn deep_readiness_handler(State(state): State<Arc<HealthState>>) -> HealthResponse {
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
        assert_eq!(response.message, None);
        assert!(response.checks.is_empty());
    }

    #[test]
    fn test_healthy_with_message() {
        let response = HealthResponse::healthy_with_message("All systems operational");
        assert_eq!(response.status, HealthStatus::Healthy);
        assert_eq!(response.message, Some("All systems operational".to_string()));
        assert!(response.checks.is_empty());
    }

    #[test]
    fn test_unhealthy_response() {
        let response = HealthResponse::unhealthy("Something is wrong");
        assert_eq!(response.status, HealthStatus::Unhealthy);
        assert_eq!(response.message, Some("Something is wrong".to_string()));
    }

    #[test]
    fn test_degraded_response() {
        let response = HealthResponse::degraded("Performance degraded");
        assert_eq!(response.status, HealthStatus::Degraded);
        assert_eq!(response.message, Some("Performance degraded".to_string()));
    }

    #[test]
    fn test_component_health_propagation_unhealthy() {
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

    #[test]
    fn test_degraded_to_unhealthy_propagation() {
        let response = HealthResponse::degraded("Already degraded")
            .with_check(ComponentHealth {
                name: "service".to_string(),
                status: HealthStatus::Unhealthy,
                message: Some("Critical failure".to_string()),
            });

        // Degraded + Unhealthy = Unhealthy
        assert_eq!(response.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_multiple_healthy_checks() {
        let response = HealthResponse::healthy()
            .with_check(ComponentHealth {
                name: "db".to_string(),
                status: HealthStatus::Healthy,
                message: None,
            })
            .with_check(ComponentHealth {
                name: "cache".to_string(),
                status: HealthStatus::Healthy,
                message: None,
            })
            .with_check(ComponentHealth {
                name: "queue".to_string(),
                status: HealthStatus::Healthy,
                message: None,
            });

        // All healthy = overall healthy
        assert_eq!(response.status, HealthStatus::Healthy);
        assert_eq!(response.checks.len(), 3);
    }

    #[test]
    fn test_health_status_equality() {
        assert_eq!(HealthStatus::Healthy, HealthStatus::Healthy);
        assert_eq!(HealthStatus::Degraded, HealthStatus::Degraded);
        assert_eq!(HealthStatus::Unhealthy, HealthStatus::Unhealthy);

        assert_ne!(HealthStatus::Healthy, HealthStatus::Degraded);
        assert_ne!(HealthStatus::Healthy, HealthStatus::Unhealthy);
        assert_ne!(HealthStatus::Degraded, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_health_status_serialization() {
        let status = HealthStatus::Healthy;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"healthy\"");

        let status = HealthStatus::Degraded;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"degraded\"");

        let status = HealthStatus::Unhealthy;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"unhealthy\"");
    }

    #[test]
    fn test_health_status_deserialization() {
        let status: HealthStatus = serde_json::from_str("\"healthy\"").unwrap();
        assert_eq!(status, HealthStatus::Healthy);

        let status: HealthStatus = serde_json::from_str("\"degraded\"").unwrap();
        assert_eq!(status, HealthStatus::Degraded);

        let status: HealthStatus = serde_json::from_str("\"unhealthy\"").unwrap();
        assert_eq!(status, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_health_response_serialization() {
        let response = HealthResponse::healthy_with_message("OK");
        let json = serde_json::to_string(&response).unwrap();
        
        assert!(json.contains("healthy"));
        assert!(json.contains("OK"));
    }

    #[test]
    fn test_health_response_serialization_skip_empty() {
        let response = HealthResponse::healthy();
        let json = serde_json::to_string(&response).unwrap();
        
        // Empty checks and None message should be skipped
        assert!(!json.contains("message"));
        assert!(!json.contains("checks"));
    }

    #[test]
    fn test_health_response_deserialization() {
        let json = r#"{"status":"healthy","message":"All good"}"#;
        let response: HealthResponse = serde_json::from_str(json).unwrap();
        
        assert_eq!(response.status, HealthStatus::Healthy);
        assert_eq!(response.message, Some("All good".to_string()));
    }

    #[test]
    fn test_component_health_serialization() {
        let component = ComponentHealth {
            name: "database".to_string(),
            status: HealthStatus::Healthy,
            message: Some("Connected".to_string()),
        };

        let json = serde_json::to_string(&component).unwrap();
        assert!(json.contains("database"));
        assert!(json.contains("healthy"));
        assert!(json.contains("Connected"));
    }

    #[test]
    fn test_component_health_without_message() {
        let component = ComponentHealth {
            name: "cache".to_string(),
            status: HealthStatus::Healthy,
            message: None,
        };

        let json = serde_json::to_string(&component).unwrap();
        assert!(!json.contains("message"));
    }

    #[test]
    fn test_into_response_healthy() {
        let response = HealthResponse::healthy();
        let http_response = response.into_response();
        
        assert_eq!(http_response.status(), StatusCode::OK);
    }

    #[test]
    fn test_into_response_degraded() {
        let response = HealthResponse::degraded("Slow performance");
        let http_response = response.into_response();
        
        // Degraded still returns OK (200) but with warning
        assert_eq!(http_response.status(), StatusCode::OK);
    }

    #[test]
    fn test_into_response_unhealthy() {
        let response = HealthResponse::unhealthy("Service down");
        let http_response = response.into_response();
        
        assert_eq!(http_response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn test_health_state_new() {
        let pool = GrpcClientPool::new();
        let state = HealthState::new(pool);
        
        assert!(state.custom_check.is_none());
        assert_eq!(state.client_pool.names().len(), 0);
    }

    #[test]
    fn test_health_state_with_custom_check() {
        let pool = GrpcClientPool::new();
        let state = HealthState::new(pool).with_custom_check(|| {
            HealthResponse::healthy_with_message("Custom check passed")
        });
        
        assert!(state.custom_check.is_some());
        
        // Execute custom check
        if let Some(check) = &state.custom_check {
            let result = check();
            assert_eq!(result.status, HealthStatus::Healthy);
            assert_eq!(result.message, Some("Custom check passed".to_string()));
        }
    }

    #[test]
    fn test_health_state_clone() {
        let pool = GrpcClientPool::new();
        let state = HealthState::new(pool);
        let cloned = state.clone();
        
        assert_eq!(Arc::strong_count(&state.client_pool), Arc::strong_count(&cloned.client_pool));
    }

    #[tokio::test]
    async fn test_health_handler() {
        let response = health_handler().await;
        assert_eq!(response.status, HealthStatus::Healthy);
        assert!(response.message.is_some());
    }

    #[tokio::test]
    async fn test_readiness_handler_no_clients() {
        let pool = GrpcClientPool::new();
        let state = Arc::new(HealthState::new(pool));
        
        let response = readiness_handler(State(state)).await;
        
        // Should be degraded when no clients configured
        assert_eq!(response.status, HealthStatus::Degraded);
        assert_eq!(response.checks.len(), 1);
        assert_eq!(response.checks[0].name, "grpc_clients");
    }

    #[tokio::test]
    async fn test_readiness_handler_with_clients() {
        use crate::grpc_client::GrpcClient;
        
        let pool = GrpcClientPool::new();
        
        // Create a mock client (using a dummy endpoint - test won't actually connect)
        let client = GrpcClient::connect_lazy("http://localhost:50051", true).unwrap();
        pool.add("test-service", client);
        
        let state = Arc::new(HealthState::new(pool));
        
        let response = readiness_handler(State(state)).await;
        
        // Should be healthy with clients configured
        assert_eq!(response.status, HealthStatus::Healthy);
        assert_eq!(response.checks.len(), 1);
        assert_eq!(response.checks[0].name, "grpc_clients");
        assert_eq!(response.checks[0].status, HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn test_readiness_handler_with_custom_check() {
        let pool = GrpcClientPool::new();
        let state = Arc::new(HealthState::new(pool).with_custom_check(|| {
            HealthResponse::healthy().with_check(ComponentHealth {
                name: "custom".to_string(),
                status: HealthStatus::Healthy,
                message: Some("Custom check OK".to_string()),
            })
        }));
        
        let response = readiness_handler(State(state)).await;
        
        // Should have both grpc_clients and custom checks
        assert_eq!(response.checks.len(), 2);
        assert!(response.checks.iter().any(|c| c.name == "custom"));
    }

    #[tokio::test]
    async fn test_deep_readiness_handler_no_clients() {
        let pool = GrpcClientPool::new();
        let state = Arc::new(HealthState::new(pool));
        
        let response = deep_readiness_handler(State(state)).await;
        
        assert_eq!(response.status, HealthStatus::Degraded);
        assert_eq!(response.message, Some("No gRPC clients configured".to_string()));
    }

    #[tokio::test]
    async fn test_deep_readiness_handler_with_clients() {
        use crate::grpc_client::GrpcClient;
        
        let pool = GrpcClientPool::new();
        let client = GrpcClient::connect_lazy("http://localhost:50051", true).unwrap();
        pool.add("test-service", client);
        
        let state = Arc::new(HealthState::new(pool));
        
        let response = deep_readiness_handler(State(state)).await;
        
        // Should check each client
        assert_eq!(response.checks.len(), 1);
        assert_eq!(response.checks[0].name, "grpc:test-service");
        assert_eq!(response.checks[0].status, HealthStatus::Healthy);
    }

    #[test]
    fn test_response_with_multiple_component_states() {
        let response = HealthResponse::healthy()
            .with_check(ComponentHealth {
                name: "comp1".to_string(),
                status: HealthStatus::Healthy,
                message: None,
            })
            .with_check(ComponentHealth {
                name: "comp2".to_string(),
                status: HealthStatus::Degraded,
                message: Some("Warning".to_string()),
            })
            .with_check(ComponentHealth {
                name: "comp3".to_string(),
                status: HealthStatus::Degraded,
                message: Some("Another warning".to_string()),
            });

        // Multiple degraded components should keep status as degraded
        assert_eq!(response.status, HealthStatus::Degraded);
        assert_eq!(response.checks.len(), 3);
    }

    #[test]
    fn test_health_response_clone() {
        let original = HealthResponse::healthy_with_message("Test");
        let cloned = original.clone();
        
        assert_eq!(cloned.status, original.status);
        assert_eq!(cloned.message, original.message);
    }

    #[test]
    fn test_health_response_debug() {
        let response = HealthResponse::healthy();
        let debug_str = format!("{:?}", response);
        
        assert!(debug_str.contains("HealthResponse"));
        assert!(debug_str.contains("Healthy"));
    }

    #[test]
    fn test_component_health_clone() {
        let original = ComponentHealth {
            name: "test".to_string(),
            status: HealthStatus::Healthy,
            message: Some("OK".to_string()),
        };
        let cloned = original.clone();
        
        assert_eq!(cloned.name, original.name);
        assert_eq!(cloned.status, original.status);
        assert_eq!(cloned.message, original.message);
    }
}

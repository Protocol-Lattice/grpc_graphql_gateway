//! Error types for the gRPC-GraphQL gateway

use thiserror::Error;

/// Result type alias using our Error type
pub type Result<T> = std::result::Result<T, Error>;

/// Main error type for the gateway
///
/// This enum covers all possible errors that can occur within the gateway,
/// including gRPC errors, schema errors, and runtime errors.
#[derive(Error, Debug)]
pub enum Error {
    /// gRPC transport errors
    #[error("gRPC error: {0}")]
    Grpc(#[from] tonic::Status),

    /// gRPC transport errors
    #[error("gRPC transport error: {0}")]
    Transport(#[from] tonic::transport::Error),

    /// GraphQL schema errors
    #[error("GraphQL schema error: {0}")]
    Schema(String),

    /// Invalid request errors
    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    /// Authentication/authorization errors
    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    /// Middleware errors
    #[error("Middleware error: {0}")]
    Middleware(String),

    /// Serialization errors
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Connection errors
    #[error("Connection error: {0}")]
    Connection(String),

    /// WebSocket errors
    #[error("WebSocket error: {0}")]
    WebSocket(String),

    /// Internal errors
    #[error("Internal error: {0}")]
    Internal(String),

    /// IO errors
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Rate limiting errors
    #[error("Too many requests: {0}")]
    TooManyRequests(String),

    /// Query depth limit exceeded (DoS protection)
    #[error("Query depth limit exceeded: {0}")]
    QueryTooDeep(String),

    /// Query complexity limit exceeded (DoS protection)
    #[error("Query complexity limit exceeded: {0}")]
    QueryTooComplex(String),

    /// Any other error
    #[error("Error: {0}")]
    Other(#[from] anyhow::Error),
}

impl Error {
    /// Convert error to GraphQL error format
    ///
    /// # Security
    ///
    /// In production (ENV=production), internal error details are sanitized
    /// to prevent information disclosure. Only safe error types show their
    /// full message to clients.
    pub fn to_graphql_error(&self) -> GraphQLError {
        let is_production = std::env::var("ENV")
            .map(|e| e == "production" || e == "prod")
            .unwrap_or(false);

        let message = if is_production {
            // SECURITY: Sanitize internal errors for production
            match self {
                Error::Grpc(_) => "Backend service error".to_string(),
                Error::Transport(_) => "Service connection error".to_string(),
                Error::Internal(_) => "Internal server error".to_string(),
                Error::Io(_) => "Internal server error".to_string(),
                Error::Connection(_) => "Service connection error".to_string(),
                Error::WebSocket(_) => "Connection error".to_string(),
                // These errors are safe to expose to clients
                Error::Schema(msg) => format!("Schema error: {}", msg),
                Error::InvalidRequest(msg) => format!("Invalid request: {}", msg),
                Error::Unauthorized(msg) => msg.clone(),
                Error::Middleware(msg) => format!("Request processing error: {}", msg),
                Error::Serialization(_) => "Data processing error".to_string(),
                Error::TooManyRequests(msg) => msg.clone(),
                Error::QueryTooDeep(msg) => msg.clone(),
                Error::QueryTooComplex(msg) => msg.clone(),
                Error::Other(_) => "An unexpected error occurred".to_string(),
            }
        } else {
            // In development, show full error details
            self.to_string()
        };

        GraphQLError {
            message,
            extensions: self.extensions(),
        }
    }

    /// Get error code for extensions
    fn extensions(&self) -> std::collections::HashMap<String, serde_json::Value> {
        let mut map = std::collections::HashMap::new();
        let code = match self {
            Error::Grpc(_) => "GRPC_ERROR",
            Error::Transport(_) => "TRANSPORT_ERROR",
            Error::Schema(_) => "SCHEMA_ERROR",
            Error::InvalidRequest(_) => "INVALID_REQUEST",
            Error::Unauthorized(_) => "UNAUTHORIZED",
            Error::Middleware(_) => "MIDDLEWARE_ERROR",
            Error::Serialization(_) => "SERIALIZATION_ERROR",
            Error::Connection(_) => "CONNECTION_ERROR",
            Error::WebSocket(_) => "WEBSOCKET_ERROR",
            Error::Internal(_) => "INTERNAL_ERROR",
            Error::Io(_) => "IO_ERROR",
            Error::TooManyRequests(_) => "TOO_MANY_REQUESTS",
            Error::QueryTooDeep(_) => "QUERY_TOO_DEEP",
            Error::QueryTooComplex(_) => "QUERY_TOO_COMPLEX",
            Error::Other(_) => "UNKNOWN_ERROR",
        };
        map.insert("code".to_string(), serde_json::json!(code));
        map
    }
}

/// GraphQL error response format
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GraphQLError {
    pub message: String,
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub extensions: std::collections::HashMap<String, serde_json::Value>,
}

impl From<Error> for GraphQLError {
    fn from(err: Error) -> Self {
        err.to_graphql_error()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::Mutex;
    
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_error_display() {
        let err = Error::Schema("invalid field".to_string());
        assert_eq!(err.to_string(), "GraphQL schema error: invalid field");

        let err = Error::InvalidRequest("missing query".to_string());
        assert_eq!(err.to_string(), "Invalid request: missing query");

        let err = Error::Unauthorized("invalid token".to_string());
        assert_eq!(err.to_string(), "Unauthorized: invalid token");

        let err = Error::Middleware("failed to process".to_string());
        assert_eq!(err.to_string(), "Middleware error: failed to process");

        let err = Error::Connection("timeout".to_string());
        assert_eq!(err.to_string(), "Connection error: timeout");

        let err = Error::WebSocket("closed unexpectedly".to_string());
        assert_eq!(err.to_string(), "WebSocket error: closed unexpectedly");

        let err = Error::Internal("panic occurred".to_string());
        assert_eq!(err.to_string(), "Internal error: panic occurred");

        let err = Error::TooManyRequests("rate limit exceeded".to_string());
        assert_eq!(err.to_string(), "Too many requests: rate limit exceeded");

        let err = Error::QueryTooDeep("max depth 10".to_string());
        assert_eq!(err.to_string(), "Query depth limit exceeded: max depth 10");

        let err = Error::QueryTooComplex("max complexity 1000".to_string());
        assert_eq!(err.to_string(), "Query complexity limit exceeded: max complexity 1000");
    }

    #[test]
    fn test_error_from_io() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "file not found");
        let err: Error = io_err.into();
        assert!(matches!(err, Error::Io(_)));
        assert!(err.to_string().contains("file not found"));
    }

    #[test]
    fn test_error_from_serde_json() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid json");
        assert!(json_err.is_err());
        let err: Error = json_err.unwrap_err().into();
        assert!(matches!(err, Error::Serialization(_)));
    }

    #[test]
    fn test_error_from_tonic_status() {
        let status = tonic::Status::internal("internal error");
        let err: Error = status.into();
        assert!(matches!(err, Error::Grpc(_)));
        assert!(err.to_string().contains("internal error"));
    }

    #[test]
    fn test_error_from_anyhow() {
        let anyhow_err = anyhow::anyhow!("something went wrong");
        let err: Error = anyhow_err.into();
        assert!(matches!(err, Error::Other(_)));
    }

    #[test]
    fn test_graphql_error_conversion_development() {
        let _guard = ENV_LOCK.lock().unwrap();
        // Development mode (ENV not set)
        std::env::remove_var("ENV");

        let err = Error::Internal("database connection failed".to_string());
        let gql_err = err.to_graphql_error();
        assert_eq!(gql_err.message, "Internal error: database connection failed");
        assert_eq!(gql_err.extensions.get("code").unwrap(), "INTERNAL_ERROR");
        
        std::env::remove_var("ENV");
    }

    #[test]
    fn test_graphql_error_conversion_production_sanitized() {
        let _guard = ENV_LOCK.lock().unwrap();
        // Production mode
        std::env::set_var("ENV", "production");

        // Sanitized errors
        let err = Error::Internal("sensitive data here".to_string());
        let gql_err = err.to_graphql_error();
        assert_eq!(gql_err.message, "Internal server error");
        assert!(!gql_err.message.contains("sensitive"));

        let err = Error::Grpc(tonic::Status::internal("internal grpc error"));
        let gql_err = err.to_graphql_error();
        assert_eq!(gql_err.message, "Backend service error");

        std::env::remove_var("ENV");
    }

    #[test]
    fn test_graphql_error_conversion_production_safe() {
        let _guard = ENV_LOCK.lock().unwrap();
        // Production mode
        std::env::set_var("ENV", "prod");

        // Safe errors (not sanitized)
        let err = Error::Schema("field 'name' not found".to_string());
        let gql_err = err.to_graphql_error();
        assert_eq!(gql_err.message, "Schema error: field 'name' not found");

        let err = Error::InvalidRequest("query is required".to_string());
        let gql_err = err.to_graphql_error();
        assert_eq!(gql_err.message, "Invalid request: query is required");

        let err = Error::Unauthorized("Invalid API key".to_string());
        let gql_err = err.to_graphql_error();
        assert_eq!(gql_err.message, "Invalid API key");

        let err = Error::TooManyRequests("Rate limit: 100 req/min".to_string());
        let gql_err = err.to_graphql_error();
        assert_eq!(gql_err.message, "Rate limit: 100 req/min");

        std::env::remove_var("ENV");
    }

    #[test]
    fn test_error_extensions() {
        let test_cases = vec![
            (Error::Grpc(tonic::Status::internal("err")), "GRPC_ERROR"),
            (Error::Schema("err".to_string()), "SCHEMA_ERROR"),
            (Error::InvalidRequest("err".to_string()), "INVALID_REQUEST"),
            (Error::Unauthorized("err".to_string()), "UNAUTHORIZED"),
            (Error::Middleware("err".to_string()), "MIDDLEWARE_ERROR"),
            (Error::Connection("err".to_string()), "CONNECTION_ERROR"),
            (Error::WebSocket("err".to_string()), "WEBSOCKET_ERROR"),
            (Error::Internal("err".to_string()), "INTERNAL_ERROR"),
            (Error::TooManyRequests("err".to_string()), "TOO_MANY_REQUESTS"),
            (Error::QueryTooDeep("err".to_string()), "QUERY_TOO_DEEP"),
            (Error::QueryTooComplex("err".to_string()), "QUERY_TOO_COMPLEX"),
        ];

        for (err, expected_code) in test_cases {
            let gql_err = err.to_graphql_error();
            assert_eq!(gql_err.extensions.get("code").unwrap(), expected_code);
        }
    }

    #[test]
    fn test_graphql_error_from_error() {
        let err = Error::Schema("test error".to_string());
        let gql_err: GraphQLError = err.into();
        assert!(gql_err.message.contains("test error"));
    }

    #[test]
    fn test_graphql_error_serialization() {
        let gql_err = GraphQLError {
            message: "Test error".to_string(),
            extensions: {
                let mut map = std::collections::HashMap::new();
                map.insert("code".to_string(), serde_json::json!("TEST_ERROR"));
                map
            },
        };

        let json = serde_json::to_string(&gql_err).unwrap();
        assert!(json.contains("Test error"));
        assert!(json.contains("TEST_ERROR"));

        let deserialized: GraphQLError = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.message, "Test error");
        assert_eq!(deserialized.extensions.get("code").unwrap(), "TEST_ERROR");
    }

    #[test]
    fn test_graphql_error_empty_extensions_skipped() {
        let gql_err = GraphQLError {
            message: "Test".to_string(),
            extensions: std::collections::HashMap::new(),
        };

        let json = serde_json::to_string(&gql_err).unwrap();
        // Empty extensions should be skipped in serialization
        assert!(!json.contains("extensions"));
    }

    #[test]
    fn test_result_type_alias() {
        fn returns_ok() -> Result<i32> {
            Ok(42)
        }

        fn returns_err() -> Result<i32> {
            Err(Error::Internal("error".to_string()))
        }

        assert_eq!(returns_ok().unwrap(), 42);
        assert!(returns_err().is_err());
    }

    #[test]
    fn test_error_chain() {
        let io_err = io::Error::new(io::ErrorKind::PermissionDenied, "access denied");
        let err = Error::from(io_err);
        let gql_err = err.to_graphql_error();
        
        // Check that error message is preserved
        assert!(gql_err.message.contains("access denied"));
        assert_eq!(gql_err.extensions.get("code").unwrap(), "IO_ERROR");
    }

    #[test]
    fn test_all_error_variants_have_extensions() {
        let errors = vec![
            Error::Grpc(tonic::Status::internal("test")),
            Error::Schema("test".to_string()),
            Error::InvalidRequest("test".to_string()),
            Error::Unauthorized("test".to_string()),
            Error::Middleware("test".to_string()),
            Error::Connection("test".to_string()),
            Error::WebSocket("test".to_string()),
            Error::Internal("test".to_string()),
            Error::Io(io::Error::new(io::ErrorKind::Other, "test")),
            Error::TooManyRequests("test".to_string()),
            Error::QueryTooDeep("test".to_string()),
            Error::QueryTooComplex("test".to_string()),
            Error::Other(anyhow::anyhow!("test")),
        ];

        for err in errors {
            let gql_err = err.to_graphql_error();
            assert!(!gql_err.extensions.is_empty(), "Error should have extensions");
            assert!(gql_err.extensions.contains_key("code"), "Extensions should contain 'code'");
        }
    }

    #[test]
    fn test_error_debug_format() {
        let err = Error::Schema("test".to_string());
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("Schema"));
        assert!(debug_str.contains("test"));
    }
}

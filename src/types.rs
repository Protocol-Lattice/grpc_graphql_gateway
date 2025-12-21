//! Type definitions for GraphQL-gRPC gateway

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// GraphQL request from client
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GraphQLRequest {
    /// GraphQL query string
    #[serde(default)]
    pub query: String,

    /// Operation name (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_name: Option<String>,

    /// Variables for the query
    #[serde(default)]
    pub variables: HashMap<String, serde_json::Value>,
}

/// GraphQL response to client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphQLResponse {
    /// Response data
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,

    /// Errors if any
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub errors: Vec<crate::error::GraphQLError>,
}

impl GraphQLResponse {
    /// Create a successful response
    pub fn success(data: serde_json::Value) -> Self {
        Self {
            data: Some(data),
            errors: Vec::new(),
        }
    }

    /// Create an error response
    pub fn error(error: crate::error::GraphQLError) -> Self {
        Self {
            data: None,
            errors: vec![error],
        }
    }

    /// Create an error response from multiple errors
    pub fn errors(errors: Vec<crate::error::GraphQLError>) -> Self {
        Self { data: None, errors }
    }
}

/// GraphQL schema type (Query, Mutation, or Subscription)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SchemaType {
    Query,
    Mutation,
    Subscription,
}

/// Configuration for a GraphQL field from gRPC method
///
/// This struct holds the metadata extracted from the protobuf options
/// (`graphql.schema`) for a specific gRPC method.
#[derive(Debug, Clone)]
pub struct FieldConfig {
    /// Field name in GraphQL schema
    pub name: String,

    /// Schema type (Query/Mutation/Subscription)
    pub schema_type: SchemaType,

    /// Whether the field is required
    pub required: bool,

    /// gRPC service name
    pub service_name: String,

    /// gRPC method name
    pub method_name: String,

    /// Whether this is a streaming method
    pub streaming: bool,
}

/// gRPC service configuration
///
/// Represents a configured gRPC service that the gateway connects to.
#[derive(Debug, Clone)]
pub struct ServiceConfig {
    /// Service name
    pub name: String,

    /// gRPC host:port
    pub endpoint: String,

    /// Whether to use insecure connection
    pub insecure: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_graphql_request_serialization() {
        let request = GraphQLRequest {
            query: "{ hello }".to_string(),
            operation_name: Some("GetHello".to_string()),
            variables: {
                let mut vars = HashMap::new();
                vars.insert("name".to_string(), json!("World"));
                vars
            },
        };

        let json_str = serde_json::to_string(&request).unwrap();
        let deserialized: GraphQLRequest = serde_json::from_str(&json_str).unwrap();

        assert_eq!(deserialized.query, "{ hello }");
        assert_eq!(deserialized.operation_name, Some("GetHello".to_string()));
        assert_eq!(deserialized.variables.get("name").unwrap(), &json!("World"));
    }

    #[test]
    fn test_graphql_request_default_fields() {
        let json_str = r#"{"query":"{ test }"}"#;
        let request: GraphQLRequest = serde_json::from_str(json_str).unwrap();

        assert_eq!(request.query, "{ test }");
        assert_eq!(request.operation_name, None);
        assert!(request.variables.is_empty());
    }

    #[test]
    fn test_graphql_request_empty_query() {
        let json_str = r#"{}"#;
        let request: GraphQLRequest = serde_json::from_str(json_str).unwrap();

        assert_eq!(request.query, "");
        assert_eq!(request.operation_name, None);
        assert!(request.variables.is_empty());
    }

    #[test]
    fn test_graphql_request_with_variables() {
        let mut vars = HashMap::new();
        vars.insert("id".to_string(), json!(123));
        vars.insert("name".to_string(), json!("test"));
        vars.insert("enabled".to_string(), json!(true));

        let request = GraphQLRequest {
            query: "query GetUser($id: Int!) { user(id: $id) { name } }".to_string(),
            operation_name: None,
            variables: vars,
        };

        let json_str = serde_json::to_string(&request).unwrap();
        let deserialized: GraphQLRequest = serde_json::from_str(&json_str).unwrap();

        assert_eq!(deserialized.variables.get("id").unwrap(), &json!(123));
        assert_eq!(deserialized.variables.get("name").unwrap(), &json!("test"));
        assert_eq!(deserialized.variables.get("enabled").unwrap(), &json!(true));
    }

    #[test]
    fn test_graphql_request_skip_none_operation_name() {
        let request = GraphQLRequest {
            query: "{ hello }".to_string(),
            operation_name: None,
            variables: HashMap::new(),
        };

        let json_str = serde_json::to_string(&request).unwrap();
        // operation_name should not be present when None
        assert!(!json_str.contains("operation_name"));
    }

    #[test]
    fn test_graphql_response_success() {
        let data = json!({
            "user": {
                "id": 1,
                "name": "Alice"
            }
        });

        let response = GraphQLResponse::success(data.clone());

        assert_eq!(response.data, Some(data));
        assert!(response.errors.is_empty());
    }

    #[test]
    fn test_graphql_response_error() {
        let error = crate::error::GraphQLError {
            message: "Field not found".to_string(),
            extensions: std::collections::HashMap::new(),
        };

        let response = GraphQLResponse::error(error.clone());

        assert_eq!(response.data, None);
        assert_eq!(response.errors.len(), 1);
        assert_eq!(response.errors[0].message, "Field not found");
    }

    #[test]
    fn test_graphql_response_errors() {
        let errors = vec![
            crate::error::GraphQLError {
                message: "Error 1".to_string(),
                extensions: std::collections::HashMap::new(),
            },
            crate::error::GraphQLError {
                message: "Error 2".to_string(),
                extensions: std::collections::HashMap::new(),
            },
        ];

        let response = GraphQLResponse::errors(errors.clone());

        assert_eq!(response.data, None);
        assert_eq!(response.errors.len(), 2);
        assert_eq!(response.errors[0].message, "Error 1");
        assert_eq!(response.errors[1].message, "Error 2");
    }

    #[test]
    fn test_graphql_response_serialization() {
        let response = GraphQLResponse::success(json!({"result": "ok"}));

        let json_str = serde_json::to_string(&response).unwrap();
        let deserialized: GraphQLResponse = serde_json::from_str(&json_str).unwrap();

        assert_eq!(deserialized.data, Some(json!({"result": "ok"})));
        assert!(deserialized.errors.is_empty());
    }

    #[test]
    fn test_graphql_response_skip_empty_errors() {
        let response = GraphQLResponse::success(json!({"test": 123}));

        let json_str = serde_json::to_string(&response).unwrap();
        // Empty errors should be skipped
        assert!(!json_str.contains("errors"));
    }

    #[test]
    fn test_graphql_response_skip_none_data() {
        let error = crate::error::GraphQLError {
            message: "Test error".to_string(),
            extensions: std::collections::HashMap::new(),
        };
        let response = GraphQLResponse::error(error);

        let json_str = serde_json::to_string(&response).unwrap();
        // None data should be skipped
        assert!(!json_str.contains("\"data\""));
    }

    #[test]
    fn test_schema_type_equality() {
        assert_eq!(SchemaType::Query, SchemaType::Query);
        assert_eq!(SchemaType::Mutation, SchemaType::Mutation);
        assert_eq!(SchemaType::Subscription, SchemaType::Subscription);

        assert_ne!(SchemaType::Query, SchemaType::Mutation);
        assert_ne!(SchemaType::Query, SchemaType::Subscription);
        assert_ne!(SchemaType::Mutation, SchemaType::Subscription);
    }

    #[test]
    fn test_schema_type_hash() {
        use std::collections::HashSet;

        let mut set = HashSet::new();
        set.insert(SchemaType::Query);
        set.insert(SchemaType::Mutation);
        set.insert(SchemaType::Subscription);
        set.insert(SchemaType::Query); // Duplicate

        assert_eq!(set.len(), 3);
        assert!(set.contains(&SchemaType::Query));
        assert!(set.contains(&SchemaType::Mutation));
        assert!(set.contains(&SchemaType::Subscription));
    }

    #[test]
    fn test_schema_type_debug() {
        let query = SchemaType::Query;
        let mutation = SchemaType::Mutation;
        let subscription = SchemaType::Subscription;

        assert_eq!(format!("{:?}", query), "Query");
        assert_eq!(format!("{:?}", mutation), "Mutation");
        assert_eq!(format!("{:?}", subscription), "Subscription");
    }

    #[test]
    fn test_schema_type_clone() {
        let original = SchemaType::Query;
        let cloned = original;

        assert_eq!(original, cloned);
    }

    #[test]
    fn test_field_config_creation() {
        let config = FieldConfig {
            name: "getUser".to_string(),
            schema_type: SchemaType::Query,
            required: true,
            service_name: "user.UserService".to_string(),
            method_name: "GetUser".to_string(),
            streaming: false,
        };

        assert_eq!(config.name, "getUser");
        assert_eq!(config.schema_type, SchemaType::Query);
        assert!(config.required);
        assert_eq!(config.service_name, "user.UserService");
        assert_eq!(config.method_name, "GetUser");
        assert!(!config.streaming);
    }

    #[test]
    fn test_field_config_streaming() {
        let config = FieldConfig {
            name: "watchUsers".to_string(),
            schema_type: SchemaType::Subscription,
            required: false,
            service_name: "user.UserService".to_string(),
            method_name: "WatchUsers".to_string(),
            streaming: true,
        };

        assert!(config.streaming);
        assert_eq!(config.schema_type, SchemaType::Subscription);
    }

    #[test]
    fn test_field_config_clone() {
        let original = FieldConfig {
            name: "test".to_string(),
            schema_type: SchemaType::Mutation,
            required: true,
            service_name: "test.Service".to_string(),
            method_name: "Test".to_string(),
            streaming: false,
        };

        let cloned = original.clone();

        assert_eq!(cloned.name, original.name);
        assert_eq!(cloned.schema_type, original.schema_type);
        assert_eq!(cloned.required, original.required);
    }

    #[test]
    fn test_service_config_creation() {
        let config = ServiceConfig {
            name: "users".to_string(),
            endpoint: "http://localhost:50051".to_string(),
            insecure: true,
        };

        assert_eq!(config.name, "users");
        assert_eq!(config.endpoint, "http://localhost:50051");
        assert!(config.insecure);
    }

    #[test]
    fn test_service_config_secure() {
        let config = ServiceConfig {
            name: "products".to_string(),
            endpoint: "https://api.example.com:443".to_string(),
            insecure: false,
        };

        assert!(!config.insecure);
        assert!(config.endpoint.starts_with("https://"));
    }

    #[test]
    fn test_service_config_clone() {
        let original = ServiceConfig {
            name: "test".to_string(),
            endpoint: "localhost:8080".to_string(),
            insecure: true,
        };

        let cloned = original.clone();

        assert_eq!(cloned.name, original.name);
        assert_eq!(cloned.endpoint, original.endpoint);
        assert_eq!(cloned.insecure, original.insecure);
    }

    #[test]
    fn test_service_config_debug() {
        let config = ServiceConfig {
            name: "test".to_string(),
            endpoint: "localhost:8080".to_string(),
            insecure: false,
        };

        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("test"));
        assert!(debug_str.contains("localhost:8080"));
    }

    #[test]
    fn test_graphql_request_complex_variables() {
        let request = GraphQLRequest {
            query: "mutation CreateUser($input: UserInput!) { createUser(input: $input) { id } }".to_string(),
            operation_name: Some("CreateUser".to_string()),
            variables: {
                let mut vars = HashMap::new();
                vars.insert("input".to_string(), json!({
                    "name": "Alice",
                    "email": "alice@example.com",
                    "age": 30,
                    "tags": ["developer", "rust"]
                }));
                vars
            },
        };

        let json_str = serde_json::to_string(&request).unwrap();
        let deserialized: GraphQLRequest = serde_json::from_str(&json_str).unwrap();

        let input = deserialized.variables.get("input").unwrap();
        assert_eq!(input["name"], "Alice");
        assert_eq!(input["email"], "alice@example.com");
        assert_eq!(input["age"], 30);
        assert_eq!(input["tags"][0], "developer");
    }

    #[test]
    fn test_graphql_response_with_data_and_errors() {
        // GraphQL spec allows partial data with errors
        let response = GraphQLResponse {
            data: Some(json!({"user": {"id": 1, "name": null}})),
            errors: vec![crate::error::GraphQLError {
                message: "Field 'email' failed to resolve".to_string(),
                extensions: {
                    let mut map = std::collections::HashMap::new();
                    map.insert("field".to_string(), json!("email"));
                    map
                },
            }],
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let deserialized: GraphQLResponse = serde_json::from_str(&json_str).unwrap();

        assert!(deserialized.data.is_some());
        assert_eq!(deserialized.errors.len(), 1);
    }
}

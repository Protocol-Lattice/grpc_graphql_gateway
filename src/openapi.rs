#![allow(clippy::type_complexity)]
#![allow(clippy::only_used_in_recursion)]
//! OpenAPI to REST Connector Parser
//!
//! This module provides functionality to automatically generate REST connectors
//! from OpenAPI (Swagger) specification files. This enables quick integration
//! of REST APIs into the GraphQL gateway without manual endpoint configuration.
//!
//! # Supported Formats
//!
//! - OpenAPI 3.0.x (JSON and YAML)
//! - OpenAPI 3.1.x (JSON and YAML)  
//! - Swagger 2.0 (JSON and YAML) - with automatic conversion
//!
//! # Example
//!
//! ```rust,ignore
//! use grpc_graphql_gateway::{Gateway, OpenApiParser};
//!
//! // Parse OpenAPI spec and create REST connector
//! let connector = OpenApiParser::from_file("petstore.yaml")?
//!     .with_base_url("https://api.petstore.io/v2")
//!     .build()?;
//!
//! let gateway = Gateway::builder()
//!     .add_rest_connector("petstore", connector)
//!     .build()?;
//! ```
//!
//! # From URL
//!
//! ```rust,ignore
//! use grpc_graphql_gateway::OpenApiParser;
//!
//! let connector = OpenApiParser::from_url("https://api.example.com/openapi.json")
//!     .await?
//!     .build()?;
//! ```

use crate::rest_connector::{
    ApiKeyInterceptor, BearerAuthInterceptor, HttpMethod, RestConnector, RestEndpoint,
    RestFieldType, RestResponseField, RestResponseSchema,
};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

type OperationFilter = Box<dyn Fn(&str, &str) -> bool + Send + Sync>;

/// OpenAPI specification parser
///
/// Parses OpenAPI/Swagger specifications and generates REST connectors
/// with endpoints, schemas, and type definitions.
pub struct OpenApiParser {
    spec: OpenApiSpec,
    base_url_override: Option<String>,
    timeout: Duration,
    operation_filter: Option<OperationFilter>,
    tag_filter: Option<Vec<String>>,
    prefix: Option<String>,
    auth_config: HashMap<String, String>,
}

impl std::fmt::Debug for OpenApiParser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenApiParser")
            .field("spec", &self.spec.info.title)
            .field("base_url_override", &self.base_url_override)
            .field("timeout", &self.timeout)
            .field(
                "operation_filter",
                &self.operation_filter.as_ref().map(|_| "<filter>"),
            )
            .field("tag_filter", &self.tag_filter)
            .field("prefix", &self.prefix)
            .finish()
    }
}

/// Parsed OpenAPI specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiSpec {
    /// OpenAPI version
    #[serde(default)]
    pub openapi: String,
    /// Swagger version (for Swagger 2.0)
    #[serde(default)]
    pub swagger: String,
    /// API info
    pub info: OpenApiInfo,
    /// Server definitions
    #[serde(default)]
    pub servers: Vec<OpenApiServer>,
    /// Path definitions
    #[serde(default)]
    pub paths: HashMap<String, PathItem>,
    /// Component schemas
    #[serde(default)]
    pub components: Option<Components>,
    /// Global security requirements
    #[serde(default)]
    pub security: Vec<HashMap<String, Vec<String>>>,
    /// Swagger 2.0 definitions (equivalent to components.schemas)
    #[serde(default)]
    pub definitions: Option<HashMap<String, SchemaObject>>,
    /// Swagger 2.0 security definitions
    #[serde(default)]
    #[serde(rename = "securityDefinitions")]
    pub security_definitions: Option<HashMap<String, SecurityScheme>>,
    /// Swagger 2.0 host
    #[serde(default)]
    pub host: Option<String>,
    /// Swagger 2.0 basePath
    #[serde(default)]
    #[serde(rename = "basePath")]
    pub base_path: Option<String>,
    /// Swagger 2.0 schemes
    #[serde(default)]
    pub schemes: Vec<String>,
}

/// API information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiInfo {
    /// API title
    pub title: String,
    /// API version
    #[serde(default)]
    pub version: String,
    /// API description
    #[serde(default)]
    pub description: Option<String>,
}

/// Server definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiServer {
    /// Server URL
    pub url: String,
    /// Server description
    #[serde(default)]
    pub description: Option<String>,
}

/// Path item with operations
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PathItem {
    /// GET operation
    #[serde(default)]
    pub get: Option<Operation>,
    /// POST operation
    #[serde(default)]
    pub post: Option<Operation>,
    /// PUT operation
    #[serde(default)]
    pub put: Option<Operation>,
    /// PATCH operation
    #[serde(default)]
    pub patch: Option<Operation>,
    /// DELETE operation
    #[serde(default)]
    pub delete: Option<Operation>,
    /// Path-level parameters
    #[serde(default)]
    pub parameters: Vec<Parameter>,
}

/// Operation definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    /// Operation ID (used as GraphQL field name)
    #[serde(rename = "operationId")]
    pub operation_id: Option<String>,
    /// Operation summary
    #[serde(default)]
    pub summary: Option<String>,
    /// Operation description
    #[serde(default)]
    pub description: Option<String>,
    /// Operation tags
    #[serde(default)]
    pub tags: Vec<String>,
    /// Operation parameters
    #[serde(default)]
    pub parameters: Vec<Parameter>,
    /// Request body (OpenAPI 3.0+)
    #[serde(rename = "requestBody")]
    pub request_body: Option<RequestBody>,
    /// Response definitions
    #[serde(default)]
    pub responses: HashMap<String, Response>,
    /// Security requirements override
    #[serde(default)]
    pub security: Option<Vec<HashMap<String, Vec<String>>>>,
    /// Whether the operation is deprecated
    #[serde(default)]
    pub deprecated: bool,
}

/// Parameter definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parameter {
    /// Parameter name
    pub name: String,
    /// Parameter location (path, query, header, cookie)
    #[serde(rename = "in")]
    pub location: String,
    /// Whether the parameter is required
    #[serde(default)]
    pub required: bool,
    /// Parameter description
    #[serde(default)]
    pub description: Option<String>,
    /// Parameter schema
    #[serde(default)]
    pub schema: Option<SchemaObject>,
    /// Parameter type (Swagger 2.0)
    #[serde(rename = "type")]
    pub param_type: Option<String>,
}

/// Request body definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestBody {
    /// Whether the request body is required
    #[serde(default)]
    pub required: bool,
    /// Content types and their schemas
    #[serde(default)]
    pub content: HashMap<String, MediaType>,
    /// Description
    #[serde(default)]
    pub description: Option<String>,
}

/// Media type definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaType {
    /// Schema for this media type
    #[serde(default)]
    pub schema: Option<SchemaObject>,
}

/// Response definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// Response description
    #[serde(default)]
    pub description: Option<String>,
    /// Content types and their schemas (OpenAPI 3.0+)
    #[serde(default)]
    pub content: HashMap<String, MediaType>,
    /// Schema (Swagger 2.0)
    #[serde(default)]
    pub schema: Option<SchemaObject>,
}

/// Components section
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Components {
    /// Schema definitions
    #[serde(default)]
    pub schemas: HashMap<String, SchemaObject>,
    /// Security schemes
    #[serde(default)]
    #[serde(rename = "securitySchemes")]
    pub security_schemes: HashMap<String, SecurityScheme>,
}

/// Schema object definition
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SchemaObject {
    /// Schema type
    #[serde(rename = "type")]
    pub schema_type: Option<String>,
    /// Schema format (e.g., int32, int64, date-time)
    #[serde(default)]
    pub format: Option<String>,
    /// Object properties
    #[serde(default)]
    pub properties: HashMap<String, SchemaObject>,
    /// Required properties
    #[serde(default)]
    pub required: Vec<String>,
    /// Array items schema
    #[serde(default)]
    pub items: Option<Box<SchemaObject>>,
    /// Reference to another schema
    #[serde(rename = "$ref")]
    pub reference: Option<String>,
    /// Description
    #[serde(default)]
    pub description: Option<String>,
    /// Enum values
    #[serde(rename = "enum")]
    pub enum_values: Option<Vec<JsonValue>>,
    /// Nullable flag
    #[serde(default)]
    pub nullable: bool,
    /// AllOf composition
    #[serde(rename = "allOf")]
    pub all_of: Option<Vec<SchemaObject>>,
    /// OneOf composition
    #[serde(rename = "oneOf")]
    pub one_of: Option<Vec<SchemaObject>>,
    /// AnyOf composition
    #[serde(rename = "anyOf")]
    pub any_of: Option<Vec<SchemaObject>>,
}

/// Security scheme definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityScheme {
    /// Scheme type (apiKey, http, oauth2, openIdConnect)
    #[serde(rename = "type")]
    pub scheme_type: String,
    /// Scheme description
    #[serde(default)]
    pub description: Option<String>,
    /// Header/query parameter name (for apiKey)
    #[serde(default)]
    pub name: Option<String>,
    /// Location (query, header, cookie) (for apiKey)
    #[serde(default)]
    #[serde(rename = "in")]
    pub location: Option<String>,
    /// HTTP scheme (bearer, basic) (for http)
    #[serde(default)]
    pub scheme: Option<String>,
    /// Bearer format hint (for http/bearer)
    #[serde(default)]
    #[serde(rename = "bearerFormat")]
    pub bearer_format: Option<String>,
}

impl OpenApiParser {
    /// Create a parser from a file path
    ///
    /// Automatically detects JSON or YAML format based on file extension.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)
            .map_err(|e| Error::Schema(format!("Failed to read OpenAPI file: {}", e)))?;

        let is_yaml = path
            .extension()
            .map(|ext| ext == "yaml" || ext == "yml")
            .unwrap_or(false);

        Self::from_string(&content, is_yaml)
    }

    /// Create a parser from a string
    ///
    /// Set `is_yaml` to true for YAML format, false for JSON.
    pub fn from_string(content: &str, is_yaml: bool) -> Result<Self> {
        let spec: OpenApiSpec = if is_yaml {
            #[cfg(feature = "yaml")]
            {
                serde_yaml::from_str(content)
                    .map_err(|e| Error::Schema(format!("Failed to parse OpenAPI YAML: {}", e)))?
            }
            #[cfg(not(feature = "yaml"))]
            {
                let _ = content;
                return Err(Error::Schema(
                    "YAML support requires the 'yaml' feature flag".to_string(),
                ));
            }
        } else {
            serde_json::from_str(content)
                .map_err(|e| Error::Schema(format!("Failed to parse OpenAPI JSON: {}", e)))?
        };

        Ok(Self {
            spec,
            base_url_override: None,
            timeout: Duration::from_secs(30),
            operation_filter: None,
            tag_filter: None,
            prefix: None,
            auth_config: HashMap::new(),
        })
    }

    /// Create a parser from JSON value
    pub fn from_json(json: JsonValue) -> Result<Self> {
        let spec: OpenApiSpec = serde_json::from_value(json)
            .map_err(|e| Error::Schema(format!("Failed to parse OpenAPI JSON: {}", e)))?;

        Ok(Self {
            spec,
            base_url_override: None,
            timeout: Duration::from_secs(30),
            operation_filter: None,
            tag_filter: None,
            prefix: None,
            auth_config: HashMap::new(),
        })
    }

    /// Create a parser by fetching an OpenAPI spec from a URL
    pub async fn from_url(url: &str) -> Result<Self> {
        let response = reqwest::get(url)
            .await
            .map_err(|e| Error::Schema(format!("Failed to fetch OpenAPI spec: {}", e)))?;

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        let is_yaml =
            content_type.contains("yaml") || url.ends_with(".yaml") || url.ends_with(".yml");

        let content = response
            .text()
            .await
            .map_err(|e| Error::Schema(format!("Failed to read OpenAPI response: {}", e)))?;

        Self::from_string(&content, is_yaml)
    }

    /// Override the base URL (ignores servers from spec)
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url_override = Some(url.into());
        self
    }

    /// Set the default timeout for all endpoints
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Filter operations by custom predicate
    ///
    /// The predicate receives (operation_id, path) and returns true to include.
    pub fn filter_operations<F>(mut self, predicate: F) -> Self
    where
        F: Fn(&str, &str) -> bool + Send + Sync + 'static,
    {
        self.operation_filter = Some(Box::new(predicate));
        self
    }

    /// Filter operations by tags
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tag_filter = Some(tags);
        self
    }

    /// Add a prefix to all operation names
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// Set credentials for a security scheme
    ///
    /// The `scheme_name` must match the security scheme name in the OpenAPI spec.
    ///
    /// # Info
    /// - For `apiKey` (header): The value is the key itself
    /// - For `http` (bearer): The value is the token (without "Bearer " prefix)
    /// - For `http` (basic): The value is the encoded string or unused (not yet fully supported)
    pub fn with_auth(mut self, scheme_name: impl Into<String>, value: impl Into<String>) -> Self {
        self.auth_config.insert(scheme_name.into(), value.into());
        self
    }

    /// Get the parsed OpenAPI spec
    pub fn spec(&self) -> &OpenApiSpec {
        &self.spec
    }

    /// Get info about the API
    pub fn info(&self) -> &OpenApiInfo {
        &self.spec.info
    }

    /// Build a REST connector from the OpenAPI spec
    pub fn build(self) -> Result<RestConnector> {
        let base_url = self.resolve_base_url()?;
        let mut builder = RestConnector::builder()
            .base_url(&base_url)
            .timeout(self.timeout);

        // Configure authentication
        let security_schemes = self.get_security_schemes();
        for (name, scheme) in &security_schemes {
            if let Some(value) = self.auth_config.get(name) {
                match scheme.scheme_type.as_str() {
                    "apiKey" => {
                        if let (Some(ref param_name), Some(ref location)) =
                            (&scheme.name, &scheme.location)
                        {
                            if location == "header" {
                                builder = builder.interceptor(Arc::new(ApiKeyInterceptor::new(
                                    param_name.clone(),
                                    value.clone(),
                                )));
                            }
                            // TODO: Support query param auth if needed
                        }
                    }
                    "http" => {
                        if let Some(ref auth_scheme) = scheme.scheme {
                            if auth_scheme.eq_ignore_ascii_case("bearer") {
                                builder = builder.interceptor(Arc::new(
                                    BearerAuthInterceptor::new(value.clone()),
                                ));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // Get schemas for reference resolution
        let schemas = self.get_schemas();

        // Process all paths and operations
        for (path, path_item) in &self.spec.paths {
            // Collect path-level parameters
            let path_params = &path_item.parameters;

            // Process each HTTP method
            if let Some(ref op) = path_item.get {
                if let Some(endpoint) =
                    self.create_endpoint(path, HttpMethod::GET, op, path_params, &schemas)?
                {
                    builder = builder.add_endpoint(endpoint);
                }
            }
            if let Some(ref op) = path_item.post {
                if let Some(endpoint) =
                    self.create_endpoint(path, HttpMethod::POST, op, path_params, &schemas)?
                {
                    builder = builder.add_endpoint(endpoint);
                }
            }
            if let Some(ref op) = path_item.put {
                if let Some(endpoint) =
                    self.create_endpoint(path, HttpMethod::PUT, op, path_params, &schemas)?
                {
                    builder = builder.add_endpoint(endpoint);
                }
            }
            if let Some(ref op) = path_item.patch {
                if let Some(endpoint) =
                    self.create_endpoint(path, HttpMethod::PATCH, op, path_params, &schemas)?
                {
                    builder = builder.add_endpoint(endpoint);
                }
            }
            if let Some(ref op) = path_item.delete {
                if let Some(endpoint) =
                    self.create_endpoint(path, HttpMethod::DELETE, op, path_params, &schemas)?
                {
                    builder = builder.add_endpoint(endpoint);
                }
            }
        }

        builder.build()
    }

    /// Resolve the base URL from spec or override
    fn resolve_base_url(&self) -> Result<String> {
        if let Some(ref url) = self.base_url_override {
            return Ok(url.clone());
        }

        // OpenAPI 3.0+ uses servers
        if let Some(server) = self.spec.servers.first() {
            return Ok(server.url.clone());
        }

        // Swagger 2.0 uses host + basePath + schemes
        if let Some(ref host) = self.spec.host {
            let scheme = self
                .spec
                .schemes
                .first()
                .map(|s| s.as_str())
                .unwrap_or("https");
            let base_path = self.spec.base_path.as_deref().unwrap_or("");
            return Ok(format!("{}://{}{}", scheme, host, base_path));
        }

        Err(Error::Schema(
            "No base URL found in OpenAPI spec. Use with_base_url() to specify one.".to_string(),
        ))
    }

    /// Get all schemas (from components or definitions)
    fn get_schemas(&self) -> HashMap<String, SchemaObject> {
        let mut schemas = HashMap::new();

        // OpenAPI 3.0+ components.schemas
        if let Some(ref components) = self.spec.components {
            schemas.extend(components.schemas.clone());
        }

        // Swagger 2.0 definitions
        if let Some(ref definitions) = self.spec.definitions {
            schemas.extend(definitions.clone());
        }

        schemas
    }

    /// Get all security schemes (from components or definitions)
    fn get_security_schemes(&self) -> HashMap<String, SecurityScheme> {
        let mut schemes = HashMap::new();

        // OpenAPI 3.0+ components.securitySchemes
        if let Some(ref components) = self.spec.components {
            schemes.extend(components.security_schemes.clone());
        }

        // Swagger 2.0 securityDefinitions
        if let Some(ref definitions) = self.spec.security_definitions {
            schemes.extend(definitions.clone());
        }

        schemes
    }

    /// Create an endpoint from an operation
    fn create_endpoint(
        &self,
        path: &str,
        method: HttpMethod,
        operation: &Operation,
        path_params: &[Parameter],
        schemas: &HashMap<String, SchemaObject>,
    ) -> Result<Option<RestEndpoint>> {
        // Generate operation ID if not present
        let operation_id = operation
            .operation_id
            .clone()
            .unwrap_or_else(|| self.generate_operation_id(path, method));

        // Apply prefix if set
        let operation_id = if let Some(ref prefix) = self.prefix {
            format!("{}{}", prefix, operation_id)
        } else {
            operation_id
        };

        // Apply operation filter
        if let Some(ref filter) = self.operation_filter {
            if !filter(&operation_id, path) {
                return Ok(None);
            }
        }

        // Apply tag filter
        if let Some(ref allowed_tags) = self.tag_filter {
            let has_matching_tag = operation.tags.iter().any(|t| allowed_tags.contains(t));
            if !has_matching_tag && !operation.tags.is_empty() {
                return Ok(None);
            }
        }

        // Skip deprecated operations
        if operation.deprecated {
            return Ok(None);
        }

        // Build the endpoint
        let mut endpoint = RestEndpoint::new(&operation_id, path).method(method);

        // Add description
        let description = operation
            .summary
            .as_ref()
            .or(operation.description.as_ref())
            .cloned();
        if let Some(desc) = description {
            endpoint = endpoint.description(desc);
        }

        // Process parameters (path + operation level)
        let all_params: Vec<_> = path_params
            .iter()
            .chain(operation.parameters.iter())
            .collect();

        for param in &all_params {
            if param.location == "query" {
                // Add query parameters with placeholder
                endpoint = endpoint.query_param(&param.name, format!("{{{}}}", param.name));
            }
        }

        // Process request body for POST/PUT/PATCH
        if let Some(ref request_body) = operation.request_body {
            if let Some(content) = request_body.content.get("application/json") {
                if let Some(ref schema) = content.schema {
                    // Generate body template from schema
                    if let Some(template) = self.schema_to_body_template(schema, schemas) {
                        endpoint = endpoint.body_template(template);
                    }
                }
            }
        }

        // Process response schema
        if let Some(response) = operation
            .responses
            .get("200")
            .or(operation.responses.get("201"))
        {
            // OpenAPI 3.0+ style
            if let Some(content) = response.content.get("application/json") {
                if let Some(ref schema) = content.schema {
                    if let Some(response_schema) =
                        self.schema_to_response_schema(schema, schemas, &operation_id)
                    {
                        endpoint = endpoint.with_response_schema(response_schema);
                    }
                }
            }
            // Swagger 2.0 style
            else if let Some(ref schema) = response.schema {
                if let Some(response_schema) =
                    self.schema_to_response_schema(schema, schemas, &operation_id)
                {
                    endpoint = endpoint.with_response_schema(response_schema);
                }
            }
        }

        Ok(Some(endpoint))
    }

    /// Generate an operation ID from path and method
    fn generate_operation_id(&self, path: &str, method: HttpMethod) -> String {
        let method_prefix = match method {
            HttpMethod::GET => "get",
            HttpMethod::POST => "create",
            HttpMethod::PUT => "update",
            HttpMethod::PATCH => "patch",
            HttpMethod::DELETE => "delete",
        };

        // Convert /users/{id}/posts to UsersIdPosts
        let path_parts: Vec<_> = path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| {
                if s.starts_with('{') && s.ends_with('}') {
                    // Parameter placeholder
                    let name = &s[1..s.len() - 1];
                    capitalize(name)
                } else {
                    capitalize(s)
                }
            })
            .collect();

        format!("{}{}", method_prefix, path_parts.join(""))
    }

    /// Convert schema to body template
    fn schema_to_body_template(
        &self,
        schema: &SchemaObject,
        schemas: &HashMap<String, SchemaObject>,
    ) -> Option<String> {
        let resolved = self.resolve_schema(schema, schemas)?;

        if resolved.schema_type.as_deref() != Some("object") {
            return None;
        }

        let mut fields = Vec::new();
        for name in resolved.properties.keys() {
            fields.push(format!("\"{}\": \"{{{}}}\"", name, name));
        }

        if fields.is_empty() {
            None
        } else {
            Some(format!("{{{}}}", fields.join(", ")))
        }
    }

    /// Convert schema to REST response schema
    fn schema_to_response_schema(
        &self,
        schema: &SchemaObject,
        schemas: &HashMap<String, SchemaObject>,
        operation_id: &str,
    ) -> Option<RestResponseSchema> {
        let resolved = self.resolve_schema(schema, schemas)?;

        // Handle arrays
        if resolved.schema_type.as_deref() == Some("array") {
            if let Some(ref items) = resolved.items {
                return self.schema_to_response_schema(items, schemas, operation_id);
            }
        }

        // Only process objects
        if resolved.schema_type.as_deref() != Some("object") && resolved.properties.is_empty() {
            return None;
        }

        // Get type name from reference or generate one
        let type_name = schema
            .reference
            .as_ref()
            .and_then(|r| r.split('/').next_back())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("{}Response", capitalize(operation_id)));

        let mut response_schema = RestResponseSchema::new(&type_name);

        if let Some(desc) = &resolved.description {
            response_schema = response_schema.description(desc.clone());
        }

        for (name, prop) in &resolved.properties {
            let field_type = self.schema_to_field_type(prop, schemas);
            let is_required = resolved.required.contains(name);

            let mut field = RestResponseField {
                name: name.clone(),
                field_type,
                nullable: !is_required || prop.nullable,
                description: prop.description.clone(),
            };

            if !is_required {
                field = field.nullable();
            }

            response_schema = response_schema.field(field);
        }

        Some(response_schema)
    }

    /// Convert schema to REST field type
    #[allow(clippy::only_used_in_recursion)]
    fn schema_to_field_type(
        &self,
        schema: &SchemaObject,
        schemas: &HashMap<String, SchemaObject>,
    ) -> RestFieldType {
        // Handle references
        if let Some(ref reference) = schema.reference {
            if let Some(type_name) = reference.split('/').next_back() {
                return RestFieldType::Object(type_name.to_string());
            }
        }

        match schema.schema_type.as_deref() {
            Some("string") => RestFieldType::String,
            Some("integer") | Some("number") => match schema.format.as_deref() {
                Some("float") | Some("double") => RestFieldType::Float,
                _ => RestFieldType::Int,
            },
            Some("boolean") => RestFieldType::Boolean,
            Some("array") => {
                if let Some(ref items) = schema.items {
                    let item_type = self.schema_to_field_type(items, schemas);
                    RestFieldType::List(Box::new(item_type))
                } else {
                    RestFieldType::List(Box::new(RestFieldType::String))
                }
            }
            Some("object") => {
                // Anonymous object, use JSON
                RestFieldType::String
            }
            _ => RestFieldType::String,
        }
    }

    /// Resolve a schema reference
    fn resolve_schema<'a>(
        &self,
        schema: &'a SchemaObject,
        schemas: &'a HashMap<String, SchemaObject>,
    ) -> Option<&'a SchemaObject> {
        if let Some(ref reference) = schema.reference {
            // Extract type name from reference like "#/components/schemas/User"
            let type_name = reference.split('/').next_back()?;
            schemas.get(type_name)
        } else {
            Some(schema)
        }
    }

    /// List all operations in the spec
    pub fn list_operations(&self) -> Vec<OperationInfo> {
        let mut operations = Vec::new();

        for (path, path_item) in &self.spec.paths {
            let mut add_op = |method: HttpMethod, op: &Option<Operation>| {
                if let Some(ref operation) = op {
                    let operation_id = operation
                        .operation_id
                        .clone()
                        .unwrap_or_else(|| self.generate_operation_id(path, method));

                    operations.push(OperationInfo {
                        operation_id,
                        path: path.clone(),
                        method,
                        summary: operation.summary.clone(),
                        tags: operation.tags.clone(),
                        deprecated: operation.deprecated,
                    });
                }
            };

            add_op(HttpMethod::GET, &path_item.get);
            add_op(HttpMethod::POST, &path_item.post);
            add_op(HttpMethod::PUT, &path_item.put);
            add_op(HttpMethod::PATCH, &path_item.patch);
            add_op(HttpMethod::DELETE, &path_item.delete);
        }

        operations
    }
}

/// Information about an operation
#[derive(Debug, Clone)]
pub struct OperationInfo {
    /// Operation ID
    pub operation_id: String,
    /// Path
    pub path: String,
    /// HTTP method
    pub method: HttpMethod,
    /// Summary
    pub summary: Option<String>,
    /// Tags
    pub tags: Vec<String>,
    /// Whether the operation is deprecated
    pub deprecated: bool,
}

/// Capitalize the first letter of a string
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rest_connector::HttpMethod;

    const PETSTORE_JSON: &str = r##"{
        "openapi": "3.0.0",
        "info": {
            "title": "Pet Store API",
            "version": "1.0.0"
        },
        "servers": [
            {"url": "https://petstore.example.com/v1"}
        ],
        "paths": {
            "/pets": {
                "get": {
                    "operationId": "listPets",
                    "summary": "List all pets",
                    "tags": ["pets"],
                    "responses": {
                        "200": {
                            "description": "A list of pets",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "array",
                                        "items": {"$ref": "#/components/schemas/Pet"}
                                    }
                                }
                            }
                        }
                    }
                },
                "post": {
                    "operationId": "createPet",
                    "summary": "Create a pet",
                    "tags": ["pets", "admin"],
                    "requestBody": {
                        "content": {
                            "application/json": {
                                "schema": {"$ref": "#/components/schemas/Pet"}
                            }
                        }
                    },
                    "responses": {
                        "201": {
                            "description": "Pet created"
                        }
                    }
                }
            },
            "/pets/{petId}": {
                "get": {
                    "operationId": "getPet",
                    "summary": "Get a pet by ID",
                    "tags": ["pets"],
                    "parameters": [
                        {
                            "name": "petId",
                            "in": "path",
                            "required": true,
                            "schema": {"type": "string"}
                        }
                    ],
                    "responses": {
                        "200": {
                            "description": "A pet",
                            "content": {
                                "application/json": {
                                    "schema": {"$ref": "#/components/schemas/Pet"}
                                }
                            }
                        }
                    }
                }
            }
        },
        "components": {
            "schemas": {
                "Pet": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "integer"},
                        "name": {"type": "string"},
                        "tag": {"type": "string"}
                    },
                    "required": ["id", "name"]
                }
            }
        }
    }"##;

    #[test]
    fn test_parse_openapi_json() {
        let parser = OpenApiParser::from_string(PETSTORE_JSON, false).unwrap();
        assert_eq!(parser.spec.info.title, "Pet Store API");
        assert_eq!(parser.spec.servers.len(), 1);
    }

    #[test]
    fn test_list_operations() {
        let parser = OpenApiParser::from_string(PETSTORE_JSON, false).unwrap();
        let operations = parser.list_operations();

        assert_eq!(operations.len(), 3);

        let op_ids: Vec<_> = operations.iter().map(|o| &o.operation_id).collect();
        assert!(op_ids.contains(&&"listPets".to_string()));
        assert!(op_ids.contains(&&"createPet".to_string()));
        assert!(op_ids.contains(&&"getPet".to_string()));
    }

    #[test]
    fn test_build_connector() {
        let connector = OpenApiParser::from_string(PETSTORE_JSON, false)
            .unwrap()
            .build()
            .unwrap();

        assert_eq!(connector.base_url(), "https://petstore.example.com/v1");
        assert!(connector.get_endpoint("listPets").is_some());
        assert!(connector.get_endpoint("createPet").is_some());
        assert!(connector.get_endpoint("getPet").is_some());
        
        let endpoint = connector.get_endpoint("getPet").unwrap();
        let schema = endpoint.response_schema.as_ref().unwrap();
        assert_eq!(schema.type_name, "Pet");
        assert!(schema.fields.iter().any(|f| f.name == "id"));
    }
    
    #[test]
    fn test_array_response_handling() {
        let connector = OpenApiParser::from_string(PETSTORE_JSON, false)
            .unwrap()
            .build()
            .unwrap();
            
        let endpoint = connector.get_endpoint("listPets").unwrap();
        let schema = endpoint.response_schema.as_ref().unwrap();
        assert_eq!(schema.type_name, "Pet");
    }

    #[test]
    fn test_base_url_override() {
        let connector = OpenApiParser::from_string(PETSTORE_JSON, false)
            .unwrap()
            .with_base_url("https://api.custom.com")
            .build()
            .unwrap();

        assert_eq!(connector.base_url(), "https://api.custom.com");
    }

    #[test]
    fn test_generate_operation_id() {
        let parser = OpenApiParser::from_string(PETSTORE_JSON, false).unwrap();

        let id = parser.generate_operation_id("/users/{id}/posts", HttpMethod::GET);
        assert_eq!(id, "getUsersIdPosts");

        let id = parser.generate_operation_id("/products", HttpMethod::POST);
        assert_eq!(id, "createProducts");
    }

    #[test]
    fn test_operation_prefix() {
        let connector = OpenApiParser::from_string(PETSTORE_JSON, false)
            .unwrap()
            .with_prefix("petstore_")
            .build()
            .unwrap();

        assert!(connector.get_endpoint("petstore_listPets").is_some());
    }
    
    #[test]
    fn test_tag_filter() {
        let connector = OpenApiParser::from_string(PETSTORE_JSON, false)
            .unwrap()
            .with_tags(vec!["admin".to_string()])
            .build()
            .unwrap();

        assert!(connector.get_endpoint("createPet").is_some());
        assert!(connector.get_endpoint("getPet").is_none());
    }
    
    #[test]
    fn test_operation_filter() {
        let connector = OpenApiParser::from_string(PETSTORE_JSON, false)
            .unwrap()
            .filter_operations(|op_id, _| op_id.starts_with("get"))
            .build()
            .unwrap();

        assert!(connector.get_endpoint("getPet").is_some());
        assert!(connector.get_endpoint("createPet").is_none());
    }
    
    #[test]
    fn test_schema_resolution() {
        let parser = OpenApiParser::from_string(PETSTORE_JSON, false).unwrap();
        let schemas = parser.get_schemas();
        
        assert!(schemas.contains_key("Pet"));
        let pet_schema = schemas.get("Pet").unwrap();
        assert_eq!(pet_schema.schema_type.as_deref(), Some("object"));
    }

    #[test]
    fn test_security_scheme_parsing_and_config() {
        let json = r##"{
            "openapi": "3.0.0",
            "info": {
                "title": "Secure API",
                "version": "1.0.0"
            },
            "servers": [
                {"url": "https://api.secure.com"}
            ],
            "paths": {
                "/secure": {
                    "get": {
                        "operationId": "getSecure",
                        "responses": {
                            "200": {"description": "OK"}
                        }
                    }
                }
            },
            "components": {
                "securitySchemes": {
                    "apiKeyAuth": {
                        "type": "apiKey",
                        "in": "header",
                        "name": "X-API-Key"
                    },
                    "bearerAuth": {
                        "type": "http",
                        "scheme": "bearer"
                    }
                }
            }
        }"##;

        let parser = OpenApiParser::from_string(json, false)
            .unwrap()
            .with_auth("apiKeyAuth", "secret-key")
            .with_auth("bearerAuth", "some-token");

        let connector = parser.build().unwrap();

        assert_eq!(connector.base_url(), "https://api.secure.com");
    }
    

}

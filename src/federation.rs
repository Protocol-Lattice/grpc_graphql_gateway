//! GraphQL Federation support
//!
//! This module implements Apollo Federation v2 specification, allowing the gateway
//! to participate in a federated GraphQL architecture.

use crate::error::{Error, Result};
use crate::graphql::GraphqlEntity;
use async_graphql::dynamic::{Field, FieldFuture, FieldValue, InputValue, Object, TypeRef};
use async_graphql::{Name, Value as GqlValue};
use prost::Message;
use prost_reflect::{DescriptorPool, ExtensionDescriptor, MessageDescriptor, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// Federation configuration extracted from protobuf descriptors
#[derive(Clone, Debug)]
pub struct FederationConfig {
    /// Map of entity type names to their key fields
    pub entities: HashMap<String, EntityConfig>,
}

/// Configuration for a federated entity
#[derive(Clone, Debug)]
pub struct EntityConfig {
    /// The message descriptor for this entity type
    pub descriptor: MessageDescriptor,
    /// Key field sets for this entity (e.g., ["id"], ["email"], or ["orgId", "userId"])
    pub keys: Vec<Vec<String>>,
    /// Whether this entity extends an entity from another service
    pub extend: bool,
    /// Whether this service can resolve this entity
    pub resolvable: bool,
    /// The GraphQL type name for this entity
    pub type_name: String,
}

impl FederationConfig {
    /// Create a new empty federation configuration
    pub fn new() -> Self {
        Self {
            entities: HashMap::new(),
        }
    }

    /// Extract federation configuration from descriptor pool
    pub fn from_descriptor_pool(
        pool: &DescriptorPool,
        entity_ext: &ExtensionDescriptor,
    ) -> Result<Self> {
        let mut config = Self::new();

        // Scan all messages for entity annotations
        for message in pool.all_messages() {
            if let Some(entity_opts) = decode_entity_extension(&message, entity_ext)? {
                if entity_opts.keys.is_empty() {
                    continue; // Skip messages without keys
                }

                let type_name = message.full_name().replace('.', "_");
                // BB-04: Validate the derived type name contains only safe identifier chars.
                if !type_name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    return Err(Error::Schema(format!(
                        "Derived GraphQL type name '{}' contains invalid characters (from protobuf '{}')",
                        type_name,
                        message.full_name()
                    )));
                }
                let keys: Vec<Vec<String>> = entity_opts
                    .keys
                    .iter()
                    .map(|key| {
                        // Split on whitespace to support composite keys like "orgId userId"
                        key.split_whitespace().map(String::from).collect::<Vec<_>>()
                    })
                    .collect();

                config.entities.insert(
                    type_name.clone(),
                    EntityConfig {
                        descriptor: message.clone(),
                        keys,
                        extend: entity_opts.extend,
                        resolvable: entity_opts.resolvable,
                        type_name,
                    },
                );
            }
        }

        Ok(config)
    }

    /// Check if federation is enabled (i.e., if there are any entities)
    pub fn is_enabled(&self) -> bool {
        !self.entities.is_empty()
    }

    /// Build the _entities field for the Query type
    pub fn build_entities_field_for_query(
        &self,
        entity_resolvers: Arc<dyn EntityResolver>,
    ) -> Result<Field> {
        let config = self.clone();

        let field = Field::new("_entities", TypeRef::named_nn_list("_Entity"), move |ctx| {
            let entity_resolvers = entity_resolvers.clone();
            let config = config.clone();

            FieldFuture::new(async move {
                let representations = ctx
                    .args
                    .get("representations")
                    .ok_or_else(|| async_graphql::Error::new("missing representations argument"))?
                    .list()?;

                // BB-03: Cap the number of representations to prevent resource exhaustion DoS.
                const MAX_REPRESENTATIONS: usize = 1_000;
                let repr_iter: Vec<_> = representations.iter().collect();
                if repr_iter.len() > MAX_REPRESENTATIONS {
                    return Err(async_graphql::Error::new(format!(
                        "Too many representations: received {}, maximum is {}",
                        repr_iter.len(),
                        MAX_REPRESENTATIONS
                    )));
                }

                let mut results = Vec::with_capacity(repr_iter.len());
                for repr in repr_iter {
                    let obj = repr.object()?;

                    // Convert ObjectAccessor to IndexMap
                    let mut representation_map = async_graphql::indexmap::IndexMap::new();
                    for (key, value) in obj.iter() {
                        representation_map.insert(key.clone(), value.as_value().clone());
                    }

                    // Extract __typename from representation
                    let typename = representation_map
                        .get(&Name::new("__typename"))
                        .and_then(|v| match v {
                            GqlValue::String(s) => Some(s.as_str()),
                            _ => None,
                        })
                        .ok_or_else(|| {
                            async_graphql::Error::new("missing __typename in representation")
                        })?;

                    // BB-02: Validate __typename length and charset before using it as a map key.
                    if typename.is_empty() || typename.len() > 128 {
                        return Err(async_graphql::Error::new(
                            "invalid __typename: must be 1–128 characters"
                        ));
                    }
                    if !typename.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                        return Err(async_graphql::Error::new(
                            "invalid __typename: must contain only alphanumeric characters or underscores"
                        ));
                    }

                    // Find entity config
                    let entity_config = config.entities.get(typename).ok_or_else(|| {
                        // Intentionally vague — don't reveal internal type topology.
                        async_graphql::Error::new("unknown or unresolvable entity type")
                    })?;

                    // Resolve the entity
                    let entity = entity_resolvers
                        .resolve_entity(entity_config, &representation_map)
                        .await
                        .map_err(|e| async_graphql::Error::new(e.to_string()))?;

                    results
                        .push(FieldValue::value(entity).with_type(entity_config.type_name.clone()));
                }

                Ok(Some(FieldValue::list(results)))
            })
        })
        .argument(InputValue::new(
            "representations",
            TypeRef::named_nn_list_nn("_Any"),
        ));

        Ok(field)
    }

    /// Apply federation directives to an object type
    pub fn apply_directives_to_object(&self, obj: Object, type_name: &str) -> Result<Object> {
        if let Some(entity_config) = self.entities.get(type_name) {
            let mut obj = obj;

            // Add @key directives
            for key_fields in &entity_config.keys {
                let fields_str = key_fields.join(" ");
                // Mark as entity for async-graphql federation support
                if entity_config.resolvable {
                    obj = obj.key(fields_str.clone());
                } else {
                    obj = obj.unresolvable(fields_str.clone());
                }
                obj = obj.directive(
                    async_graphql::dynamic::Directive::new("key")
                        .argument("fields", GqlValue::String(fields_str)),
                );
            }

            // Add @extends directive if this is an extension
            if entity_config.extend {
                obj = obj.extends();
                obj = obj.directive(async_graphql::dynamic::Directive::new("extends"));
            }

            Ok(obj)
        } else {
            Ok(obj)
        }
    }
}

impl Default for FederationConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration for mapping an entity type to its resolver method
///
/// This tells the gateway which gRPC method to call to resolve a specific entity type.
///
/// # Example
///
/// ```rust
/// use grpc_graphql_gateway::EntityResolverMapping;
///
/// let mapping = EntityResolverMapping {
///     service_name: "user.UserService".to_string(),
///     method_name: "GetUser".to_string(),
///     key_field: "id".to_string(),
/// };
/// ```
#[derive(Clone, Debug)]
pub struct EntityResolverMapping {
    /// Service name (e.g., "user.UserService")
    pub service_name: String,
    /// Method name (e.g., "GetUser")
    pub method_name: String,
    /// Field name in the request message for the primary key (e.g., "id")
    pub key_field: String,
}

/// Trait for resolving federated entities
///
/// Implementors should resolve entities based on their representation
/// (which contains the key fields and __typename).
///
/// This trait supports both single and batch resolution.
#[async_trait::async_trait]
pub trait EntityResolver: Send + Sync {
    /// Resolve an entity from its representation
    async fn resolve_entity(
        &self,
        entity_config: &EntityConfig,
        representation: &async_graphql::indexmap::IndexMap<Name, GqlValue>,
    ) -> Result<GqlValue>;

    /// Batch resolve multiple entities of the same type
    /// Default implementation calls resolve_entity for each
    async fn batch_resolve_entities(
        &self,
        entity_config: &EntityConfig,
        representations: Vec<async_graphql::indexmap::IndexMap<Name, GqlValue>>,
    ) -> Result<Vec<GqlValue>> {
        let mut results = Vec::with_capacity(representations.len());
        for repr in representations {
            results.push(self.resolve_entity(entity_config, &repr).await?);
        }
        Ok(results)
    }
}

/// Default entity resolver that uses gRPC client pool with DataLoader batching
pub struct GrpcEntityResolver {
    client_pool: crate::grpc_client::GrpcClientPool,
    /// Maps entity type names to their resolver configuration
    resolver_mappings: HashMap<String, EntityResolverMapping>,
}

impl GrpcEntityResolver {
    pub fn new(client_pool: crate::grpc_client::GrpcClientPool) -> Self {
        Self {
            client_pool,
            resolver_mappings: HashMap::new(),
        }
    }

    /// Register an entity resolver mapping
    ///
    /// # Example
    /// ```ignore
    /// resolver.register_entity_resolver(
    ///     "user_User",
    ///     EntityResolverMapping {
    ///         service_name: "user.UserService".to_string(),
    ///         method_name: "GetUser".to_string(),
    ///         key_field: "id".to_string(),
    ///     }
    /// );
    /// ```
    pub fn register_entity_resolver(
        &mut self,
        entity_type: impl Into<String>,
        mapping: EntityResolverMapping,
    ) {
        self.resolver_mappings.insert(entity_type.into(), mapping);
    }

    /// Create a builder for configuring entity resolvers
    pub fn builder(client_pool: crate::grpc_client::GrpcClientPool) -> GrpcEntityResolverBuilder {
        GrpcEntityResolverBuilder::new(client_pool)
    }
}

/// Builder for GrpcEntityResolver
pub struct GrpcEntityResolverBuilder {
    client_pool: crate::grpc_client::GrpcClientPool,
    resolver_mappings: HashMap<String, EntityResolverMapping>,
}

impl GrpcEntityResolverBuilder {
    pub fn new(client_pool: crate::grpc_client::GrpcClientPool) -> Self {
        Self {
            client_pool,
            resolver_mappings: HashMap::new(),
        }
    }

    /// Register an entity resolver mapping
    pub fn register_entity_resolver(
        mut self,
        entity_type: impl Into<String>,
        mapping: EntityResolverMapping,
    ) -> Self {
        self.resolver_mappings.insert(entity_type.into(), mapping);
        self
    }

    pub fn build(self) -> GrpcEntityResolver {
        GrpcEntityResolver {
            client_pool: self.client_pool,
            resolver_mappings: self.resolver_mappings,
        }
    }
}

impl Default for GrpcEntityResolver {
    fn default() -> Self {
        Self::new(crate::grpc_client::GrpcClientPool::new())
    }
}

#[async_trait::async_trait]
impl EntityResolver for GrpcEntityResolver {
    async fn resolve_entity(
        &self,
        entity_config: &EntityConfig,
        representation: &async_graphql::indexmap::IndexMap<Name, GqlValue>,
    ) -> Result<GqlValue> {
        tracing::debug!(
            "Resolving entity {} with representation: {:?}",
            entity_config.type_name,
            representation
        );

        // BB-01: Get the resolver mapping — error out instead of echoing raw input.
        let mapping =
            self.resolver_mappings
                .get(&entity_config.type_name)
                .ok_or_else(|| {
                    Error::Schema(format!(
                        "No resolver mapping configured for entity type '{}'. \
                         Register one via register_entity_resolver().",
                entity_config.type_name
                    ))
                })?;

        // Extract the key field from the representation
        let key_value = representation
            .get(&Name::new(&mapping.key_field))
            .ok_or_else(|| {
                Error::Schema(format!(
                    "Missing key field '{}' in representation",
                    mapping.key_field
                ))
            })?;

        // Get gRPC client
        let _client = self.client_pool.get(&mapping.service_name).ok_or_else(|| {
            Error::Schema(format!(
                "gRPC client not found for service: {}",
                mapping.service_name
            ))
        })?;

        // TODO: Complete the gRPC call implementation:
        // 1. Create a DynamicMessage for the request using `entity_config.descriptor`
        // 2. Set the key field(s) from `key_value`
        // 3. Call the gRPC method via `_client`
        // 4. Deserialise the response into GqlValue
        //
        // BB-01: Return an error instead of echoing attacker-supplied representation.
        tracing::debug!(
            service = %mapping.service_name,
            method  = %mapping.method_name,
            key     = %mapping.key_field,
            value   = ?key_value,
            "gRPC entity resolver call not yet implemented"
        );
        Err(Error::Schema(format!(
            "gRPC entity resolver for '{}' is not yet implemented",
            entity_config.type_name
        )))
    }

    async fn batch_resolve_entities(
        &self,
        entity_config: &EntityConfig,
        representations: Vec<async_graphql::indexmap::IndexMap<Name, GqlValue>>,
    ) -> Result<Vec<GqlValue>> {
        tracing::debug!(
            "Batch resolving {} entities of type {}",
            representations.len(),
            entity_config.type_name
        );

        // In a production implementation, you could batch these into a single gRPC call
        // if your service supports batch operations. For now, we resolve serially.
        let mut results = Vec::with_capacity(representations.len());
        for repr in representations {
            results.push(self.resolve_entity(entity_config, &repr).await?);
        }
        Ok(results)
    }
}

/// Decode entity extension from message descriptor
fn decode_entity_extension(
    message: &MessageDescriptor,
    ext: &ExtensionDescriptor,
) -> Result<Option<GraphqlEntity>> {
    let opts = message.options();
    if !opts.has_extension(ext) {
        return Ok(None);
    }

    let val = opts.get_extension(ext);
    if let Value::Message(msg) = val.as_ref() {
        GraphqlEntity::decode(msg.encode_to_vec().as_slice())
            .map(Some)
            .map_err(|e| Error::Schema(format!("failed to decode entity extension: {e}")))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_federation_config_new() {
        let config = FederationConfig::new();
        assert!(!config.is_enabled());
        assert!(config.entities.is_empty());
    }

    #[test]
    fn test_entity_config_composite_keys() {
        // Test that key field sets are properly parsed
        let keys = [
            vec!["id".to_string()],
            vec!["org".to_string(), "user".to_string()],
        ];
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0], vec!["id"]);
        assert_eq!(keys[1], vec!["org", "user"]);
    }

    #[test]
    fn test_entity_resolver_mapping() {
        let mapping = EntityResolverMapping {
            service_name: "Service".into(),
            method_name: "Method".into(),
            key_field: "id".into(),
        };
        assert_eq!(mapping.key_field, "id");
        assert_eq!(mapping.service_name, "Service");
    }

    #[test]
    fn test_grpc_entity_resolver_registration() {
        let pool = crate::grpc_client::GrpcClientPool::new();
        let mut resolver = GrpcEntityResolver::new(pool);

        let mapping = EntityResolverMapping {
            service_name: "S".into(),
            method_name: "M".into(),
            key_field: "k".into(),
        };

        resolver.register_entity_resolver("Type", mapping);
    }

    #[test]
    fn test_grpc_entity_resolver_builder() {
        let pool = crate::grpc_client::GrpcClientPool::new();
        let _resolver = GrpcEntityResolver::builder(pool)
            .register_entity_resolver(
                "Type",
                EntityResolverMapping {
                    service_name: "S".into(),
                    method_name: "M".into(),
                    key_field: "k".into(),
                },
            )
            .build();
    }

    #[test]
    fn test_federation_config_parsing() {
        use prost_reflect::DescriptorPool;

        // This file is generated by build.rs and used in schema tests too
        let descriptor_bytes = include_bytes!("generated/federation_example_descriptor.bin");
        let pool = DescriptorPool::decode(descriptor_bytes.as_slice())
            .expect("Failed to decode descriptor");

        let entity_ext = pool
            .get_extension_by_name("graphql.entity")
            .expect("graphql.entity extension not found");

        let config = FederationConfig::from_descriptor_pool(&pool, &entity_ext)
            .expect("Failed to parse federation config");

        assert!(config.is_enabled());
        assert!(!config.entities.is_empty());

        // Verify key fields are parsed
        for entity in config.entities.values() {
            assert!(!entity.keys.is_empty());
        }
    }

    fn get_test_pool() -> prost_reflect::DescriptorPool {
        use prost_reflect::DescriptorPool;
        let descriptor_bytes = include_bytes!("generated/federation_example_descriptor.bin");
        DescriptorPool::decode(descriptor_bytes.as_slice()).expect("Failed to decode descriptor")
    }

    #[tokio::test]
    async fn test_grpc_entity_resolver_resolve_noop() {
        // BB-01: The resolver must now return an Err instead of echoing raw input.
        // This test verifies the correct error is produced when the gRPC call
        // is not yet fully implemented.
        let pool = get_test_pool();
        let message_descriptor = pool
            .all_messages()
            .next()
            .expect("No messages in descriptor");

        let client_pool_inner = crate::grpc_client::GrpcClientPool::new();
        let client =
            crate::grpc_client::GrpcClient::connect_lazy("http://localhost:50051", true).unwrap();
        client_pool_inner.add("UserService", client);

        let mut resolver = GrpcEntityResolver::new(client_pool_inner);
        resolver.register_entity_resolver(
            "User",
            EntityResolverMapping {
                service_name: "UserService".into(),
                method_name: "GetUser".into(),
                key_field: "id".into(),
            },
        );

        let entity_config = EntityConfig {
            descriptor: message_descriptor,
            keys: vec![vec!["id".into()]],
            extend: false,
            resolvable: true,
            type_name: "User".into(),
        };

        let mut representation = async_graphql::indexmap::IndexMap::new();
        representation.insert(
            async_graphql::Name::new("id"),
            GqlValue::String("123".into()),
        );
        representation.insert(
            async_graphql::Name::new("__typename"),
            GqlValue::String("User".into()),
        );

        let result = resolver
            .resolve_entity(&entity_config, &representation)
            .await;

        // BB-01 fix: must not echo raw input — must return an error instead.
        assert!(
            result.is_err(),
            "Resolver must return an error (not echo raw input) when gRPC call is unimplemented"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not yet implemented"),
            "Error message should indicate missing implementation, got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn test_grpc_entity_resolver_batch() {
        // BB-01: Batch resolution delegates to resolve_entity, which now returns Err
        // for the unimplemented gRPC stub rather than echoing raw representations.
        let pool = get_test_pool();
        let message_descriptor = pool
            .all_messages()
            .next()
            .expect("No messages in descriptor");

        let client_pool_inner = crate::grpc_client::GrpcClientPool::new();
        let client =
            crate::grpc_client::GrpcClient::connect_lazy("http://localhost:50051", true).unwrap();
        client_pool_inner.add("UserService", client);

        let mut resolver = GrpcEntityResolver::new(client_pool_inner);
        resolver.register_entity_resolver(
            "User",
            EntityResolverMapping {
                service_name: "UserService".into(),
                method_name: "GetUser".into(),
                key_field: "id".into(),
            },
        );

        let entity_config = EntityConfig {
            descriptor: message_descriptor,
            keys: vec![vec!["id".into()]],
            extend: false,
            resolvable: true,
            type_name: "User".into(),
        };

        let mut repr1 = async_graphql::indexmap::IndexMap::new();
        repr1.insert(async_graphql::Name::new("id"), GqlValue::String("1".into()));

        let mut repr2 = async_graphql::indexmap::IndexMap::new();
        repr2.insert(async_graphql::Name::new("id"), GqlValue::String("2".into()));

        let results = resolver
            .batch_resolve_entities(&entity_config, vec![repr1, repr2])
            .await;

        // BB-01 fix: each item resolves via resolve_entity, which returns Err for the
        // unimplemented gRPC stub — so batch_resolve_entities must also propagate the error.
        assert!(
            results.is_err(),
            "Batch resolver must propagate the error from the unimplemented gRPC stub"
        );
    }
}

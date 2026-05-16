//! DataLoader implementation for batching entity resolution requests
//!
//! This module provides a DataLoader that batches multiple entity resolution
//! requests to prevent N+1 query problems in federated GraphQL.

use crate::federation::{EntityConfig, EntityResolver};
use crate::gbp::{GbpDecoder, GbpEncoder};
use crate::{Error, Result};
use async_graphql::dataloader::{DataLoader, HashMapCache, Loader};
use async_graphql::{indexmap::IndexMap, Name, Value as GqlValue};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

type Representation = IndexMap<Name, GqlValue>;

/// DataLoader for batching entity resolution requests
///
/// This prevents N+1 query problems by batching multiple entity resolution
/// requests for the same entity type into a single batch operation.
///
/// It works by collecting concurrent load requests and dispatching them
/// to the [`EntityResolver::batch_resolve_entities`] method. Entity representations
/// are normalized (including nested objects) so they can be cached and deduplicated
/// reliably across a GraphQL request.
pub struct EntityDataLoader {
    entity_configs: Arc<HashMap<String, EntityConfig>>,
    loader: Arc<DataLoader<EntityBatcher, HashMapCache>>,
}

#[derive(Clone)]
struct EntityBatcher {
    resolver: Arc<dyn EntityResolver>,
    entity_configs: Arc<HashMap<String, EntityConfig>>,
    redis: Option<Arc<redis::Client>>,
    ttl_secs: u64,
}

impl EntityDataLoader {
    /// Create a new EntityDataLoader with in-memory caching
    pub fn new(
        resolver: Arc<dyn EntityResolver>,
        entity_configs: HashMap<String, EntityConfig>,
    ) -> Self {
        let entity_configs = Arc::new(entity_configs);
        let batcher = EntityBatcher {
            resolver,
            entity_configs: entity_configs.clone(),
            redis: None,
            ttl_secs: 0,
        };
        let loader = DataLoader::with_cache(batcher, tokio::spawn, HashMapCache::default());

        Self {
            entity_configs,
            loader: Arc::new(loader),
        }
    }

    /// Create a new EntityDataLoader with Redis distributed caching
    pub fn new_with_redis(
        resolver: Arc<dyn EntityResolver>,
        entity_configs: HashMap<String, EntityConfig>,
        redis_client: redis::Client,
        ttl_secs: u64,
    ) -> Self {
        let entity_configs = Arc::new(entity_configs);
        let batcher = EntityBatcher {
            resolver,
            entity_configs: entity_configs.clone(),
            redis: Some(Arc::new(redis_client)),
            ttl_secs,
        };
        let loader = DataLoader::with_cache(batcher, tokio::spawn, HashMapCache::default());

        Self {
            entity_configs,
            loader: Arc::new(loader),
        }
    }

    /// Load an entity, batching with other concurrent loads of the same type
    pub async fn load(
        &self,
        entity_type: &str,
        representation: Representation,
    ) -> Result<GqlValue> {
        let key = RepresentationKey::new(entity_type, representation);
        self.loader
            .load_one(key)
            .await
            .map_err(Error::Schema)?
            .ok_or_else(|| Error::Schema("Entity resolver returned no value".to_string()))
    }

    /// Load multiple entities of the same type in a batch
    ///
    /// # Errors
    /// Returns an error if `representations` exceeds `MAX_BATCH_SIZE` (500).
    pub async fn load_many(
        &self,
        entity_type: &str,
        representations: Vec<Representation>,
    ) -> Result<Vec<GqlValue>> {
        // BB-05: Enforce a hard upper bound to prevent memory exhaustion.
        const MAX_BATCH_SIZE: usize = 500;
        if representations.len() > MAX_BATCH_SIZE {
            return Err(Error::Schema(format!(
                "Batch size {} exceeds the maximum of {}",
                representations.len(),
                MAX_BATCH_SIZE
            )));
        }

        let keys: Vec<_> = representations
            .into_iter()
            .map(|repr| RepresentationKey::new(entity_type, repr))
            .collect();
        let values = self
            .loader
            .load_many(keys.clone())
            .await
            .map_err(Error::Schema)?;

        let mut ordered = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(value) = values.get(&key) {
                ordered.push(value.clone());
            } else {
                // BB-07: Do not leak internal entity type names to callers.
                tracing::error!(
                    entity_type = %key.entity_type,
                    "Missing value for entity during batch resolution"
                );
                return Err(Error::Schema(
                    "Internal error: batch resolver returned an incomplete result set".to_string(),
                ));
            }
        }
        Ok(ordered)
    }
}

impl Clone for EntityDataLoader {
    fn clone(&self) -> Self {
        Self {
            entity_configs: Arc::clone(&self.entity_configs),
            loader: Arc::clone(&self.loader),
        }
    }
}

#[async_trait::async_trait]
impl Loader<RepresentationKey> for EntityBatcher {
    type Value = GqlValue;
    type Error = String;

    async fn load(
        &self,
        keys: &[RepresentationKey],
    ) -> std::result::Result<HashMap<RepresentationKey, Self::Value>, Self::Error> {
        let mut results = HashMap::with_capacity(keys.len());
        let mut remaining_keys = Vec::with_capacity(keys.len());

        // Step 1: Check Redis distributed cache first if enabled
        if let Some(redis) = &self.redis {
            let mut conn = redis
                .get_multiplexed_async_connection()
                .await
                .map_err(|e| format!("Redis connection error: {}", e))?;

            for key in keys {
                let redis_key = self.get_redis_key(key);
                let data: Option<Vec<u8>> = conn.get(&redis_key).await.ok();
                
                if let Some(bytes) = data {
                    let mut decoder = GbpDecoder::new();
                    if let Ok(json_val) = decoder.decode(&bytes) {
                        // Convert back to async_graphql::Value
                        if let Ok(gql_val) = serde_json::from_value(json_val) {
                            results.insert(key.clone(), gql_val);
                            continue;
                        }
                    }
                }
                remaining_keys.push(key);
            }
        } else {
            for key in keys {
                remaining_keys.push(key);
            }
        }

        if remaining_keys.is_empty() {
            return Ok(results);
        }

        // Step 2: Batch resolve remaining keys from gRPC subgraphs
        let mut grouped: HashMap<Arc<str>, Vec<&RepresentationKey>> = HashMap::new();
        for key in remaining_keys {
            grouped
                .entry(Arc::clone(&key.entity_type))
                .or_default()
                .push(key);
        }

        for (entity_type, group_keys) in grouped {
            let config = self
                .entity_configs
                .get(entity_type.as_ref())
                .ok_or_else(|| {
                    tracing::error!(entity_type = %entity_type, "Unknown entity type in dataloader");
                    "Internal error: unknown entity type".to_string()
                })?;

            let representations: Vec<_> = group_keys
                .iter()
                .map(|key| (*key.representation).clone())
                .collect();

            let values = if representations.len() == 1 {
                vec![self
                    .resolver
                    .resolve_entity(config, &representations[0])
                    .await
                    .map_err(|e| e.to_string())?]
            } else {
                self.resolver
                    .batch_resolve_entities(config, representations)
                    .await
                    .map_err(|e| e.to_string())?
            };

            if values.len() != group_keys.len() {
                tracing::error!(
                    entity_type = %entity_type,
                    got  = values.len(),
                    want = group_keys.len(),
                    "Batch resolver returned wrong number of values"
                );
                return Err(
                    "Internal error: batch resolver returned unexpected number of values"
                        .to_string(),
                );
            }

            // Step 3: Store newly resolved entities in Redis if enabled
            if let Some(redis) = &self.redis {
                if let Ok(mut conn) = redis.get_multiplexed_async_connection().await {
                    for (key, value) in group_keys.iter().zip(values.iter()) {
                        let redis_key = self.get_redis_key(key);
                        // Convert GqlValue -> serde_json::Value -> GBP
                        if let Ok(json_val) = serde_json::to_value(value) {
                            let mut encoder = GbpEncoder::new();
                            let bytes = encoder.encode(&json_val);
                            let _: redis::RedisResult<()> = conn.set_ex(redis_key, bytes, self.ttl_secs).await;
                        }
                    }
                }
            }

            for (key, value) in group_keys.into_iter().zip(values.into_iter()) {
                results.insert(key.clone(), value);
            }
        }

        Ok(results)
    }
}

impl EntityBatcher {
    fn get_redis_key(&self, key: &RepresentationKey) -> String {
        let mut hasher = Sha256::new();
        hasher.update(key.entity_type.as_bytes());
        hasher.update(b":");
        if let Ok(json) = serde_json::to_string(&key.normalized) {
            hasher.update(json.as_bytes());
        }
        let hash = ::hex::encode(hasher.finalize());
        format!("gql:dl:{}", hash)
    }
}

/// Cache key for DataLoader that normalizes nested representations.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct RepresentationKey {
    entity_type: Arc<str>,
    normalized: NormalizedValue,
    representation: Arc<Representation>,
}

impl RepresentationKey {
    fn new(entity_type: &str, representation: Representation) -> Self {
        let normalized = NormalizedValue::from(&GqlValue::Object(representation.clone()));
        Self {
            entity_type: Arc::from(entity_type.to_owned()),
            normalized,
            representation: Arc::new(representation),
        }
    }
}

impl PartialEq for RepresentationKey {
    fn eq(&self, other: &Self) -> bool {
        self.entity_type == other.entity_type && self.normalized == other.normalized
    }
}

impl Eq for RepresentationKey {}

impl Hash for RepresentationKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.entity_type.hash(state);
        self.normalized.hash(state);
    }
}

/// Normalized representation of a GraphQL value for hashing.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
enum NormalizedValue {
    Null,
    Boolean(bool),
    Number(String),
    String(String),
    Enum(String),
    List(Vec<NormalizedValue>),
    Object(Vec<(String, NormalizedValue)>),
    Binary([u8; 32]),
}

impl From<&GqlValue> for NormalizedValue {
    fn from(value: &GqlValue) -> Self {
        match value {
            GqlValue::Null => Self::Null,
            GqlValue::Boolean(b) => Self::Boolean(*b),
            GqlValue::Number(n) => Self::Number(n.to_string()),
            GqlValue::String(s) => Self::String(s.clone()),
            GqlValue::Enum(e) => Self::Enum(e.to_string()),
            GqlValue::List(items) => Self::List(items.iter().map(Self::from).collect()),
            GqlValue::Object(obj) => Self::Object(normalize_object(obj)),
            GqlValue::Binary(bytes) => Self::Binary(*blake3::hash(bytes).as_bytes()),
        }
    }
}

fn normalize_object(obj: &Representation) -> Vec<(String, NormalizedValue)> {
    let mut entries: Vec<(String, NormalizedValue)> = obj
        .iter()
        .map(|(key, value)| (key.to_string(), NormalizedValue::from(value)))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::federation::GrpcEntityResolver;
    use async_graphql::{Name, Number, Value as GqlValue};
    use prost_reflect::DescriptorPool;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn test_dataloader_creation() {
        let resolver = Arc::new(GrpcEntityResolver::default());
        let configs = HashMap::new();
        let loader = EntityDataLoader::new(resolver, configs);
        assert_eq!(loader.entity_configs.len(), 0);
    }

    #[tokio::test]
    async fn test_dataloader_clone() {
        let resolver = Arc::new(GrpcEntityResolver::default());
        let configs = HashMap::new();
        let loader1 = EntityDataLoader::new(resolver, configs);
        let loader2 = loader1.clone();
        assert!(Arc::ptr_eq(
            &loader1.entity_configs,
            &loader2.entity_configs
        ));
    }

    #[tokio::test]
    async fn test_normalizes_nested_fields_for_cache_keys() {
        let resolver = Arc::new(CountingResolver::default());
        let mut configs = HashMap::new();
        configs.insert("federation_example_User".to_string(), user_entity_config());
        let loader = EntityDataLoader::new(resolver.clone(), configs);

        let first = loader
            .load(
                "federation_example_User",
                nested_representation("u1", false),
            )
            .await
            .unwrap();
        let second = loader
            .load("federation_example_User", nested_representation("u1", true))
            .await
            .unwrap();

        assert_eq!(first, second);
        assert_eq!(resolver.single_calls.load(Ordering::SeqCst), 1);
    }

    #[derive(Default)]
    struct CountingResolver {
        single_calls: AtomicUsize,
        batch_calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl EntityResolver for CountingResolver {
        async fn resolve_entity(
            &self,
            _entity_config: &EntityConfig,
            representation: &Representation,
        ) -> Result<GqlValue> {
            self.single_calls.fetch_add(1, Ordering::SeqCst);
            Ok(GqlValue::Object(representation.clone()))
        }

        async fn batch_resolve_entities(
            &self,
            _entity_config: &EntityConfig,
            representations: Vec<Representation>,
        ) -> Result<Vec<GqlValue>> {
            self.batch_calls.fetch_add(1, Ordering::SeqCst);
            Ok(representations.into_iter().map(GqlValue::Object).collect())
        }
    }

    fn user_entity_config() -> EntityConfig {
        let pool = DescriptorPool::decode(
            include_bytes!("generated/federation_example_descriptor.bin").as_ref(),
        )
        .expect("descriptor decode");
        let descriptor = pool
            .get_message_by_name("federation_example.User")
            .expect("user descriptor");

        EntityConfig {
            descriptor,
            keys: vec![vec!["id".to_string()]],
            extend: false,
            resolvable: true,
            type_name: "federation_example_User".to_string(),
        }
    }

    fn simple_representation(id: &str) -> Representation {
        let mut repr = IndexMap::new();
        repr.insert(Name::new("id"), GqlValue::String(id.to_string()));
        repr.insert(
            Name::new("__typename"),
            GqlValue::String("User".to_string()),
        );
        repr
    }

    fn nested_representation(id: &str, flip_order: bool) -> Representation {
        let mut profile = IndexMap::new();
        if flip_order {
            profile.insert(Name::new("region"), GqlValue::String("us".to_string()));
            profile.insert(Name::new("id"), GqlValue::String(format!("{id}-profile")));
        } else {
            profile.insert(Name::new("id"), GqlValue::String(format!("{id}-profile")));
            profile.insert(Name::new("region"), GqlValue::String("us".to_string()));
        }

        let mut repr = IndexMap::new();
        if flip_order {
            repr.insert(Name::new("profile"), GqlValue::Object(profile));
            repr.insert(
                Name::new("__typename"),
                GqlValue::String("federation_example_User".into()),
            );
            repr.insert(Name::new("id"), GqlValue::String(id.to_string()));
        } else {
            repr.insert(
                Name::new("__typename"),
                GqlValue::String("federation_example_User".into()),
            );
            repr.insert(Name::new("id"), GqlValue::String(id.to_string()));
            repr.insert(Name::new("profile"), GqlValue::Object(profile));
        }

        repr
    }
}

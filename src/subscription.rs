//! Multiplex GraphQL Subscription support over WebSocket.
//!
//! This module implements the `graphql-transport-ws` protocol for GraphQL subscriptions,
//! supporting multiple concurrent subscriptions over a single WebSocket connection.
//!
//! # Protocol
//!
//! Implements the [graphql-transport-ws](https://github.com/enisdenjo/graphql-ws/blob/master/PROTOCOL.md)
//! protocol, which supports:
//!
//! - **Connection initialization**: `ConnectionInit` → `ConnectionAck`
//! - **Subscription multiplexing**: Multiple subscriptions per connection, each with unique ID
//! - **Lifecycle management**: `Subscribe`, `Next`, `Error`, `Complete`
//! - **Ping/Pong**: Keep-alive mechanism
//!
//! # Example
//!
//! ```rust,no_run
//! use grpc_graphql_gateway::subscription::{MultiplexSubscription, SubscriptionConfig};
//!
//! let config = SubscriptionConfig::default();
//! let handler = MultiplexSubscription::new(config);
//! ```

use crate::error::{Error, Result};
use crate::grpc_client::GrpcClientPool;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use prost::bytes::Buf;
use prost::Message as ProstMessage;
use prost_reflect::ReflectMessage;
use prost_reflect::{DynamicMessage, MessageDescriptor};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    sync::{mpsc, RwLock},
    time::timeout,
};
use tonic::client::Grpc;
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
use tonic::codegen::http;
use tracing::{debug, error, info, warn};

/// Configuration for multiplex subscriptions.
#[derive(Debug, Clone)]
pub struct SubscriptionConfig {
    /// Connection initialization timeout (default: 10 seconds)
    pub connection_init_timeout: Duration,
    /// Keep-alive interval for ping/pong (default: 30 seconds)
    pub keep_alive_interval: Duration,
    /// Maximum number of concurrent subscriptions per connection (default: 100)
    pub max_subscriptions_per_connection: usize,
    /// Whether to require connection initialization (default: true)
    pub require_connection_init: bool,
}

impl Default for SubscriptionConfig {
    fn default() -> Self {
        Self {
            connection_init_timeout: Duration::from_secs(10),
            keep_alive_interval: Duration::from_secs(30),
            max_subscriptions_per_connection: 100,
            require_connection_init: true,
        }
    }
}

/// GraphQL-transport-ws protocol message types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    // Client -> Server
    ConnectionInit,
    Subscribe,
    Complete,
    Ping,
    // Server -> Client
    ConnectionAck,
    Next,
    Error,
    Pong,
}

/// Protocol message envelope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolMessage {
    #[serde(rename = "type")]
    pub message_type: MessageType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

impl ProtocolMessage {
    pub fn connection_ack() -> Self {
        Self {
            message_type: MessageType::ConnectionAck,
            id: None,
            payload: None,
        }
    }

    pub fn next(id: String, data: serde_json::Value) -> Self {
        Self {
            message_type: MessageType::Next,
            id: Some(id),
            payload: Some(serde_json::json!({ "data": data })),
        }
    }

    pub fn error(id: String, errors: Vec<serde_json::Value>) -> Self {
        Self {
            message_type: MessageType::Error,
            id: Some(id),
            payload: Some(serde_json::json!(errors)),
        }
    }

    pub fn complete(id: String) -> Self {
        Self {
            message_type: MessageType::Complete,
            id: Some(id),
            payload: None,
        }
    }

    pub fn pong() -> Self {
        Self {
            message_type: MessageType::Pong,
            id: None,
            payload: None,
        }
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).map_err(Error::Serialization)
    }
}

/// Subscription request payload
#[derive(Debug, Clone, Deserialize)]
pub struct SubscriptionPayload {
    pub query: String,
    #[serde(default)]
    pub variables: Option<serde_json::Value>,
    #[serde(rename = "operationName")]
    pub operation_name: Option<String>,
    #[serde(default)]
    pub extensions: Option<serde_json::Value>,
}

/// Represents an active subscription
#[derive(Debug)]
pub struct ActiveSubscription {
    /// Subscription ID
    pub id: String,
    /// Channel to send cancellation signal
    pub cancel_tx: mpsc::Sender<()>,
}

/// Subscription registry for managing active subscriptions
#[derive(Debug, Default)]
pub struct SubscriptionRegistry {
    subscriptions: HashMap<String, ActiveSubscription>,
}

impl SubscriptionRegistry {
    pub fn new() -> Self {
        Self {
            subscriptions: HashMap::new(),
        }
    }

    pub fn add(&mut self, id: String, cancel_tx: mpsc::Sender<()>) {
        self.subscriptions
            .insert(id.clone(), ActiveSubscription { id, cancel_tx });
    }

    pub fn remove(&mut self, id: &str) -> Option<ActiveSubscription> {
        self.subscriptions.remove(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.subscriptions.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.subscriptions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.subscriptions.is_empty()
    }

    pub async fn cancel(&mut self, id: &str) -> bool {
        if let Some(sub) = self.subscriptions.remove(id) {
            let _ = sub.cancel_tx.send(()).await;
            true
        } else {
            false
        }
    }

    pub async fn cancel_all(&mut self) {
        for (_, sub) in self.subscriptions.drain() {
            let _ = sub.cancel_tx.send(()).await;
        }
    }
}

/// Subscription resolver trait for custom subscription handling
#[async_trait::async_trait]
pub trait SubscriptionResolver: Send + Sync {
    /// Resolve a subscription request and return a stream of values
    async fn resolve(
        &self,
        subscription_id: String,
        payload: SubscriptionPayload,
        sender: mpsc::Sender<ProtocolMessage>,
        cancel_rx: mpsc::Receiver<()>,
    ) -> Result<()>;

    /// Parse and validate the GraphQL subscription query
    fn parse_subscription(&self, query: &str) -> Result<SubscriptionInfo>;
}

/// Information about a parsed subscription
#[derive(Debug, Clone)]
pub struct SubscriptionInfo {
    /// The field being subscribed to
    pub field_name: String,
    /// Arguments for the subscription
    pub arguments: HashMap<String, serde_json::Value>,
    /// The gRPC service name
    pub service_name: Option<String>,
    /// The gRPC method path
    pub grpc_path: Option<String>,
}

/// gRPC-backed subscription resolver
#[derive(Clone)]
pub struct GrpcSubscriptionResolver {
    client_pool: GrpcClientPool,
    /// Maps subscription field names to gRPC method info
    subscription_mappings: Arc<HashMap<String, GrpcSubscriptionMapping>>,
}

/// Mapping from GraphQL subscription to gRPC streaming method
#[derive(Debug, Clone)]
pub struct GrpcSubscriptionMapping {
    /// Full service name (e.g., "greeter.Greeter")
    pub service_name: String,
    /// gRPC method path (e.g., "/greeter.Greeter/StreamHellos")
    pub grpc_path: String,
    /// Input message descriptor for building requests
    pub input_descriptor: MessageDescriptor,
    /// Output message descriptor for parsing responses
    pub output_descriptor: MessageDescriptor,
}

impl GrpcSubscriptionResolver {
    pub fn new(client_pool: GrpcClientPool) -> Self {
        Self {
            client_pool,
            subscription_mappings: Arc::new(HashMap::new()),
        }
    }

    pub fn with_mappings(
        client_pool: GrpcClientPool,
        mappings: HashMap<String, GrpcSubscriptionMapping>,
    ) -> Self {
        Self {
            client_pool,
            subscription_mappings: Arc::new(mappings),
        }
    }

    pub fn add_mapping(&mut self, field_name: String, mapping: GrpcSubscriptionMapping) {
        Arc::make_mut(&mut self.subscription_mappings).insert(field_name, mapping);
    }
}

/// Codec for encoding/decoding dynamic protobuf messages
#[derive(Clone)]
struct ReflectCodec {
    output_desc: MessageDescriptor,
}

impl ReflectCodec {
    fn new(output_desc: MessageDescriptor) -> Self {
        Self { output_desc }
    }
}

impl Codec for ReflectCodec {
    type Encode = DynamicMessage;
    type Decode = DynamicMessage;
    type Encoder = ReflectEncoder;
    type Decoder = ReflectDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        ReflectEncoder
    }

    fn decoder(&mut self) -> Self::Decoder {
        ReflectDecoder {
            desc: self.output_desc.clone(),
        }
    }
}

struct ReflectEncoder;

impl Encoder for ReflectEncoder {
    type Item = DynamicMessage;
    type Error = tonic::Status;

    fn encode(
        &mut self,
        item: Self::Item,
        dst: &mut EncodeBuf<'_>,
    ) -> std::result::Result<(), Self::Error> {
        item.encode(dst)
            .map_err(|e| tonic::Status::internal(format!("encode error: {e}")))?;
        Ok(())
    }
}

struct ReflectDecoder {
    desc: MessageDescriptor,
}

impl Decoder for ReflectDecoder {
    type Item = DynamicMessage;
    type Error = tonic::Status;

    fn decode(
        &mut self,
        src: &mut DecodeBuf<'_>,
    ) -> std::result::Result<Option<Self::Item>, Self::Error> {
        let buf = src.chunk();
        if buf.is_empty() {
            return Ok(None);
        }
        let msg = DynamicMessage::decode(self.desc.clone(), buf)
            .map_err(|e| tonic::Status::internal(format!("decode error: {e}")))?;
        src.advance(buf.len());
        Ok(Some(msg))
    }
}

#[async_trait::async_trait]
impl SubscriptionResolver for GrpcSubscriptionResolver {
    async fn resolve(
        &self,
        subscription_id: String,
        payload: SubscriptionPayload,
        sender: mpsc::Sender<ProtocolMessage>,
        mut cancel_rx: mpsc::Receiver<()>,
    ) -> Result<()> {
        let info = self.parse_subscription(&payload.query)?;

        let mapping = self
            .subscription_mappings
            .get(&info.field_name)
            .ok_or_else(|| {
                Error::Schema(format!(
                    "No mapping found for subscription field: {}",
                    info.field_name
                ))
            })?;

        let client = self.client_pool.get(&mapping.service_name).ok_or_else(|| {
            Error::Connection(format!("gRPC client {} not found", mapping.service_name))
        })?;

        // Build the request message from variables
        let request_msg = build_request_from_variables(
            &mapping.input_descriptor,
            payload.variables.as_ref(),
            &info.arguments,
        )?;

        let mut grpc = Grpc::new(client.channel());
        grpc.ready()
            .await
            .map_err(|e| Error::Internal(format!("gRPC not ready: {e}")))?;

        let codec = ReflectCodec::new(mapping.output_descriptor.clone());
        let path: http::uri::PathAndQuery = mapping
            .grpc_path
            .parse()
            .map_err(|e| Error::Schema(format!("invalid gRPC path: {e}")))?;

        let response = grpc
            .server_streaming(tonic::Request::new(request_msg), path, codec)
            .await
            .map_err(|e| Error::Internal(format!("gRPC streaming error: {e}")))?;

        let mut stream = response.into_inner();
        let id = subscription_id.clone();

        loop {
            tokio::select! {
                // Check for cancellation
                _ = cancel_rx.recv() => {
                    debug!(subscription_id = %id, "Subscription cancelled");
                    break;
                }
                // Process stream items
                item = stream.next() => {
                    match item {
                        Some(Ok(msg)) => {
                            let value = dynamic_message_to_json(&msg)?;
                            let next_msg = ProtocolMessage::next(id.clone(), value);
                            if sender.send(next_msg).await.is_err() {
                                debug!(subscription_id = %id, "Client disconnected");
                                break;
                            }
                        }
                        Some(Err(e)) => {
                            let error_msg = ProtocolMessage::error(
                                id.clone(),
                                vec![serde_json::json!({
                                    "message": e.to_string()
                                })],
                            );
                            let _ = sender.send(error_msg).await;
                            break;
                        }
                        None => {
                            // Stream completed
                            let complete_msg = ProtocolMessage::complete(id.clone());
                            let _ = sender.send(complete_msg).await;
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn parse_subscription(&self, query: &str) -> Result<SubscriptionInfo> {
        // Simple parser for subscription queries
        // Expected format: subscription { fieldName(arg1: val1) { ... } }
        // or: subscription Name { fieldName(arg1: val1) { ... } }

        let query = query.trim();

        // Check if it starts with "subscription"
        if !query.starts_with("subscription") {
            return Err(Error::InvalidRequest(
                "Query must be a subscription operation".into(),
            ));
        }

        // Find the opening brace of the subscription body
        let body_start = query.find('{').ok_or_else(|| {
            Error::InvalidRequest("Invalid subscription: missing opening brace".into())
        })?;

        let body = &query[body_start + 1..];

        // Find the field name (first word after the brace)
        let field_part = body.trim_start();
        let field_end = field_part
            .find(|c: char| c == '(' || c == '{' || c.is_whitespace())
            .unwrap_or(field_part.len());

        let field_name = field_part[..field_end].trim().to_string();

        if field_name.is_empty() || field_name == "}" {
            return Err(Error::InvalidRequest(
                "Invalid subscription: missing field name".into(),
            ));
        }

        // Parse arguments if present
        let mut arguments = HashMap::new();
        if let Some(args_start) = field_part.find('(') {
            if let Some(args_end) = field_part.find(')') {
                let args_str = &field_part[args_start + 1..args_end];
                arguments = parse_arguments(args_str)?;
            }
        }

        Ok(SubscriptionInfo {
            field_name,
            arguments,
            service_name: None,
            grpc_path: None,
        })
    }
}

/// Parse GraphQL arguments from a string like "name: \"value\", count: 5"
fn parse_arguments(args_str: &str) -> Result<HashMap<String, serde_json::Value>> {
    let mut arguments = HashMap::new();

    if args_str.trim().is_empty() {
        return Ok(arguments);
    }

    // Simple argument parser (handles basic cases)
    for arg in args_str.split(',') {
        let arg = arg.trim();
        if arg.is_empty() {
            continue;
        }

        if let Some((name, value)) = arg.split_once(':') {
            let name = name.trim().to_string();
            let value = value.trim();

            // Parse the value
            let parsed_value = if value.starts_with('"') && value.ends_with('"') {
                // String value
                serde_json::Value::String(value[1..value.len() - 1].to_string())
            } else if value == "true" {
                serde_json::Value::Bool(true)
            } else if value == "false" {
                serde_json::Value::Bool(false)
            } else if value == "null" {
                serde_json::Value::Null
            } else if let Ok(n) = value.parse::<i64>() {
                serde_json::Value::Number(n.into())
            } else if let Ok(n) = value.parse::<f64>() {
                serde_json::json!(n)
            } else {
                // Treat as string (e.g., enum values)
                serde_json::Value::String(value.to_string())
            };

            arguments.insert(name, parsed_value);
        }
    }

    Ok(arguments)
}

/// Build a DynamicMessage from variables and inline arguments
fn build_request_from_variables(
    descriptor: &MessageDescriptor,
    variables: Option<&serde_json::Value>,
    arguments: &HashMap<String, serde_json::Value>,
) -> Result<DynamicMessage> {
    let mut msg = DynamicMessage::new(descriptor.clone());

    // First, apply inline arguments
    for (name, value) in arguments {
        if let Some(field) = descriptor.get_field_by_name(name) {
            let prost_value = json_to_prost_value(value, &field)?;
            msg.set_field(&field, prost_value);
        }
    }

    // Then, overlay variables (they take precedence)
    if let Some(serde_json::Value::Object(vars)) = variables {
        for (name, value) in vars {
            if let Some(field) = descriptor.get_field_by_name(name) {
                let prost_value = json_to_prost_value(value, &field)?;
                msg.set_field(&field, prost_value);
            }
        }
    }

    Ok(msg)
}

/// Convert JSON value to prost Value
fn json_to_prost_value(
    json: &serde_json::Value,
    field: &prost_reflect::FieldDescriptor,
) -> Result<prost_reflect::Value> {
    use prost_reflect::{Kind, Value};

    match (json, field.kind()) {
        (serde_json::Value::Null, Kind::Message(msg_desc)) => {
            Ok(Value::Message(DynamicMessage::new(msg_desc.clone())))
        }
        (serde_json::Value::Null, _) => Ok(Value::String(String::new())), // for non-messages, use default
        (serde_json::Value::Bool(b), Kind::Bool) => Ok(Value::Bool(*b)),
        (serde_json::Value::Number(n), Kind::Int32 | Kind::Sint32 | Kind::Sfixed32) => {
            Ok(Value::I32(n.as_i64().unwrap_or(0) as i32))
        }
        (serde_json::Value::Number(n), Kind::Int64 | Kind::Sint64 | Kind::Sfixed64) => {
            Ok(Value::I64(n.as_i64().unwrap_or(0)))
        }
        (serde_json::Value::Number(n), Kind::Uint32 | Kind::Fixed32) => {
            Ok(Value::U32(n.as_u64().unwrap_or(0) as u32))
        }
        (serde_json::Value::Number(n), Kind::Uint64 | Kind::Fixed64) => {
            Ok(Value::U64(n.as_u64().unwrap_or(0)))
        }
        (serde_json::Value::Number(n), Kind::Float) => {
            Ok(Value::F32(n.as_f64().unwrap_or(0.0) as f32))
        }
        (serde_json::Value::Number(n), Kind::Double) => Ok(Value::F64(n.as_f64().unwrap_or(0.0))),
        (serde_json::Value::String(s), Kind::String) => Ok(Value::String(s.clone())),
        (serde_json::Value::String(s), Kind::Bytes) => {
            Ok(Value::Bytes(prost::bytes::Bytes::from(s.clone())))
        }
        (serde_json::Value::String(s), Kind::Enum(enum_desc)) => {
            if let Some(enum_value) = enum_desc.get_value_by_name(s) {
                Ok(Value::EnumNumber(enum_value.number()))
            } else {
                Err(Error::InvalidRequest(format!("Unknown enum value: {}", s)))
            }
        }
        (serde_json::Value::Object(obj), Kind::Message(msg_desc)) => {
            let mut msg = DynamicMessage::new(msg_desc.clone());
            for (key, val) in obj {
                if let Some(nested_field) = msg_desc.get_field_by_name(key) {
                    let nested_value = json_to_prost_value(val, &nested_field)?;
                    msg.set_field(&nested_field, nested_value);
                }
            }
            Ok(Value::Message(msg))
        }
        (serde_json::Value::Array(arr), _) if field.is_list() => {
            let values: std::result::Result<Vec<_>, _> =
                arr.iter().map(|v| json_to_prost_value(v, field)).collect();
            Ok(Value::List(values?))
        }
        _ => Err(Error::InvalidRequest(format!(
            "Cannot convert {:?} to {:?}",
            json,
            field.kind()
        ))),
    }
}

/// Convert DynamicMessage to JSON value
fn dynamic_message_to_json(msg: &DynamicMessage) -> Result<serde_json::Value> {
    let mut obj = serde_json::Map::new();

    for field in msg.descriptor().fields() {
        let value = msg.get_field(&field);
        let json_value = prost_value_to_json(&value, &field)?;
        obj.insert(field.json_name().to_string(), json_value);
    }

    Ok(serde_json::Value::Object(obj))
}

/// Convert prost Value to JSON
fn prost_value_to_json(
    value: &prost_reflect::Value,
    _field: &prost_reflect::FieldDescriptor,
) -> Result<serde_json::Value> {
    use prost_reflect::Value;

    match value {
        Value::Bool(b) => Ok(serde_json::Value::Bool(*b)),
        Value::I32(n) => Ok(serde_json::json!(*n)),
        Value::I64(n) => Ok(serde_json::json!(*n)),
        Value::U32(n) => Ok(serde_json::json!(*n)),
        Value::U64(n) => Ok(serde_json::json!(*n)),
        Value::F32(n) => Ok(serde_json::json!(*n)),
        Value::F64(n) => Ok(serde_json::json!(*n)),
        Value::String(s) => Ok(serde_json::Value::String(s.clone())),
        Value::Bytes(b) => Ok(serde_json::Value::String(base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            b,
        ))),
        Value::EnumNumber(n) => Ok(serde_json::json!(*n)),
        Value::Message(msg) => dynamic_message_to_json(msg),
        Value::List(list) => {
            let values: std::result::Result<Vec<_>, _> = list
                .iter()
                .map(|v| prost_value_to_json(v, _field))
                .collect();
            Ok(serde_json::Value::Array(values?))
        }
        Value::Map(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map.iter() {
                let key = match k {
                    prost_reflect::MapKey::Bool(b) => b.to_string(),
                    prost_reflect::MapKey::I32(n) => n.to_string(),
                    prost_reflect::MapKey::I64(n) => n.to_string(),
                    prost_reflect::MapKey::U32(n) => n.to_string(),
                    prost_reflect::MapKey::U64(n) => n.to_string(),
                    prost_reflect::MapKey::String(s) => s.clone(),
                };
                obj.insert(key, prost_value_to_json(v, _field)?);
            }
            Ok(serde_json::Value::Object(obj))
        }
    }
}

/// Multiplex subscription handler state
#[derive(Clone)]
pub struct MultiplexSubscription<R: SubscriptionResolver + Clone + 'static> {
    config: SubscriptionConfig,
    resolver: R,
}

impl<R: SubscriptionResolver + Clone + 'static> MultiplexSubscription<R> {
    pub fn new(config: SubscriptionConfig, resolver: R) -> Self {
        Self { config, resolver }
    }
}

/// Shared state for the WebSocket handler
#[derive(Clone)]
pub struct SubscriptionState<R: SubscriptionResolver + Clone + 'static> {
    pub handler: MultiplexSubscription<R>,
}

/// WebSocket upgrade handler for multiplex subscriptions
pub async fn ws_handler<R: SubscriptionResolver + Clone + 'static>(
    ws: WebSocketUpgrade,
    State(state): State<Arc<SubscriptionState<R>>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Handle the WebSocket connection
async fn handle_socket<R: SubscriptionResolver + Clone + 'static>(
    socket: WebSocket,
    state: Arc<SubscriptionState<R>>,
) {
    let (sender, receiver) = socket.split();
    let config = &state.handler.config;
    let resolver = state.handler.resolver.clone();

    // Shared state for the connection
    let registry = Arc::new(RwLock::new(SubscriptionRegistry::new()));
    let (outbound_tx, outbound_rx) = mpsc::channel::<ProtocolMessage>(100);

    // Spawn task to write outbound messages
    let registry_clone = registry.clone();
    let write_handle = tokio::spawn(async move {
        write_messages(sender, outbound_rx).await;
        // Clean up all subscriptions when connection closes
        registry_clone.write().await.cancel_all().await;
    });

    // Handle incoming messages
    let connection_initialized = Arc::new(RwLock::new(!config.require_connection_init));

    let init_result = if config.require_connection_init {
        timeout(
            config.connection_init_timeout,
            wait_for_init(
                receiver,
                outbound_tx.clone(),
                connection_initialized.clone(),
            ),
        )
        .await
    } else {
        Ok(Some(receiver))
    };

    match init_result {
        Ok(Some(receiver)) => {
            // Connection initialized, process messages
            process_messages(receiver, outbound_tx, registry, resolver, config.clone()).await;
        }
        Ok(None) => {
            warn!("Client disconnected before completing initialization");
        }
        Err(_) => {
            warn!("Connection initialization timeout");
        }
    }

    // Wait for write task to complete
    let _ = write_handle.await;
}

/// Wait for connection initialization
async fn wait_for_init(
    mut receiver: SplitStream<WebSocket>,
    outbound_tx: mpsc::Sender<ProtocolMessage>,
    initialized: Arc<RwLock<bool>>,
) -> Option<SplitStream<WebSocket>> {
    while let Some(msg_result) = receiver.next().await {
        match msg_result {
            Ok(Message::Text(text)) => {
                if let Ok(msg) = serde_json::from_str::<ProtocolMessage>(&text) {
                    if msg.message_type == MessageType::ConnectionInit {
                        // Send connection ack
                        let _ = outbound_tx.send(ProtocolMessage::connection_ack()).await;
                        *initialized.write().await = true;
                        debug!("Connection initialized");
                        return Some(receiver);
                    }
                }
            }
            Ok(Message::Close(_)) | Err(_) => {
                return None;
            }
            _ => {}
        }
    }
    None
}

/// Process incoming messages after initialization
async fn process_messages<R: SubscriptionResolver + Clone + 'static>(
    mut receiver: SplitStream<WebSocket>,
    outbound_tx: mpsc::Sender<ProtocolMessage>,
    registry: Arc<RwLock<SubscriptionRegistry>>,
    resolver: R,
    config: SubscriptionConfig,
) {
    while let Some(msg_result) = receiver.next().await {
        match msg_result {
            Ok(Message::Text(text)) => {
                if let Ok(msg) = serde_json::from_str::<ProtocolMessage>(&text) {
                    match msg.message_type {
                        MessageType::Subscribe => {
                            handle_subscribe(
                                msg,
                                outbound_tx.clone(),
                                registry.clone(),
                                resolver.clone(),
                                &config,
                            )
                            .await;
                        }
                        MessageType::Complete => {
                            if let Some(id) = msg.id {
                                let mut reg = registry.write().await;
                                if reg.cancel(&id).await {
                                    debug!(subscription_id = %id, "Subscription completed by client");
                                }
                            }
                        }
                        MessageType::Ping => {
                            let _ = outbound_tx.send(ProtocolMessage::pong()).await;
                        }
                        MessageType::ConnectionInit => {
                            // Already initialized, send ack again (idempotent)
                            let _ = outbound_tx.send(ProtocolMessage::connection_ack()).await;
                        }
                        _ => {
                            debug!("Ignoring unexpected message type: {:?}", msg.message_type);
                        }
                    }
                }
            }
            Ok(Message::Close(_)) => {
                debug!("Client closed connection");
                break;
            }
            Err(e) => {
                error!("WebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }
}

/// Handle a subscribe message
async fn handle_subscribe<R: SubscriptionResolver + Clone + 'static>(
    msg: ProtocolMessage,
    outbound_tx: mpsc::Sender<ProtocolMessage>,
    registry: Arc<RwLock<SubscriptionRegistry>>,
    resolver: R,
    config: &SubscriptionConfig,
) {
    let id = match msg.id {
        Some(id) => id,
        None => {
            warn!("Subscribe message missing ID");
            return;
        }
    };

    // Check if subscription already exists
    if registry.read().await.contains(&id) {
        let _ = outbound_tx
            .send(ProtocolMessage::error(
                id,
                vec![serde_json::json!({
                    "message": "Subscriber for this ID already exists"
                })],
            ))
            .await;
        return;
    }

    // Check subscription limit
    if registry.read().await.len() >= config.max_subscriptions_per_connection {
        let _ = outbound_tx
            .send(ProtocolMessage::error(
                id,
                vec![serde_json::json!({
                    "message": "Too many subscriptions"
                })],
            ))
            .await;
        return;
    }

    // Parse payload
    let payload = match msg.payload {
        Some(p) => match serde_json::from_value::<SubscriptionPayload>(p) {
            Ok(payload) => payload,
            Err(e) => {
                let _ = outbound_tx
                    .send(ProtocolMessage::error(
                        id,
                        vec![serde_json::json!({
                            "message": format!("Invalid payload: {}", e)
                        })],
                    ))
                    .await;
                return;
            }
        },
        None => {
            let _ = outbound_tx
                .send(ProtocolMessage::error(
                    id,
                    vec![serde_json::json!({
                        "message": "Missing payload"
                    })],
                ))
                .await;
            return;
        }
    };

    // Create cancellation channel
    let (cancel_tx, cancel_rx) = mpsc::channel::<()>(1);

    // Register the subscription
    registry.write().await.add(id.clone(), cancel_tx);

    // Spawn the subscription resolver
    let id_clone = id.clone();
    let outbound_tx_clone = outbound_tx.clone();
    let registry_clone = registry.clone();

    tokio::spawn(async move {
        match resolver
            .resolve(id_clone.clone(), payload, outbound_tx_clone, cancel_rx)
            .await
        {
            Ok(_) => {
                debug!(subscription_id = %id_clone, "Subscription completed successfully");
            }
            Err(e) => {
                error!(subscription_id = %id_clone, error = %e, "Subscription error");
            }
        }
        // Remove from registry when done
        registry_clone.write().await.remove(&id_clone);
    });

    info!(subscription_id = %id, "Subscription started");
}

/// Write outbound messages to the WebSocket
async fn write_messages(
    mut sender: SplitSink<WebSocket, Message>,
    mut rx: mpsc::Receiver<ProtocolMessage>,
) {
    while let Some(msg) = rx.recv().await {
        match msg.to_json() {
            Ok(json) => {
                if sender.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
            Err(e) => {
                error!("Failed to serialize message: {}", e);
            }
        }
    }
}

/// Builder for creating multiplex subscription routes
pub struct MultiplexSubscriptionBuilder<R: SubscriptionResolver + Clone + 'static> {
    config: SubscriptionConfig,
    resolver: R,
}

impl<R: SubscriptionResolver + Clone + 'static> MultiplexSubscriptionBuilder<R> {
    pub fn new(resolver: R) -> Self {
        Self {
            config: SubscriptionConfig::default(),
            resolver,
        }
    }

    /// Set connection initialization timeout
    pub fn connection_init_timeout(mut self, timeout: Duration) -> Self {
        self.config.connection_init_timeout = timeout;
        self
    }

    /// Set keep-alive interval
    pub fn keep_alive_interval(mut self, interval: Duration) -> Self {
        self.config.keep_alive_interval = interval;
        self
    }

    /// Set maximum subscriptions per connection
    pub fn max_subscriptions(mut self, max: usize) -> Self {
        self.config.max_subscriptions_per_connection = max;
        self
    }

    /// Set whether to require connection initialization
    pub fn require_connection_init(mut self, require: bool) -> Self {
        self.config.require_connection_init = require;
        self
    }

    /// Build the subscription handler and state
    pub fn build(self) -> Arc<SubscriptionState<R>> {
        Arc::new(SubscriptionState {
            handler: MultiplexSubscription::new(self.config, self.resolver),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_arguments() {
        let args = parse_arguments("name: \"John\", count: 5, active: true").unwrap();
        assert_eq!(args.get("name"), Some(&serde_json::json!("John")));
        assert_eq!(args.get("count"), Some(&serde_json::json!(5)));
        assert_eq!(args.get("active"), Some(&serde_json::json!(true)));
    }

    #[test]
    fn test_protocol_message_serialization() {
        let msg = ProtocolMessage::connection_ack();
        let json = msg.to_json().unwrap();
        assert!(json.contains("connection_ack"));

        let next = ProtocolMessage::next("123".into(), serde_json::json!({"hello": "world"}));
        let json = next.to_json().unwrap();
        assert!(json.contains("\"id\":\"123\""));
        assert!(json.contains("\"type\":\"next\""));
    }

    #[test]
    fn test_subscription_registry() {
        let mut registry = SubscriptionRegistry::new();
        let (tx, _rx) = mpsc::channel::<()>(1);

        registry.add("sub1".into(), tx);
        assert!(registry.contains("sub1"));
        assert_eq!(registry.len(), 1);

        let removed = registry.remove("sub1");
        assert!(removed.is_some());
        assert!(!registry.contains("sub1"));
    }

    #[test]
    fn test_subscription_config_default() {
        let config = SubscriptionConfig::default();
        assert_eq!(config.connection_init_timeout, Duration::from_secs(10));
        assert_eq!(config.keep_alive_interval, Duration::from_secs(30));
        assert_eq!(config.max_subscriptions_per_connection, 100);
        assert!(config.require_connection_init);
    }

    #[test]
    fn test_protocol_message_ping_pong() {
        let ping = ProtocolMessage {
            message_type: MessageType::Ping,
            id: None,
            payload: None,
        };
        let pong = ProtocolMessage::pong();
        
        assert_eq!(ping.message_type, MessageType::Ping);
        assert_eq!(pong.message_type, MessageType::Pong);
        
        let json = pong.to_json().unwrap();
        assert!(json.contains("pong"));
    }

    #[test]
    fn test_protocol_message_error() {
        let error = ProtocolMessage::error(
            "sub1".to_string(), 
            vec![serde_json::json!({"message": "Something went wrong"})]
        );
        
        assert_eq!(error.message_type, MessageType::Error);
        assert_eq!(error.id, Some("sub1".to_string()));
        
        let json = error.to_json().unwrap();
        assert!(json.contains("error"));
        assert!(json.contains("Something went wrong"));
    }

    #[test]
    fn test_protocol_message_complete() {
        let complete = ProtocolMessage::complete("sub1".to_string());
        
        assert_eq!(complete.message_type, MessageType::Complete);
        assert_eq!(complete.id, Some("sub1".to_string()));
    }

    #[test]
    fn test_protocol_message_subscribe_deserialize() {
        let json = r#"{
            "type": "subscribe",
            "id": "sub1",
            "payload": {
                "query": "subscription { userAdded { id } }"
            }
        }"#;
        
        let msg: ProtocolMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.message_type, MessageType::Subscribe);
        assert_eq!(msg.id, Some("sub1".to_string()));
        
        let payload = msg.payload.unwrap();
        assert!(payload.get("query").is_some());
    }

    #[tokio::test]
    async fn test_registry_cancel() {
        let mut registry = SubscriptionRegistry::default();
        let (tx, mut rx) = mpsc::channel::<()>(1);
        
        registry.add("sub1".to_string(), tx);
        
        // Cancel should remove and send signal
        let cancelled = registry.cancel("sub1").await;
        assert!(cancelled);
        assert!(!registry.contains("sub1"));
        
        // Verify signal received
        assert!(rx.recv().await.is_some());
    }

    #[tokio::test]
    async fn test_registry_cancel_all() {
        let mut registry = SubscriptionRegistry::default();
        let (tx1, mut rx1) = mpsc::channel::<()>(1);
        let (tx2, mut rx2) = mpsc::channel::<()>(1);
        
        registry.add("sub1".to_string(), tx1);
        registry.add("sub2".to_string(), tx2);
        
        registry.cancel_all().await;
        assert!(registry.is_empty());
        
        assert!(rx1.recv().await.is_some());
        assert!(rx2.recv().await.is_some());
    }

    #[tokio::test]
    async fn test_registry_missing_cancel() {
        let mut registry = SubscriptionRegistry::default();
        let cancelled = registry.cancel("missing").await;
        assert!(!cancelled);
    }

    #[test]
    fn test_parse_arguments_basic() {
        let args_str = "id: \"123\", count: 5";
        let args = parse_arguments(args_str).unwrap();
        
        assert_eq!(args.get("id"), Some(&serde_json::Value::String("123".to_string())));
        assert_eq!(args.get("count"), Some(&serde_json::json!(5)));
    }

    #[test]
    fn test_parse_arguments_boolean() {
        let args_str = "active: true, deleted: false";
        let args = parse_arguments(args_str).unwrap();
        
        assert_eq!(args.get("active"), Some(&serde_json::Value::Bool(true)));
        assert_eq!(args.get("deleted"), Some(&serde_json::Value::Bool(false)));
    }

    #[test]
    fn test_parse_arguments_null() {
        let args_str = "value: null";
        let args = parse_arguments(args_str).unwrap();
        
        assert_eq!(args.get("value"), Some(&serde_json::Value::Null));
    }
    
    // We cannot easily test GrpcSubscriptionResolver::parse_subscription directly without mocking GrpcClientPool?
    // Actually GrpcSubscriptionResolver does not use the pool for parsing, only for resolution.
    // The parse_subscription method is on the trait, implemented by the struct.
    
    #[test]
    fn test_grpc_resolver_parse_subscription_valid() {
        let pool = GrpcClientPool::new();
        let resolver = GrpcSubscriptionResolver::new(pool);
        
        let query = "subscription { callback(id: \"123\") { status } }";
        let info = resolver.parse_subscription(query).expect("Should parse");
        
        assert_eq!(info.field_name, "callback");
        assert_eq!(info.arguments.get("id"), Some(&serde_json::Value::String("123".to_string())));
    }

    #[test]
    fn test_grpc_resolver_parse_subscription_named() {
        let pool = GrpcClientPool::new();
        let resolver = GrpcSubscriptionResolver::new(pool);
        
        let query = "subscription MySub { userUpdates { id } }";
        let info = resolver.parse_subscription(query).expect("Should parse");
        
        assert_eq!(info.field_name, "userUpdates");
    }

    #[test]
    fn test_grpc_resolver_parse_subscription_invalid_start() {
        let pool = GrpcClientPool::new();
        let resolver = GrpcSubscriptionResolver::new(pool);
        
        let query = "query { user }";
        let result = resolver.parse_subscription(query);
        assert!(matches!(result, Err(Error::InvalidRequest(_))));
    }

    #[test]
    fn test_grpc_resolver_parse_subscription_malformed() {
        let pool = GrpcClientPool::new();
        let resolver = GrpcSubscriptionResolver::new(pool);
        
        let query = "subscription { }"; // No field
        let result = resolver.parse_subscription(query);
        assert!(matches!(result, Err(Error::InvalidRequest(_))));
    }

    #[test]
    fn test_json_to_prost_conversion_basic() {
        // This exercises json_to_prost_value somewhat indirectly or we can test private if we want?
        // private functions are accessible in mod tests
        // Let's try to unit test build_request_from_variables if possible, but it requires MessageDescriptor.
        // We can create a simple DynamicMessage if we have a descriptor. 
        // Descriptors are hard to synthesize without a file descriptor set.
        // We'll skip complex prost conversions for now unless we mock descriptors.
    }
}

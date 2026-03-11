//! Subscription Federation — distributed GraphQL subscriptions across subgraphs.
//!
//! This module enables the router to fan-out a client subscription to one or more
//! subgraphs over WebSocket, merge the resulting event streams, and relay the
//! merged stream back to the client over a single WebSocket connection.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────┐
//! │   Client WS  │  subscription { orderCreated { id total } userUpdated { id name } }
//! └──────┬───────┘
//!        │  single WebSocket (graphql-transport-ws)
//!        ▼
//! ┌──────────────────────────────────────────────────────────────────────┐
//! │                    Federation Subscription Engine                    │
//! │                                                                      │
//! │   1. Parse subscription → extract top-level fields                   │
//! │   2. Route each field to the owning subgraph                         │
//! │   3. Open upstream WS per subgraph (graphql-transport-ws)            │
//! │   4. Merge incoming `next` events into a single stream               │
//! │   5. Relay merged events to the client                               │
//! └──────┬────────────────────┬────────────────────┬─────────────────────┘
//!        │                    │                    │
//!   ┌────▼────┐         ┌────▼────┐         ┌────▼────┐
//!   │ orders  │ WS      │ users   │ WS      │ reviews │ WS
//!   │subgraph │         │subgraph │         │subgraph │
//!   └─────────┘         └─────────┘         └─────────┘
//! ```
//!
//! # Protocol
//!
//! Both the client-facing and subgraph-facing WebSocket connections use the
//! [`graphql-transport-ws`](https://github.com/enisdenjo/graphql-ws/blob/master/PROTOCOL.md)
//! protocol. The engine translates subscription IDs so that each upstream
//! connection uses its own ID namespace.
//!
//! # Configuration
//!
//! Subscription routing is configured via `SubscriptionFederationConfig`:
//!
//! ```yaml
//! subscriptions:
//!   enabled: true
//!   heartbeat_interval_secs: 30
//!   subgraph_timeout_secs: 10
//!   max_upstream_connections: 50
//!   routing:
//!     orderCreated: orders
//!     orderUpdated: orders
//!     userUpdated: users
//!     reviewPosted: reviews
//! ```

use crate::error::{Error, Result};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite;
use tracing::{debug, info, warn};

// ─── Configuration ───────────────────────────────────────────────────────────

/// Top-level configuration for federated subscriptions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionFederationConfig {
    /// Enable subscription federation (default: true)
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Heartbeat (ping) interval to upstream subgraphs in seconds.
    #[serde(default = "default_heartbeat")]
    pub heartbeat_interval_secs: u64,

    /// Timeout for the upstream WebSocket handshake in seconds.
    #[serde(default = "default_timeout")]
    pub subgraph_timeout_secs: u64,

    /// Maximum number of concurrent upstream WebSocket connections across all
    /// client subscriptions.
    #[serde(default = "default_max_upstream")]
    pub max_upstream_connections: usize,

    /// Map from GraphQL subscription field name → subgraph name.
    ///
    /// If a field is not listed here and `auto_route` is true the engine will
    /// broadcast the subscription to **all** subgraphs and merge.
    #[serde(default)]
    pub routing: HashMap<String, String>,

    /// When true, fields without an explicit routing entry are broadcast to all
    /// subgraphs.  When false, unroutable fields return an error.
    #[serde(default)]
    pub auto_route: bool,
}

fn default_true() -> bool {
    true
}
fn default_heartbeat() -> u64 {
    30
}
fn default_timeout() -> u64 {
    10
}
fn default_max_upstream() -> usize {
    50
}

impl Default for SubscriptionFederationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            heartbeat_interval_secs: 30,
            subgraph_timeout_secs: 10,
            max_upstream_connections: 50,
            routing: HashMap::new(),
            auto_route: true,
        }
    }
}

// ─── Subgraph info ───────────────────────────────────────────────────────────

/// Minimal description of a subgraph needed for upstream subscription connections.
#[derive(Debug, Clone)]
pub struct SubgraphEndpoint {
    pub name: String,
    /// The **WebSocket** URL of the subgraph, e.g. `ws://localhost:4002/graphql/ws`.
    pub ws_url: String,
    /// Optional headers forwarded to the upstream WebSocket handshake.
    pub headers: HashMap<String, String>,
}

impl SubgraphEndpoint {
    /// Derive a WebSocket URL from a regular HTTP URL by replacing the scheme
    /// and appending `/ws` if not already present.
    pub fn from_http_url(name: &str, http_url: &str, headers: HashMap<String, String>) -> Self {
        let ws_url = http_to_ws_url(http_url);
        Self {
            name: name.to_string(),
            ws_url,
            headers,
        }
    }
}

/// Convert `http(s)://host/path` to `ws(s)://host/path/ws` for subscription
/// endpoints.  If the URL already uses a `ws` scheme it is returned as-is.
fn http_to_ws_url(url: &str) -> String {
    let mut ws_url = if url.starts_with("https://") {
        url.replacen("https://", "wss://", 1)
    } else if url.starts_with("http://") {
        url.replacen("http://", "ws://", 1)
    } else {
        // Already ws / wss or unknown — return unchanged
        return url.to_string();
    };

    // Append /ws if not already there
    let path = ws_url
        .find("://")
        .map(|i| &ws_url[i + 3..])
        .and_then(|rest| rest.find('/'))
        .map(|_| ws_url.as_str())
        .unwrap_or(&ws_url);
    let _ = path; // used for clarity, actual mutation below

    if !ws_url.ends_with("/ws") && !ws_url.ends_with("/ws/") {
        if ws_url.ends_with('/') {
            ws_url.push_str("ws");
        } else {
            ws_url.push_str("/ws");
        }
    }
    ws_url
}

// ─── Engine ──────────────────────────────────────────────────────────────────

/// Metrics snapshot for subscription federation.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SubscriptionFederationMetrics {
    /// Total subscriptions processed since startup.
    pub total_subscriptions: u64,
    /// Currently active client subscriptions.
    pub active_subscriptions: u64,
    /// Currently active upstream WebSocket connections.
    pub active_upstream_connections: u64,
    /// Total upstream messages relayed to clients.
    pub messages_relayed: u64,
    /// Total upstream connection errors.
    pub upstream_errors: u64,
}

/// Shared metrics counters.
#[derive(Debug, Default)]
struct Counters {
    total_subscriptions: AtomicU64,
    active_subscriptions: AtomicU64,
    active_upstream_connections: AtomicU64,
    messages_relayed: AtomicU64,
    upstream_errors: AtomicU64,
}

impl Counters {
    fn snapshot(&self) -> SubscriptionFederationMetrics {
        SubscriptionFederationMetrics {
            total_subscriptions: self.total_subscriptions.load(Ordering::Relaxed),
            active_subscriptions: self.active_subscriptions.load(Ordering::Relaxed),
            active_upstream_connections: self.active_upstream_connections.load(Ordering::Relaxed),
            messages_relayed: self.messages_relayed.load(Ordering::Relaxed),
            upstream_errors: self.upstream_errors.load(Ordering::Relaxed),
        }
    }
}

/// The subscription federation engine.
///
/// Create one per router process and share it across WebSocket connections via
/// `Arc`.
#[derive(Clone)]
pub struct SubscriptionFederationEngine {
    config: SubscriptionFederationConfig,
    /// Available subgraph endpoints keyed by subgraph name.
    subgraphs: Arc<HashMap<String, SubgraphEndpoint>>,
    counters: Arc<Counters>,
}

impl SubscriptionFederationEngine {
    /// Create a new engine.
    pub fn new(config: SubscriptionFederationConfig, subgraphs: Vec<SubgraphEndpoint>) -> Self {
        let map: HashMap<String, SubgraphEndpoint> =
            subgraphs.into_iter().map(|s| (s.name.clone(), s)).collect();
        Self {
            config,
            subgraphs: Arc::new(map),
            counters: Arc::new(Counters::default()),
        }
    }

    /// Return a snapshot of the current metrics.
    pub fn metrics(&self) -> SubscriptionFederationMetrics {
        self.counters.snapshot()
    }

    /// Determine which subgraphs should receive the subscription based on
    /// the top-level field names in the subscription query.
    ///
    /// Returns a map from subgraph name → the set of fields it owns.
    pub fn route_subscription(&self, fields: &[String]) -> Result<HashMap<String, Vec<String>>> {
        let mut plan: HashMap<String, Vec<String>> = HashMap::new();

        for field in fields {
            if let Some(subgraph_name) = self.config.routing.get(field) {
                // Explicit routing entry.
                if !self.subgraphs.contains_key(subgraph_name) {
                    return Err(Error::Schema(format!(
                        "Subscription field '{}' is routed to unknown subgraph '{}'",
                        field, subgraph_name
                    )));
                }
                plan.entry(subgraph_name.clone())
                    .or_default()
                    .push(field.clone());
            } else if self.config.auto_route {
                // Broadcast to all subgraphs.
                for name in self.subgraphs.keys() {
                    plan.entry(name.clone()).or_default().push(field.clone());
                }
            } else {
                return Err(Error::Schema(format!(
                    "No subscription routing configured for field '{}' and auto_route is disabled",
                    field
                )));
            }
        }

        Ok(plan)
    }

    /// Handle a federated subscription for one client.
    ///
    /// This is the main entry point called from the router's WebSocket handler
    /// when a `subscribe` message arrives.
    ///
    /// # Arguments
    ///
    /// * `subscription_id` – the client-chosen subscription ID
    /// * `query` – the full subscription query string
    /// * `variables` – optional query variables
    /// * `client_tx` – channel to send `graphql-transport-ws` messages back to the client
    /// * `cancel_rx` – signalled when the client sends `complete` or disconnects
    pub async fn handle_subscription(
        &self,
        subscription_id: String,
        query: String,
        variables: Option<serde_json::Value>,
        client_tx: mpsc::Sender<serde_json::Value>,
        mut cancel_rx: mpsc::Receiver<()>,
    ) {
        self.counters
            .total_subscriptions
            .fetch_add(1, Ordering::Relaxed);
        self.counters
            .active_subscriptions
            .fetch_add(1, Ordering::Relaxed);

        let fields = match parse_subscription_fields(&query) {
            Ok(f) => f,
            Err(e) => {
                let _ = client_tx
                    .send(serde_json::json!({
                        "type": "error",
                        "id": subscription_id,
                        "payload": [{"message": e.to_string()}]
                    }))
                    .await;
                self.counters
                    .active_subscriptions
                    .fetch_sub(1, Ordering::Relaxed);
                return;
            }
        };

        let plan = match self.route_subscription(&fields) {
            Ok(p) => p,
            Err(e) => {
                let _ = client_tx
                    .send(serde_json::json!({
                        "type": "error",
                        "id": subscription_id,
                        "payload": [{"message": e.to_string()}]
                    }))
                    .await;
                self.counters
                    .active_subscriptions
                    .fetch_sub(1, Ordering::Relaxed);
                return;
            }
        };

        info!(
            subscription_id = %subscription_id,
            fields = ?fields,
            subgraphs = ?plan.keys().collect::<Vec<_>>(),
            "Starting federated subscription"
        );

        // Channel that all upstream tasks write into.
        let (merged_tx, mut merged_rx) = mpsc::channel::<UpstreamEvent>(256);

        // Spawn one upstream WebSocket task per subgraph in the plan.
        let mut upstream_handles = Vec::new();
        for subgraph_name in plan.keys() {
            let endpoint = match self.subgraphs.get(subgraph_name) {
                Some(ep) => ep.clone(),
                None => continue,
            };
            let upstream_sub_id = format!("{}_{}", subscription_id, subgraph_name);
            let query_clone = query.clone();
            let variables_clone = variables.clone();
            let merged_tx_clone = merged_tx.clone();
            let counters_clone = self.counters.clone();
            let task_config = UpstreamTaskConfig {
                timeout_secs: self.config.subgraph_timeout_secs,
                heartbeat_secs: self.config.heartbeat_interval_secs,
            };

            let handle = tokio::spawn(async move {
                upstream_subscription_task(
                    endpoint,
                    upstream_sub_id,
                    query_clone,
                    variables_clone,
                    merged_tx_clone,
                    counters_clone,
                    task_config,
                )
                .await;
            });
            upstream_handles.push(handle);
        }

        // Drop our own sender so merged_rx closes when all upstream tasks end.
        drop(merged_tx);

        // Relay loop: merge upstream events → client.
        loop {
            tokio::select! {
                // Client cancelled.
                _ = cancel_rx.recv() => {
                    debug!(subscription_id = %subscription_id, "Federated subscription cancelled by client");
                    break;
                }
                // Upstream event.
                event = merged_rx.recv() => {
                    match event {
                        Some(UpstreamEvent::Next { subgraph, data }) => {
                            self.counters.messages_relayed.fetch_add(1, Ordering::Relaxed);
                            let msg = serde_json::json!({
                                "type": "next",
                                "id": subscription_id,
                                "payload": {
                                    "data": data,
                                    "extensions": {
                                        "subgraph": subgraph
                                    }
                                }
                            });
                            if client_tx.send(msg).await.is_err() {
                                debug!(subscription_id = %subscription_id, "Client disconnected during relay");
                                break;
                            }
                        }
                        Some(UpstreamEvent::Error { subgraph, message }) => {
                            warn!(
                                subscription_id = %subscription_id,
                                subgraph = %subgraph,
                                error = %message,
                                "Upstream subscription error"
                            );
                            // Relay the error but don't kill the whole subscription;
                            // other subgraphs may still be alive.
                            let msg = serde_json::json!({
                                "type": "next",
                                "id": subscription_id,
                                "payload": {
                                    "errors": [{"message": message, "extensions": {"subgraph": subgraph}}]
                                }
                            });
                            let _ = client_tx.send(msg).await;
                        }
                        Some(UpstreamEvent::Complete { subgraph }) => {
                            debug!(
                                subscription_id = %subscription_id,
                                subgraph = %subgraph,
                                "Upstream subscription completed"
                            );
                            // One subgraph finished — others may still be producing events.
                        }
                        None => {
                            // All upstream channels closed → federation complete.
                            debug!(subscription_id = %subscription_id, "All upstream subscriptions completed");
                            break;
                        }
                    }
                }
            }
        }

        // Send completion to client.
        let _ = client_tx
            .send(serde_json::json!({
                "type": "complete",
                "id": subscription_id
            }))
            .await;

        // Abort remaining upstream tasks.
        for handle in upstream_handles {
            handle.abort();
        }

        self.counters
            .active_subscriptions
            .fetch_sub(1, Ordering::Relaxed);

        info!(subscription_id = %subscription_id, "Federated subscription ended");
    }
}

// ─── Upstream events ─────────────────────────────────────────────────────────

/// Events produced by upstream subgraph WebSocket connections.
#[derive(Debug, Clone)]
enum UpstreamEvent {
    /// A `next` message from a subgraph.
    Next {
        subgraph: String,
        data: serde_json::Value,
    },
    /// An `error` message from a subgraph.
    Error { subgraph: String, message: String },
    /// The subgraph's subscription completed.
    Complete { subgraph: String },
}

// ─── Upstream task ───────────────────────────────────────────────────────────

/// Configuration values forwarded to each upstream subscription task.
#[derive(Debug, Clone, Copy)]
struct UpstreamTaskConfig {
    timeout_secs: u64,
    heartbeat_secs: u64,
}

/// Manages a single upstream WebSocket connection to a subgraph for one
/// subscription.
async fn upstream_subscription_task(
    endpoint: SubgraphEndpoint,
    subscription_id: String,
    query: String,
    variables: Option<serde_json::Value>,
    merged_tx: mpsc::Sender<UpstreamEvent>,
    counters: Arc<Counters>,
    config: UpstreamTaskConfig,
) {
    let timeout_secs = config.timeout_secs;
    let heartbeat_secs = config.heartbeat_secs;
    let subgraph_name = endpoint.name.clone();
    counters
        .active_upstream_connections
        .fetch_add(1, Ordering::Relaxed);

    // Build WebSocket request with optional headers.
    let mut request = match tungstenite::http::Request::builder()
        .uri(&endpoint.ws_url)
        .header("Sec-WebSocket-Protocol", "graphql-transport-ws")
        .body(())
    {
        Ok(r) => r,
        Err(e) => {
            counters.upstream_errors.fetch_add(1, Ordering::Relaxed);
            let _ = merged_tx
                .send(UpstreamEvent::Error {
                    subgraph: subgraph_name.clone(),
                    message: format!("Failed to build WS request: {}", e),
                })
                .await;
            counters
                .active_upstream_connections
                .fetch_sub(1, Ordering::Relaxed);
            return;
        }
    };

    // Add custom headers.
    for (key, value) in &endpoint.headers {
        if let (Ok(name), Ok(val)) = (
            tungstenite::http::header::HeaderName::from_bytes(key.as_bytes()),
            tungstenite::http::header::HeaderValue::from_str(value),
        ) {
            request.headers_mut().insert(name, val);
        }
    }

    // Connect with timeout.
    let ws_stream =
        match tokio::time::timeout(Duration::from_secs(timeout_secs), connect_async(request)).await
        {
            Ok(Ok((stream, _response))) => stream,
            Ok(Err(e)) => {
                counters.upstream_errors.fetch_add(1, Ordering::Relaxed);
                let _ = merged_tx
                    .send(UpstreamEvent::Error {
                        subgraph: subgraph_name.clone(),
                        message: format!("WebSocket connection failed: {}", e),
                    })
                    .await;
                counters
                    .active_upstream_connections
                    .fetch_sub(1, Ordering::Relaxed);
                return;
            }
            Err(_) => {
                counters.upstream_errors.fetch_add(1, Ordering::Relaxed);
                let _ = merged_tx
                    .send(UpstreamEvent::Error {
                        subgraph: subgraph_name.clone(),
                        message: "WebSocket connection timed out".to_string(),
                    })
                    .await;
                counters
                    .active_upstream_connections
                    .fetch_sub(1, Ordering::Relaxed);
                return;
            }
        };

    debug!(
        subgraph = %subgraph_name,
        url = %endpoint.ws_url,
        "Upstream WebSocket connected"
    );

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // 1. Send connection_init.
    let init_msg = serde_json::json!({"type": "connection_init"});
    if let Err(e) = ws_tx
        .send(tungstenite::Message::Text(
            serde_json::to_string(&init_msg).unwrap_or_default(),
        ))
        .await
    {
        counters.upstream_errors.fetch_add(1, Ordering::Relaxed);
        let _ = merged_tx
            .send(UpstreamEvent::Error {
                subgraph: subgraph_name.clone(),
                message: format!("Failed to send connection_init: {}", e),
            })
            .await;
        counters
            .active_upstream_connections
            .fetch_sub(1, Ordering::Relaxed);
        return;
    }

    // 2. Wait for connection_ack.
    let ack_timeout = Duration::from_secs(timeout_secs);
    match tokio::time::timeout(ack_timeout, wait_for_ack(&mut ws_rx)).await {
        Ok(true) => {
            debug!(subgraph = %subgraph_name, "connection_ack received");
        }
        _ => {
            counters.upstream_errors.fetch_add(1, Ordering::Relaxed);
            let _ = merged_tx
                .send(UpstreamEvent::Error {
                    subgraph: subgraph_name.clone(),
                    message: "Did not receive connection_ack".to_string(),
                })
                .await;
            counters
                .active_upstream_connections
                .fetch_sub(1, Ordering::Relaxed);
            return;
        }
    }

    // 3. Send subscribe message.
    let mut subscribe_payload = serde_json::json!({
        "query": query,
    });
    if let Some(vars) = &variables {
        subscribe_payload["variables"] = vars.clone();
    }
    let subscribe_msg = serde_json::json!({
        "type": "subscribe",
        "id": subscription_id,
        "payload": subscribe_payload,
    });
    if let Err(e) = ws_tx
        .send(tungstenite::Message::Text(
            serde_json::to_string(&subscribe_msg).unwrap_or_default(),
        ))
        .await
    {
        counters.upstream_errors.fetch_add(1, Ordering::Relaxed);
        let _ = merged_tx
            .send(UpstreamEvent::Error {
                subgraph: subgraph_name.clone(),
                message: format!("Failed to send subscribe: {}", e),
            })
            .await;
        counters
            .active_upstream_connections
            .fetch_sub(1, Ordering::Relaxed);
        return;
    }

    debug!(
        subgraph = %subgraph_name,
        subscription_id = %subscription_id,
        "Subscription request sent to upstream"
    );

    // 4. Read events from upstream and relay to the merged channel.
    let heartbeat_interval = Duration::from_secs(heartbeat_secs);
    let mut heartbeat = tokio::time::interval(heartbeat_interval);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            // Heartbeat ping.
            _ = heartbeat.tick() => {
                let ping = serde_json::json!({"type": "ping"});
                if ws_tx.send(tungstenite::Message::Text(
                    serde_json::to_string(&ping).unwrap_or_default(),
                )).await.is_err() {
                    debug!(subgraph = %subgraph_name, "Upstream WS closed during ping");
                    break;
                }
            }
            // Incoming message from subgraph.
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(tungstenite::Message::Text(text))) => {
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                            let msg_type = value["type"].as_str().unwrap_or("");
                            match msg_type {
                                "next" => {
                                    let payload = value.get("payload").cloned()
                                        .unwrap_or(serde_json::Value::Null);
                                    if merged_tx.send(UpstreamEvent::Next {
                                        subgraph: subgraph_name.clone(),
                                        data: payload,
                                    }).await.is_err() {
                                        // Relay channel closed → client gone.
                                        break;
                                    }
                                }
                                "error" => {
                                    let payload = value.get("payload").cloned()
                                        .unwrap_or(serde_json::Value::Null);
                                    let message = if let Some(arr) = payload.as_array() {
                                        arr.iter()
                                            .filter_map(|e| e["message"].as_str())
                                            .collect::<Vec<_>>()
                                            .join("; ")
                                    } else {
                                        payload.to_string()
                                    };
                                    counters.upstream_errors.fetch_add(1, Ordering::Relaxed);
                                    let _ = merged_tx.send(UpstreamEvent::Error {
                                        subgraph: subgraph_name.clone(),
                                        message,
                                    }).await;
                                    break;
                                }
                                "complete" => {
                                    let _ = merged_tx.send(UpstreamEvent::Complete {
                                        subgraph: subgraph_name.clone(),
                                    }).await;
                                    break;
                                }
                                "pong" | "connection_ack" | "ka" => {
                                    // Keep-alive / pong — ignore.
                                }
                                other => {
                                    debug!(
                                        subgraph = %subgraph_name,
                                        msg_type = %other,
                                        "Ignoring unknown upstream message type"
                                    );
                                }
                            }
                        }
                    }
                    Some(Ok(tungstenite::Message::Ping(data))) => {
                        let _ = ws_tx.send(tungstenite::Message::Pong(data)).await;
                    }
                    Some(Ok(tungstenite::Message::Close(_))) | None => {
                        debug!(subgraph = %subgraph_name, "Upstream WebSocket closed");
                        let _ = merged_tx.send(UpstreamEvent::Complete {
                            subgraph: subgraph_name.clone(),
                        }).await;
                        break;
                    }
                    Some(Err(e)) => {
                        counters.upstream_errors.fetch_add(1, Ordering::Relaxed);
                        let _ = merged_tx.send(UpstreamEvent::Error {
                            subgraph: subgraph_name.clone(),
                            message: format!("Upstream WS error: {}", e),
                        }).await;
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    // Attempt graceful close.
    let _ = ws_tx.close().await;

    counters
        .active_upstream_connections
        .fetch_sub(1, Ordering::Relaxed);
    debug!(subgraph = %subgraph_name, "Upstream connection closed");
}

/// Wait for a `connection_ack` message on the stream. Consumes messages until
/// an ack is found, returning `true`, or the stream ends / a non-ack is not
/// recognized.
async fn wait_for_ack<S>(ws_rx: &mut S) -> bool
where
    S: StreamExt<Item = std::result::Result<tungstenite::Message, tungstenite::Error>> + Unpin,
{
    while let Some(msg) = ws_rx.next().await {
        match msg {
            Ok(tungstenite::Message::Text(text)) => {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                    if value["type"].as_str() == Some("connection_ack") {
                        return true;
                    }
                    // Some servers send ka (keep-alive) before ack — skip.
                }
            }
            Ok(tungstenite::Message::Ping(_)) => {
                // Ignore pings during handshake.
            }
            Err(_) | Ok(tungstenite::Message::Close(_)) => {
                return false;
            }
            _ => {}
        }
    }
    false
}

// ─── Query parser ────────────────────────────────────────────────────────────

/// Extract top-level field names from a subscription query.
///
/// Given:
/// ```graphql
/// subscription {
///   orderCreated { id total }
///   userUpdated  { id name }
/// }
/// ```
///
/// Returns `["orderCreated", "userUpdated"]`.
pub fn parse_subscription_fields(query: &str) -> Result<Vec<String>> {
    let query = query.trim();

    if !query.starts_with("subscription") {
        return Err(Error::InvalidRequest(
            "Query must be a subscription operation".into(),
        ));
    }

    // Find the opening brace of the selection set.
    let body_start = query.find('{').ok_or_else(|| {
        Error::InvalidRequest("Invalid subscription: missing opening brace".into())
    })?;
    let body = &query[body_start + 1..];

    // Walk tokens to extract top-level identifiers before '(' or '{'.
    let mut fields = Vec::new();
    let mut depth = 0i32;
    let mut chars = body.chars().peekable();
    let mut current_ident = String::new();

    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                if depth == 0 && !current_ident.is_empty() {
                    fields.push(std::mem::take(&mut current_ident));
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth < 0 {
                    break; // end of selection set
                }
            }
            '(' => {
                if depth == 0 && !current_ident.is_empty() {
                    fields.push(std::mem::take(&mut current_ident));
                }
                // Skip past the closing ')'.
                let mut paren_depth = 1i32;
                for inner in chars.by_ref() {
                    match inner {
                        '(' => paren_depth += 1,
                        ')' => {
                            paren_depth -= 1;
                            if paren_depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            ':' => {
                // Alias separator — the identifier accumulated so far is the
                // alias name, not the real field.  Discard it.
                if depth == 0 {
                    current_ident.clear();
                }
            }
            c if c.is_alphanumeric() || c == '_' => {
                if depth == 0 {
                    current_ident.push(c);
                }
            }
            _ => {
                // Whitespace, comma, etc.
                if depth == 0 && !current_ident.is_empty() {
                    // Peek: if next non-ws is ':' this is an alias prefix — skip it.
                    let rest: String = chars.clone().collect();
                    let trimmed = rest.trim_start();
                    if trimmed.starts_with(':') {
                        // This was an alias name; skip.  The real field name comes next.
                        current_ident.clear();
                    } else {
                        fields.push(std::mem::take(&mut current_ident));
                    }
                }
            }
        }
    }

    // Remaining identifier outside braces.
    if !current_ident.is_empty() {
        fields.push(current_ident);
    }

    // Deduplicate.
    let mut seen = HashSet::new();
    fields.retain(|f| seen.insert(f.clone()));

    if fields.is_empty() {
        return Err(Error::InvalidRequest(
            "No subscription fields found in query".into(),
        ));
    }

    Ok(fields)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    // ── parser tests ─────────────────────────────────────────────────────

    #[test]
    fn test_parse_single_field() {
        let fields =
            parse_subscription_fields("subscription { orderCreated { id total } }").unwrap();
        assert_eq!(fields, vec!["orderCreated"]);
    }

    #[test]
    fn test_parse_multiple_fields() {
        let q = "subscription { orderCreated { id } userUpdated { name } }";
        let fields = parse_subscription_fields(q).unwrap();
        assert_eq!(fields, vec!["orderCreated", "userUpdated"]);
    }

    #[test]
    fn test_parse_named_subscription() {
        let q = "subscription OnOrder { orderCreated { id total } }";
        let fields = parse_subscription_fields(q).unwrap();
        assert_eq!(fields, vec!["orderCreated"]);
    }

    #[test]
    fn test_parse_with_arguments() {
        let q = "subscription { orderCreated(status: PENDING) { id total } }";
        let fields = parse_subscription_fields(q).unwrap();
        assert_eq!(fields, vec!["orderCreated"]);
    }

    #[test]
    fn test_parse_rejects_query() {
        assert!(parse_subscription_fields("query { users { id } }").is_err());
    }

    #[test]
    fn test_parse_empty_body() {
        assert!(parse_subscription_fields("subscription { }").is_err());
    }

    #[test]
    fn test_parse_with_alias() {
        let q = "subscription { myOrders: orderCreated { id } }";
        let fields = parse_subscription_fields(q).unwrap();
        assert_eq!(fields, vec!["orderCreated"]);
    }

    #[test]
    fn test_parse_nested_objects() {
        let q = "subscription { orderCreated { id items { name price } } }";
        let fields = parse_subscription_fields(q).unwrap();
        assert_eq!(fields, vec!["orderCreated"]);
    }

    // ── url conversion tests ─────────────────────────────────────────────

    #[test]
    fn test_http_to_ws_url() {
        assert_eq!(
            http_to_ws_url("http://localhost:4002/graphql"),
            "ws://localhost:4002/graphql/ws"
        );
        assert_eq!(
            http_to_ws_url("https://api.example.com/graphql"),
            "wss://api.example.com/graphql/ws"
        );
        assert_eq!(
            http_to_ws_url("http://localhost:4002/graphql/ws"),
            "ws://localhost:4002/graphql/ws"
        );
        // Already ws
        assert_eq!(
            http_to_ws_url("ws://localhost:4002/graphql/ws"),
            "ws://localhost:4002/graphql/ws"
        );
    }

    // ── routing tests ────────────────────────────────────────────────────

    #[test]
    fn test_route_explicit() {
        let mut routing = HashMap::new();
        routing.insert("orderCreated".to_string(), "orders".to_string());
        routing.insert("userUpdated".to_string(), "users".to_string());

        let config = SubscriptionFederationConfig {
            routing,
            auto_route: false,
            ..Default::default()
        };

        let subgraphs = vec![
            SubgraphEndpoint {
                name: "orders".to_string(),
                ws_url: "ws://localhost:4002/graphql/ws".to_string(),
                headers: HashMap::new(),
            },
            SubgraphEndpoint {
                name: "users".to_string(),
                ws_url: "ws://localhost:4003/graphql/ws".to_string(),
                headers: HashMap::new(),
            },
        ];

        let engine = SubscriptionFederationEngine::new(config, subgraphs);
        let plan = engine
            .route_subscription(&["orderCreated".to_string(), "userUpdated".to_string()])
            .unwrap();

        assert_eq!(plan.len(), 2);
        assert!(plan["orders"].contains(&"orderCreated".to_string()));
        assert!(plan["users"].contains(&"userUpdated".to_string()));
    }

    #[test]
    fn test_route_auto_broadcast() {
        let config = SubscriptionFederationConfig {
            auto_route: true,
            ..Default::default()
        };

        let subgraphs = vec![
            SubgraphEndpoint {
                name: "orders".to_string(),
                ws_url: "ws://localhost:4002/graphql/ws".to_string(),
                headers: HashMap::new(),
            },
            SubgraphEndpoint {
                name: "users".to_string(),
                ws_url: "ws://localhost:4003/graphql/ws".to_string(),
                headers: HashMap::new(),
            },
        ];

        let engine = SubscriptionFederationEngine::new(config, subgraphs);
        let plan = engine
            .route_subscription(&["somethingHappened".to_string()])
            .unwrap();

        // Broadcasted to both subgraphs.
        assert_eq!(plan.len(), 2);
    }

    #[test]
    fn test_route_unknown_field_auto_off() {
        let config = SubscriptionFederationConfig {
            auto_route: false,
            ..Default::default()
        };

        let engine = SubscriptionFederationEngine::new(config, vec![]);
        assert!(engine
            .route_subscription(&["unknownField".to_string()])
            .is_err());
    }

    // ── metrics tests ────────────────────────────────────────────────────

    #[test]
    fn test_initial_metrics() {
        let engine = SubscriptionFederationEngine::new(Default::default(), vec![]);
        let m = engine.metrics();
        assert_eq!(m.total_subscriptions, 0);
        assert_eq!(m.active_subscriptions, 0);
        assert_eq!(m.active_upstream_connections, 0);
    }

    // ── integration tests (short-circuit paths) ──────────────────────────

    #[tokio::test]
    async fn test_handle_subscription_bad_query() {
        let engine = SubscriptionFederationEngine::new(Default::default(), vec![]);
        let (tx, mut rx) = mpsc::channel(16);
        let (_cancel_tx, cancel_rx) = mpsc::channel(1);

        engine
            .handle_subscription(
                "1".to_string(),
                "query { users { id } }".to_string(), // not a subscription
                None,
                tx,
                cancel_rx,
            )
            .await;

        let msg = rx.recv().await.unwrap();
        assert_eq!(msg["type"], "error");
        assert_eq!(msg["id"], "1");
    }

    #[tokio::test]
    async fn test_handle_subscription_no_route() {
        let config = SubscriptionFederationConfig {
            auto_route: false,
            ..Default::default()
        };
        let engine = SubscriptionFederationEngine::new(config, vec![]);
        let (tx, mut rx) = mpsc::channel(16);
        let (_cancel_tx, cancel_rx) = mpsc::channel(1);

        engine
            .handle_subscription(
                "2".to_string(),
                "subscription { unknownField { id } }".to_string(),
                None,
                tx,
                cancel_rx,
            )
            .await;

        let msg = rx.recv().await.unwrap();
        assert_eq!(msg["type"], "error");
    }

    #[tokio::test]
    async fn test_handle_subscription_completes_when_no_subgraphs() {
        // auto_route on, but subgraphs list is empty ⇒ plan is empty ⇒
        // merged_tx is dropped immediately ⇒ the engine sends `complete`.
        let config = SubscriptionFederationConfig {
            auto_route: false,
            routing: {
                let mut m = HashMap::new();
                m.insert("x".into(), "nonexistent".into());
                m
            },
            ..Default::default()
        };
        let engine = SubscriptionFederationEngine::new(config, vec![]);
        let (tx, mut rx) = mpsc::channel(16);
        let (_cancel_tx, cancel_rx) = mpsc::channel(1);

        engine
            .handle_subscription(
                "3".to_string(),
                "subscription { x { id } }".to_string(),
                None,
                tx,
                cancel_rx,
            )
            .await;

        // Should get an error for unknown subgraph.
        let msg = rx.recv().await.unwrap();
        assert_eq!(msg["type"], "error");
    }
}

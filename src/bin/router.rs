//! GBP Router - Configuration Driven
//!
//! A high-performance GraphQL Federation Router with Live Query support.
//! Reads configuration from `router.yaml`.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, Json, State,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use grpc_graphql_gateway::{
    global_live_query_store, has_live_directive, strip_live_directive,
    live_query::{ActiveLiveQuery, LiveQueryStrategy, SharedLiveQueryStore},
    router::{DdosConfig, DdosProtection, GbpRouter, RouterConfig, SubgraphConfig},
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, RwLock};
use futures::stream::StreamExt;
use futures::SinkExt;

#[derive(Debug, Deserialize)]
struct YamlConfig {
    server: ServerConfig,
    #[allow(dead_code)]
    cors: CorsConfig,
    subgraphs: Vec<SubgraphConfig>,
    #[serde(default)]
    rate_limit: Option<DdosConfig>,
}

#[derive(Debug, Deserialize)]
struct ServerConfig {
    listen: String,
    #[serde(default = "default_workers")]
    workers: usize,
}

fn default_workers() -> usize {
    16
}

#[derive(Debug, Deserialize)]
struct CorsConfig {
    #[allow(dead_code)]
    allow_origins: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct GraphQlQuery {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    variables: Option<Value>,
    #[serde(default)]
    #[serde(rename = "operationName")]
    #[allow(dead_code)]
    operation_name: Option<String>,
    #[serde(default)]
    extensions: Option<Value>,
}

struct AppState {
    router: GbpRouter,
    ddos: DdosProtection,
    // Pre-allocated encoder for hot path
    encoder_pool: Arc<RwLock<Vec<grpc_graphql_gateway::gbp::GbpEncoder>>>,
    // Live query store for WebSocket subscriptions
    live_query_store: SharedLiveQueryStore,
}

impl AppState {
    fn new(router: GbpRouter, ddos: DdosProtection, pool_size: usize) -> Self {
        // Pre-allocate encoder pool
        let mut encoders = Vec::with_capacity(pool_size);
        for _ in 0..pool_size {
            encoders.push(grpc_graphql_gateway::gbp::GbpEncoder::new());
        }
        Self {
            router,
            ddos,
            encoder_pool: Arc::new(RwLock::new(encoders)),
            live_query_store: global_live_query_store(),
        }
    }
}

fn main() {
    // Determine config path from args or defaults
    let args: Vec<String> = std::env::args().collect();
    let config_path = if args.len() > 1 {
        args[1].clone()
    } else if std::path::Path::new("router.yaml").exists() {
        "router.yaml".to_string()
    } else if std::path::Path::new("examples/router.yaml").exists() {
        "examples/router.yaml".to_string()
    } else {
        "router.yaml".to_string()
    };

    println!("📝 Loading configuration from: {}", config_path);

    // We use a custom runtime builder to support configuration of worker threads
    // First, load config to see how many workers we need
    let config_content = std::fs::read_to_string(&config_path).unwrap_or_else(|_| {
        eprintln!("⚠️  Could not read {}, using defaults", config_path);
        r#"
server:
  listen: "0.0.0.0:4000"
  workers: 16
cors:
  allow_origins: ["*"]
subgraphs: []
        "#.to_string()
    });

    let config: YamlConfig = serde_yaml::from_str(&config_content).expect("Failed to parse router configuration");

    // Configure and start Tokio runtime
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(config.server.workers)
        .enable_all()
        .build()
        .unwrap()
        .block_on(async_main(config, config_path));
}

async fn async_main(yaml_config: YamlConfig, _config_path: String) {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .init();

    // Parse port from listen address
    let port = yaml_config.server.listen
        .split(':')
        .last()
        .and_then(|p| p.parse().ok())
        .unwrap_or(4000);

    // Convert YamlConfig to RouterConfig
    let router_config = RouterConfig {
        port,
        subgraphs: yaml_config.subgraphs.clone(),
        force_gbp: true, // Always enable GBP for this high-perf router
        apq: Some(grpc_graphql_gateway::persisted_queries::PersistedQueryConfig::default()),
        request_collapsing: Some(grpc_graphql_gateway::request_collapsing::RequestCollapsingConfig::default()),
    };

    // Setup DDoS protection
    let ddos_config = yaml_config.rate_limit.clone().unwrap_or(DdosConfig::relaxed());
    let ddos = DdosProtection::new(ddos_config.clone());

    let router = GbpRouter::new(router_config.clone());
    let state = Arc::new(AppState::new(router, ddos, 64));

    // Optimized Axum Router
    let app = Router::new()
        .route("/graphql", post(graphql_handler))
        .route("/graphql/live", get(live_query_ws_handler))
        .route("/health", get(health_handler))
        .with_state(state);

    println!();
    println!("  ╔═══════════════════════════════════════════════════════════╗");
    println!("  ║         GBP Router + Live Queries (Configured)            ║");
    println!("  ╠═══════════════════════════════════════════════════════════╣");
    println!("  ║  Listening:     {}                       ║", yaml_config.server.listen);
    println!("  ║  Workers:       {}                                       ║", yaml_config.server.workers);
    println!("  ║  Subgraphs:     {}                                       ║", yaml_config.subgraphs.len());
    for sg in &yaml_config.subgraphs {
        println!("  ║   - {:<12} {} ║", sg.name, sg.url);
    }
    println!("  ║  GBP Enabled:   ✅ (99% compression)                      ║");
    println!("  ║  DDoS Shield:   {} global RPS, {} per-IP            ║", ddos_config.global_rps, ddos_config.per_ip_rps);
    println!("  ╚═══════════════════════════════════════════════════════════╝");
    println!();

    let listener = TcpListener::bind(&yaml_config.server.listen).await.unwrap();

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}

#[inline(always)]
async fn graphql_handler(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<GraphQlQuery>,
) -> impl IntoResponse {
    // Extract client IP (fast path)
    let client_ip = headers
        .get("x-forwarded-for")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse::<IpAddr>().ok())
        .unwrap_or(addr.ip());

    // DDoS check (< 1µs for allowed requests)
    if !state.ddos.check(client_ip).await {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({
                "errors": [{"message": "Rate limit exceeded", "extensions": {"code": "RATE_LIMITED"}}]
            }))
        ).into_response();
    }

    let start = std::time::Instant::now();

    // Check for GBP client
    let accept_gbp = headers
        .get("accept")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.contains("application/x-gbp"))
        .unwrap_or(false);

    // Execute federated query
    // Execute federated query
    match state.router.execute_scatter_gather(
            payload.query.as_deref(), 
            payload.variables.as_ref(), 
            payload.extensions.as_ref()
    ).await {
        Ok(data) => {
            let duration = start.elapsed();

            let response_data = json!({
                "data": data,
                "extensions": {
                    "duration_ms": duration.as_secs_f64() * 1000.0,
                    "powered_by": "GBP Ultra - Configurable"
                }
            });

            if accept_gbp {
                // Use pooled encoder
                // We access the lock briefly to get/return encoder
                // Ideally this would be a thread-local or pure stack if inexpensive construction
                // But GbpEncoder reuses buffers, so pooling is good.
                
                // Note: In a real "Speed of Light" implementation, we might stick to thread-local
                // encoders to avoid this mutex entirely.
                // For now, minimal locking.
                let mut encoders = state.encoder_pool.write().await;
                let mut encoder = encoders.pop().unwrap_or_else(grpc_graphql_gateway::gbp::GbpEncoder::new);
                drop(encoders); // release lock ASAP

                let bytes = encoder.encode(&response_data);
                
                // Return encoder to pool
                let mut encoders = state.encoder_pool.write().await;
                if encoders.len() < 64 {
                    encoders.push(encoder);
                }
                drop(encoders);

                return (
                    [(axum::http::header::CONTENT_TYPE, "application/x-gbp")],
                    bytes,
                )
                    .into_response();
            }

            Json(response_data).into_response()
        }
        Err(e) => {
             // Handle APQ errors or standard errors
             let mut payload = json!({
                 "errors": [{"message": e.to_string()}]
             });
             
             // Check if it's an APQ error to add proper extensions
             if e.to_string() == "PersistedQueryNotFound" {
                 payload = json!({
                     "errors": [{
                         "message": "PersistedQueryNotFound",
                         "extensions": { "code": "PERSISTED_QUERY_NOT_FOUND" }
                     }]
                 });
             }
             
             Json(payload).into_response()
        },
    }
}

async fn health_handler() -> impl IntoResponse {
    Json(json!({
        "status": "healthy",
        "version": "1.0.0",
        "engine": "GBP Router + Live Queries"
    }))
}

/// WebSocket handler for live queries
/// 
/// Implements the graphql-transport-ws protocol with live query support
/// Supports the @live directive for real-time GraphQL subscriptions
async fn live_query_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.protocols(["graphql-transport-ws"])
        .on_upgrade(move |socket| handle_live_socket(socket, state))
}

/// Handle individual WebSocket connection for live queries
async fn handle_live_socket(socket: WebSocket, state: Arc<AppState>) {
    let (sender, mut receiver) = socket.split();
    let connection_id = uuid::Uuid::new_v4().to_string();
    
    tracing::info!(connection_id = %connection_id, "New live query WebSocket connection");

    // Create channels for this connection
    let (update_tx, mut update_rx) = mpsc::channel::<grpc_graphql_gateway::live_query::LiveQueryUpdate>(100);
    let (outgoing_tx, mut outgoing_rx) = mpsc::channel::<Message>(100);

    // Spawn task to forward all messages to client
    let connection_id_clone = connection_id.clone();
    tokio::spawn(async move {
        let mut sender = sender;
        while let Some(msg) = outgoing_rx.recv().await {
            if sender.send(msg).await.is_err() {
                tracing::warn!(connection_id = %connection_id_clone, "Failed to send message, client disconnected");
                break;
            }
        }
    });
    
    // Spawn task to forward live query updates
    let outgoing_tx_clone = outgoing_tx.clone();
    tokio::spawn(async move {
        while let Some(update) = update_rx.recv().await {
            let msg = json!({
                "type": "next",
                "id": update.id,
                "payload": {
                    "data": update.data,
                    "revision": update.revision,
                    "timestamp": update.timestamp,
                }
            });
            
            if let Ok(text) = serde_json::to_string(&msg) {
                let _ = outgoing_tx_clone.send(Message::Text(text.into())).await;
            }
        }
    });

    // Handle messages from client
    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                if let Ok(msg_value) = serde_json::from_str::<Value>(&text) {
                    let msg_type = msg_value["type"].as_str().unwrap_or("");
                    
                    match msg_type {
                        "connection_init" => {
                            // Acknowledge connection
                            let ack = json!({
                                "type": "connection_ack",
                                "payload": {
                                    "live_queries": "enabled",
                                    "compression": "gbp"
                                }
                            });
                            
                            if let Ok(text) = serde_json::to_string(&ack) {
                                let _ = outgoing_tx.send(Message::Text(text.into())).await;
                                tracing::debug!("Connection acknowledged");
                            }
                        }
                        
                        "subscribe" => {
                            let id = msg_value["id"].as_str().unwrap_or("unknown").to_string();
                            let payload = &msg_value["payload"];
                            let query = payload["query"].as_str();
                            let variables = payload.get("variables");
                            
                            if let Some(query_str) = query {
                                tokio::spawn(handle_live_query_subscription(
                                    state.clone(),
                                    connection_id.clone(),
                                    id,
                                    query_str.to_string(),
                                    variables.cloned(),
                                    update_tx.clone(),
                                ));
                            }
                        }
                        
                        "complete" => {
                            let id = msg_value["id"].as_str().unwrap_or("unknown");
                            state.live_query_store.unregister(id);
                            tracing::debug!(subscription_id = %id, "Live query completed by client");
                        }
                        
                        "ping" => {
                            let pong = json!({"type": "pong"});
                            if let Ok(text) = serde_json::to_string(&pong) {
                                let _ = outgoing_tx.send(Message::Text(text.into())).await;
                            }
                        }
                        
                        _ => {
                            tracing::debug!(msg_type = %msg_type, "Unhandled WebSocket message type");
                        }
                    }
                }
            }
            
            Ok(Message::Close(_)) => {
                tracing::info!(connection_id = %connection_id, "Client closed WebSocket connection");
                break;
            }
            
            Err(e) => {
                tracing::error!(error = %e, "WebSocket error");
                break;
            }
            
            _ => {}
        }
    }

    // Cleanup: unregister all queries for this connection
    state.live_query_store.unregister_connection(&connection_id);
    tracing::info!(connection_id = %connection_id, "WebSocket connection closed");
}

/// Handle a single live query subscription
async fn handle_live_query_subscription(
    state: Arc<AppState>,
    connection_id: String,
    subscription_id: String,
    query: String,
    variables: Option<Value>,
    update_tx: mpsc::Sender<grpc_graphql_gateway::live_query::LiveQueryUpdate>,
) {
    // Check for @live directive
    if !has_live_directive(&query) {
        tracing::warn!("Query does not have @live directive");
        return;
    }

    // Strip @live directive for execution
    let clean_query = strip_live_directive(&query);
    
    // Execute the initial query through the router
    let initial_result = state.router.execute_scatter_gather(
        Some(&clean_query),
        variables.as_ref(),
        None,
    ).await;

    match initial_result {
        Ok(data) => {
            // Register this as a live query
            let live_query = ActiveLiveQuery {
                id: subscription_id.clone(),
                operation_name: "federated".to_string(),
                query: clean_query.clone(),
                variables: variables.clone(),
                triggers: vec!["*.*".to_string()], // Match all changes
                throttle_ms: 100,
                ttl_seconds: 0,
                strategy: LiveQueryStrategy::Invalidation,
                poll_interval_ms: 0,
                last_hash: None,
                last_update: Instant::now(),
                created_at: Instant::now(),
                connection_id: connection_id.clone(),
            };

            if let Err(e) = state.live_query_store.register(live_query, update_tx.clone()) {
                tracing::error!(error = %e, "Failed to register live query");
                return;
            }

            // Send initial result
            let initial_update = grpc_graphql_gateway::live_query::LiveQueryUpdate {
                id: subscription_id.clone(),
                data,
                is_initial: true,
                revision: 0,
                cache_control: None,
                changed_fields: None,
                batched: None,
                timestamp: Some(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                ),
            };

            if let Err(e) = update_tx.send(initial_update).await {
                tracing::error!(error = %e, "Failed to send initial update");
                state.live_query_store.unregister(&subscription_id);
                return;
            }

            tracing::info!(
                subscription_id = %subscription_id,
                connection_id = %connection_id,
                "Live query registered"
            );

            // Listen for invalidation events
            let mut invalidation_rx = state.live_query_store.subscribe_invalidations();
            
            loop {
                match invalidation_rx.recv().await {
                    Ok(event) => {
                        // Check if this subscription is affected
                        if let Some(query_info) = state.live_query_store.get(&subscription_id) {
                            if query_info.should_update(&event) && query_info.throttle_elapsed() {
                                // Re-execute the query
                                match state.router.execute_scatter_gather(
                                    Some(&clean_query),
                                    variables.as_ref(),
                                    None,
                                ).await {
                                    Ok(updated_data) => {
                                        if let Err(e) = state.live_query_store.send_update(
                                            &subscription_id,
                                            updated_data,
                                            false,
                                        ).await {
                                            tracing::error!(error = %e, "Failed to send update");
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        tracing::error!(error = %e, "Failed to re-execute query");
                                    }
                                }
                            }
                        } else {
                            // Query was unregistered
                            break;
                        }
                    }
                    Err(_) => {
                        // Channel closed
                        break;
                    }
                }
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "Initial query execution failed");
        }
    }
}

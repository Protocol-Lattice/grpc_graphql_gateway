//! GBP Router - Configuration Driven
//!
//! A high-performance GraphQL Federation Router.
//! Reads configuration from `router.yaml`.

use axum::{
    extract::{ConnectInfo, Json, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use grpc_graphql_gateway::router::{DdosConfig, DdosProtection, GbpRouter, RouterConfig, SubgraphConfig};
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::RwLock;

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
        .route("/health", get(health_handler))
        .with_state(state);

    println!();
    println!("  ╔═══════════════════════════════════════════════════════════╗");
    println!("  ║             GBP Router (Configured)                       ║");
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
        "engine": "GBP Router"
    }))
}

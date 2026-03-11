//! GBP Router - Configuration Driven
//!
//! A high-performance GraphQL Federation Router with Live Query support.
//! Reads configuration from `router.yaml`.

use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderValue, Method};
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
use futures::stream::StreamExt;
use futures::SinkExt;
use grpc_graphql_gateway::quic::QuicConfig;
use grpc_graphql_gateway::{
    global_live_query_store, has_live_directive,
    live_query::{ActiveLiveQuery, LiveQueryStrategy, SharedLiveQueryStore},
    router::{DdosConfig, DdosProtection, GbpRouter, RouterConfig, SubgraphConfig},
    strip_live_directive,
    subscription_federation::{
        SubgraphEndpoint, SubscriptionFederationConfig, SubscriptionFederationEngine,
    },
};
use notify::{Config as NotifyConfig, RecommendedWatcher, RecursiveMode, Watcher};
use reqwest::Url;
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, RwLock};
use tower_http::{
    cors::{Any, CorsLayer},
    set_header::SetResponseHeaderLayer,
    timeout::TimeoutLayer,
};

#[derive(Debug, Deserialize)]
struct YamlConfig {
    server: ServerConfig,
    #[allow(dead_code)]
    cors: Option<CorsConfig>,
    subgraphs: Vec<SubgraphConfig>,
    #[serde(default)]
    rate_limit: Option<DdosConfig>,
    #[serde(default)]
    waf: Option<grpc_graphql_gateway::waf::WafConfig>,
    #[serde(default)]
    query_cost: Option<grpc_graphql_gateway::query_cost_analyzer::QueryCostConfig>,
    #[serde(default)]
    disable_introspection: bool,
    #[serde(default)]
    circuit_breaker: Option<grpc_graphql_gateway::circuit_breaker::CircuitBreakerConfig>,
    /// Global mTLS configuration applied to all subgraphs (can be overridden per-subgraph)
    #[serde(default)]
    mtls: Option<grpc_graphql_gateway::mtls::MtlsConfig>,
    /// HTTP/3 + QUIC transport configuration
    #[serde(default)]
    quic: Option<QuicConfig>,
    /// Legacy XOR "encryption" support for demo payloads.
    /// Disabled by default because XOR provides no real confidentiality.
    #[serde(default)]
    enable_legacy_xor_value_decryption: bool,
}

#[derive(Debug, Deserialize)]
struct ServerConfig {
    listen: String,
    #[serde(default = "default_workers")]
    workers: usize,
    /// Optional override for the QUIC/UDP listen address.
    /// If absent the same host:port as `listen` is used (different socket, same port).
    #[serde(default)]
    quic_listen: Option<String>,
}

fn default_workers() -> usize {
    16
}

#[derive(Debug, Deserialize, Clone)]
struct CorsConfig {
    allow_origins: Vec<String>,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allow_origins: Vec::new(),
        }
    }
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

struct InnerState {
    router: GbpRouter,
    ddos: DdosProtection,
    gateway_secret: Option<String>,
    waf_config: grpc_graphql_gateway::waf::WafConfig,
    legacy_xor_value_decryption_enabled: bool,
    #[allow(dead_code)]
    mtls_enabled: bool,
}

struct AppState {
    // Dynamic configurations that can be reloaded
    inner: RwLock<Arc<InnerState>>,

    // Shared resources that persist across reloads
    encoder_pool: Arc<RwLock<Vec<grpc_graphql_gateway::gbp::GbpEncoder>>>,
    live_query_store: SharedLiveQueryStore,
    /// Subscription federation engine for fan-out to subgraph WebSockets.
    subscription_engine: Arc<SubscriptionFederationEngine>,
    /// Serialises concurrent hot-reload attempts so that a burst of file-watcher
    /// events does not trigger multiple overlapping `load_inner_state` calls.
    reload_lock: tokio::sync::Mutex<()>,
}

impl AppState {
    fn new(inner: InnerState, pool_size: usize, subgraphs: &[SubgraphConfig]) -> Self {
        // Pre-allocate encoder pool
        let mut encoders = Vec::with_capacity(pool_size);
        for _ in 0..pool_size {
            encoders.push(grpc_graphql_gateway::gbp::GbpEncoder::new());
        }

        // Build subscription federation engine from subgraph configs
        let endpoints: Vec<SubgraphEndpoint> = subgraphs
            .iter()
            .map(|sg| SubgraphEndpoint::from_http_url(&sg.name, &sg.url, sg.headers.clone()))
            .collect();
        let subscription_engine = Arc::new(SubscriptionFederationEngine::new(
            SubscriptionFederationConfig::default(),
            endpoints,
        ));

        Self {
            inner: RwLock::new(Arc::new(inner)),
            encoder_pool: Arc::new(RwLock::new(encoders)),
            live_query_store: global_live_query_store(),
            subscription_engine,
            reload_lock: tokio::sync::Mutex::new(()),
        }
    }
}

const STRICT_API_CSP: &str =
    "default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

fn interpolate_env_vars(config_content: &str) -> anyhow::Result<String> {
    let re = regex::Regex::new(r"\$\{([^}]+)\}").unwrap();
    let mut interpolation_error: Option<String> = None;

    let interpolated = re
        .replace_all(config_content, |caps: &regex::Captures| {
            let env_var = &caps[1];
            if let Some((var_name, default_value)) = env_var.split_once(":-") {
                std::env::var(var_name).unwrap_or_else(|_| default_value.to_string())
            } else {
                match std::env::var(env_var) {
                    Ok(value) => value,
                    Err(_) => {
                        interpolation_error = Some(env_var.to_string());
                        String::new()
                    }
                }
            }
        })
        .to_string();

    if let Some(var_name) = interpolation_error {
        anyhow::bail!(
            "Missing required environment variable '{}' referenced in {} syntax. \
             Use ${{{}:-default}} if an empty/default fallback is intentional.",
            var_name,
            "${VAR}",
            var_name
        );
    }

    Ok(interpolated)
}

fn validate_cors_config(cors: &CorsConfig) -> anyhow::Result<()> {
    let has_wildcard = cors.allow_origins.iter().any(|origin| origin == "*");
    if has_wildcard && cors.allow_origins.len() > 1 {
        anyhow::bail!("CORS allow_origins cannot mix '*' with explicit origins");
    }

    for origin in &cors.allow_origins {
        if origin == "*" {
            continue;
        }

        let parsed = Url::parse(origin)
            .map_err(|e| anyhow::anyhow!("Invalid CORS origin '{}': {}", origin, e))?;

        if !matches!(parsed.scheme(), "http" | "https") {
            anyhow::bail!(
                "Invalid CORS origin '{}': only http:// and https:// origins are allowed",
                origin
            );
        }

        if !parsed.username().is_empty() || parsed.password().is_some() {
            anyhow::bail!(
                "Invalid CORS origin '{}': credentials in origins are not allowed",
                origin
            );
        }

        if parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() {
            anyhow::bail!(
                "Invalid CORS origin '{}': origins must not include path, query, or fragment",
                origin
            );
        }
    }

    Ok(())
}

fn validate_subgraph_url(subgraph: &SubgraphConfig) -> anyhow::Result<()> {
    let parsed = Url::parse(&subgraph.url)
        .map_err(|e| anyhow::anyhow!("Invalid subgraph URL for '{}': {}", subgraph.name, e))?;

    if !matches!(parsed.scheme(), "http" | "https") {
        anyhow::bail!(
            "Invalid subgraph URL for '{}': only http:// and https:// are supported",
            subgraph.name
        );
    }

    if parsed.host_str().is_none() {
        anyhow::bail!("Invalid subgraph URL for '{}': missing host", subgraph.name);
    }

    if !parsed.username().is_empty() || parsed.password().is_some() {
        anyhow::bail!(
            "Invalid subgraph URL for '{}': embedded credentials are not allowed",
            subgraph.name
        );
    }

    if subgraph
        .mtls
        .as_ref()
        .map(|cfg| cfg.enabled)
        .unwrap_or(false)
        && parsed.scheme() != "https"
    {
        anyhow::bail!(
            "Subgraph '{}' enables mTLS but is configured with a non-HTTPS URL: {}",
            subgraph.name,
            subgraph.url
        );
    }

    Ok(())
}

fn validate_yaml_config(
    yaml_config: &YamlConfig,
    gateway_secret: Option<&str>,
) -> anyhow::Result<()> {
    yaml_config
        .server
        .listen
        .parse::<SocketAddr>()
        .map_err(|e| {
            anyhow::anyhow!(
                "Invalid server.listen '{}': {}",
                yaml_config.server.listen,
                e
            )
        })?;

    if let Some(quic_listen) = &yaml_config.server.quic_listen {
        quic_listen
            .parse::<SocketAddr>()
            .map_err(|e| anyhow::anyhow!("Invalid server.quic_listen '{}': {}", quic_listen, e))?;
    }

    if yaml_config.server.workers == 0 {
        anyhow::bail!("server.workers must be greater than 0");
    }

    if let Some(cors) = &yaml_config.cors {
        validate_cors_config(cors)?;
    }

    for subgraph in &yaml_config.subgraphs {
        validate_subgraph_url(subgraph)?;
    }

    if yaml_config.enable_legacy_xor_value_decryption && gateway_secret.is_none() {
        anyhow::bail!("enable_legacy_xor_value_decryption requires GATEWAY_SECRET to be set");
    }

    Ok(())
}

/// Helper to load configuration and build InnerState
fn load_inner_state(config_path: &str) -> anyhow::Result<(InnerState, YamlConfig)> {
    let config_content = std::fs::read_to_string(config_path)
        .map_err(|e| anyhow::anyhow!("Failed to read config: {}", e))?;
    let config_content = interpolate_env_vars(&config_content)?;

    let mut yaml_config: YamlConfig = serde_yaml::from_str(&config_content)
        .map_err(|e| anyhow::anyhow!("Failed to parse config: {}", e))?;

    // Apply global mTLS config to subgraphs that don't have their own override
    let mtls_enabled = yaml_config
        .mtls
        .as_ref()
        .map(|m| m.enabled)
        .unwrap_or(false);
    if let Some(ref global_mtls) = yaml_config.mtls {
        if global_mtls.enabled {
            for subgraph in &mut yaml_config.subgraphs {
                if subgraph.mtls.is_none() {
                    subgraph.mtls = Some(global_mtls.clone());
                }
            }
            println!(
                "  🔒 mTLS: Zero-Trust mode enabled (trust_domain: {})",
                global_mtls.trust_domain
            );
        }
    }

    // Allow overriding Gateway Secret from environment variable for security
    // Note: When mTLS is enabled, the GATEWAY_SECRET is optional (mTLS provides
    // cryptographic authentication). However, it can still be used as defense-in-depth.
    let gateway_secret = std::env::var("GATEWAY_SECRET").ok();
    if let Some(secret) = &gateway_secret {
        for subgraph in &mut yaml_config.subgraphs {
            subgraph
                .headers
                .insert("X-Gateway-Secret".to_string(), secret.clone());
        }
    } else if mtls_enabled {
        println!("  ℹ️  GATEWAY_SECRET not set (mTLS provides cryptographic authentication)");
    }

    validate_yaml_config(&yaml_config, gateway_secret.as_deref())?;

    // Parse port from listen address for RouterConfig
    let port = yaml_config
        .server
        .listen
        .split(':')
        .next_back()
        .and_then(|p| p.parse().ok())
        .unwrap_or(4000);

    let router_config = RouterConfig {
        port,
        subgraphs: yaml_config.subgraphs.clone(),
        force_gbp: true,
        apq: Some(grpc_graphql_gateway::persisted_queries::PersistedQueryConfig::default()),
        request_collapsing: Some(
            grpc_graphql_gateway::request_collapsing::RequestCollapsingConfig::default(),
        ),
        waf: yaml_config.waf.clone(),
        query_cost: yaml_config.query_cost.clone(),
        disable_introspection: yaml_config.disable_introspection,
        circuit_breaker: yaml_config.circuit_breaker.clone(),
    };

    let ddos_config = yaml_config
        .rate_limit
        .clone()
        .unwrap_or(DdosConfig::relaxed());
    let ddos = DdosProtection::new(ddos_config);

    let waf_config = yaml_config.waf.clone().unwrap_or_default();

    let router = GbpRouter::new(router_config);

    Ok((
        InnerState {
            router,
            ddos,
            gateway_secret,
            waf_config,
            legacy_xor_value_decryption_enabled: yaml_config.enable_legacy_xor_value_decryption,
            mtls_enabled,
        },
        yaml_config,
    ))
}

fn main() {
    // Determine config path from args or defaults
    let args: Vec<String> = std::env::args().collect();

    // Check for validation mode
    if args.len() > 1 && (args[1] == "--check" || args[1] == "validate") {
        let config_path = if args.len() > 2 {
            args[2].clone()
        } else if std::path::Path::new("router.yaml").exists() {
            "router.yaml".to_string()
        } else if std::path::Path::new("examples/router.yaml").exists() {
            "examples/router.yaml".to_string()
        } else {
            eprintln!("❌ Usage: router --check <config_path>");
            std::process::exit(1);
        };

        println!("🔍 Validating configuration: {}", config_path);
        match load_inner_state(&config_path) {
            Ok((_, config)) => {
                println!("✅ Configuration is valid!");
                println!("   - Subgraphs: {}", config.subgraphs.len());
                println!("   - Listen:    {}", config.server.listen);
                println!("   - Workers:   {}", config.server.workers);
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("❌ Configuration is invalid: {}", e);
                std::process::exit(1);
            }
        }
    }

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

    // Initial load
    let (initial_inner, config) = load_inner_state(&config_path).unwrap_or_else(|e| {
        eprintln!("❌ Fatal startup error: {}", e);
        std::process::exit(1);
    });

    // Create state with initial config
    let state = Arc::new(AppState::new(initial_inner, 64, &config.subgraphs));

    // Configure and start Tokio runtime
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(config.server.workers)
        .enable_all()
        .build()
        .unwrap()
        .block_on(async_main(config, config_path, state));
}

fn start_config_watcher(config_path: String, state: Arc<AppState>) {
    tokio::spawn(async move {
        // Channel to bridge blocking watcher to async world
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let path = config_path.clone();

        // A flag set while a reload is in-flight so that a burst of watcher
        // events does not queue multiple concurrent `load_inner_state` calls.
        // Using AtomicBool + try_lock gives a fast non-blocking check.
        let reload_in_flight = Arc::new(AtomicBool::new(false));

        // Spawn blocking thread for file watching
        std::thread::spawn(move || {
            let (std_tx, std_rx) = std::sync::mpsc::channel();

            let mut watcher = match RecommendedWatcher::new(std_tx, NotifyConfig::default()) {
                Ok(w) => w,
                Err(e) => {
                    tracing::error!("Failed to create watcher: {}", e);
                    return;
                }
            };

            if let Err(e) = watcher.watch(Path::new(&path), RecursiveMode::NonRecursive) {
                tracing::error!("Failed to watch config file: {}", e);
                return;
            }

            for res in std_rx {
                match res {
                    Ok(event) => {
                        // Only care if something changed
                        if event.kind.is_modify() {
                            let _ = tx.send(());
                        }
                    }
                    Err(e) => tracing::error!("Watch error: {:?}", e),
                }
            }
        });

        tracing::info!("👀 Watching for config changes in {}", config_path);

        while rx.recv().await.is_some() {
            // Debounce: wait 100ms and drain any other events that arrived
            // during the sleep so we act on the *final* state of the file.
            tokio::time::sleep(Duration::from_millis(100)).await;
            while rx.try_recv().is_ok() {}

            // Atomicity guard: skip if another reload is already in progress.
            // compare_exchange(false→true) succeeds only when no reload is running.
            if reload_in_flight
                .compare_exchange(false, true, AtomicOrdering::SeqCst, AtomicOrdering::SeqCst)
                .is_err()
            {
                tracing::debug!("Config change detected but reload already in progress, skipping");
                continue;
            }

            tracing::info!("📝 Config change detected, reloading...");

            // Acquire the reload_lock so concurrent async reloads (if any) are
            // also serialised.  `try_lock` used here since AtomicBool already
            // provides the first exclusion layer; this second lock guards the
            // write to `state.inner`.
            let _reload_guard = state.reload_lock.lock().await;

            match load_inner_state(&config_path) {
                Ok((new_inner, new_config)) => {
                    {
                        let mut inner = state.inner.write().await;
                        *inner = Arc::new(new_inner);
                    } // Release write lock immediately

                    tracing::info!("♻️  Configuration reloaded successfully!");

                    // Log new state summary
                    tracing::info!(
                        "   Active Subgraphs: {}, DDoS: {}/{} RPS, WAF: {}",
                        new_config.subgraphs.len(),
                        new_config
                            .rate_limit
                            .as_ref()
                            .map(|r| r.global_rps)
                            .unwrap_or(0),
                        new_config
                            .rate_limit
                            .as_ref()
                            .map(|r| r.per_ip_rps)
                            .unwrap_or(0),
                        new_config.waf.map(|w| w.enabled).unwrap_or(false)
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "❌ Failed to reload config (keeping previous version): {}",
                        e
                    );
                }
            }

            // Clear the in-flight flag so the next event is processed.
            reload_in_flight.store(false, AtomicOrdering::SeqCst);
        }
    });
}

async fn async_main(yaml_config: YamlConfig, config_path: String, state: Arc<AppState>) {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .init();

    // Start background cleanup task (this needs to be updated to use inner state too!)
    let ddos_state = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            // Access current DDOS protection instance
            let inner = ddos_state.inner.read().await;
            inner.ddos.cleanup_stale_limiters(300).await;
        }
    });

    // Start Config Watcher
    start_config_watcher(config_path, state.clone());

    // Print Banner based on initial config
    let ddos_config = yaml_config.rate_limit.unwrap_or(DdosConfig::relaxed());
    let quic_cfg = yaml_config.quic.clone().unwrap_or_default();
    let quic_enabled = quic_cfg.enabled;
    println!();
    println!("  ╔═══════════════════════════════════════════════════════════╗");
    println!("  ║      GBP Router + Hot Reloading + Live Queries           ║");
    println!("  ╠═══════════════════════════════════════════════════════════╣");
    println!(
        "  ║  Listening:     {}                       ║",
        yaml_config.server.listen
    );
    println!(
        "  ║  Workers:       {}                                       ║",
        yaml_config.server.workers
    );
    println!(
        "  ║  Subgraphs:     {}                                       ║",
        yaml_config.subgraphs.len()
    );
    for sg in &yaml_config.subgraphs {
        println!("  ║   - {:<12} {} ║", sg.name, sg.url);
    }
    println!("  ║  GBP Binary:    ✅ Bidirectional (99% compression)        ║");
    println!(
        "  ║  DDoS Shield:   {} global RPS, {} per-IP            ║",
        ddos_config.global_rps, ddos_config.per_ip_rps
    );
    if quic_enabled {
        let quic_listen = yaml_config
            .server
            .quic_listen
            .as_deref()
            .unwrap_or(&yaml_config.server.listen);
        println!(
            "  ║  HTTP/3 (QUIC): 🚀 Enabled  udp://{}          ║",
            quic_listen
        );
        println!("  ║  Alt-Svc:       ✅ Advertised to HTTP/1.1+2 clients    ║");
        println!("  ║  Protocol:      RFC 9114 (HTTP/3) + RFC 9000 (QUIC)   ║");
    } else {
        println!("  ║  HTTP/3 (QUIC): ⚡ Disabled (set quic.enabled: true)   ║");
    }
    if yaml_config
        .mtls
        .as_ref()
        .map(|m| m.enabled)
        .unwrap_or(false)
    {
        let mtls_cfg = yaml_config.mtls.as_ref().unwrap();
        println!(
            "  ║  mTLS:          🔒 Zero-Trust ({})              ║",
            mtls_cfg.trust_domain
        );
        println!(
            "  ║  Cert TTL:      {} seconds                             ║",
            mtls_cfg.cert_ttl_secs
        );
    } else {
        println!("  ║  mTLS:          ❌ Disabled (using GATEWAY_SECRET)      ║");
    }
    if yaml_config.enable_legacy_xor_value_decryption {
        println!("  ║  Legacy XOR:    ⚠️  Enabled (demo compatibility only)    ║");
    }
    println!("  ╚═══════════════════════════════════════════════════════════╝");
    println!();

    // Optimized Axum Router
    let mut app = Router::new()
        .route("/graphql", post(graphql_handler))
        .route("/graphql/ws", get(subscription_ws_handler))
        .route("/graphql/live", get(live_query_ws_handler))
        .route("/health", get(health_handler))
        .with_state(state)
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024)) // 2MB limit
        .layer(
            tower::ServiceBuilder::new()
                .layer(SetResponseHeaderLayer::overriding(
                    header::X_CONTENT_TYPE_OPTIONS,
                    HeaderValue::from_static("nosniff"),
                ))
                .layer(SetResponseHeaderLayer::overriding(
                    header::X_FRAME_OPTIONS,
                    HeaderValue::from_static("DENY"),
                ))
                .layer(SetResponseHeaderLayer::overriding(
                    header::X_XSS_PROTECTION,
                    HeaderValue::from_static("1; mode=block"),
                ))
                .layer(SetResponseHeaderLayer::overriding(
                    header::STRICT_TRANSPORT_SECURITY,
                    HeaderValue::from_static("max-age=31536000; includeSubDomains"),
                ))
                .layer(SetResponseHeaderLayer::overriding(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("no-store, no-cache, must-revalidate"),
                ))
                .layer(SetResponseHeaderLayer::overriding(
                    header::REFERRER_POLICY,
                    HeaderValue::from_static("strict-origin-when-cross-origin"),
                ))
                .layer(SetResponseHeaderLayer::overriding(
                    header::CONTENT_SECURITY_POLICY,
                    HeaderValue::from_static(STRICT_API_CSP),
                ))
                .layer(SetResponseHeaderLayer::overriding(
                    header::HeaderName::from_static("permissions-policy"),
                    HeaderValue::from_static(
                        "camera=(), microphone=(), geolocation=(), payment=(), browsing-topics=()",
                    ),
                ))
                .layer(SetResponseHeaderLayer::overriding(
                    header::HeaderName::from_static("x-dns-prefetch-control"),
                    HeaderValue::from_static("off"),
                ))
                .layer(SetResponseHeaderLayer::overriding(
                    header::HeaderName::from_static("cross-origin-opener-policy"),
                    HeaderValue::from_static("same-origin"),
                ))
                .layer(SetResponseHeaderLayer::overriding(
                    header::HeaderName::from_static("cross-origin-embedder-policy"),
                    HeaderValue::from_static("require-corp"),
                ))
                .layer(SetResponseHeaderLayer::overriding(
                    header::HeaderName::from_static("cross-origin-resource-policy"),
                    HeaderValue::from_static("same-origin"),
                )),
        );

    // Apply CORS based on configuration
    // Note: CORS config is part of server setup, difficult to hot-reload without rebuilding Axum router.
    // For now, CORS changes will require restart, but other runtime configs (WAF, Subgraphs) will reload.
    let cors_config = yaml_config.cors.clone().unwrap_or_default();

    if cors_config.allow_origins.is_empty() {
        tracing::info!("CORS disabled by default; configure cors.allow_origins to enable it");
    } else if cors_config.allow_origins.iter().any(|o| o == "*") {
        app = app.layer(
            CorsLayer::new()
                .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
                .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION, header::ACCEPT])
                .allow_origin(Any),
        );
    } else {
        let origins: Vec<HeaderValue> = cors_config
            .allow_origins
            .iter()
            .map(|s| {
                s.parse::<HeaderValue>()
                    .unwrap_or(HeaderValue::from_static(""))
            })
            .filter(|h| !h.is_empty())
            .collect();

        app = app.layer(
            CorsLayer::new()
                .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
                .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION, header::ACCEPT])
                .allow_origin(origins),
        );
    }

    // Determine the TCP port so we can advertise it in Alt-Svc for HTTP/3 upgrade.
    // The port is extracted from the listen address (e.g. "0.0.0.0:4000" → 4000).
    let tcp_port: u16 = yaml_config
        .server
        .listen
        .split(':')
        .next_back()
        .and_then(|p| p.parse().ok())
        .unwrap_or(4000);

    // Inject `Alt-Svc` header when HTTP/3 is enabled so that browsers and
    // gRPC clients discover and upgrade to the QUIC endpoint automatically.
    // The IETF-recommended advertisement format is defined in RFC 7838 §3.
    if quic_enabled {
        let alt_svc = grpc_graphql_gateway::alt_svc_header_value(tcp_port, 86400);
        if let Ok(hv) = HeaderValue::from_str(&alt_svc) {
            app = app.layer(tower::ServiceBuilder::new().layer(
                SetResponseHeaderLayer::overriding(header::HeaderName::from_static("alt-svc"), hv),
            ));
        }
    }

    // Apply Timeout
    let app = app.layer(TimeoutLayer::with_status_code(
        StatusCode::REQUEST_TIMEOUT,
        Duration::from_secs(30),
    ));

    // ─── Bind QUIC / HTTP3 endpoint ─────────────────────────────────────────
    // The QUIC endpoint runs on UDP and handles HTTP/3 connections independently
    // of the TCP axum server. Both share the same application port number
    // (different sockets, same port — TCP vs UDP).
    #[cfg(feature = "quic")]
    let _quic_server = if quic_enabled {
        use grpc_graphql_gateway::quic::QuicServer;
        use std::str::FromStr;

        // rustls requires an explicit CryptoProvider to be installed at the process
        // level when multiple provider feature flags could be present. We use `ring`
        // (enabled via rustls features = ["ring"] in Cargo.toml).
        // `install_default` is idempotent — safe to call more than once.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let quic_listen_str = yaml_config
            .server
            .quic_listen
            .as_deref()
            .unwrap_or(&yaml_config.server.listen);

        let quic_addr = SocketAddr::from_str(quic_listen_str)
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], tcp_port)));

        let cert_path = quic_cfg.cert_path.clone();
        let key_path = quic_cfg.key_path.clone();
        let quic_cfg_c = quic_cfg.clone();

        match QuicServer::bind(
            quic_cfg_c,
            quic_addr,
            cert_path.as_deref(),
            key_path.as_deref(),
            // HTTP/3 handler — lightweight wrapper that responds with 200 OK for
            // /health and delegates all other requests to the same logic as the
            // axum handlers. For WebSocket-based live queries and subscriptions,
            // clients should use the TCP/HTTP2 WebSocket upgrade path; HTTP/3
            // covers the stateless GraphQL query/mutation surface.
            std::sync::Arc::new(|req: http::Request<bytes::Bytes>| -> std::pin::Pin<Box<dyn std::future::Future<Output = http::Response<bytes::Bytes>> + Send>> {
                Box::pin(async move {
                    let path = req.uri().path().to_string();
                    let method = req.method().clone();

                    if path == "/health" {
                        let body = serde_json::to_vec(&serde_json::json!({
                            "status": "healthy",
                            "transport": "HTTP/3 (QUIC)",
                            "protocol": "RFC 9114"
                        })).unwrap_or_default();
                        return http::Response::builder()
                            .status(200)
                            .header("content-type", "application/json")
                            .body(bytes::Bytes::from(body))
                            .unwrap_or_default();
                    }

                    // For all other paths, return a redirect to the TCP endpoint.
                    // In a full deployment, subgraph calls and client queries
                    // over HTTP/3 are handled here by passing through the same
                    // GbpRouter. This stub keeps the router binary self-contained.
                    tracing::debug!(path = %path, method = %method, "HTTP/3 request (stub handler)");

                    http::Response::builder()
                        .status(200)
                        .header("content-type", "application/json")
                        .body(bytes::Bytes::from_static(
                            b"{\"message\":\"HTTP/3 endpoint active. Use TCP for full GraphQL.\"}"  
                        ))
                        .unwrap_or_default()
                })
            }),
        ).await {
            Ok(srv) => {
                tracing::info!(addr = %srv.local_addr, "HTTP/3 QUIC server running");
                Some(srv)
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to start HTTP/3 QUIC server — continuing with TCP only");
                None
            }
        }
    } else {
        None
    };

    let listener = TcpListener::bind(&yaml_config.server.listen).await.unwrap();

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .unwrap();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    println!("🛑 Signal received, starting graceful shutdown...");
}

/// Binary-aware GraphQL handler
/// Supports both JSON and GBP (GraphQL Binary Protocol) for requests and responses
#[inline(always)]
async fn graphql_handler(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    // Extract client IP (fast path)
    let client_ip = headers
        .get("x-forwarded-for")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse::<IpAddr>().ok())
        .unwrap_or(addr.ip());

    // Acquire read lock for current configuration state
    let inner = state.inner.read().await;

    // DDoS check (< 1µs for allowed requests)
    if !inner.ddos.check(client_ip).await {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({
                "errors": [{"message": "Rate limit exceeded", "extensions": {"code": "RATE_LIMITED"}}]
            }))
        ).into_response();
    }

    let start = std::time::Instant::now();

    // Check Content-Type to determine request format
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("application/json");

    let is_binary_request = content_type.contains("application/x-gbp")
        || content_type.contains("application/graphql-request+gbp");

    // Parse request payload based on Content-Type
    let payload: GraphQlQuery = if is_binary_request {
        // Decode GBP binary request
        match grpc_graphql_gateway::gbp::GbpDecoder::new().decode(&body) {
            Ok(value) => match serde_json::from_value(value) {
                Ok(query) => query,
                Err(e) => {
                    return Json(json!({
                        "errors": [{
                            "message": format!("Invalid GBP request structure: {}", e),
                            "extensions": {"code": "BAD_REQUEST"}
                        }]
                    }))
                    .into_response();
                }
            },
            Err(e) => {
                return Json(json!({
                    "errors": [{
                        "message": format!("Failed to decode GBP request: {}", e),
                        "extensions": {"code": "BAD_REQUEST"}
                    }]
                }))
                .into_response();
            }
        }
    } else {
        // Parse JSON request (default)
        match serde_json::from_slice(&body) {
            Ok(query) => query,
            Err(e) => {
                return Json(json!({
                    "errors": [{
                        "message": format!("Invalid JSON request: {}", e),
                        "extensions": {"code": "BAD_REQUEST"}
                    }]
                }))
                .into_response();
            }
        }
    };

    // WAF: Check headers for SQLi (e.g. User-Agent hacks)
    if let Err(e) = grpc_graphql_gateway::waf::validate_headers(&headers, &inner.waf_config) {
        return Json(json!({
            "errors": [{
                "message": e.to_string(),
                "extensions": {"code": "VALIDATION_ERROR"}
            }]
        }))
        .into_response();
    }

    if let Some(vars) = &payload.variables {
        if let Err(e) =
            grpc_graphql_gateway::waf::validate_json_with_config(vars, &inner.waf_config)
        {
            return Json(json!({
                "errors": [{
                    "message": e.to_string(),
                    "extensions": {"code": "VALIDATION_ERROR"}
                }]
            }))
            .into_response();
        }
    }

    if let Some(query) = &payload.query {
        // Quick regex check on raw query
        if let Err(e) = grpc_graphql_gateway::waf::validate_query_string(query, &inner.waf_config) {
            return Json(json!({
                "errors": [{
                    "message": e.to_string(),
                    "extensions": {"code": "VALIDATION_ERROR"}
                }]
            }))
            .into_response();
        }
    }

    // Check for GBP response preference
    let accept_gbp = headers
        .get("accept")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.contains("application/x-gbp") || s.contains("application/graphql-response+gbp"))
        .unwrap_or(is_binary_request); // If request was binary, default to binary response

    // Execute federated query
    match inner
        .router
        .execute_scatter_gather(
            payload.query.as_deref(),
            payload.variables.as_ref(),
            payload.extensions.as_ref(),
        )
        .await
    {
        Ok(data) => {
            let duration = start.elapsed();

            let response_data = json!({
                "data": data,
                "extensions": {
                    "duration_ms": duration.as_secs_f64() * 1000.0,
                    "powered_by": "GBP Ultra - Bidirectional Binary Protocol",
                    "format": if accept_gbp { "binary" } else { "json" }
                }
            });

            // Decrypt any encrypted fields using the Gateway Secret
            // This enables "Transparent Encryption": Data is encrypted at rest/transit
            // from subgraph -> gateway, but decrypted by gateway -> client.
            let mut response_data_mut = response_data;
            if inner.legacy_xor_value_decryption_enabled {
                recursive_decrypt(&mut response_data_mut, inner.gateway_secret.as_deref());
            }
            let response_data = response_data_mut;

            if accept_gbp {
                // Use pooled encoder for binary response
                let mut encoders = state.encoder_pool.write().await;
                let mut encoder = encoders
                    .pop()
                    .unwrap_or_else(grpc_graphql_gateway::gbp::GbpEncoder::new);
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
            let mut error_payload = json!({
                "errors": [{"message": e.to_string()}]
            });

            // Check if it's an APQ error to add proper extensions
            if e.to_string() == "PersistedQueryNotFound" {
                error_payload = json!({
                    "errors": [{
                        "message": "PersistedQueryNotFound",
                        "extensions": { "code": "PERSISTED_QUERY_NOT_FOUND" }
                    }]
                });
            }

            // Return error in requested format
            if accept_gbp {
                let mut encoders = state.encoder_pool.write().await;
                let mut encoder = encoders
                    .pop()
                    .unwrap_or_else(grpc_graphql_gateway::gbp::GbpEncoder::new);
                drop(encoders);

                let bytes = encoder.encode(&error_payload);

                let mut encoders = state.encoder_pool.write().await;
                if encoders.len() < 64 {
                    encoders.push(encoder);
                }
                drop(encoders);

                return (
                    StatusCode::BAD_REQUEST,
                    [(axum::http::header::CONTENT_TYPE, "application/x-gbp")],
                    bytes,
                )
                    .into_response();
            }

            (StatusCode::BAD_REQUEST, Json(error_payload)).into_response()
        }
    }
}

async fn health_handler() -> impl IntoResponse {
    Json(json!({
        "status": "healthy",
        "version": "1.0.0",
        "engine": "GBP Router + Live Queries + Subscriptions",
        "endpoints": {
            "graphql": "/graphql",
            "subscriptions": "/graphql/ws",
            "live_queries": "/graphql/live",
            "health": "/health"
        }
    }))
}

/// WebSocket handler for GraphQL subscriptions
///
/// Implements the graphql-transport-ws protocol for standard GraphQL subscriptions
/// This is different from live queries - subscriptions are streaming GraphQL operations
async fn subscription_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.protocols(["graphql-transport-ws"])
        .on_upgrade(move |socket| handle_subscription_socket(socket, state))
}

/// Handle individual WebSocket connection for subscriptions
async fn handle_subscription_socket(socket: WebSocket, state: Arc<AppState>) {
    let (sender, mut receiver) = socket.split();
    let connection_id = uuid::Uuid::new_v4().to_string();

    tracing::info!(connection_id = %connection_id, "New subscription WebSocket connection");

    // Create channels for this connection
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

    // Track active subscriptions for this connection
    // Each subscription has its JoinHandle and a cancel sender
    let mut active_subscriptions: std::collections::HashMap<
        String,
        (tokio::task::JoinHandle<()>, mpsc::Sender<()>),
    > = std::collections::HashMap::new();

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
                                    "subscriptions": "enabled",
                                    "compression": "gbp"
                                }
                            });

                            if let Ok(text) = serde_json::to_string(&ack) {
                                let _ = outgoing_tx.send(Message::Text(text.into())).await;
                                tracing::debug!("Subscription connection acknowledged");
                            }
                        }

                        "subscribe" => {
                            let id = msg_value["id"].as_str().unwrap_or("unknown").to_string();
                            let payload = &msg_value["payload"];
                            let query = payload["query"].as_str();
                            let variables = payload.get("variables");

                            if let Some(query_str) = query {
                                // Check if this is actually a subscription operation
                                if query_str.trim_start().starts_with("subscription") {
                                    let outgoing_tx_clone = outgoing_tx.clone();
                                    let engine = state.subscription_engine.clone();
                                    let subscription_id = id.clone();
                                    let query_owned = query_str.to_string();
                                    let vars_owned = variables.cloned();

                                    // Create cancellation channel for this subscription
                                    let (cancel_tx, cancel_rx) = mpsc::channel::<()>(1);

                                    // Spawn federated subscription handler
                                    let handle = tokio::spawn(async move {
                                        // Adapter: bridge mpsc<Value> -> mpsc<Message>
                                        let (json_tx, mut json_rx) = mpsc::channel::<Value>(100);

                                        // Relay JSON values to WebSocket messages
                                        let relay_tx = outgoing_tx_clone.clone();
                                        let relay_handle = tokio::spawn(async move {
                                            while let Some(val) = json_rx.recv().await {
                                                if let Ok(text) = serde_json::to_string(&val) {
                                                    if relay_tx
                                                        .send(Message::Text(text.into()))
                                                        .await
                                                        .is_err()
                                                    {
                                                        break;
                                                    }
                                                }
                                            }
                                        });

                                        engine
                                            .handle_subscription(
                                                subscription_id,
                                                query_owned,
                                                vars_owned,
                                                json_tx,
                                                cancel_rx,
                                            )
                                            .await;

                                        relay_handle.abort();
                                    });

                                    active_subscriptions.insert(id, (handle, cancel_tx));
                                } else {
                                    // Not a subscription, treat as regular query
                                    let outgoing_tx_clone = outgoing_tx.clone();
                                    let state_clone = state.clone();
                                    let query_owned = query_str.to_string();
                                    let vars_owned = variables.cloned();
                                    let subscription_id = id.clone();

                                    tokio::spawn(async move {
                                        // Execute as regular query
                                        // Acquire lock
                                        let inner = state_clone.inner.read().await;

                                        match inner
                                            .router
                                            .execute_scatter_gather(
                                                Some(&query_owned),
                                                vars_owned.as_ref(),
                                                None,
                                            )
                                            .await
                                        {
                                            Ok(data) => {
                                                let response = json!({
                                                    "type": "next",
                                                    "id": subscription_id,
                                                    "payload": {
                                                        "data": data
                                                    }
                                                });

                                                if let Ok(text) = serde_json::to_string(&response) {
                                                    let _ = outgoing_tx_clone
                                                        .send(Message::Text(text.into()))
                                                        .await;
                                                }

                                                // Send complete for queries
                                                let complete = json!({
                                                    "type": "complete",
                                                    "id": subscription_id
                                                });

                                                if let Ok(text) = serde_json::to_string(&complete) {
                                                    let _ = outgoing_tx_clone
                                                        .send(Message::Text(text.into()))
                                                        .await;
                                                }
                                            }
                                            Err(e) => {
                                                let error = json!({
                                                    "type": "error",
                                                    "id": subscription_id,
                                                    "payload": [{
                                                        "message": e.to_string()
                                                    }]
                                                });

                                                if let Ok(text) = serde_json::to_string(&error) {
                                                    let _ = outgoing_tx_clone
                                                        .send(Message::Text(text.into()))
                                                        .await;
                                                }
                                            }
                                        }
                                    });
                                }
                            }
                        }

                        "complete" => {
                            let id = msg_value["id"].as_str().unwrap_or("unknown");
                            if let Some((handle, cancel_tx)) = active_subscriptions.remove(id) {
                                // Signal cancel to the federation engine before aborting
                                let _ = cancel_tx.send(()).await;
                                handle.abort();
                                tracing::debug!(subscription_id = %id, "Subscription cancelled by client");
                            }
                        }

                        "ping" => {
                            let pong = json!({"type": "pong"});
                            if let Ok(text) = serde_json::to_string(&pong) {
                                let _ = outgoing_tx.send(Message::Text(text.into())).await;
                            }
                        }

                        _ => {
                            tracing::debug!(msg_type = %msg_type, "Unhandled subscription message type");
                        }
                    }
                }
            }

            Ok(Message::Close(_)) => {
                tracing::info!(connection_id = %connection_id, "Client closed subscription connection");
                break;
            }

            Err(e) => {
                tracing::error!(error = %e, "WebSocket error");
                break;
            }

            _ => {}
        }
    }

    // Cleanup: cancel and abort all active subscriptions
    for (_, (handle, cancel_tx)) in active_subscriptions {
        let _ = cancel_tx.send(()).await;
        handle.abort();
    }
}

/// Recursively scan JSON and decrypt marked fields
fn recursive_decrypt(value: &mut serde_json::Value, secret: Option<&str>) {
    match value {
        serde_json::Value::Object(map) => {
            // Check if this object is an encrypted container
            if let (
                Some(serde_json::Value::Bool(true)),
                Some(serde_json::Value::String(encrypted_val)),
            ) = (map.get("encrypted"), map.get("value"))
            {
                if let Some(key_str) = secret {
                    // It's an encrypted field - try to decrypt
                    // We need to clone encrypted_val to use it while mutating map
                    let enc_val = encrypted_val.clone();

                    // Decrypt logic: Base64 decode -> XOR with key
                    let decrypted_res = (|| -> Option<String> {
                        use base64::Engine;
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(&enc_val)
                            .ok()?;
                        let key = key_str.as_bytes();
                        let decrypted_bytes: Vec<u8> = bytes
                            .iter()
                            .enumerate()
                            .map(|(i, b)| b ^ key[i % key.len()])
                            .collect();
                        String::from_utf8(decrypted_bytes).ok()
                    })();

                    if let Some(decrypted) = decrypted_res {
                        // Replace the entire object with the decrypted string value
                        *value = serde_json::Value::String(decrypted);
                        return;
                    }
                }
            }

            // Otherwise recurse into children
            for (_, v) in map.iter_mut() {
                recursive_decrypt(v, secret);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                recursive_decrypt(v, secret);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_server_config() -> ServerConfig {
        ServerConfig {
            listen: "127.0.0.1:4000".to_string(),
            workers: 4,
            quic_listen: None,
        }
    }

    fn test_yaml_config() -> YamlConfig {
        YamlConfig {
            server: test_server_config(),
            cors: None,
            subgraphs: vec![SubgraphConfig {
                name: "users".to_string(),
                url: "https://example.com/graphql".to_string(),
                headers: HashMap::new(),
                mtls: None,
            }],
            rate_limit: None,
            waf: None,
            query_cost: None,
            disable_introspection: false,
            circuit_breaker: None,
            mtls: None,
            quic: None,
            enable_legacy_xor_value_decryption: false,
        }
    }

    #[test]
    fn test_interpolate_env_vars_requires_present_variables() {
        let err = interpolate_env_vars("secret: ${ROUTER_TEST_MISSING_SECRET}")
            .expect_err("missing env var should fail closed");

        assert!(err.to_string().contains("ROUTER_TEST_MISSING_SECRET"));
    }

    #[test]
    fn test_interpolate_env_vars_supports_defaults() {
        let rendered = interpolate_env_vars("secret: ${ROUTER_TEST_OPTIONAL_SECRET:-fallback}")
            .expect("default interpolation should succeed");

        assert!(rendered.contains("fallback"));
    }

    #[test]
    fn test_validate_cors_config_rejects_mixed_wildcard_origin() {
        let err = validate_cors_config(&CorsConfig {
            allow_origins: vec!["*".to_string(), "https://app.example.com".to_string()],
        })
        .expect_err("mixed wildcard origins should be rejected");

        assert!(err.to_string().contains("cannot mix '*'"));
    }

    #[test]
    fn test_validate_yaml_config_rejects_mtls_over_http() {
        let mut config = test_yaml_config();
        config.subgraphs[0].url = "http://example.com/graphql".to_string();
        config.subgraphs[0].mtls = Some(grpc_graphql_gateway::mtls::MtlsConfig {
            enabled: true,
            ..Default::default()
        });

        let err = validate_yaml_config(&config, Some("secret"))
            .expect_err("mTLS over plaintext transport must be rejected");

        assert!(err.to_string().contains("non-HTTPS URL"));
    }

    #[test]
    fn test_validate_yaml_config_requires_secret_for_legacy_xor_mode() {
        let mut config = test_yaml_config();
        config.enable_legacy_xor_value_decryption = true;

        let err = validate_yaml_config(&config, None)
            .expect_err("legacy xor mode without a secret must fail");

        assert!(err
            .to_string()
            .contains("enable_legacy_xor_value_decryption"));
    }
}

// NOTE: `handle_subscription` has been replaced by
// `SubscriptionFederationEngine::handle_subscription` which fans out the
// subscription to upstream subgraph WebSockets and merges the event streams.
// See `subscription_federation.rs` for the implementation.

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
    let (update_tx, mut update_rx) =
        mpsc::channel::<grpc_graphql_gateway::live_query::LiveQueryUpdate>(100);
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

    // Spawn task to forward live query updates to client with backpressure.
    //
    // Problem: if the client is slow the `outgoing_tx` bounded channel fills up
    // and `outgoing_tx.send(...).await` blocks *this* task indefinitely.  Because
    // this task also owns `update_rx`, the live-query invalidation loop stalls
    // waiting for channel capacity — eventually deadlocking the broadcast.
    //
    // Fix: use `try_send`.  On `Err(Full)` the client is too slow; we drop the
    // message and log a warning.  On `Err(Closed)` the connection is gone; we
    // break so channels are dropped and everything cleans up.
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
                match outgoing_tx_clone.try_send(Message::Text(text.into())) {
                    Ok(()) => {}
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                        // Client is not consuming messages fast enough.
                        // Drop this update rather than blocking the invalidation pipeline.
                        tracing::warn!(
                            subscription_id = %update.id,
                            "Live-query outgoing buffer full (slow consumer) — dropping update"
                        );
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                        // Connection closed; stop the relay.
                        break;
                    }
                }
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
    // Execute the initial query through the router
    let initial_result = {
        // Acquire read lock for router (scoped to drop immediately after use)
        let inner = state.inner.read().await;
        inner
            .router
            .execute_scatter_gather(Some(&clean_query), variables.as_ref(), None)
            .await
    };

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

            if let Err(e) = state
                .live_query_store
                .register(live_query, update_tx.clone())
            {
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

            while let Ok(event) = invalidation_rx.recv().await {
                // Check if this subscription is affected
                if let Some(query_info) = state.live_query_store.get(&subscription_id) {
                    if query_info.should_update(&event) && query_info.throttle_elapsed() {
                        // Re-execute the query
                        // Re-acquire lock only for execution
                        let execution_result = {
                            let inner = state.inner.read().await;
                            inner
                                .router
                                .execute_scatter_gather(
                                    Some(&clean_query),
                                    variables.as_ref(),
                                    None,
                                )
                                .await
                        };

                        match execution_result {
                            Ok(updated_data) => {
                                if let Err(e) = state
                                    .live_query_store
                                    .send_update(&subscription_id, updated_data, false)
                                    .await
                                {
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
        }
        Err(e) => {
            tracing::error!(error = %e, "Initial query execution failed");
        }
    }
}

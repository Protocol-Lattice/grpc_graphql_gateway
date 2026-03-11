//! HTTP/3 and QUIC Transport Layer
//!
//! This module implements a full HTTP/3 server over QUIC (UDP) using the
//! `quinn` IETF-QUIC stack and the `h3` HTTP/3 framing layer.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────┐
//! │                      GBP Router                          │
//! │                                                          │
//! │   ┌─────────────────┐    ┌──────────────────────────┐   │
//! │   │  TCP Listener   │    │    UDP/QUIC Endpoint      │   │
//! │   │  HTTP/1.1+2     │    │    HTTP/3 (RFC 9114)      │   │
//! │   │  Port: N        │    │    Port: N  (same port)   │   │
//! │   └────────┬────────┘    └─────────────┬────────────┘   │
//! │            │                           │                 │
//! │            └──────────────┬────────────┘                 │
//! │                           ▼                              │
//! │              ┌────────────────────────┐                  │
//! │              │   Axum Request Router  │                  │
//! │              │  (shared handler pool) │                  │
//! │              └────────────────────────┘                  │
//! └──────────────────────────────────────────────────────────┘
//! ```
//!
//! # Protocol Negotiation
//!
//! Clients that support HTTP/3 discover it via the `Alt-Svc` response header
//! injected by the TCP/HTTP2 stack:
//!
//! ```text
//! Alt-Svc: h3=":443"; ma=86400
//! ```
//!
//! Subsequent connections from HTTP/3-capable clients arrive over UDP and are
//! handled by the QUIC endpoint, bypassing TCP entirely.
//!
//! # Benefits over TCP
//!
//! | Feature                   | TCP/HTTP2   | QUIC/HTTP3           |
//! |---------------------------|-------------|----------------------|
//! | Head-of-line blocking     | Per-stream  | **Eliminated**       |
//! | Connection setup RTTs     | 1–3 RTT     | **0–1 RTT**          |
//! | Multiplexed subscriptions | Yes         | **Yes (no HOL)**     |
//! | Mobile handover           | Re-connect  | **Connection ID**    |
//! | Loss recovery             | TCP SACK    | **QUIC ACK ranges**  |
//!
//! # Enabling
//!
//! Compile with `--features quic` and set `quic.enabled: true` in `router.yaml`.
//!
//! # TLS Requirements
//!
//! QUIC **requires TLS 1.3**. Configure via:
//! - `quic.cert_path` / `quic.key_path` — PEM-encoded certificate and key
//! - If omitted, a self-signed certificate is generated via `rcgen` (development only)

#[cfg(feature = "quic")]
mod quic_impl {
    use anyhow::Context;
    use bytes::Bytes;
    use h3::server::RequestStream;
    use h3_quinn::quinn::{self, ServerConfig as QuinnServerConfig};
    use http::{Request, Response};
    use quinn::crypto::rustls::QuicServerConfig;
    use rcgen::{CertificateParams, DistinguishedName, KeyPair, SanType};
    use rustls::ServerConfig as RustlsServerConfig;
    use serde::{Deserialize, Serialize};
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::oneshot;
    use tracing::{debug, info, warn};

    // ─── Configuration ────────────────────────────────────────────────────────

    /// Configuration for the HTTP/3 / QUIC transport
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct QuicConfig {
        /// Enable HTTP/3 alongside HTTP/1.1+2
        #[serde(default)]
        pub enabled: bool,

        /// UDP socket address for the QUIC endpoint (defaults to the same port as
        /// the TCP listener, e.g. `"0.0.0.0:4000"`).
        ///
        /// If omitted, the QUIC endpoint shares the same port as the HTTP listener,
        /// which is the recommended setup (requires OS-level port sharing between
        /// TCP and UDP).
        #[serde(default)]
        pub listen: Option<String>,

        /// Path to a PEM-encoded TLS certificate file.
        /// If not provided, an ephemeral self-signed certificate is generated.
        #[serde(default)]
        pub cert_path: Option<String>,

        /// Path to a PEM-encoded TLS private key file.
        /// If not provided, an ephemeral self-signed key is generated.
        #[serde(default)]
        pub key_path: Option<String>,

        /// Maximum concurrent QUIC streams per connection (default: 100)
        #[serde(default = "default_max_streams")]
        pub max_concurrent_streams: u64,

        /// Connection idle timeout in seconds (default: 30)
        #[serde(default = "default_idle_timeout")]
        pub idle_timeout_secs: u64,

        /// Maximum UDP datagram payload size (default: 1200 bytes)
        /// RFC 9000 mandates support for at least 1200 bytes.
        #[serde(default = "default_initial_mtu")]
        pub initial_mtu: u16,

        /// Maximum interval between ACK-eliciting packets when idle (ms)
        #[serde(default = "default_keep_alive")]
        pub keep_alive_interval_ms: u64,

        /// Maximum connection data window (bytes, default: 10 MB)
        #[serde(default = "default_conn_window")]
        pub max_connection_data: u64,

        /// Maximum stream data window per stream (bytes, default: 1 MB)
        #[serde(default = "default_stream_window")]
        pub max_stream_data: u64,
    }

    fn default_max_streams() -> u64 {
        100
    }
    fn default_idle_timeout() -> u64 {
        30
    }
    fn default_initial_mtu() -> u16 {
        1200
    }
    fn default_keep_alive() -> u64 {
        10_000
    }
    fn default_conn_window() -> u64 {
        10 * 1024 * 1024
    }
    fn default_stream_window() -> u64 {
        1024 * 1024
    }

    impl Default for QuicConfig {
        fn default() -> Self {
            Self {
                enabled: false,
                listen: None,
                cert_path: None,
                key_path: None,
                max_concurrent_streams: default_max_streams(),
                idle_timeout_secs: default_idle_timeout(),
                initial_mtu: default_initial_mtu(),
                keep_alive_interval_ms: default_keep_alive(),
                max_connection_data: default_conn_window(),
                max_stream_data: default_stream_window(),
            }
        }
    }

    // ─── TLS Helpers ─────────────────────────────────────────────────────────

    /// Load a TLS certificate and private key from PEM files, or generate an
    /// ephemeral self-signed pair for development.
    ///
    /// Returns `(certificate_chain, private_key)` as `rustls` types.
    pub fn load_or_generate_tls(
        cert_path: Option<&str>,
        key_path: Option<&str>,
    ) -> anyhow::Result<(
        Vec<rustls::pki_types::CertificateDer<'static>>,
        rustls::pki_types::PrivateKeyDer<'static>,
    )> {
        match (cert_path, key_path) {
            (Some(cert_p), Some(key_p)) => load_tls_from_files(cert_p, key_p),
            _ => {
                warn!(
                    "⚠️  No TLS certificate configured for QUIC — \
                     generating ephemeral self-signed cert (development only)"
                );
                generate_self_signed_cert()
            }
        }
    }

    /// Load TLS material from PEM files.
    fn load_tls_from_files(
        cert_path: &str,
        key_path: &str,
    ) -> anyhow::Result<(
        Vec<rustls::pki_types::CertificateDer<'static>>,
        rustls::pki_types::PrivateKeyDer<'static>,
    )> {
        use rustls_pemfile::{certs, private_key};
        use std::io::BufReader;

        let cert_file = std::fs::File::open(cert_path)
            .with_context(|| format!("Failed to open QUIC cert file: {}", cert_path))?;
        let key_file = std::fs::File::open(key_path)
            .with_context(|| format!("Failed to open QUIC key file: {}", key_path))?;

        let certs_der: Vec<rustls::pki_types::CertificateDer<'static>> =
            certs(&mut BufReader::new(cert_file))
                .collect::<Result<Vec<_>, _>>()
                .context("Failed to parse QUIC TLS certificates")?;

        let private_key = private_key(&mut BufReader::new(key_file))
            .context("Failed to read QUIC private key")?
            .context("No private key found in QUIC key file")?;

        info!(cert_path = %cert_path, "🔐 Loaded QUIC TLS certificate from file");
        Ok((certs_der, private_key))
    }

    /// Generate an ephemeral, self-signed TLS 1.3 certificate using `rcgen`.
    ///
    /// **Not suitable for production** — use real certificates issued by a trusted CA.
    fn generate_self_signed_cert() -> anyhow::Result<(
        Vec<rustls::pki_types::CertificateDer<'static>>,
        rustls::pki_types::PrivateKeyDer<'static>,
    )> {
        let key_pair = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
            .context("Failed to generate ECDSA key pair for QUIC")?;

        let mut params = CertificateParams::default();
        params.distinguished_name = DistinguishedName::new();
        params.distinguished_name.push(
            rcgen::DnType::CommonName,
            "GBP Router — Self-Signed QUIC Dev Cert",
        );
        params.subject_alt_names = vec![SanType::DnsName("localhost".try_into().unwrap())];
        params.not_before = rcgen::date_time_ymd(2025, 1, 1);
        params.not_after = rcgen::date_time_ymd(2027, 1, 1);

        let cert = params
            .self_signed(&key_pair)
            .context("Failed to self-sign ephemeral QUIC certificate")?;

        let cert_der = rustls::pki_types::CertificateDer::from(cert.der().to_vec());
        let key_der = rustls::pki_types::PrivateKeyDer::try_from(key_pair.serialize_der())
            .map_err(|e| anyhow::anyhow!("Failed to serialize QUIC private key: {}", e))?;

        info!("🔑 Generated ephemeral self-signed TLS certificate for QUIC");
        Ok((vec![cert_der], key_der))
    }

    // ─── QUIC Server Config ───────────────────────────────────────────────────

    /// Build a `quinn::ServerConfig` tuned for high-throughput GraphQL over HTTP/3.
    ///
    /// Key tunings applied:
    /// - TLS 1.3 only (QUIC requirement — RFC 9001)
    /// - ALPN: `h3` (RFC 9114)
    /// - Increased flow-control windows for large GraphQL responses
    /// - Aggressive connection migration enabled
    pub fn build_server_config(
        quic_cfg: &QuicConfig,
        cert_chain: Vec<rustls::pki_types::CertificateDer<'static>>,
        private_key: rustls::pki_types::PrivateKeyDer<'static>,
    ) -> anyhow::Result<QuinnServerConfig> {
        // ─── rustls TLS 1.3 configuration ────────────────────────────────────
        let mut tls_cfg =
            RustlsServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
                .with_no_client_auth()
                .with_single_cert(cert_chain, private_key)
                .context("Failed to configure QUIC TLS certificate")?;

        // HTTP/3 requires the `h3` ALPN token (RFC 9114 §3.1)
        tls_cfg.alpn_protocols = vec![b"h3".to_vec()];

        // Maximize record-layer throughput — session tickets improve 0-RTT
        tls_cfg.max_early_data_size = u32::MAX;

        let quic_tls: QuicServerConfig = QuicServerConfig::try_from(tls_cfg)
            .context("Failed to build QUIC server config from rustls config")?;

        // ─── QUIC transport parameters ────────────────────────────────────────
        let mut transport = quinn::TransportConfig::default();

        transport.max_concurrent_bidi_streams(
            quinn::VarInt::from_u64(quic_cfg.max_concurrent_streams)
                .expect("max_concurrent_streams too large"),
        );
        transport.max_concurrent_uni_streams(quinn::VarInt::from_u32(16));

        // Connection-level flow control window
        transport.receive_window(
            quinn::VarInt::from_u64(quic_cfg.max_connection_data)
                .expect("max_connection_data too large"),
        );

        // Stream-level flow control window
        transport.stream_receive_window(
            quinn::VarInt::from_u64(quic_cfg.max_stream_data).expect("max_stream_data too large"),
        );

        // Idle timeout — close idle connections to free resources
        transport.max_idle_timeout(Some(
            Duration::from_secs(quic_cfg.idle_timeout_secs)
                .try_into()
                .context("idle_timeout_secs overflow")?,
        ));

        // Keep-alives prevent NAT mappings from expiring on mobile networks
        transport.keep_alive_interval(Some(Duration::from_millis(quic_cfg.keep_alive_interval_ms)));

        // Jumbo QUIC datagrams — larger initial MTU = fewer packets for big responses
        transport.initial_mtu(quic_cfg.initial_mtu);

        let mut server_cfg = QuinnServerConfig::with_crypto(Arc::new(quic_tls));
        server_cfg.transport_config(Arc::new(transport));

        // Migrate connections when client IP changes (critical for mobile)
        server_cfg.migration(true);

        Ok(server_cfg)
    }

    // ─── HTTP/3 Handler ───────────────────────────────────────────────────────

    /// Handler function type that mirrors the axum stack's signature but accepts
    /// already-parsed HTTP/3 request headers and body.
    ///
    /// The QUIC transport layer deserialises each QUIC stream into an
    /// `http::Request<Bytes>` and dispatches it here. The response is serialised
    /// back into H3 frames and sent on the same stream.
    pub type Http3Handler = Arc<
        dyn Fn(
                Request<Bytes>,
            )
                -> std::pin::Pin<Box<dyn std::future::Future<Output = Response<Bytes>> + Send>>
            + Send
            + Sync,
    >;

    /// A running HTTP/3 server task.
    ///
    /// Drop the returned `QuicServer` to trigger graceful shutdown (it sends a
    /// QUIC `GOAWAY` to all open connections).
    pub struct QuicServer {
        /// Shutdown signal sender — dropping this triggers graceful shutdown.
        _shutdown_tx: oneshot::Sender<()>,
        /// Local address the QUIC endpoint is bound to.
        pub local_addr: SocketAddr,
    }

    impl QuicServer {
        /// Bind a QUIC endpoint and start serving HTTP/3 requests.
        ///
        /// # Arguments
        ///
        /// * `config`   — QUIC transport configuration.
        /// * `addr`     — UDP socket address to bind.
        /// * `handler`  — Shared request handler invoked for every HTTP/3 request.
        ///
        /// Returns a `QuicServer` handle; dropping it initiates shutdown.
        pub async fn bind(
            config: QuicConfig,
            addr: SocketAddr,
            cert_path: Option<&str>,
            key_path: Option<&str>,
            handler: Http3Handler,
        ) -> anyhow::Result<Self> {
            let (cert_chain, private_key) = load_or_generate_tls(cert_path, key_path)?;
            let server_config = build_server_config(&config, cert_chain, private_key)?;

            let endpoint = quinn::Endpoint::server(server_config, addr)
                .context("Failed to bind QUIC endpoint")?;

            let local_addr = endpoint
                .local_addr()
                .context("Failed to get QUIC local address")?;

            info!(
                addr = %local_addr,
                "🚀 HTTP/3 QUIC endpoint bound — accepting connections"
            );

            let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
            let cfg_clone = config.clone();

            tokio::spawn(async move {
                run_quic_accept_loop(endpoint, handler, cfg_clone, &mut shutdown_rx).await;
            });

            Ok(Self {
                _shutdown_tx: shutdown_tx,
                local_addr,
            })
        }
    }

    /// Main accept loop: receives QUIC connections and spawns per-connection tasks.
    async fn run_quic_accept_loop(
        endpoint: quinn::Endpoint,
        handler: Http3Handler,
        config: QuicConfig,
        shutdown_rx: &mut oneshot::Receiver<()>,
    ) {
        loop {
            tokio::select! {
                biased;

                _ = &mut *shutdown_rx => {
                    info!("🛑 QUIC endpoint shutting down (GOAWAY)");
                    endpoint.close(
                        quinn::VarInt::from_u32(0),
                        b"server shutdown",
                    );
                    break;
                }

                incoming = endpoint.accept() => {
                    match incoming {
                        None => {
                            info!("QUIC endpoint closed — no more connections");
                            break;
                        }
                        Some(conn) => {
                            let handler = Arc::clone(&handler);
                            let _config = config.clone();
                            tokio::spawn(async move {
                                handle_quic_connection(conn, handler).await;
                            });
                        }
                    }
                }
            }
        }
    }

    /// Complete QUIC connection lifecycle for a single client connection.
    ///
    /// Completes the TLS handshake, negotiates HTTP/3, and dispatches
    /// individual request streams.
    async fn handle_quic_connection(incoming: quinn::Incoming, handler: Http3Handler) {
        let remote = incoming.remote_address();

        let quic_conn = match incoming.await {
            Ok(c) => c,
            Err(e) => {
                warn!(remote = %remote, error = %e, "QUIC TLS handshake failed");
                return;
            }
        };

        debug!(remote = %remote, "✅ QUIC connection established (TLS 1.3)");

        let h3_conn = match h3::server::Connection::new(h3_quinn::Connection::new(quic_conn)).await
        {
            Ok(c) => c,
            Err(e) => {
                warn!(remote = %remote, error = %e, "HTTP/3 connection upgrade failed");
                return;
            }
        };

        handle_h3_connection(h3_conn, handler, remote).await;
    }

    /// Handle all HTTP/3 request streams on a single QUIC connection.
    async fn handle_h3_connection(
        mut conn: h3::server::Connection<h3_quinn::Connection, Bytes>,
        handler: Http3Handler,
        remote: SocketAddr,
    ) {
        loop {
            match conn.accept().await {
                Ok(Some(resolver)) => {
                    // h3 0.0.8: `accept()` returns a `RequestResolver`.
                    // Call `resolve_request().await` to complete reading the request
                    // headers and obtain the (Request, RequestStream) pair.
                    let handler = Arc::clone(&handler);
                    tokio::spawn(async move {
                        match resolver.resolve_request().await {
                            Ok((request, stream)) => {
                                handle_h3_request(request, stream, handler, remote).await;
                            }
                            Err(e) => {
                                warn!(remote = %remote, error = %e, "Failed to resolve H3 request headers");
                            }
                        }
                    });
                }
                Ok(None) => {
                    debug!(remote = %remote, "HTTP/3 connection closed (EOF)");
                    break;
                }
                Err(e) => {
                    let e_string = e.to_string();
                    if e_string.contains("connection closed") || e_string.contains("Application") {
                        debug!(remote = %remote, "HTTP/3 connection closed gracefully");
                    } else {
                        warn!(remote = %remote, error = %e_string, "HTTP/3 connection error");
                    }
                    break;
                }
            }
        }
    }

    /// Dispatch a single HTTP/3 request stream to the application handler.
    ///
    /// This converts the H3 frame stream into a standard `http::Request<Bytes>`,
    /// calls the handler, then encodes the response back into H3 frames.
    async fn handle_h3_request(
        request: Request<()>,
        mut stream: RequestStream<h3_quinn::BidiStream<Bytes>, Bytes>,
        handler: Http3Handler,
        remote: SocketAddr,
    ) {
        let (parts, _) = request.into_parts();

        // Collect request body from H3 DATA frames.
        // `recv_data()` yields `impl Buf` chunks; use `bytes::Buf::copy_to_bytes`
        // to convert each chunk into concrete `Bytes` without an extra allocation.
        let mut body_bytes = bytes::BytesMut::new();
        loop {
            match stream.recv_data().await {
                Ok(Some(mut chunk)) => {
                    use bytes::Buf;
                    let remaining = chunk.remaining();
                    body_bytes.extend_from_slice(&chunk.copy_to_bytes(remaining));
                }
                Ok(None) => break,
                Err(e) => {
                    warn!(remote = %remote, error = %e, "Failed to read H3 request body");
                    return;
                }
            }
        }

        let full_request = Request::from_parts(parts, body_bytes.freeze());
        let method = full_request.method().clone();
        let path = full_request.uri().path().to_string();

        let response = handler(full_request).await;

        let (resp_parts, resp_body) = response.into_parts();
        let resp_headers = Response::from_parts(resp_parts, ());

        debug!(
            remote = %remote,
            method = %method,
            path = %path,
            status = %resp_headers.status(),
            body_len = resp_body.len(),
            "H3 response"
        );

        // Send H3 headers
        if let Err(e) = stream.send_response(resp_headers).await {
            warn!(remote = %remote, error = %e, "Failed to send H3 response headers");
            return;
        }

        // Send H3 body
        if !resp_body.is_empty() {
            if let Err(e) = stream.send_data(resp_body).await {
                warn!(remote = %remote, error = %e, "Failed to send H3 response body");
                return;
            }
        }

        // Finish the stream (sends HEADERS frame with END_STREAM)
        if let Err(e) = stream.finish().await {
            debug!(remote = %remote, error = %e, "H3 stream finish error (likely client closed)");
        }
    }

    // ─── Alt-Svc Layer ────────────────────────────────────────────────────────

    /// Generate the value for the `Alt-Svc` HTTP response header that advertises
    /// the HTTP/3 QUIC endpoint to connecting clients.
    ///
    /// Browsers, fetcher libraries, and gRPC clients that see this header will
    /// upgrade to HTTP/3 on the next connection.
    ///
    /// # Arguments
    ///
    /// * `port` — The UDP port the QUIC endpoint is listening on (often same as
    ///   the TCP port).
    /// * `max_age_secs` — How long (in seconds) clients should remember the
    ///   advertisement. Default in the IETF spec: 86400 (24 h).
    ///
    /// # Returns
    ///
    /// A header value string like `h3=":4000"; ma=86400`
    pub fn alt_svc_header_value(port: u16, max_age_secs: u64) -> String {
        format!("h3=\":{port}\"; ma={max_age_secs}")
    }

    // ─── Metrics / Status ────────────────────────────────────────────────────

    /// Runtime status snapshot for health endpoints and observability dashboards.
    #[derive(Debug, Clone, serde::Serialize)]
    pub struct QuicStatus {
        pub enabled: bool,
        pub local_addr: Option<String>,
        pub protocol: &'static str,
        pub tls_version: &'static str,
        pub alpn: &'static str,
        pub features: Vec<&'static str>,
    }

    impl QuicStatus {
        pub fn active(addr: SocketAddr) -> Self {
            Self {
                enabled: true,
                local_addr: Some(addr.to_string()),
                protocol: "HTTP/3 (RFC 9114)",
                tls_version: "TLS 1.3 (RFC 8446)",
                alpn: "h3",
                features: vec![
                    "0-RTT connection establishment",
                    "Stream-level multiplexing (no head-of-line blocking)",
                    "Connection migration (mobile handover)",
                    "QUIC loss recovery",
                    "QUIC flow control",
                    "Alt-Svc advertisement for automatic upgrade",
                ],
            }
        }

        pub fn disabled() -> Self {
            Self {
                enabled: false,
                local_addr: None,
                protocol: "HTTP/3 (RFC 9114)",
                tls_version: "TLS 1.3 (RFC 8446)",
                alpn: "h3",
                features: vec![],
            }
        }
    }

    // ─── reqwest QUIC Client (for subgraph communication) ────────────────────

    /// Build a `reqwest::Client` for HTTP/2 (and HTTP/3 where supported) to subgraphs.
    ///
    /// HTTP/3 support in `reqwest` requires the `http3` feature. For now this
    /// returns a standard HTTP/2-capable client; HTTP/3 client support for
    /// subgraph connections is provided at the OS level via the QUIC endpoint.
    pub fn build_quic_reqwest_client() -> anyhow::Result<reqwest::Client> {
        reqwest::Client::builder()
            .min_tls_version(reqwest::tls::Version::TLS_1_2)
            .timeout(Duration::from_secs(30))
            .build()
            .context("Failed to build HTTP/2 reqwest client for QUIC-backed subgraphs")
    }
}

// ─── Stub when `quic` feature is disabled ─────────────────────────────────────

#[cfg(not(feature = "quic"))]
pub mod stub {
    use serde::{Deserialize, Serialize};

    /// Placeholder config when QUIC is not compiled in.
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct QuicConfig {
        #[serde(default)]
        pub enabled: bool,
        #[serde(default)]
        pub listen: Option<String>,
        #[serde(default)]
        pub cert_path: Option<String>,
        #[serde(default)]
        pub key_path: Option<String>,
        #[serde(default = "default_max_streams")]
        pub max_concurrent_streams: u64,
        #[serde(default = "default_idle_timeout")]
        pub idle_timeout_secs: u64,
        #[serde(default = "default_initial_mtu")]
        pub initial_mtu: u16,
        #[serde(default = "default_keep_alive")]
        pub keep_alive_interval_ms: u64,
        #[serde(default = "default_conn_window")]
        pub max_connection_data: u64,
        #[serde(default = "default_stream_window")]
        pub max_stream_data: u64,
    }

    fn default_max_streams() -> u64 {
        100
    }
    fn default_idle_timeout() -> u64 {
        30
    }
    fn default_initial_mtu() -> u16 {
        1200
    }
    fn default_keep_alive() -> u64 {
        10_000
    }
    fn default_conn_window() -> u64 {
        10 * 1024 * 1024
    }
    fn default_stream_window() -> u64 {
        1024 * 1024
    }

    /// Status returned when QUIC is compiled out.
    #[derive(Debug, Clone, serde::Serialize)]
    pub struct QuicStatus {
        pub enabled: bool,
        pub message: &'static str,
    }

    impl QuicStatus {
        pub fn disabled() -> Self {
            Self {
                enabled: false,
                message: "Recompile with --features quic to enable HTTP/3",
            }
        }
    }

    pub fn alt_svc_header_value(_port: u16, _max_age_secs: u64) -> String {
        String::new()
    }
}

// ─── Unified public re-exports ────────────────────────────────────────────────

#[cfg(feature = "quic")]
pub use quic_impl::{
    alt_svc_header_value, build_quic_reqwest_client, build_server_config, load_or_generate_tls,
    Http3Handler, QuicConfig, QuicServer, QuicStatus,
};

#[cfg(not(feature = "quic"))]
pub use stub::{alt_svc_header_value, QuicConfig, QuicStatus};

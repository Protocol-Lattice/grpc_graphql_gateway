//! HTTP/3 / QUIC Test Client
//!
//! A command-line tool that sends HTTP/3 requests to the GBP Router's QUIC
//! endpoint using the same quinn + h3 stack as the server itself.
//!
//! macOS system curl does NOT support HTTP/3 (it lacks QUIC transport).
//! This client fills that gap so we can validate end-to-end QUIC connectivity
//! without needing a special curl build.
//!
//! # Usage
//!
//! ```bash
//! # Health check (GET /health over HTTP/3)
//! cargo run --bin h3_client --features quic
//!
//! # Custom URL
//! cargo run --bin h3_client --features quic -- --url https://127.0.0.1:4000/health
//!
//! # POST GraphQL query
//! cargo run --bin h3_client --features quic -- \
//!     --url https://127.0.0.1:4000/graphql \
//!     --method POST \
//!     --body '{"query":"{ __typename }"}'
//!
//! # Verbose output (show full response headers)
//! cargo run --bin h3_client --features quic -- --verbose
//! ```
//!
//! # Requires
//!
//! Server must be running:
//! ```bash
//! cargo run --bin router --features quic -- examples/router-quic-test.yaml
//! ```

#[cfg(not(feature = "quic"))]
fn main() {
    eprintln!("❌  The `quic` feature is not enabled.");
    eprintln!("    Rebuild with:  cargo run --bin h3_client --features quic");
    std::process::exit(1);
}

#[cfg(feature = "quic")]
fn main() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to build Tokio runtime")
        .block_on(run())
        .unwrap_or_else(|e| {
            eprintln!("❌  Fatal error: {e}");
            std::process::exit(1);
        });
}

#[cfg(feature = "quic")]
async fn run() -> anyhow::Result<()> {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Instant;

    use anyhow::Context;
    use h3_quinn::quinn;
    use http::{Method, Uri};
    use rustls::RootCertStore;

    // Install the ring CryptoProvider so rustls doesn't panic trying to
    // auto-detect which provider to use (required when compiled with quic feature).
    let _ = rustls::crypto::ring::default_provider().install_default();

    // ── CLI argument parsing (no external deps, just std::env) ──────────────
    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut url_str    = "https://127.0.0.1:4000/health".to_string();
    let mut method_str = "GET".to_string();
    let mut body_str   = String::new();
    let mut verbose    = false;
    let mut insecure   = true;  // accept self-signed certs by default
    let mut repeat: u32 = 1;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--url"     | "-u" => { i += 1; url_str    = args[i].clone(); }
            "--method"  | "-X" => { i += 1; method_str = args[i].clone(); }
            "--body"    | "-d" => { i += 1; body_str   = args[i].clone(); }
            "--repeat"  | "-n" => { i += 1; repeat     = args[i].parse().unwrap_or(1); }
            "--verbose" | "-v" => { verbose = true; }
            "--secure"         => { insecure = false; }
            "--help" | "-h"    => {
                print_help();
                return Ok(());
            }
            _ => {
                // positional: treat as URL
                url_str = args[i].clone();
            }
        }
        i += 1;
    }

    let uri: Uri = url_str.parse().context("Invalid URL")?;
    let host = uri.host().context("URL has no host")?.to_string();
    let port = uri.port_u16().unwrap_or(4000);
    let method: Method = method_str.parse().context("Invalid HTTP method")?;

    // Resolve the server address
    let server_addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], port)));

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║              GBP Router  ·  HTTP/3 QUIC Test Client         ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║  Target : {url_str:<51}║");
    println!("║  Method : {method:<51}║");
    println!("║  Server : {server_addr:<51}║");
    println!("║  TLS    : {} (self-signed ok)                    ║",
        if insecure { "⚠️  INSECURE" } else { "🔒 Verified " });
    if repeat > 1 {
        println!("║  Repeat : {repeat:<51}║");
    }
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    // ── TLS configuration ────────────────────────────────────────────────────
    // For development we accept self-signed certificates.
    let tls_config = if insecure {
        // Custom verifier: accept any cert (dev only)
        let mut cfg = rustls::ClientConfig::builder_with_protocol_versions(
            &[&rustls::version::TLS13],
        )
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
        .with_no_client_auth();
        cfg.alpn_protocols = vec![b"h3".to_vec()];
        cfg
    } else {
        // Production: use system root CAs
        let mut root_store = RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let mut cfg = rustls::ClientConfig::builder_with_protocol_versions(
            &[&rustls::version::TLS13],
        )
        .with_root_certificates(root_store)
        .with_no_client_auth();
        cfg.alpn_protocols = vec![b"h3".to_vec()];
        cfg
    };

    // ── QUIC client endpoint ────────────────────────────────────────────────
    let quic_client_cfg = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(tls_config)
            .context("Failed to build QUIC client config")?,
    ));

    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap())
        .context("Failed to create QUIC client endpoint")?;
    endpoint.set_default_client_config(quic_client_cfg);

    // ── Run requests ─────────────────────────────────────────────────────────
    let mut total_bytes: u64 = 0;
    let mut total_ms: u128 = 0;
    let mut successes = 0u32;

    for req_num in 1..=repeat {
        if repeat > 1 {
            println!("── Request {req_num}/{repeat} ────────────────────────────────────────────");
        }

        let t0 = Instant::now();
        match send_h3_request(
            &endpoint,
            server_addr,
            &host,
            uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/"),
            method.clone(),
            if body_str.is_empty() { None } else { Some(body_str.as_bytes().to_vec()) },
            verbose,
        ).await {
            Ok((status, headers, body)) => {
                let elapsed = t0.elapsed();
                total_ms += elapsed.as_millis();
                total_bytes += body.len() as u64;
                successes += 1;

                println!("✅  HTTP/3 {status}  ({} ms)", elapsed.as_millis());

                if verbose {
                    println!("\n── Response Headers ──────────────────────────────────────────────");
                    for (k, v) in &headers {
                        println!("  {k}: {}", v.to_str().unwrap_or("<binary>"));
                    }
                }

                println!("\n── Response Body ─────────────────────────────────────────────────");
                let body_str = std::str::from_utf8(&body).unwrap_or("<binary>");
                // Pretty-print JSON if possible
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(body_str) {
                    println!("{}", serde_json::to_string_pretty(&json).unwrap_or_default());
                } else {
                    println!("{body_str}");
                }
                println!();
            }
            Err(e) => {
                let elapsed = t0.elapsed();
                total_ms += elapsed.as_millis();
                eprintln!("❌  Request failed ({} ms): {e}", elapsed.as_millis());
                if verbose {
                    eprintln!("    Cause: {e:?}");
                }
            }
        }
    }

    // ── Summary ──────────────────────────────────────────────────────────────
    if repeat > 1 {
        println!("── Summary ───────────────────────────────────────────────────────");
        println!("  Requests   : {repeat}");
        println!("  Successes  : {successes}");
        println!("  Total time : {} ms", total_ms);
        println!("  Avg time   : {} ms / req", total_ms / repeat as u128);
        println!("  Total bytes: {total_bytes}");
        println!("  Protocol   : HTTP/3 (QUIC, RFC 9114)");
        println!("  Transport  : UDP (RFC 9000)");
        println!("  TLS        : 1.3 (RFC 8446)");
        println!("  ALPN       : h3");
    }

    endpoint.wait_idle().await;
    Ok(())
}

/// Send a single HTTP/3 request and return (status_code, headers, body_bytes).
#[cfg(feature = "quic")]
async fn send_h3_request(
    endpoint: &h3_quinn::quinn::Endpoint,
    addr: std::net::SocketAddr,
    host: &str,
    path: &str,
    method: http::Method,
    body: Option<Vec<u8>>,
    verbose: bool,
) -> anyhow::Result<(u16, http::HeaderMap, bytes::Bytes)> {
    use anyhow::Context;
    use bytes::{Buf, BytesMut};

    // QUIC connection
    let conn = endpoint
        .connect(addr, host)
        .context("Failed to initiate QUIC connection")?
        .await
        .context("QUIC handshake failed")?;

    if verbose {
        println!("🤝  QUIC handshake complete — TLS 1.3, ALPN: h3");
        println!("    remote: {addr}");
    }

    // HTTP/3 connection over QUIC
    let (mut driver, mut send_req) =
        h3::client::new(h3_quinn::Connection::new(conn))
            .await
            .context("HTTP/3 connection negotiation failed")?;

    // Drive the h3 connection in background
    let drive = tokio::spawn(async move {
        let _ = futures::future::poll_fn(|cx| driver.poll_close(cx)).await;
    });

    // Build request
    let content_type = if body.is_some() { "application/json" } else { "" };
    let mut req_builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("user-agent", "gbp-h3-test-client/1.0")
        .header("accept", "application/json");

    if !content_type.is_empty() {
        req_builder = req_builder.header("content-type", content_type);
    }

    let request = req_builder
        .body(())
        .context("Failed to build HTTP/3 request")?;

    if verbose {
        println!("\n── Request Headers ───────────────────────────────────────────────");
        for (k, v) in request.headers() {
            println!("  {k}: {}", v.to_str().unwrap_or("<binary>"));
        }
    }

    // Send request headers
    let mut stream = send_req
        .send_request(request)
        .await
        .context("Failed to send HTTP/3 request headers")?;

    // Send body if present
    if let Some(body_bytes) = body {
        stream
            .send_data(bytes::Bytes::from(body_bytes))
            .await
            .context("Failed to send HTTP/3 request body")?;
    }
    stream.finish().await.context("Failed to finish HTTP/3 request stream")?;

    // Receive response headers
    let response = stream
        .recv_response()
        .await
        .context("Failed to receive HTTP/3 response headers")?;

    let status = response.status().as_u16();
    let headers = response.headers().clone();

    // Receive response body
    let mut body_buf = BytesMut::new();
    while let Some(mut chunk) = stream
        .recv_data()
        .await
        .context("Failed to receive HTTP/3 response body")?
    {
        let remaining = chunk.remaining();
        body_buf.extend_from_slice(&chunk.copy_to_bytes(remaining));
    }

    drive.abort();
    Ok((status, headers, body_buf.freeze()))
}

fn print_help() {
    println!("GBP Router — HTTP/3 QUIC Test Client");
    println!();
    println!("USAGE:");
    println!("  cargo run --bin h3_client --features quic -- [OPTIONS]");
    println!();
    println!("OPTIONS:");
    println!("  -u, --url <URL>       Target URL [default: https://127.0.0.1:4000/health]");
    println!("  -X, --method <METHOD> HTTP method [default: GET]");
    println!("  -d, --body <JSON>     Request body (for POST requests)");
    println!("  -n, --repeat <N>      Send N requests (for latency benchmarking)");
    println!("  -v, --verbose         Show full request/response headers");
    println!("      --secure          Verify TLS certificate (reject self-signed)");
    println!("  -h, --help            Show this help");
    println!();
    println!("EXAMPLES:");
    println!("  # Health check");
    println!("  cargo run --bin h3_client --features quic");
    println!();
    println!("  # POST GraphQL query");
    println!("  cargo run --bin h3_client --features quic -- \\");
    println!("      -X POST -u https://127.0.0.1:4000/graphql \\");
    println!("      -d '{{\"query\":\"{{ __typename }}\"}}' -v");
    println!();
    println!("  # Latency benchmark (100 requests)");
    println!("  cargo run --bin h3_client --features quic -- -n 100");
    println!();
    println!("REQUIRES:");
    println!("  Router running with QUIC enabled:");
    println!("  cargo run --bin router --features quic -- examples/router-quic-test.yaml");
}

// ── TLS certificate verifier (dev only) ──────────────────────────────────────

/// A rustls certificate verifier that accepts any server cert without checking
/// its chain of trust. **Never use this in production.**
#[cfg(feature = "quic")]
#[derive(Debug)]
struct SkipServerVerification;

#[cfg(feature = "quic")]
impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

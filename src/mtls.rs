//! Zero-Trust Mutual TLS (mTLS) for Subgraph Communication
//!
//! This module implements automatic mutual TLS between the router and subgraphs
//! using short-lived certificates compatible with the SPIFFE/SPIRE identity framework.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                         Trust Domain (e.g. cluster.local)               │
//! │                                                                         │
//! │   ┌──────────────┐    mTLS (short-lived certs)    ┌──────────────────┐  │
//! │   │              │◄──────────────────────────────►│                  │  │
//! │   │   GBP Router │    SPIFFE ID:                  │  Subgraph: users │  │
//! │   │              │    spiffe://cluster.local/      │                  │  │
//! │   │  (gateway)   │    ns/default/sa/router        │  SPIFFE ID:      │  │
//! │   │              │                                │  spiffe://...    │  │
//! │   └──────┬───────┘                                │  /sa/users       │  │
//! │          │                                        └──────────────────┘  │
//! │          │  mTLS                                                        │
//! │          ▼                                                              │
//! │   ┌──────────────────┐     ┌──────────────────┐                        │
//! │   │ Subgraph:        │     │ Subgraph:        │                        │
//! │   │ products         │     │ reviews          │                        │
//! │   └──────────────────┘     └──────────────────┘                        │
//! │                                                                         │
//! │   ┌──────────────────────────────────────────────────────────────────┐  │
//! │   │                    Certificate Authority (CA)                    │  │
//! │   │  • Issues short-lived SVIDs (default: 1 hour TTL)               │  │
//! │   │  • Auto-rotation before expiry (at 50% lifetime)                │  │
//! │   │  • SPIFFE-compatible trust bundle                               │  │
//! │   └──────────────────────────────────────────────────────────────────┘  │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Security Properties
//!
//! - **Mutual Authentication**: Both router and subgraph present certificates
//! - **Short-Lived Certificates**: Default 1-hour TTL limits blast radius of compromise
//! - **Automatic Rotation**: Certificates renewed at 50% lifetime (30 minutes by default)
//! - **SPIFFE Identity**: Workload identity via `spiffe://` URI in SAN extension
//! - **No Shared Secrets**: Eliminates the `GATEWAY_SECRET` environment variable attack surface
//! - **Crypto Binding**: Uses ECDSA P-256 for fast, secure key generation
//!
//! # Usage
//!
//! ```rust,ignore
//! use grpc_graphql_gateway::mtls::{MtlsConfig, MtlsProvider};
//!
//! let config = MtlsConfig {
//!     enabled: true,
//!     trust_domain: "cluster.local".to_string(),
//!     service_name: "router".to_string(),
//!     cert_ttl_secs: 3600,
//!     ..Default::default()
//! };
//!
//! let provider = MtlsProvider::new(config)?;
//! let reqwest_client = provider.build_client()?;
//! ```

use reqwest::tls::Identity;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, RwLock};
use tracing::{error, info, warn};

// ─── SPIFFE Identity ────────────────────────────────────────────────────────

/// A SPIFFE Verifiable Identity Document (SVID)
///
/// Represents a workload identity with X.509 certificate and private key.
/// SVIDs are short-lived and automatically rotated.
#[derive(Clone, Debug)]
pub struct Svid {
    /// SPIFFE ID (e.g., `spiffe://cluster.local/ns/default/sa/router`)
    pub spiffe_id: String,
    /// PEM-encoded X.509 certificate chain
    pub cert_pem: Vec<u8>,
    /// PEM-encoded private key (ECDSA P-256)
    pub key_pem: Vec<u8>,
    /// When this SVID was issued
    pub issued_at: SystemTime,
    /// When this SVID expires
    pub expires_at: SystemTime,
    /// Serial number of the certificate
    pub serial: String,
}

impl Svid {
    /// Check if this SVID is still valid (not expired)
    pub fn is_valid(&self) -> bool {
        SystemTime::now() < self.expires_at
    }

    /// Check if this SVID should be renewed (past 50% of its lifetime)
    pub fn should_renew(&self) -> bool {
        let issued = self
            .issued_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let expires = self
            .expires_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();

        let lifetime = expires.as_secs().saturating_sub(issued.as_secs());
        let elapsed = now.as_secs().saturating_sub(issued.as_secs());

        // Renew at 50% of lifetime
        elapsed >= lifetime / 2
    }

    /// Remaining time until expiry
    pub fn remaining(&self) -> Duration {
        self.expires_at
            .duration_since(SystemTime::now())
            .unwrap_or(Duration::ZERO)
    }

    /// Build a reqwest `Identity` from this SVID
    pub fn to_identity(&self) -> Result<Identity, MtlsError> {
        // Combine cert + key into PKCS#12 / PEM identity
        let mut pem_bundle = Vec::with_capacity(self.key_pem.len() + self.cert_pem.len());
        pem_bundle.extend_from_slice(&self.key_pem);
        pem_bundle.extend_from_slice(&self.cert_pem);

        Identity::from_pem(&pem_bundle)
            .map_err(|e| MtlsError::Identity(format!("Failed to create TLS identity: {}", e)))
    }
}

// ─── Configuration ──────────────────────────────────────────────────────────

/// Configuration for mutual TLS between router and subgraphs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MtlsConfig {
    /// Whether mTLS is enabled
    #[serde(default)]
    pub enabled: bool,

    /// SPIFFE trust domain (e.g., "cluster.local", "example.org")
    #[serde(default = "default_trust_domain")]
    pub trust_domain: String,

    /// Service name for SPIFFE ID construction
    /// Results in: `spiffe://{trust_domain}/ns/{namespace}/sa/{service_name}`
    #[serde(default = "default_service_name")]
    pub service_name: String,

    /// Kubernetes-style namespace for SPIFFE ID
    #[serde(default = "default_namespace")]
    pub namespace: String,

    /// Certificate TTL in seconds (default: 3600 = 1 hour)
    #[serde(default = "default_cert_ttl")]
    pub cert_ttl_secs: u64,

    /// Path to CA certificate file (PEM). If not set, an ephemeral CA is generated.
    #[serde(default)]
    pub ca_cert_path: Option<String>,

    /// Path to CA private key file (PEM). If not set, an ephemeral CA is generated.
    #[serde(default)]
    pub ca_key_path: Option<String>,

    /// Path to export the trust bundle (CA cert) for subgraphs to consume
    #[serde(default)]
    pub trust_bundle_path: Option<String>,

    /// Whether to allow falling back to plain HTTP if mTLS setup fails
    #[serde(default)]
    pub allow_fallback: bool,

    /// Whether to verify the subgraph's TLS certificate hostname
    #[serde(default = "default_true")]
    pub verify_hostname: bool,
}

fn default_trust_domain() -> String {
    "cluster.local".to_string()
}

fn default_service_name() -> String {
    "router".to_string()
}

fn default_namespace() -> String {
    "default".to_string()
}

fn default_cert_ttl() -> u64 {
    3600 // 1 hour
}

fn default_true() -> bool {
    true
}

impl Default for MtlsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            trust_domain: default_trust_domain(),
            service_name: default_service_name(),
            namespace: default_namespace(),
            cert_ttl_secs: default_cert_ttl(),
            ca_cert_path: None,
            ca_key_path: None,
            trust_bundle_path: None,
            allow_fallback: false,
            verify_hostname: true,
        }
    }
}

// ─── Errors ─────────────────────────────────────────────────────────────────

/// Errors that can occur during mTLS operations
#[derive(Debug, thiserror::Error)]
pub enum MtlsError {
    #[error("Certificate generation failed: {0}")]
    CertGeneration(String),

    #[error("Failed to load CA: {0}")]
    CaLoad(String),

    #[error("Certificate expired: {0}")]
    Expired(String),

    #[error("Identity creation failed: {0}")]
    Identity(String),

    #[error("TLS configuration error: {0}")]
    TlsConfig(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("SPIFFE ID validation error: {0}")]
    SpiffeId(String),
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Return a usable temporary directory path.
///
/// `std::fs::canonicalize` on Windows prepends the `\\?\` extended-length
/// prefix to disambiguate 8.3 short-names (e.g. `RUNNER~1`).  The OpenSSL
/// CLI does **not** understand this prefix and fails with:
///   "No such file or directory"
/// We strip the prefix so that OpenSSL receives a plain `C:\...` path.
fn resolve_temp_dir() -> std::path::PathBuf {
    let raw = std::env::temp_dir();
    match std::fs::canonicalize(&raw) {
        Ok(canonical) => {
            // On Windows, canonicalize returns `\\?\C:\...` – strip the prefix.
            #[cfg(windows)]
            {
                let s = canonical.to_string_lossy();
                if let Some(stripped) = s.strip_prefix(r"\\?\") {
                    return std::path::PathBuf::from(stripped);
                }
            }
            canonical
        }
        // If canonicalize itself fails (e.g. temp dir doesn't exist yet),
        // fall back to the original path.
        Err(_) => raw,
    }
}

// ─── Certificate Authority ──────────────────────────────────────────────────

/// An in-process Certificate Authority that issues short-lived SVIDs.
///
/// In production, this could be replaced by an external SPIRE server;
/// the CA interface is designed to be compatible with that model.
pub struct CertificateAuthority {
    /// CA certificate (PEM)
    ca_cert_pem: Vec<u8>,
    /// CA private key (PEM, ECDSA P-256)
    ca_key_pem: Vec<u8>,
    /// Trust domain
    trust_domain: String,
    /// Serial number counter
    serial_counter: std::sync::atomic::AtomicU64,
}

impl CertificateAuthority {
    /// Create a new ephemeral CA with a self-signed root certificate
    pub fn new_ephemeral(trust_domain: &str) -> Result<Self, MtlsError> {
        use std::process::Command;

        // Generate CA private key using openssl
        let ca_key_output = Command::new("openssl")
            .args(["ecparam", "-genkey", "-name", "prime256v1", "-noout"])
            .output()
            .map_err(|e| MtlsError::CertGeneration(format!("Failed to run openssl ecparam: {}. Is openssl installed?", e)))?;

        if !ca_key_output.status.success() {
            return Err(MtlsError::CertGeneration(format!(
                "openssl ecparam failed: {}",
                String::from_utf8_lossy(&ca_key_output.stderr)
            )));
        }

        let ca_key_pem = ca_key_output.stdout;

        // Create a temporary file for the key to pass to openssl req
        // Use a unique suffix to avoid race conditions in parallel tests
        static CA_TEMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique_id = CA_TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp_base = resolve_temp_dir();
        let key_tmp = tmp_base.join(format!("gbp_ca_key_{}_{}.pem", std::process::id(), unique_id));
        std::fs::write(&key_tmp, &ca_key_pem)
            .map_err(|e| MtlsError::CertGeneration(format!("Failed to write temp key: {}", e)))?;

        // Generate self-signed CA certificate
        // NOTE: Use native OS path (to_string_lossy without \\ -> / replacement) for file
        // arguments. On Windows, replacing backslashes causes OpenSSL's store loader to
        // misinterpret the path as a URI, especially with 8.3 short-name paths.
        let ca_cert_output = Command::new("openssl")
            .env("MSYS_NO_PATHCONV", "1") // Prevent Windows MSYS mangling of /O=... paths
            .args([
                "req",
                "-new",
                "-x509",
                "-key",
                &key_tmp.to_string_lossy(),
                "-sha256",
                "-days",
                "365",
                "-subj",
                &format!("/O=GBP Gateway/CN=GBP mTLS CA [{}]", trust_domain),
            ])
            .output()
            .map_err(|e| MtlsError::CertGeneration(format!("Failed to run openssl req: {}", e)))?;

        // Clean up temp file
        let _ = std::fs::remove_file(&key_tmp);

        if !ca_cert_output.status.success() {
            return Err(MtlsError::CertGeneration(format!(
                "openssl req failed: {}",
                String::from_utf8_lossy(&ca_cert_output.stderr)
            )));
        }

        let ca_cert_pem = ca_cert_output.stdout;

        info!(
            trust_domain = %trust_domain,
            "🔐 Ephemeral mTLS Certificate Authority initialized"
        );

        Ok(Self {
            ca_cert_pem,
            ca_key_pem,
            trust_domain: trust_domain.to_string(),
            serial_counter: std::sync::atomic::AtomicU64::new(1),
        })
    }

    /// Load CA from existing certificate and key files
    pub fn from_files(
        cert_path: &str,
        key_path: &str,
        trust_domain: &str,
    ) -> Result<Self, MtlsError> {
        let ca_cert_pem = std::fs::read(cert_path)
            .map_err(|e| MtlsError::CaLoad(format!("Failed to read CA cert {}: {}", cert_path, e)))?;
        let ca_key_pem = std::fs::read(key_path)
            .map_err(|e| MtlsError::CaLoad(format!("Failed to read CA key {}: {}", key_path, e)))?;

        info!(
            cert_path = %cert_path,
            trust_domain = %trust_domain,
            "🔐 Loaded mTLS CA from files"
        );

        Ok(Self {
            ca_cert_pem,
            ca_key_pem,
            trust_domain: trust_domain.to_string(),
            serial_counter: std::sync::atomic::AtomicU64::new(1),
        })
    }

    /// Issue a short-lived SVID for a workload
    pub fn issue_svid(
        &self,
        namespace: &str,
        service_name: &str,
        ttl: Duration,
    ) -> Result<Svid, MtlsError> {
        use std::process::Command;

        let spiffe_id = format!(
            "spiffe://{}/ns/{}/sa/{}",
            self.trust_domain, namespace, service_name
        );

        let serial = self
            .serial_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let serial_hex = format!("{:016X}", serial);

        let ttl_secs = ttl.as_secs();

        // Generate workload private key
        let key_output = Command::new("openssl")
            .args(["ecparam", "-genkey", "-name", "prime256v1", "-noout"])
            .output()
            .map_err(|e| {
                MtlsError::CertGeneration(format!("Failed to generate workload key: {}", e))
            })?;

        if !key_output.status.success() {
            return Err(MtlsError::CertGeneration(format!(
                "Workload key generation failed: {}",
                String::from_utf8_lossy(&key_output.stderr)
            )));
        }

        let key_pem = key_output.stdout;

        // Create temp files for signing
        // Use a global atomic counter to avoid race conditions when tests run in parallel
        static SVID_TEMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let tmp_dir = resolve_temp_dir();
        let pid = std::process::id();
        let unique_id = SVID_TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let key_path = tmp_dir.join(format!("gbp_svid_key_{pid}_{unique_id}.pem"));
        let ca_cert_path = tmp_dir.join(format!("gbp_ca_cert_{pid}_{unique_id}.pem"));
        let ca_key_path = tmp_dir.join(format!("gbp_ca_key_{pid}_{unique_id}.pem"));
        let csr_path = tmp_dir.join(format!("gbp_svid_csr_{pid}_{unique_id}.pem"));
        let ext_path = tmp_dir.join(format!("gbp_svid_ext_{pid}_{unique_id}.cnf"));

        std::fs::write(&key_path, &key_pem)
            .map_err(|e| MtlsError::CertGeneration(format!("Write key: {}", e)))?;
        std::fs::write(&ca_cert_path, &self.ca_cert_pem)
            .map_err(|e| MtlsError::CertGeneration(format!("Write CA cert: {}", e)))?;
        std::fs::write(&ca_key_path, &self.ca_key_pem)
            .map_err(|e| MtlsError::CertGeneration(format!("Write CA key: {}", e)))?;

        // Generate CSR
        // NOTE: Use native OS paths (to_string_lossy without \\ -> / replacement) for
        // file arguments. On Windows, replacing backslashes causes OpenSSL's store
        // loader to misinterpret the path as a URI, especially with 8.3 short-name
        // paths like RUNNER~1.
        let csr_output = Command::new("openssl")
            .env("MSYS_NO_PATHCONV", "1") // Prevent Windows MSYS mangling of /O=... paths
            .args([
                "req",
                "-new",
                "-key",
                &key_path.to_string_lossy(),
                "-subj",
                &format!("/O=GBP Workload/CN={}", service_name),
                "-out",
                &csr_path.to_string_lossy(),
            ])
            .output()
            .map_err(|e| MtlsError::CertGeneration(format!("CSR generation failed: {}", e)))?;

        if !csr_output.status.success() {
            Self::cleanup_temp_files(&[&key_path, &ca_cert_path, &ca_key_path, &csr_path, &ext_path]);
            return Err(MtlsError::CertGeneration(format!(
                "CSR generation failed: {}",
                String::from_utf8_lossy(&csr_output.stderr)
            )));
        }

        // Create extensions file for SPIFFE SAN URI
        // Note: serialNumber is NOT a valid X.509 v3 extension — it's a certificate
        // field set via -set_serial. We track serial via our own counter instead.
        let ext_content = format!(
            "[v3_svid]\n\
             basicConstraints = CA:FALSE\n\
             keyUsage = digitalSignature, keyEncipherment\n\
             extendedKeyUsage = serverAuth, clientAuth\n\
             subjectAltName = URI:{}\n",
            spiffe_id
        );
        std::fs::write(&ext_path, &ext_content)
            .map_err(|e| MtlsError::CertGeneration(format!("Write extensions: {}", e)))?;

        // Compute TTL in days (minimum 1 day for openssl, but we set actual usage check at runtime)
        let ttl_days = std::cmp::max(1, ttl_secs / 86400);

        // Sign the certificate with an explicit serial number
        let cert_output = Command::new("openssl")
            .args([
                "x509",
                "-req",
                "-in",
                &csr_path.to_string_lossy(),
                "-CA",
                &ca_cert_path.to_string_lossy(),
                "-CAkey",
                &ca_key_path.to_string_lossy(),
                "-set_serial",
                &format!("0x{}", serial_hex),
                "-days",
                &ttl_days.to_string(),
                "-sha256",
                "-extfile",
                &ext_path.to_string_lossy(),
                "-extensions",
                "v3_svid",
            ])
            .output()
            .map_err(|e| MtlsError::CertGeneration(format!("Certificate signing failed: {}", e)))?;

        // Clean up temp files
        Self::cleanup_temp_files(&[&key_path, &ca_cert_path, &ca_key_path, &csr_path, &ext_path]);

        if !cert_output.status.success() {
            return Err(MtlsError::CertGeneration(format!(
                "Certificate signing failed: {}",
                String::from_utf8_lossy(&cert_output.stderr)
            )));
        }

        let cert_pem = cert_output.stdout;

        let now = SystemTime::now();
        let expires_at = now + ttl;

        info!(
            spiffe_id = %spiffe_id,
            serial = %serial_hex,
            ttl_secs = ttl_secs,
            "🔑 Issued SVID certificate"
        );

        Ok(Svid {
            spiffe_id,
            cert_pem,
            key_pem,
            issued_at: now,
            expires_at,
            serial: serial_hex,
        })
    }

    /// Get the CA certificate (trust bundle) in PEM format
    pub fn trust_bundle(&self) -> &[u8] {
        &self.ca_cert_pem
    }

    /// Export the trust bundle to a file for subgraphs to consume
    pub fn export_trust_bundle(&self, path: &str) -> Result<(), MtlsError> {
        std::fs::write(path, &self.ca_cert_pem)?;
        info!(path = %path, "📦 Exported mTLS trust bundle");
        Ok(())
    }

    /// Clean up temporary files
    fn cleanup_temp_files(paths: &[&PathBuf]) {
        for path in paths {
            let _ = std::fs::remove_file(path);
        }
    }
}

// ─── mTLS Provider ──────────────────────────────────────────────────────────

/// The main mTLS provider that manages certificate lifecycle and builds TLS clients.
///
/// This is the primary interface for the router to obtain mTLS-configured HTTP clients.
/// It handles:
/// - CA initialization (ephemeral or from files)
/// - SVID issuance and rotation
/// - Building `reqwest::Client` instances with mTLS identity
pub struct MtlsProvider {
    config: MtlsConfig,
    ca: Arc<CertificateAuthority>,
    current_svid: Arc<RwLock<Svid>>,
    /// Guards the rotation critical section so only one task issues a new
    /// certificate at a time.  After acquiring this lock a caller must
    /// re-read `current_svid` (double-check pattern) before making a CA
    /// call, to avoid redundant SVID issuances when another holder already
    /// completed rotation.
    rotation_lock: Arc<Mutex<()>>,
}

impl MtlsProvider {
    /// Create a new mTLS provider from configuration
    pub fn new(config: MtlsConfig) -> Result<Self, MtlsError> {
        // Initialize CA
        let ca = if let (Some(cert_path), Some(key_path)) =
            (&config.ca_cert_path, &config.ca_key_path)
        {
            CertificateAuthority::from_files(cert_path, key_path, &config.trust_domain)?
        } else {
            CertificateAuthority::new_ephemeral(&config.trust_domain)?
        };

        let ca = Arc::new(ca);

        // Export trust bundle if path is configured
        if let Some(ref bundle_path) = config.trust_bundle_path {
            ca.export_trust_bundle(bundle_path)?;
        }

        // Issue initial SVID
        let ttl = Duration::from_secs(config.cert_ttl_secs);
        let svid = ca.issue_svid(&config.namespace, &config.service_name, ttl)?;
        let current_svid = Arc::new(RwLock::new(svid));

        info!(
            trust_domain = %config.trust_domain,
            service = %config.service_name,
            namespace = %config.namespace,
            cert_ttl_secs = config.cert_ttl_secs,
            "🔒 mTLS Provider initialized (Zero-Trust mode)"
        );

        Ok(Self {
            config,
            ca,
            current_svid,
            rotation_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Get the current SVID, rotating if necessary.
    ///
    /// Uses a double-check pattern around a `rotation_lock` Mutex so that
    /// concurrent callers never race into the CA simultaneously:
    ///
    /// 1. Fast path: read SVID under shared lock; return immediately if fresh.
    /// 2. Acquire the exclusive rotation lock (serialises all rotation attempts).
    /// 3. Re-read the SVID — another task may have already rotated it while we
    ///    were waiting for the rotation lock.  Return early if now fresh.
    /// 4. Issue a new SVID and write it under the RwLock.
    pub async fn get_svid(&self) -> Result<Svid, MtlsError> {
        // 1. Fast path
        {
            let svid = self.current_svid.read().await;
            if svid.is_valid() && !svid.should_renew() {
                return Ok(svid.clone());
            }
        }

        // 2. Acquire rotation serialisation lock
        let _rotation_guard = self.rotation_lock.lock().await;

        // 3. Double-check: another task may have rotated while we waited
        {
            let svid = self.current_svid.read().await;
            if svid.is_valid() && !svid.should_renew() {
                return Ok(svid.clone());
            }
        }

        // 4. Actually rotate (still holding rotation_lock)
        self.rotate_locked().await
    }

    /// Internal rotation helper — must be called while `rotation_lock` is held.
    async fn rotate_locked(&self) -> Result<Svid, MtlsError> {
        let ttl = Duration::from_secs(self.config.cert_ttl_secs);
        let new_svid = self
            .ca
            .issue_svid(&self.config.namespace, &self.config.service_name, ttl)?;

        info!(
            serial = %new_svid.serial,
            remaining_secs = new_svid.remaining().as_secs(),
            "🔄 SVID rotated"
        );

        let mut current = self.current_svid.write().await;
        *current = new_svid.clone();
        Ok(new_svid)
    }

    /// Build a `reqwest::Client` configured with the current mTLS identity
    /// and the CA trust bundle for verifying subgraph certificates
    pub async fn build_client(&self) -> Result<reqwest::Client, MtlsError> {
        let svid = self.get_svid().await?;
        self.build_client_with_svid(&svid)
    }

    /// Force SVID rotation (acquires rotation lock; safe to call concurrently).
    pub async fn rotate(&self) -> Result<Svid, MtlsError> {
        let _guard = self.rotation_lock.lock().await;
        self.rotate_locked().await
    }

    /// Build a `reqwest::Client` with a specific SVID
    pub fn build_client_with_svid(&self, svid: &Svid) -> Result<reqwest::Client, MtlsError> {
        let identity = svid.to_identity()?;

        let ca_cert = reqwest::tls::Certificate::from_pem(self.ca.trust_bundle())
            .map_err(|e| MtlsError::TlsConfig(format!("Failed to load CA certificate: {}", e)))?;

        let mut builder = reqwest::Client::builder()
            .identity(identity)
            .add_root_certificate(ca_cert)
            .min_tls_version(reqwest::tls::Version::TLS_1_2)
            .timeout(Duration::from_secs(30));

        if !self.config.verify_hostname {
            builder = builder.danger_accept_invalid_hostnames(true);
        }

        builder
            .build()
            .map_err(|e| MtlsError::TlsConfig(format!("Failed to build TLS client: {}", e)))
    }

    /// Get a reference to the CA for issuing subgraph SVIDs
    pub fn ca(&self) -> &CertificateAuthority {
        &self.ca
    }

    /// Get the trust bundle (CA certificate) in PEM format
    pub fn trust_bundle(&self) -> &[u8] {
        self.ca.trust_bundle()
    }

    /// Get the current configuration
    pub fn config(&self) -> &MtlsConfig {
        &self.config
    }

    /// Start a background task that automatically rotates the SVID
    /// before it expires. Returns a handle to the spawned task.
    pub fn start_rotation_task(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let provider = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                // Check every 30 seconds
                tokio::time::sleep(Duration::from_secs(30)).await;

                let svid = provider.current_svid.read().await;
                if svid.should_renew() {
                    let serial = svid.serial.clone();
                    let remaining = svid.remaining();
                    drop(svid); // Release read lock before acquiring rotation lock

                    info!(
                        old_serial = %serial,
                        remaining_secs = remaining.as_secs(),
                        "🔄 SVID approaching expiry, rotating..."
                    );

                    match provider.rotate().await {
                        Ok(new_svid) => {
                            info!(
                                new_serial = %new_svid.serial,
                                expires_in_secs = new_svid.remaining().as_secs(),
                                "✅ SVID rotation successful"
                            );
                        }
                        Err(e) => {
                            error!(
                                error = %e,
                                "❌ SVID rotation failed! Current certificate may expire soon."
                            );
                        }
                    }
                } else if !svid.is_valid() {
                    drop(svid);
                    warn!("⚠️ Current SVID has expired! Attempting emergency rotation...");

                    match provider.rotate().await {
                        Ok(new_svid) => {
                            info!(
                                new_serial = %new_svid.serial,
                                "✅ Emergency SVID rotation successful"
                            );
                        }
                        Err(e) => {
                            error!(
                                error = %e,
                                "❌ Emergency SVID rotation failed! mTLS connections will fail."
                            );
                        }
                    }
                }
            }
        })
    }

    /// Get a snapshot of the current mTLS status for health checks
    pub async fn status(&self) -> MtlsStatus {
        let svid = self.current_svid.read().await;
        MtlsStatus {
            enabled: self.config.enabled,
            trust_domain: self.config.trust_domain.clone(),
            spiffe_id: svid.spiffe_id.clone(),
            cert_serial: svid.serial.clone(),
            cert_valid: svid.is_valid(),
            remaining_secs: svid.remaining().as_secs(),
            needs_renewal: svid.should_renew(),
        }
    }
}

/// Status information for health checks and observability
#[derive(Debug, Clone, Serialize)]
pub struct MtlsStatus {
    pub enabled: bool,
    pub trust_domain: String,
    pub spiffe_id: String,
    pub cert_serial: String,
    pub cert_valid: bool,
    pub remaining_secs: u64,
    pub needs_renewal: bool,
}

// ─── Subgraph SVID Helper ───────────────────────────────────────────────────

/// Issue an SVID for a subgraph service.
///
/// This is used when the router also manages subgraph identities
/// (e.g., in a development/testing scenario or Kubernetes sidecar pattern).
pub fn issue_subgraph_svid(
    ca: &CertificateAuthority,
    subgraph_name: &str,
    namespace: &str,
    ttl: Duration,
) -> Result<Svid, MtlsError> {
    ca.issue_svid(namespace, subgraph_name, ttl)
}

/// Write SVID to files for subgraph consumption
pub fn export_svid(
    svid: &Svid,
    cert_path: &str,
    key_path: &str,
) -> Result<(), MtlsError> {
    std::fs::write(cert_path, &svid.cert_pem)?;
    std::fs::write(key_path, &svid.key_pem)?;
    info!(
        spiffe_id = %svid.spiffe_id,
        cert_path = %cert_path,
        key_path = %key_path,
        "📦 Exported subgraph SVID"
    );
    Ok(())
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = MtlsConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.trust_domain, "cluster.local");
        assert_eq!(config.cert_ttl_secs, 3600);
        assert_eq!(config.service_name, "router");
        assert_eq!(config.namespace, "default");
        assert!(!config.allow_fallback);
        assert!(config.verify_hostname);
    }

    #[test]
    fn test_ephemeral_ca_creation() {
        let ca = CertificateAuthority::new_ephemeral("test.local");
        assert!(ca.is_ok(), "Ephemeral CA should be created successfully");
        let ca = ca.unwrap();
        assert!(!ca.trust_bundle().is_empty());
    }

    #[test]
    fn test_svid_issuance() {
        let ca = CertificateAuthority::new_ephemeral("test.local").unwrap();
        let svid = ca.issue_svid("default", "my-service", Duration::from_secs(3600));
        assert!(svid.is_ok(), "SVID should be issued successfully");

        let svid = svid.unwrap();
        assert_eq!(
            svid.spiffe_id,
            "spiffe://test.local/ns/default/sa/my-service"
        );
        assert!(svid.is_valid());
        assert!(!svid.should_renew());
        assert!(!svid.cert_pem.is_empty());
        assert!(!svid.key_pem.is_empty());
    }

    #[test]
    fn test_svid_identity_creation() {
        let ca = CertificateAuthority::new_ephemeral("test.local").unwrap();
        let svid = ca
            .issue_svid("default", "test-svc", Duration::from_secs(3600))
            .unwrap();

        let identity = svid.to_identity();
        assert!(
            identity.is_ok(),
            "Identity should be created from valid SVID"
        );
    }

    #[test]
    fn test_multiple_svid_serials() {
        let ca = CertificateAuthority::new_ephemeral("test.local").unwrap();
        let svid1 = ca
            .issue_svid("default", "svc1", Duration::from_secs(3600))
            .unwrap();
        let svid2 = ca
            .issue_svid("default", "svc2", Duration::from_secs(3600))
            .unwrap();

        assert_ne!(
            svid1.serial, svid2.serial,
            "Each SVID should have a unique serial number"
        );
    }

    #[test]
    fn test_trust_bundle_export() {
        let ca = CertificateAuthority::new_ephemeral("test.local").unwrap();
        let tmp_path = std::env::temp_dir()
            .join("test_trust_bundle.pem")
            .to_str()
            .unwrap()
            .to_string();

        let result = ca.export_trust_bundle(&tmp_path);
        assert!(result.is_ok());

        let content = std::fs::read(&tmp_path).unwrap();
        assert!(!content.is_empty());
        assert!(String::from_utf8_lossy(&content).contains("BEGIN CERTIFICATE"));

        let _ = std::fs::remove_file(&tmp_path);
    }

    #[tokio::test]
    async fn test_mtls_provider() {
        let config = MtlsConfig {
            enabled: true,
            trust_domain: "test.local".to_string(),
            service_name: "test-router".to_string(),
            cert_ttl_secs: 3600,
            ..Default::default()
        };

        let provider = MtlsProvider::new(config);
        assert!(provider.is_ok(), "Provider should initialize successfully");

        let provider = provider.unwrap();
        let svid = provider.get_svid().await;
        assert!(svid.is_ok());

        let status = provider.status().await;
        assert!(status.enabled);
        assert_eq!(status.trust_domain, "test.local");
        assert!(status.cert_valid);
    }

    #[tokio::test]
    async fn test_svid_rotation() {
        let config = MtlsConfig {
            enabled: true,
            cert_ttl_secs: 3600,
            ..Default::default()
        };

        let provider = MtlsProvider::new(config).unwrap();

        let svid1 = provider.get_svid().await.unwrap();
        let svid2 = provider.rotate().await.unwrap();

        assert_ne!(svid1.serial, svid2.serial, "Rotated SVID should have new serial");
    }
}

# Mutual TLS (mTLS) & Zero-Trust Networking

The gateway implements **mutual TLS** for subgraph communication, providing cryptographic authentication for every service-to-service call. This follows the Zero-Trust principle: *never trust, always verify*.

## Architecture

```
┌──────────────┐          mTLS          ┌──────────────┐
│              │◄──────────────────────►│              │
│   Gateway    │  SPIFFE SVID Identity  │  Subgraph    │
│   (Router)   │  X.509 Certificates    │  (Users)     │
│              │  ECDSA P-256           │              │
├──────────────┤                        └──────────────┘
│ Ephemeral CA │          mTLS          ┌──────────────┐
│ SVID Rotation│◄──────────────────────►│              │
│ Trust Bundle │  Both sides verify     │  Subgraph    │
│              │  each other's cert     │  (Products)  │
└──────────────┘                        └──────────────┘
```

Both the router and subgraphs present certificates and verify each other's identity. There are no plaintext connections.

## Security Properties

| Property | Description |
|---|---|
| **Mutual Authentication** | Both client and server present and verify X.509 certificates |
| **SPIFFE Identity** | Workload identities follow the SPIFFE standard: `spiffe://{domain}/ns/{namespace}/sa/{service}` |
| **Short-Lived Certificates** | Default TTL of 1 hour; automatically rotated at 50% lifetime |
| **Ephemeral CA** | In-process Certificate Authority using ECDSA P-256 — no external PKI needed for development |
| **ECDSA P-256** | Modern elliptic curve cryptography for key generation |
| **TLS 1.2+** | Minimum TLS version enforced on all connections |

## Quick Start

### 1. Enable mTLS in `router.yaml`

```yaml
# Global mTLS configuration
mtls:
  enabled: true
  trust_domain: "cluster.local"
  service_name: "router"
  namespace: "default"
  cert_ttl_secs: 3600              # 1 hour
  trust_bundle_path: "/etc/certs/trust-bundle.pem"  # Optional
  verify_hostname: true
  allow_fallback: false            # Strict mode — no plain HTTP

# Per-subgraph mTLS overrides
subgraphs:
  - name: "users"
    url: "https://users-service:4001/graphql"
    mtls:
      enabled: true
      trust_domain: "cluster.local"
      service_name: "users-service"
```

### 2. Ephemeral CA (Development / Testing)

For local development, the gateway automatically creates an ephemeral CA at startup:

```rust
use grpc_graphql_gateway::mtls::{MtlsConfig, MtlsProvider};

let config = MtlsConfig {
    enabled: true,
    trust_domain: "dev.local".to_string(),
    service_name: "gateway".to_string(),
    namespace: "default".to_string(),
    cert_ttl_secs: 3600,
    ..Default::default()
};

let provider = MtlsProvider::new(config)?;
// Provider automatically generates CA + initial SVID
```

### 3. External CA (Production)

For production, provide your own CA certificate and key:

```yaml
mtls:
  enabled: true
  trust_domain: "production.example.com"
  ca_cert_path: "/etc/certs/ca.pem"
  ca_key_path: "/etc/certs/ca-key.pem"
  trust_bundle_path: "/etc/certs/trust-bundle.pem"
```

```rust
use grpc_graphql_gateway::mtls::CertificateAuthority;

let ca = CertificateAuthority::from_files(
    "/etc/certs/ca.pem",
    "/etc/certs/ca-key.pem",
    "production.example.com",
)?;
```

## SPIFFE Identity Model

Every workload gets a SPIFFE Verifiable Identity Document (SVID) with a URI like:

```
spiffe://cluster.local/ns/default/sa/router
spiffe://cluster.local/ns/default/sa/users-service
spiffe://cluster.local/ns/payments/sa/billing-service
```

### Components

| Component | Example | Description |
|---|---|---|
| Trust Domain | `cluster.local` | Organizational boundary |
| Namespace | `default` | Kubernetes namespace or logical group |
| Service Account | `router` | Unique service identifier |

## Certificate Lifecycle

```
┌──────────┐     Issue      ┌──────────┐
│          │───────────────►│          │
│    CA    │                │   SVID   │  TTL: 1 hour
│          │◄───────────────│          │  Auto-rotate at 30 min
│          │    Rotate      │          │
└──────────┘                └──────────┘
                                │
                                ▼
                      ┌──────────────────┐
                      │  reqwest::Client  │
                      │  with mTLS ID    │
                      └──────────────────┘
```

### Automatic Rotation

The `MtlsProvider` starts a background task that:

1. **Checks every 30 seconds** if the current SVID needs renewal
2. **Renews at 50% lifetime** (i.e., 30 minutes for a 1-hour TTL)
3. **Emergency rotation** if the certificate has already expired
4. **Atomic swap** — in-flight requests continue with the old cert; new requests get the fresh one

```rust
use std::sync::Arc;

let provider = Arc::new(MtlsProvider::new(config)?);
let rotation_handle = provider.start_rotation_task();

// Provider will automatically rotate SVIDs in the background
```

## Health Check Integration

The mTLS status is available for health probes:

```rust
let status = provider.status().await;

println!("Trust Domain: {}", status.trust_domain);
println!("SPIFFE ID:    {}", status.spiffe_id);
println!("Cert Valid:   {}", status.cert_valid);
println!("Remaining:    {}s", status.remaining_secs);
println!("Needs Renewal:{}", status.needs_renewal);
```

The `MtlsStatus` struct:

```rust
pub struct MtlsStatus {
    pub enabled: bool,
    pub trust_domain: String,
    pub spiffe_id: String,
    pub cert_serial: String,
    pub cert_valid: bool,
    pub remaining_secs: u64,
    pub needs_renewal: bool,
}
```

## Building mTLS Clients

The provider builds `reqwest::Client` instances pre-configured with:
- The current SVID as the TLS client identity
- The CA trust bundle for verifying subgraph certificates
- TLS 1.2 minimum version
- Optional hostname verification

```rust
// Build a client with the current SVID
let client = provider.build_client().await?;

// Use it for subgraph requests
let response = client
    .post("https://users-service:4001/graphql")
    .json(&graphql_request)
    .send()
    .await?;
```

## Issuing Subgraph SVIDs

For testing or development, the gateway can issue SVIDs for subgraphs:

```rust
use grpc_graphql_gateway::mtls::{issue_subgraph_svid, export_svid};
use std::time::Duration;

let ca = provider.ca();

// Issue an SVID for the users subgraph
let svid = issue_subgraph_svid(
    ca,
    "users-service",
    "default",
    Duration::from_secs(3600),
)?;

// Export to files for the subgraph to use
export_svid(&svid, "/etc/certs/users-cert.pem", "/etc/certs/users-key.pem")?;

// Export trust bundle for subgraphs to verify the gateway
ca.export_trust_bundle("/etc/certs/trust-bundle.pem")?;
```

## Configuration Reference

### `MtlsConfig`

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | `bool` | `false` | Enable/disable mTLS |
| `trust_domain` | `String` | `"cluster.local"` | SPIFFE trust domain |
| `service_name` | `String` | `"router"` | Service identity name |
| `namespace` | `String` | `"default"` | Kubernetes namespace |
| `cert_ttl_secs` | `u64` | `3600` | Certificate TTL in seconds |
| `ca_cert_path` | `Option<String>` | `None` | External CA cert (PEM). Ephemeral CA if not set |
| `ca_key_path` | `Option<String>` | `None` | External CA key (PEM). Ephemeral CA if not set |
| `trust_bundle_path` | `Option<String>` | `None` | Path to export trust bundle |
| `allow_fallback` | `bool` | `false` | Allow plain HTTP if mTLS fails |
| `verify_hostname` | `bool` | `true` | Verify subgraph TLS hostname |

## Production Recommendations

### 1. Use an External CA
In production, provide your own CA rather than relying on the ephemeral CA:
```yaml
mtls:
  ca_cert_path: "/etc/certs/ca.pem"
  ca_key_path: "/etc/certs/ca-key.pem"
```

### 2. Short TTLs
Keep certificate TTLs short (1 hour or less). The automatic rotation handles renewals seamlessly:
```yaml
mtls:
  cert_ttl_secs: 3600  # 1 hour
```

### 3. Never Allow Fallback in Production
```yaml
mtls:
  allow_fallback: false  # Strict mode
```

### 4. Export Trust Bundles for Subgraphs
Subgraphs need the CA trust bundle to verify the gateway's certificate:
```yaml
mtls:
  trust_bundle_path: "/shared/certs/trust-bundle.pem"
```

### 5. Monitor Certificate Health
Integrate `MtlsStatus` into your health check endpoint to detect certificate issues before they cause outages.

### 6. Kubernetes Integration
In Kubernetes, mount cert paths from Secrets or use a sidecar like SPIRE for production-grade SPIFFE identity management:

```yaml
# Kubernetes deployment snippet
volumes:
  - name: certs
    secret:
      secretName: gateway-mtls-certs
containers:
  - name: gateway
    volumeMounts:
      - name: certs
        mountPath: /etc/certs
        readOnly: true
```

## Prerequisites

- **OpenSSL** must be installed on the system (used for certificate generation via CLI)
- For production, pre-generated CA certificates are recommended over the ephemeral CA

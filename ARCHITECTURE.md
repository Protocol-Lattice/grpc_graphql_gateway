# Deployment Architecture Summary

## 🏗️ Complete Infrastructure

```
┌─────────────────────────────────────────────────────────────────┐
│                        Internet / Users                          │
└──────────────────────────┬──────────────────────────────────────┘
                           │
                  ┌────────▼────────┐
                  │  LoadBalancer   │ ← External access
                  │  (AWS NLB/GCP)  │ ← Health checks
                  └────────┬────────┘ ← SSL termination
                           │
         ┌─────────────────┼─────────────────┐
         │                 │                 │
    ┌────▼────┐      ┌────▼────┐     ┌────▼────┐
    │  Pod 1  │      │  Pod 2  │     │  Pod 3  │ ← HPA manages count
    │ Gateway │      │ Gateway │     │ Gateway │ ← VPA adjusts resources
    └────┬────┘      └────┬────┘     └────┬────┘
         │                │                │
         └────────┬───────┴────────┬───────┘
                  │                │
            ┌─────▼─────┐    ┌────▼──────┐
            │   Redis   │    │  Backend  │
            │   Cache   │    │  Services │
            └───────────┘    └───────────┘
```

## 📦 Docker Images

### 1. Main Gateway Image
```dockerfile
FROM rust:1.75-slim AS builder
# Build greeter and federation binaries
FROM debian:bookworm-slim
# Runtime with minimal dependencies
```

**Features**:
- Multi-stage build (minimal size)
- Non-root user (security)
- Health checks
- Supports both greeter and federation modes

### 2. Federation Image
```dockerfile
FROM rust:1.75-slim AS builder
# Build federation with all subgraphs
FROM debian:bookworm-slim
# Runs user, product, review subgraphs
```

**Ports**:
- 8891: User subgraph
- 8892: Product subgraph
- 8893: Review subgraph
- 50051-50053: gRPC ports
- 9090: Metrics

## ☸️ Kubernetes Resources

### Core Resources
```
Deployment
├── ReplicaSet (managed by HPA)
├── Pods (3-50 replicas)
│   ├── Container: gateway
│   ├── Liveness probe: /health
│   └── Readiness probe: /health
└── PodDisruptionBudget (min 2 available)
```

### Services
```
Service (ClusterIP)
└── Session affinity: ClientIP

LoadBalancer (optional)
├── External IP
├── Health checks
└── Traffic policy: Local/Cluster
```

### Autoscaling
```
HorizontalPodAutoscaler
├── Min replicas: 3
├── Max replicas: 10
├── Metrics: CPU 70%, Memory 80%
└── Behavior: gradual scale-up/down

VerticalPodAutoscaler (optional)
├── Update mode: Off/Auto
├── Min resources: 100m CPU, 128Mi RAM
├── Max resources: 2000m CPU, 2Gi RAM
└── Recommendations: continuous
```

### Networking
```
Ingress (NGINX)
├── TLS: cert-manager
├── Load balancing: round_robin
├── Rate limiting: 1000 RPS
└── CORS: enabled

NetworkPolicy (optional)
├── Ingress: from ingress-nginx
└── Egress: DNS + backend services
```

## 🔄 Scaling Strategies

### Horizontal Scaling (HPA)
| Metric | Threshold | Action |
|--------|-----------|--------|
| CPU > 70% | Scale up | Add pods (max 50) |
| Memory > 80% | Scale up | Add pods |
| CPU < 40% | Scale down | Remove pods (min 3) |

**Behavior**:
- Scale up: Fast (4 pods/30s)
- Scale down: Gradual (2 pods/60s)
- Stabilization: 5min

### Vertical Scaling (VPA)
| Mode | Purpose | Use Case |
|------|---------|----------|
| Off | Recommendations only | Safe with HPA |
| Initial | Set on creation | Initial sizing |
| Auto | Continuous updates | Full automation |

**Controls**:
- CPU: 100m - 2000m
- Memory: 128Mi - 2Gi

### Load Balancing
| Strategy | Method | Benefit |
|----------|--------|---------|
| Round Robin | Ingress annotation | Even distribution |
| Least Connections | Ingress annotation | Optimal utilization |
| IP Hash | Service affinity | Sticky sessions |

## 🚀 Federation Architecture

```
┌─────────────────────────────────────────┐
│          Apollo Router (Port 4000)       │
│          ┌─────────────────┐             │
│          │ Query Planner    │             │
│          └────────┬─────────┘             │
└──────────────────┼───────────────────────┘
                   │
      ┌────────────┼────────────┐
      │            │            │
┌─────▼─────┐ ┌───▼──────┐ ┌──▼───────┐
│   User    │ │ Product  │ │  Review  │
│ Subgraph  │ │ Subgraph │ │ Subgraph │
│ (3 pods)  │ │ (3 pods) │ │ (3 pods) │
│ Port 8891 │ │Port 8892 │ │Port 8893 │
└───────────┘ └──────────┘ └──────────┘
     │             │             │
     └─────────────┼─────────────┘
                   │
            ┌──────▼──────┐
            │   Backend   │
            │   Services  │
            └─────────────┘
```

**Each Subgraph**:
- Independent scaling (HPA)
- Separate resource limits
- Entity resolution with DataLoader
- Metrics on port 9090

## 📊 Monitoring Stack

```
┌──────────────┐
│  Prometheus  │ ← Scrapes metrics (port 9090)
└──────┬───────┘
       │
┌──────▼───────┐
│   Grafana    │ ← Visualizes metrics
└──────────────┘
       │
       ├─ Request rate
       ├─ Error rate
       ├─ Latency (p50, p95, p99)
       ├─ Pod count (HPA)
       └─ Resource usage (VPA)
```

## 🔒 Security Layers

```
1. Network
   └─ NetworkPolicy: restrict traffic

2. Container
   ├─ Non-root user (UID 1000)
   ├─ Read-only filesystem
   └─ Dropped capabilities

3. Pod
   └─ Security context enforced

4. Service
   ├─ TLS termination
   └─ Source IP restrictions

5. Application
   ├─ Rate limiting
   ├─ CORS policies
   └─ Query whitelisting
```

## 📈 Resource Planning

### Development
```yaml
replicas: 1
cpu: 250m
memory: 256Mi
HPA: disabled
VPA: Off (recommendations)
```

### Staging
```yaml
replicas: 2
cpu: 500m
memory: 512Mi
HPA: 2-5 replicas
VPA: Initial
```

### Production
```yaml
replicas: 5
cpu: 1000m
memory: 1Gi
HPA: 5-50 replicas
VPA: Off (with HPA)
LoadBalancer: enabled
PDB: min 3 available
```

## 🎯 Deployment Commands

```bash
# Development
docker-compose -f docker-compose.federation.yml up

# Staging
helm install gateway ./helm/grpc-graphql-gateway \
  --namespace staging \
  -f helm/values-staging.yaml

# Production
helm install gateway ./helm/grpc-graphql-gateway \
  --namespace production \
  -f helm/values-autoscaling-complete.yaml
```

## 📝 Testing

```bash
# Load test
k6 run --vus 100 --duration 5m loadtest.js

# Watch scaling
watch 'kubectl get pods,hpa,vpa -n production'

# Check load distribution
kubectl get pods -o wide -l app=gateway

# View metrics
curl http://<lb-ip>/metrics
```

## 🔗 References

- Dockerfiles: `/Dockerfile`, `/Dockerfile.federation`
- Helm Chart: `/helm/grpc-graphql-gateway/`
- Docker Compose: `/docker-compose.federation.yml`
- Docs: `/docs/src/production/`
- Quick Start: `/DEPLOYMENT.md`

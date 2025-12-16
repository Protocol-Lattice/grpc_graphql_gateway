# Helm Deployment

This guide covers deploying the gRPC-GraphQL Gateway to Kubernetes using Helm charts with load balancing and high availability.

## Prerequisites

- Kubernetes cluster (v1.19+)
- Helm 3.x installed (`brew install helm`)
- kubectl configured
- Docker image of your gateway

## Quick Start

### Install from Source

```bash
# Clone repository
git clone https://github.com/Protocol-Lattice/grpc_graphql_gateway.git
cd grpc_graphql_gateway

# Install chart
helm install my-gateway ./helm/grpc-graphql-gateway \
  --namespace grpc-gateway \
  --create-namespace
```

### Install from Helm Repository

```bash
# Add helm repository (once published)
helm repo add protocol-lattice https://protocol-lattice.github.io/grpc_graphql_gateway
helm repo update

# Install
helm install my-gateway protocol-lattice/grpc-graphql-gateway \
  --namespace grpc-gateway \
  --create-namespace
```

## Configuration Options

### Basic Deployment

```yaml
# values.yaml
replicaCount: 3

image:
  repository: ghcr.io/protocol-lattice/grpc-graphql-gateway
  tag: "0.2.9"

service:
  type: ClusterIP
  httpPort: 8080
```

### With Ingress

```yaml
ingress:
  enabled: true
  className: nginx
  annotations:
    cert-manager.io/cluster-issuer: "letsencrypt-prod"
  hosts:
    - host: api.example.com
      paths:
        - path: /graphql
          pathType: Prefix
  tls:
    - secretName: gateway-tls
      hosts:
        - api.example.com
```

### With Horizontal Pod Autoscaler

```yaml
autoscaling:
  enabled: true
  minReplicas: 3
  maxReplicas: 10
  targetCPUUtilizationPercentage: 70
  targetMemoryUtilizationPercentage: 80
```

### With LoadBalancer

```yaml
loadBalancer:
  enabled: true
  externalTrafficPolicy: Local  # Preserve source IP
  annotations:
    # AWS NLB
    service.beta.kubernetes.io/aws-load-balancer-type: "nlb"
    # GCP
    # cloud.google.com/load-balancer-type: "Internal"
```

## Load Balancing Strategies

### Round Robin (Default)

```yaml
service:
  sessionAffinity: None

ingress:
  annotations:
    nginx.ingress.kubernetes.io/load-balance: "round_robin"
```

### Sticky Sessions

```yaml
service:
  sessionAffinity: ClientIP
  sessionAffinityConfig:
    clientIP:
      timeoutSeconds: 10800
```

### Least Connections

```yaml
ingress:
  annotations:
    nginx.ingress.kubernetes.io/load-balance: "least_conn"
```

## High Availability Setup

```yaml
# Minimum 3 replicas
replicaCount: 3

# Pod Disruption Budget
podDisruptionBudget:
  enabled: true
  minAvailable: 2

# Spread across nodes
affinity:
  podAntiAffinity:
    preferredDuringSchedulingIgnoredDuringExecution:
      - weight: 100
        podAffinityTerm:
          labelSelector:
            matchExpressions:
              - key: app.kubernetes.io/name
                operator: In
                values:
                  - grpc-graphql-gateway
          topologyKey: kubernetes.io/hostname
```

## Federation Deployment

Deploy multiple subgraphs with independent scaling:

```bash
# User subgraph
helm install user-subgraph ./helm/grpc-graphql-gateway \
  -f helm/values-federation-user.yaml \
  --namespace federation \
  --create-namespace

# Product subgraph
helm install product-subgraph ./helm/grpc-graphql-gateway \
  -f helm/values-federation-product.yaml \
  --namespace federation

# Review subgraph
helm install review-subgraph ./helm/grpc-graphql-gateway \
  -f helm/values-federation-review.yaml \
  --namespace federation
```

Or use the automated script:

```bash
./helm/deploy-federation.sh
```

## Monitoring & Observability

### Prometheus Metrics

```yaml
serviceMonitor:
  enabled: true
  interval: 30s
  labels:
    release: prometheus
```

### Pod Annotations

```yaml
podAnnotations:
  prometheus.io/scrape: "true"
  prometheus.io/port: "9090"
  prometheus.io/path: "/metrics"
```

## Security

### Network Policies

```yaml
networkPolicy:
  enabled: true
  ingress:
    - from:
      - namespaceSelector:
          matchLabels:
            name: ingress-nginx
```

### Pod Security

```yaml
podSecurityContext:
  runAsNonRoot: true
  runAsUser: 1000
  fsGroup: 1000

securityContext:
  capabilities:
    drop:
    - ALL
  readOnlyRootFilesystem: true
  allowPrivilegeEscalation: false
```

## Common Operations

### Upgrade

```bash
helm upgrade my-gateway ./helm/grpc-graphql-gateway \
  -f custom-values.yaml \
  --namespace grpc-gateway
```

### Rollback

```bash
# View history
helm history my-gateway -n grpc-gateway

# Rollback
helm rollback my-gateway 1 -n grpc-gateway
```

### Uninstall

```bash
helm uninstall my-gateway --namespace grpc-gateway
```

### View Rendered Templates

```bash
helm template my-gateway ./helm/grpc-graphql-gateway \
  -f custom-values.yaml \
  --output-dir ./rendered
```

## Troubleshooting

### Pods Not Starting

```bash
kubectl describe pod <pod-name> -n grpc-gateway
kubectl logs <pod-name> -n grpc-gateway
```

### HPA Not Scaling

```bash
# Check metrics server
kubectl top nodes
kubectl get hpa -n grpc-gateway
```

### Service Not Accessible

```bash
kubectl get svc -n grpc-gateway
kubectl describe svc my-gateway -n grpc-gateway
kubectl get endpoints -n grpc-gateway
```

## Best Practices

1. **Always use PodDisruptionBudget** for production
2. **Enable HPA** for automatic scaling
3. **Use anti-affinity** to spread pods across nodes
4. **Configure health checks** properly
5. **Set resource limits** to prevent resource exhaustion
6. **Use secrets** for sensitive data
7. **Enable monitoring** with ServiceMonitor
8. **Test in staging** before production deployment

## Next Steps

- [Autoscaling & Load Balancing](./autoscaling.md)
- [Security Checklist](./security-checklist.md)
- [Monitoring](../advanced/metrics.md)

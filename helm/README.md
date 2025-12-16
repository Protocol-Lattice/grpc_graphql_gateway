# Helm Charts

This directory contains Helm charts and deployment configurations for the gRPC GraphQL Gateway.

## 📁 Contents

```
helm/
├── grpc-graphql-gateway/           # Main Helm chart
│   ├── Chart.yaml                  # Chart metadata
│   ├── values.yaml                 # Default configuration
│   └── templates/                  # Kubernetes manifests
│       ├── deployment.yaml
│       ├── service.yaml
│       ├── hpa.yaml                # Horizontal autoscaling
│       ├── vpa.yaml                # Vertical autoscaling
│       ├── loadbalancer.yaml       # External load balancer
│       └── ...
│
├── deploy-federation.sh            # Deploy complete federation
├── validate-chart.sh               # Validate and package chart
│
├── values-autoscaling-complete.yaml    # Full autoscaling example
├── values-federation-user.yaml         # User subgraph config
├── values-federation-product.yaml      # Product subgraph config
└── values-federation-review.yaml       # Review subgraph config
```

## 🚀 Quick Start

### Single Gateway

```bash
# From project root
helm install my-gateway ./helm/grpc-graphql-gateway \
  --namespace grpc-gateway \
  --create-namespace
```

### Federation (3 Subgraphs)

```bash
# Automated deployment
./helm/deploy-federation.sh

# Or manually deploy each subgraph
helm install user-subgraph ./helm/grpc-graphql-gateway \
  -f helm/values-federation-user.yaml \
  --namespace federation-example \
  --create-namespace
```

## 📋 Available Scripts

### `deploy-federation.sh`
Deploys complete federation architecture with 3 subgraphs:
- User Subgraph (port 8891)
- Product Subgraph (port 8892)
- Review Subgraph (port 8893)

**Usage:**
```bash
cd helm/
./deploy-federation.sh
```

### `validate-chart.sh`
Validates, lints, and packages the Helm chart:
```bash
./helm/validate-chart.sh
```

## 📊 Configuration Examples

### Autoscaling

```bash
helm install gateway ./helm/grpc-graphql-gateway \
  -f helm/values-autoscaling-complete.yaml
```

Features:
- HPA: 5-50 pods
- VPA: Resource recommendations
- LoadBalancer: External access
- Multi-AZ distribution

### Custom Values

Create `my-values.yaml`:
```yaml
replicaCount: 5

autoscaling:
  enabled: true
  minReplicas: 5
  maxReplicas: 20

loadBalancer:
  enabled: true
  annotations:
    service.beta.kubernetes.io/aws-load-balancer-type: "nlb"
```

Install:
```bash
helm install gateway ./helm/grpc-graphql-gateway \
  -f my-values.yaml
```

## 🔧 Development

### Validate Chart

```bash
# Lint chart
helm lint ./helm/grpc-graphql-gateway

# Dry-run install
helm install --dry-run --debug test ./helm/grpc-graphql-gateway

# Render templates
helm template test ./helm/grpc-graphql-gateway
```

### Package Chart

```bash
# Package
helm package ./helm/grpc-graphql-gateway

# Create repository index
helm repo index ./helm --url http://localhost:8080/helm

# Serve locally
python3 -m http.server 8080 --directory ./helm
```

## 🌐 Using Helm Repository

```bash
# Add local repo
helm repo add local http://localhost:8080/helm
helm repo update

# Install from repo
helm install my-gateway local/grpc-graphql-gateway
```

## 📚 Documentation

- **Complete Guide**: [docs/src/production/helm-deployment.md](../docs/src/production/helm-deployment.md)
- **Autoscaling**: [docs/src/production/autoscaling.md](../docs/src/production/autoscaling.md)
- **Quick Start**: [DEPLOYMENT.md](../DEPLOYMENT.md)

## 🗑️ Cleanup

```bash
# Uninstall gateway
helm uninstall my-gateway -n grpc-gateway

# Uninstall federation
helm uninstall user-subgraph product-subgraph review-subgraph -n federation-example
kubectl delete namespace federation-example
```

## 🤝 Contributing

When updating the chart:

1. Update `Chart.yaml` version
2. Run `helm lint`
3. Test installation
4. Update documentation
5. Package: `helm package ./helm/grpc-graphql-gateway`

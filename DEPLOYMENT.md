# Quick Start: Docker & Kubernetes Deployment

This guide gets you up and running quickly with Docker and Kubernetes deployments.

## 🐳 Docker

### Build Images

```bash
# Build all images
./build-docker.sh v0.2.9

# Or build manually
docker build -t grpc-graphql-gateway:latest .
docker build -t grpc-graphql-gateway-federation:latest -f Dockerfile.federation .
```

### Run with Docker

```bash
# Run single gateway
docker run -p 8080:8080 -p 9090:9090 \
  -e RUST_LOG=info \
  grpc-graphql-gateway:latest

# Run federation example
docker-compose -f docker-compose.federation.yml up
```

### Test

```bash
# GraphQL query
curl -X POST http://localhost:8080/graphql \
  -H 'Content-Type: application/json' \
  -d '{"query": "{ __typename }"}'

# Metrics
curl http://localhost:9090/metrics
```

## ☸️ Kubernetes - Quick Deployment

### Prerequisites

```bash
# Ensure you have
kubectl cluster-info
helm version
```

### Deploy Single Gateway

```bash
# Install with defaults
helm install my-gateway ./helm/grpc-graphql-gateway \
  --create-namespace \
  --namespace grpc-gateway

# With custom values
helm install my-gateway ./helm/grpc-graphql-gateway \
  -f helm/values-autoscaling-complete.yaml \
  --namespace grpc-gateway
```

### Deploy Federation

```bash
# Run the deployment script
./helm/deploy-federation.sh

# Or apply manifests directly
kubectl apply -f helm/federation-example.yaml
```

### Check Status

```bash
# Pods
kubectl get pods -n grpc-gateway

# Services
kubectl get svc -n grpc-gateway

# HPA (if enabled)
kubectl get hpa -n grpc-gateway

# VPA (if enabled)  
kubectl get vpa -n grpc-gateway
```

## 📊 Autoscaling Setup

### Horizontal Scaling Only

```bash
helm install my-gateway ./helm/grpc-graphql-gateway \
  --set autoscaling.enabled=true \
  --set autoscaling.minReplicas=3 \
  --set autoscaling.maxReplicas=10 \
  --namespace grpc-gateway
```

### Vertical Scaling (Recommendations)

```bash
# Install VPA operator first
git clone https://github.com/kubernetes/autoscaler.git
cd autoscaler/vertical-pod-autoscaler
./hack/vpa-up.sh

# Deploy with VPA
helm install my-gateway ./helm/grpc-graphql-gateway \
  --set verticalPodAutoscaler.enabled=true \
  --set verticalPodAutoscaler.updateMode=Off \
  --namespace grpc-gateway

# View recommendations
kubectl describe vpa my-gateway -n grpc-gateway
```

### Both HPA + VPA

```bash
helm install my-gateway ./helm/grpc-graphql-gateway \
  -f helm/values-autoscaling-complete.yaml \
  --namespace grpc-gateway
```

## 🌐 LoadBalancer

### Enable LoadBalancer

```bash
helm install my-gateway ./helm/grpc-graphql-gateway \
  --set loadBalancer.enabled=true \
  --set loadBalancer.externalTrafficPolicy=Local \
  --namespace grpc-gateway

# Get external IP
kubectl get svc my-gateway-lb -n grpc-gateway
```

### Cloud Provider Specific

**AWS**:
```bash
helm install my-gateway ./helm/grpc-graphql-gateway \
  --set loadBalancer.enabled=true \
  --set loadBalancer.annotations."service\.beta\.kubernetes\.io/aws-load-balancer-type"="nlb" \
  --namespace grpc-gateway
```

**GCP**:
```bash
helm install my-gateway ./helm/grpc-graphql-gateway \
  --set loadBalancer.enabled=true \
  --set loadBalancer.annotations."cloud\.google\.com/load-balancer-type"="Internal" \
  --namespace grpc-gateway
```

## 🔍 Monitoring

```bash
# Watch pods scale
watch kubectl get pods,hpa -n grpc-gateway

# View metrics
kubectl port-forward -n grpc-gateway svc/my-gateway 9090:9090
curl http://localhost:9090/metrics

# View logs
kubectl logs -f -l app.kubernetes.io/name=grpc-graphql-gateway -n grpc-gateway
```

## 🧹 Cleanup

```bash
# Uninstall gateway
helm uninstall my-gateway -n grpc-gateway

# Delete namespace
kubectl delete namespace grpc-gateway

# Delete federation
kubectl delete namespace federation-example

# Stop Docker Compose
docker-compose -f docker-compose.federation.yml down
```

## 📚 Full Documentation

- **Docker**: See `Dockerfile` and `docker-compose.federation.yml`
- **Helm**: See `helm/grpc-graphql-gateway/README.md`
- **Autoscaling**: See `docs/src/production/autoscaling.md`
- **Federation**: See `docs/src/federation/overview.md`

## 🆘 Troubleshooting

**Pods not starting?**
```bash
kubectl describe pod <pod-name> -n grpc-gateway
kubectl logs <pod-name> -n grpc-gateway
```

**HPA not scaling?**
```bash
# Check metrics server
kubectl top nodes
kubectl get hpa -n grpc-gateway
```

**LoadBalancer pending?**
```bash
# Check provider configuration
kubectl describe svc my-gateway-lb -n grpc-gateway
```

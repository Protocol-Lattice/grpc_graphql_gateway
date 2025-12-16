# Autoscaling and Load Balancing

This guide covers setting up comprehensive autoscaling and load balancing for the gRPC GraphQL Gateway.

## Overview

The gateway supports three types of scaling and load balancing:

1. **Horizontal Pod Autoscaler (HPA)** - Scales the number of pods based on metrics
2. **Vertical Pod Autoscaler (VPA)** - Adjusts resource requests/limits for pods
3. **LoadBalancer** - External load balancing for traffic distribution

## Horizontal Pod Autoscaler (HPA)

HPA automatically scales the number of pods based on observed CPU, memory, or custom metrics.

### Basic Configuration

```yaml
autoscaling:
  enabled: true
  minReplicas: 3
  maxReplicas: 10
  targetCPUUtilizationPercentage: 70
  targetMemoryUtilizationPercentage: 80
```

### Custom Metrics

For advanced scaling based on custom metrics:

```yaml
autoscaling:
  enabled: true
  minReplicas: 5
  maxReplicas: 50
  customMetrics:
    - type: Pods
      pods:
        metric:
          name: http_requests_per_second
        target:
          type: AverageValue
          averageValue: "1000"
```

### Deployment

```bash
helm install my-gateway ./grpc-graphql-gateway \
  --set autoscaling.enabled=true \
  --set autoscaling.minReplicas=3 \
  --set autoscaling.maxReplicas=10
```

### Monitoring HPA

```bash
# Watch HPA status
kubectl get hpa -w

# Describe HPA for detailed metrics
kubectl describe hpa my-gateway

# View current metrics
kubectl top pods -l app.kubernetes.io/name=grpc-graphql-gateway
```

## Vertical Pod Autoscaler (VPA)

VPA automatically adjusts CPU and memory requests/limits based on actual usage.

### Prerequisites

Install VPA in your cluster:

```bash
git clone https://github.com/kubernetes/autoscaler.git
cd autoscaler/vertical-pod-autoscaler
./hack/vpa-up.sh
```

### Configuration

```yaml
verticalPodAutoscaler:
  enabled: true
  updateMode: "Auto"  # Off, Initial, Recreate, Auto
  minAllowed:
    cpu: 100m
    memory: 128Mi
  maxAllowed:
    cpu: 2000m
    memory: 2Gi
  controlledResources:
    - cpu
    - memory
```

### Update Modes

| Mode | Description | Use Case |
|------|-------------|----------|
| **Off** | Only provides recommendations | Safe to use with HPA |
| **Initial** | Applies recommendations on pod creation only | Good for initial sizing |
| **Recreate** | Updates running pods (requires restart) | When you want automatic updates |
| **Auto** | Automatically applies recommendations | Full automation |

### Using VPA with HPA

⚠️ **Important**: VPA and HPA should not target the same metrics (CPU/Memory).

**Recommended Setup**:

```yaml
# Use VPA in "Off" mode for recommendations
verticalPodAutoscaler:
  enabled: true
  updateMode: "Off"
  
# Use HPA for horizontal scaling
autoscaling:
  enabled: true
  minReplicas: 3
  maxReplicas: 10
```

**Alternative**: Use VPA for CPU/Memory and HPA for custom metrics.

### Viewing VPA Recommendations

```bash
# Get VPA status
kubectl describe vpa my-gateway

# View recommendations
kubectl get vpa my-gateway -o jsonpath='{.status.recommendation}'
```

## LoadBalancer Service

LoadBalancer provides external access with cloud provider integration.

### Basic Configuration

```yaml
loadBalancer:
  enabled: true
  httpPort: 80
  grpcPort: 50051
  externalTrafficPolicy: Cluster
```

### AWS Network Load Balancer

```yaml
loadBalancer:
  enabled: true
  annotations:
    service.beta.kubernetes.io/aws-load-balancer-type: "nlb"
    service.beta.kubernetes.io/aws-load-balancer-cross-zone-load-balancing-enabled: "true"
    service.beta.kubernetes.io/aws-load-balancer-internal: "false"
  externalTrafficPolicy: Local  # Preserve source IP
  loadBalancerSourceRanges:
    - "10.0.0.0/8"  # Restrict to VPC
```

### Google Cloud Load Balancer

```yaml
loadBalancer:
  enabled: true
  annotations:
    cloud.google.com/load-balancer-type: "Internal"
    cloud.google.com/backend-config: '{"default": "backend-config"}'
  externalTrafficPolicy: Cluster
```

### Azure Load Balancer

```yaml
loadBalancer:
  enabled: true
  annotations:
    service.beta.kubernetes.io/azure-load-balancer-internal: "true"
  loadBalancerIP: "10.0.0.10"  # Static internal IP
```

### External Traffic Policy

| Policy | Pros | Cons |
|--------|------|------|
| **Cluster** | Even load distribution across nodes | Loses source IP |
| **Local** | Preserves source IP, lower latency | May cause uneven load distribution |

## Complete Example

### Production Deployment with All Features

```yaml
# values-production.yaml
loadBalancer:
  enabled: true
  annotations:
    service.beta.kubernetes.io/aws-load-balancer-type: "nlb"
  externalTrafficPolicy: Local
  httpPort: 80
  loadBalancerSourceRanges:
    - "0.0.0.0/0"

autoscaling:
  enabled: true
  minReplicas: 5
  maxReplicas: 50
  targetCPUUtilizationPercentage: 70
  targetMemoryUtilizationPercentage: 80

verticalPodAutoscaler:
  enabled: true
  updateMode: "Off"  # Get recommendations without conflicts
  minAllowed:
    cpu: 250m
    memory: 256Mi
  maxAllowed:
    cpu: 4000m
    memory: 4Gi

resources:
  limits:
    cpu: 2000m
    memory: 2Gi
  requests:
    cpu: 1000m
    memory: 1Gi

podDisruptionBudget:
  enabled: true
  minAvailable: 3

affinity:
  podAntiAffinity:
    requiredDuringSchedulingIgnoredDuringExecution:
      - labelSelector:
          matchExpressions:
            - key: app.kubernetes.io/name
              operator: In
              values:
                - grpc-graphql-gateway
        topologyKey: kubernetes.io/hostname
```

Deploy:

```bash
helm install gateway ./grpc-graphql-gateway \
  -f helm/values-production.yaml \
  --namespace production \
  --create-namespace
```

## Load Balancing Strategies

### At Service Level

```yaml
service:
  sessionAffinity: ClientIP  # Sticky sessions
  sessionAffinityConfig:
    clientIP:
      timeoutSeconds: 10800  # 3 hours
```

### At Ingress Level

```yaml
ingress:
  annotations:
    # Round Robin (default)
    nginx.ingress.kubernetes.io/load-balance: "round_robin"
    
    # Least Connections
    # nginx.ingress.kubernetes.io/load-balance: "least_conn"
    
    # IP Hash
    # nginx.ingress.kubernetes.io/load-balance: "ip_hash"
```

## Monitoring and Troubleshooting

### Check Load Distribution

```bash
# View pod distribution across nodes
kubectl get pods -o wide -l app.kubernetes.io/name=grpc-graphql-gateway

# Check service endpoints
kubectl get endpoints my-gateway

# Check LoadBalancer status
kubectl get svc my-gateway-lb
```

### Monitor Autoscaling

```bash
# Watch HPA
watch kubectl get hpa

# Monitor resource usage
kubectl top pods

# Check VPA recommendations
kubectl describe vpa my-gateway
```

### Load Testing

```bash
# Install k6
brew install k6

# Run load test
k6 run --vus 100 --duration 5m - <<EOF
import http from 'k6/http';

export default function () {
  const query = JSON.stringify({
    query: '{ __typename }'
  });
  
  http.post('http://<loadbalancer-ip>/graphql', query, {
    headers: { 'Content-Type': 'application/json' },
  });
}
EOF

# Watch scaling in action
watch kubectl get pods,hpa
```

## Best Practices

1. **Start Conservative**: Begin with moderate min/max replicas and adjust based on observed patterns

2. **VPA + HPA**: Use VPA in "Off" mode alongside HPA to get recommendations without conflicts

3. **LoadBalancer**: Use `externalTrafficPolicy: Local` when you need source IP preservation

4. **PodDisruptionBudget**: Always configure PDB to maintain availability during updates

5. **Multi-AZ**: Use pod anti-affinity to spread pods across availability zones

6. **Gradual Rollouts**: Test autoscaling in staging before production

7. **Monitor Costs**: Set reasonable maxReplicas to prevent runaway costs

8. **Health Checks**: Ensure liveness and readiness probes are properly configured

## Federation with Autoscaling

For federated deployments, each subgraph can scale independently:

```bash
# Deploy user subgraph with autoscaling
helm install user-subgraph ./grpc-graphql-gateway \
  -f helm/values-federation-user.yaml \
  --set autoscaling.maxReplicas=20

# Deploy product subgraph with different scaling
helm install product-subgraph ./grpc-graphql-gateway \
  -f helm/values-federation-product.yaml \
  --set autoscaling.maxReplicas=30
```

## Next Steps

- [Helm Deployment Guide](./helm-deployment.md)
- [Performance Tuning](../performance/caching.md)
- [Monitoring](../advanced/metrics.md)

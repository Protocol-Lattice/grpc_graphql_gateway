# Health Checks

Enable Kubernetes-compatible health check endpoints for container orchestration.

## Enabling Health Checks

```rust
let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .enable_health_checks()
    .add_grpc_client("service", client)
    .build()?;
```

## Endpoints

| Endpoint | Purpose | Success Response |
|----------|---------|------------------|
| `GET /health` | Liveness probe | `200 OK` if server is running |
| `GET /ready` | Readiness probe | `200 OK` if gRPC clients configured |

## Response Format

```json
{
  "status": "healthy",
  "components": {
    "grpc_clients": {
      "status": "healthy",
      "count": 3
    }
  }
}
```

## Kubernetes Configuration

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: graphql-gateway
spec:
  template:
    spec:
      containers:
        - name: gateway
          image: your-gateway:latest
          ports:
            - containerPort: 8888
          livenessProbe:
            httpGet:
              path: /health
              port: 8888
            initialDelaySeconds: 5
            periodSeconds: 10
            failureThreshold: 3
          readinessProbe:
            httpGet:
              path: /ready
              port: 8888
            initialDelaySeconds: 5
            periodSeconds: 10
            failureThreshold: 3
```

## Health States

| State | Description |
|-------|-------------|
| `healthy` | All components working |
| `degraded` | Partial functionality |
| `unhealthy` | Service unavailable |

## Custom Health Checks

The gateway automatically checks:
- Server is running (liveness)
- gRPC clients are configured (readiness)

For additional checks, consider using middleware or external health check services.

## Load Balancer Integration

Health endpoints work with:
- AWS ALB/NLB health checks
- Google Cloud Load Balancer
- Azure Load Balancer
- HAProxy/Nginx health checks

## Testing Health Endpoints

```bash
# Liveness check
curl http://localhost:8888/health
# {"status":"healthy"}

# Readiness check  
curl http://localhost:8888/ready
# {"status":"healthy","components":{"grpc_clients":{"status":"healthy","count":2}}}
```

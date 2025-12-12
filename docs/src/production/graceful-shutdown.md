# Graceful Shutdown

Enable production-ready server lifecycle management with graceful shutdown.

## Enabling Graceful Shutdown

```rust
use grpc_graphql_gateway::{Gateway, ShutdownConfig};
use std::time::Duration;

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .with_graceful_shutdown(ShutdownConfig {
        timeout: Duration::from_secs(30),          // Wait up to 30s
        handle_signals: true,                       // Handle SIGTERM/SIGINT
        force_shutdown_delay: Duration::from_secs(5),
    })
    .build()?;

gateway.serve("0.0.0.0:8888").await?;
```

## How It Works

1. **Signal Received**: SIGTERM, SIGINT, or Ctrl+C is received
2. **Stop Accepting**: Server stops accepting new connections
3. **Drain Requests**: In-flight requests are allowed to complete
4. **Cleanup**: Active subscriptions cancelled, resources released
5. **Exit**: Server shuts down gracefully

## Configuration Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `timeout` | Duration | 30s | Max wait for in-flight requests |
| `handle_signals` | bool | true | Handle OS signals automatically |
| `force_shutdown_delay` | Duration | 5s | Wait before forcing shutdown |

## Custom Shutdown Signal

Trigger shutdown from your own logic:

```rust
use tokio::sync::oneshot;

let (tx, rx) = oneshot::channel::<()>();

// Trigger shutdown after some condition
tokio::spawn(async move {
    tokio::time::sleep(Duration::from_secs(60)).await;
    let _ = tx.send(());
});

Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .serve_with_shutdown("0.0.0.0:8888", async { let _ = rx.await; })
    .await?;
```

## Kubernetes Integration

The gateway responds correctly to Kubernetes termination:

```yaml
spec:
  terminationGracePeriodSeconds: 30
  containers:
    - name: gateway
      lifecycle:
        preStop:
          exec:
            command: ["sleep", "5"]
```

## Benefits

- ✅ No dropped requests during deployment
- ✅ Automatic OS signal handling
- ✅ Configurable drain timeout
- ✅ Active subscription cleanup
- ✅ Kubernetes-compatible

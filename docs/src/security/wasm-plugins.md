# WASM Plugin Sandboxing

Extend the gateway with custom logic written in **any language that compiles to WebAssembly** — Rust, Go, AssemblyScript, C/C++, Zig, and more — while keeping the router process completely safe from faulty plugins.

> **Feature flag**: Enable with `cargo build --features wasm`

## Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                        Gateway Process                          │
│                                                                  │
│  ┌──────────────┐   ┌──────────────┐   ┌──────────────┐        │
│  │  WASM Plugin  │   │  WASM Plugin  │   │  WASM Plugin  │        │
│  │  (Rust .wasm) │   │  (Go .wasm)   │   │  (AS .wasm)   │        │
│  │              │   │              │   │              │        │
│  │ ┌──────────┐ │   │ ┌──────────┐ │   │ ┌──────────┐ │        │
│  │ │  Memory   │ │   │ │  Memory   │ │   │ │  Memory   │ │        │
│  │ │(sandboxed)│ │   │ │(sandboxed)│ │   │ │(sandboxed)│ │        │
│  │ └──────────┘ │   │ └──────────┘ │   │ └──────────┘ │        │
│  └──────┬───────┘   └──────┬───────┘   └──────┬───────┘        │
│         │                  │                  │                │
│         └──────────────────┼──────────────────┘                │
│                            │                                    │
│                    ┌───────▼───────┐                            │
│                    │   Host ABI    │                            │
│                    │  (log, headers│                            │
│                    │   metadata,   │                            │
│                    │    config)    │                            │
│                    └───────────────┘                            │
└──────────────────────────────────────────────────────────────────┘
```

Each plugin runs in its own **isolated WebAssembly linear memory**. A crashing, infinite-looping, or memory-hungry plugin cannot affect the host process or other plugins.

## Security Model

| Protection | Mechanism | Default |
|---|---|---|
| **CPU Budget** | Fuel metering (instruction counting) | 1,000,000 fuel per invocation |
| **Memory Limit** | `ResourceLimiter` on wasmtime `Store` | 16 MB per plugin |
| **No System Access** | WASM has no WASI — no filesystem, network, or OS access | Always enforced |
| **Crash Isolation** | Fresh `Store` per invocation; traps become `Error::WasmPlugin` | Always enforced |
| **Table Limits** | Bounded indirect function tables | 10,000 elements |

## Quick Start

### 1. Build with WASM support

```bash
cargo build --release --features wasm
```

### 2. Configure plugins in `router.yaml`

```yaml
wasm_plugins:
  plugins:
    - name: "auth-handler"
      path: "plugins/auth.wasm"
      max_memory_bytes: 16777216   # 16 MB
      max_fuel: 1000000            # CPU budget
      config:
        api_key_header: "X-API-Key"
        allowed_origins: ["https://app.example.com"]
```

### 3. Register via GatewayBuilder (programmatic)

```rust
use grpc_graphql_gateway::{Gateway, WasmPluginConfig};

let gateway = Gateway::builder()
    .register_wasm_plugin(WasmPluginConfig {
        name: "auth-handler".into(),
        path: "plugins/auth.wasm".into(),
        max_memory_bytes: 16 * 1024 * 1024,
        max_fuel: 1_000_000,
        config: serde_json::json!({"api_key_header": "X-API-Key"}),
    })?
    // ... other configuration
    .build()?;
```

### 4. Auto-discover plugins from a directory

```rust
use grpc_graphql_gateway::{Gateway, WasmResourceLimits};

let gateway = Gateway::builder()
    .load_wasm_plugins_from_dir("./plugins", WasmResourceLimits::default())?
    .build()?;
```

## Guest ABI

WASM plugins interact with the gateway through a well-defined ABI. Your module must export specific functions and may call host functions.

### Required Guest Exports

| Export | Signature | Description |
|---|---|---|
| `memory` | `Memory` | Linear memory (auto-exported by most compilers) |
| `alloc` | `(size: i32) -> i32` | Allocate `size` bytes, return pointer |

### Optional Lifecycle Hooks

| Export | Signature | Description |
|---|---|---|
| `on_request` | `(ptr: i32, len: i32) -> i32` | Called before request processing. Return `0` = allow, non-zero = reject |
| `on_response` | `(ptr: i32, len: i32) -> i32` | Called after response generation |
| `on_subgraph_request` | `(ptr: i32, len: i32) -> i32` | Called before each subgraph gRPC call |

The `ptr` and `len` arguments point to a JSON payload in guest memory:

**`on_request` payload:**
```json
{
  "request_id": "abc-123",
  "client_ip": "192.168.1.1",
  "query": "query { users { id name } }",
  "operation_name": "GetUsers"
}
```

**`on_subgraph_request` payload:**
```json
{
  "service_name": "users-service",
  "headers": {
    "authorization": "Bearer ...",
    "x-request-id": "abc-123"
  }
}
```

### Host Functions Available to Guests

All host functions are in the `"env"` module:

| Function | Signature | Description |
|---|---|---|
| `host_log` | `(level: i32, ptr: i32, len: i32)` | Log a message (0=trace, 1=debug, 2=info, 3=warn, 4=error) |
| `host_get_header` | `(key_ptr: i32, key_len: i32) -> i64` | Read a request header. Returns `(ptr << 32) \| len` or `0` if not found |
| `host_set_header` | `(key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32)` | Set a response header / metadata |
| `host_get_metadata` | `(key_ptr: i32, key_len: i32) -> i64` | Read metadata |
| `host_set_metadata` | `(key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32)` | Set metadata (propagated to gRPC `MetadataMap`) |
| `host_get_config` | `() -> i64` | Get plugin config JSON. Returns `(ptr << 32) \| len` |

## Writing a Plugin (Rust)

```rust
// lib.rs — compile with: cargo build --target wasm32-unknown-unknown --release

use std::alloc::{alloc, Layout};

#[no_mangle]
pub extern "C" fn alloc(size: i32) -> i32 {
    let layout = Layout::from_size_align(size as usize, 1).unwrap();
    unsafe { alloc(layout) as i32 }
}

// Import host functions
extern "C" {
    fn host_log(level: i32, ptr: i32, len: i32);
    fn host_get_header(key_ptr: i32, key_len: i32) -> i64;
    fn host_set_metadata(key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32);
}

fn log_info(msg: &str) {
    unsafe { host_log(2, msg.as_ptr() as i32, msg.len() as i32) }
}

#[no_mangle]
pub extern "C" fn on_request(ptr: i32, len: i32) -> i32 {
    log_info("Auth plugin: checking request");

    // Check for API key header
    let key = b"x-api-key";
    let result = unsafe { host_get_header(key.as_ptr() as i32, key.len() as i32) };

    if result == 0 {
        log_info("Auth plugin: missing API key, rejecting");
        return 1; // Reject
    }

    log_info("Auth plugin: API key present, allowing");
    0 // Allow
}
```

Build it:
```bash
rustup target add wasm32-unknown-unknown
cargo build --target wasm32-unknown-unknown --release
cp target/wasm32-unknown-unknown/release/my_plugin.wasm plugins/
```

## Writing a Plugin (AssemblyScript)

```typescript
// assembly/index.ts

@external("env", "host_log")
declare function host_log(level: i32, ptr: i32, len: i32): void;

@external("env", "host_get_header")
declare function host_get_header(key_ptr: i32, key_len: i32): i64;

export function alloc(size: i32): i32 {
  return heap.alloc(size as usize) as i32;
}

export function on_request(ptr: i32, len: i32): i32 {
  // Log
  const msg = "AS plugin: checking request";
  const msgBuf = String.UTF8.encode(msg);
  host_log(2, changetype<i32>(msgBuf), msgBuf.byteLength);

  // Check header
  const key = "authorization";
  const keyBuf = String.UTF8.encode(key);
  const result = host_get_header(changetype<i32>(keyBuf), keyBuf.byteLength);

  return result == 0 ? 1 : 0; // Reject if no auth header
}
```

## Configuration Reference

### `WasmPluginConfig`

| Field | Type | Default | Description |
|---|---|---|---|
| `name` | `String` | *required* | Human-readable plugin name |
| `path` | `PathBuf` | *required* | Path to `.wasm` file |
| `max_memory_bytes` | `usize` | `16777216` (16 MB) | Maximum linear memory |
| `max_fuel` | `u64` | `1000000` | CPU instruction budget per invocation |
| `config` | `serde_json::Value` | `null` | Arbitrary JSON config passed to plugin |

### `WasmResourceLimits`

| Field | Type | Default | Description |
|---|---|---|---|
| `max_memory_bytes` | `usize` | `16777216` | Memory limit |
| `max_fuel` | `u64` | `1000000` | Fuel limit |
| `max_tables` | `usize` | `4` | Max indirect call tables |
| `max_table_elements` | `usize` | `10000` | Max table entries |
| `max_instances` | `u32` | `1` | Max instances per plugin |

## Error Handling

WASM plugin errors produce a distinct error code `WASM_PLUGIN_ERROR`:

```json
{
  "errors": [{
    "message": "Plugin execution error",
    "extensions": {
      "code": "WASM_PLUGIN_ERROR"
    }
  }]
}
```

In non-production mode, the full error details are included:

```json
{
  "errors": [{
    "message": "WASM plugin exceeded CPU budget (fuel exhausted) in 'on_request'",
    "extensions": {
      "code": "WASM_PLUGIN_ERROR"
    }
  }]
}
```

## Production Recommendations

1. **Set conservative fuel limits** — Start with 500K fuel and increase as needed. Monitor logs for fuel exhaustion.
2. **Limit memory** — 8-16 MB is sufficient for most plugins. Only increase for data-heavy transformations.
3. **Pre-compile modules** — Wasmtime compiles WASM to native code at load time. This is a one-time cost; instantiation is fast.
4. **Monitor plugin performance** — Check `fuel_used` in debug logs to right-size your fuel budgets.
5. **Use directory auto-loading** for GitOps workflows — drop `.wasm` files into `./plugins/` and restart.
6. **Test plugins in staging** before production — a plugin returning non-zero from `on_request` will reject all requests.
